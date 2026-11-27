//! High-level operations on a [`Project`]. This is the single API surface
//! shared by CLI, MCP server and GUI.

use crate::animation::{Animation, Channel, ChannelPath, KeyValue, Space, Target};

/// Upper bound for one paint operation (fill_rect, paint_line): keeps a
/// bogus coordinate range from an agent or file from allocating without
/// bound. Generous for real tilemaps (1000×1000 cells).
pub const MAX_PAINT_CELLS: usize = 1_000_000;
use crate::id::{AnimationId, MaterialId, MeshId, NodeId, SkeletonId, TileMapId, TileSetId};
use crate::mesh::Skin;
use crate::primitives::{self, PrimitiveKind};
use crate::project::Project;
use crate::scene::{Node, NodeKind};
use crate::skeleton::Skeleton;
use crate::transform::Transform;
use anyhow::{anyhow, bail, Context, Result};
use glam::{Mat4, Vec3};
use std::collections::HashMap;

/// Per-joint local-transform overrides produced by animating a skeleton.
#[derive(Clone, Debug, Default)]
pub struct Pose {
    /// joint index -> animated local transform
    pub joints: HashMap<usize, Transform>,
    /// animated transform of the skeleton instance node itself
    pub node_override: Option<Transform>,
}

// ---------------------------------------------------------------- nodes

pub fn add_node(p: &mut Project, parent: Option<NodeId>, name: &str, transform: Transform, kind: NodeKind) -> NodeId {
    p.scene.insert(
        parent,
        Node {
            name: name.to_string(),
            transform,
            kind,
            ..Default::default()
        },
    )
}

pub fn add_primitive(
    p: &mut Project,
    parent: Option<NodeId>,
    name: &str,
    kind: PrimitiveKind,
    a: f32,
    b: f32,
    segments: u32,
    transform: Transform,
) -> (NodeId, MeshId) {
    add_primitive_with_material(p, parent, name, kind, a, b, segments, transform, None)
}

/// Like [`add_primitive`], but immediately assigns a material to the instance.
pub fn add_primitive_with_material(
    p: &mut Project,
    parent: Option<NodeId>,
    name: &str,
    kind: PrimitiveKind,
    a: f32,
    b: f32,
    segments: u32,
    transform: Transform,
    material: Option<MaterialId>,
) -> (NodeId, MeshId) {
    let mut mesh = primitives::generate(kind, a, b, segments);
    if mesh.normals.is_none() {
        mesh.compute_normals();
    }
    let mesh_id = p.library.add_mesh(mesh);
    let node = add_node(
        p,
        parent,
        name,
        transform,
        NodeKind::MeshInstance {
            mesh: mesh_id,
            material,
            skeleton_node: None,
        },
    );
    (node, mesh_id)
}

/// Attach a material to an existing mesh instance node.
pub fn set_node_material(p: &mut Project, node: NodeId, material: Option<MaterialId>) -> Result<()> {
    let n = p.scene.get_mut(node).context("node not found")?;
    match &mut n.kind {
        NodeKind::MeshInstance { material: slot, .. } => {
            *slot = material;
            Ok(())
        }
        _ => bail!("node {node} is not a mesh instance"),
    }
}

pub fn set_transform(p: &mut Project, node: NodeId, t: Transform) -> Result<()> {
    let n = p.scene.get_mut(node).context("node not found")?;
    n.transform = t;
    Ok(())
}

pub fn get_transform(p: &Project, node: NodeId) -> Result<Transform> {
    p.scene
        .get(node)
        .map(|n| n.transform)
        .context("node not found")
}

pub fn world_transform(p: &Project, node: NodeId) -> Transform {
    p.scene.world_transform(node)
}

// ------------------------------------------------------------- skeletons

pub fn add_skeleton_instance(
    p: &mut Project,
    parent: Option<NodeId>,
    name: &str,
    skeleton: SkeletonId,
    transform: Transform,
) -> Result<NodeId> {
    p.library
        .skeletons
        .get(&skeleton)
        .context("skeleton asset not found")?;
    Ok(add_node(p, parent, name, transform, NodeKind::SkeletonInstance { skeleton }))
}

pub fn add_humanoid(p: &mut Project, parent: Option<NodeId>, name: &str) -> (NodeId, SkeletonId) {
    let skel = Skeleton::humanoid(format!("{name} rig"));
    let skel_id = p.library.add_skeleton(skel);
    let node = add_node(
        p,
        parent,
        name,
        Transform::default(),
        NodeKind::SkeletonInstance { skeleton: skel_id },
    );
    (node, skel_id)
}

pub fn add_joint(
    p: &mut Project,
    skeleton: SkeletonId,
    parent_joint: Option<usize>,
    name: &str,
    local: Transform,
) -> Result<usize> {
    let sk = p
        .library
        .skeletons
        .get_mut(&skeleton)
        .context("skeleton asset not found")?;
    if sk.joint_by_name(name).is_some() {
        bail!("joint '{name}' already exists in skeleton '{}'", sk.name);
    }
    Ok(sk.add_joint(name, parent_joint, local))
}

/// World-space bind-pose position of a joint of a skeleton instance node.
pub fn joint_world_position(p: &Project, skel_node: NodeId, joint: usize) -> Vec3 {
    let base = p.scene.world_transform(skel_node).to_mat4();
    let global = p
        .library
        .skeletons
        .get(&skeleton_of(p, skel_node).unwrap_or(SkeletonId(u32::MAX)))
        .map(|s| s.global_transform(joint))
        .unwrap_or_default();
    (base * global.to_mat4()).transform_point3(Vec3::ZERO)
}

/// Skeleton asset referenced by a skeleton-instance node.
pub fn skeleton_of(p: &Project, skel_node: NodeId) -> Option<SkeletonId> {
    match p.scene.get(skel_node).map(|n| &n.kind) {
        Some(NodeKind::SkeletonInstance { skeleton }) => Some(*skeleton),
        _ => None,
    }
}

// ---------------------------------------------------------------- skinning

/// Bind a mesh instance to a skeleton instance with automatic weights:
/// each vertex is influenced by its two nearest joints with distance falloff.
///
/// Assumes mesh and skeleton share the rest pose (typical when the mesh was
/// modelled around the rig).
pub fn auto_bind(p: &mut Project, mesh_node: NodeId, skel_node: NodeId) -> Result<()> {
    let skel_id = skeleton_of(p, skel_node).ok_or_else(|| anyhow!("node {} is not a skeleton instance", skel_node))?;
    let mesh_id = match p.scene.get(mesh_node).map(|n| &n.kind) {
        Some(NodeKind::MeshInstance { mesh, .. }) => *mesh,
        _ => bail!("node {} is not a mesh instance", mesh_node),
    };

    let skeleton = p
        .library
        .skeletons
        .get(&skel_id)
        .context("skeleton asset missing")?
        .clone();

    // joint positions in mesh-local space
    let mesh_world = p.scene.world_transform(mesh_node).to_mat4();
    let mesh_world_inv = mesh_world.inverse();
    let skel_base = p.scene.world_transform(skel_node).to_mat4();
    let joint_local: Vec<Vec3> = (0..skeleton.joints.len())
        .map(|j| {
            (mesh_world_inv * skel_base * skeleton.global_transform(j).to_mat4())
                .transform_point3(Vec3::ZERO)
        })
        .collect();

    let mesh = p.library.meshes.get_mut(&mesh_id).context("mesh asset missing")?;
    let mut joints = Vec::with_capacity(mesh.positions.len());
    let mut weights = Vec::with_capacity(mesh.positions.len());

    for pos in &mesh.positions {
        let v = Vec3::from_array(*pos);
        let mut best: [(f32, usize); 2] = [(f32::MAX, 0), (f32::MAX, 0)];
        for (ji, jp) in joint_local.iter().enumerate() {
            let d = v.distance(*jp);
            if d < best[0].0 {
                best[1] = best[0];
                best[0] = (d, ji);
            } else if d < best[1].0 {
                best[1] = (d, ji);
            }
        }
        let (d0, j0) = best[0];
        let (d1, j1) = best[1];
        let w0 = if d0 + d1 > 1e-6 { 1.0 - d0 / (d0 + d1) } else { 1.0 };
        let w1 = 1.0 - w0;
        joints.push([j0 as u16, j1 as u16, 0, 0]);
        weights.push([w0, w1, 0.0, 0.0]);
    }

    if let Some(skin) = &mut mesh.skin {
        skin.joints = joints;
        skin.weights = weights;
        skin.skeleton = skel_id;
    } else {
        mesh.skin = Some(Skin {
            skeleton: skel_id,
            joints,
            weights,
        });
    }
    mesh.touch();
    // remember the binding on the node too
    if let Some(NodeKind::MeshInstance { skeleton_node, .. }) = p.scene.get_mut(mesh_node).map(|n| &mut n.kind) {
        *skeleton_node = Some(skel_node);
    }
    Ok(())
}

// --------------------------------------------------------------- animation

pub fn add_keyframe_joint(
    p: &mut Project,
    anim: AnimationId,
    joint_name: &str,
    path: ChannelPath,
    time: f32,
    value: KeyValue,
) -> Result<()> {
    let a = p
        .library
        .animations
        .get_mut(&anim)
        .context("animation not found")?;
    a.get_or_create(Target::Joint(joint_name.to_string()), path)
        .set_key(time, value);
    Ok(())
}

pub fn add_keyframe_node(
    p: &mut Project,
    anim: AnimationId,
    node: NodeId,
    path: ChannelPath,
    time: f32,
    value: KeyValue,
) -> Result<()> {
    let a = p
        .library
        .animations
        .get_mut(&anim)
        .context("animation not found")?;
    a.get_or_create(Target::Node(node), path)
        .set_key(time, value);
    Ok(())
}

/// Sample `anim` at time `t` and resolve joint-name targets against the
/// skeleton instance `skel_node` (retargeting). Unknown joint names are skipped.
pub fn sample_pose(p: &Project, skel_node: NodeId, anim: &Animation, t: f32) -> Pose {
    let mut pose = Pose::default();
    let Some(skel_id) = skeleton_of(p, skel_node) else {
        return pose;
    };
    let Some(skeleton) = p.library.skeletons.get(&skel_id) else {
        return pose;
    };
    let apply = |tr: &mut Transform, path: ChannelPath, value: KeyValue| match (path, value) {
        (ChannelPath::Translation, KeyValue::Vec3(v)) => tr.translation = v,
        (ChannelPath::Rotation, KeyValue::Quat(q)) => tr.rotation = q,
        (ChannelPath::Scale, KeyValue::Vec3(s)) => tr.scale = s,
        _ => {}
    };
    // Relative channels compose onto the current value instead of replacing it.
    let write = |entry: &mut Transform, channel: &Channel, value: KeyValue| {
        if channel.space == Space::RelativeToBind {
            let mut delta = Transform::default();
            apply(&mut delta, channel.path, value);
            *entry = (*entry).then(&delta);
        } else {
            apply(entry, channel.path, value);
        }
    };
    for (channel, value) in anim.sample(t) {
        match &channel.target {
            Target::Joint(name) => {
                if let Some(ji) = skeleton.joint_by_name(name) {
                    let entry = pose
                        .joints
                        .entry(ji)
                        .or_insert_with(|| skeleton.joints[ji].local);
                    write(entry, channel, value);
                }
            }
            Target::Node(n) => {
                if *n == skel_node {
                    let base = pose
                        .node_override
                        .get_or_insert_with(|| p.scene.get(*n).map(|x| x.transform).unwrap_or_default());
                    write(base, channel, value);
                }
            }
        }
    }
    pose
}

/// Effective local transform of a joint given a pose (falls back to bind pose).
pub fn joint_local(skeleton: &Skeleton, pose: &Pose, joint: usize) -> Transform {
    pose.joints
        .get(&joint)
        .copied()
        .unwrap_or_else(|| skeleton.joints[joint].local)
}

/// Global joint transform under a pose (skeleton space).
pub fn joint_global(skeleton: &Skeleton, pose: &Pose, joint: usize) -> Transform {
    let local = joint_local(skeleton, pose, joint);
    match skeleton.joints[joint].parent {
        Some(parent) => joint_global(skeleton, pose, parent).then(&local),
        None => local,
    }
}

/// World-space skinning matrices (global joint transform, relative to mesh node)
/// for a mesh bound to `skel_node`, under `pose`.
///
/// Each entry is `inv(meshWorld) * skelWorld * jointGlobal_pose(j) * inv(jointGlobal_bind(j))`,
/// matching glTF skinning semantics: at bind pose the result is the identity.
pub fn skinning_matrices(
    p: &Project,
    mesh_node: NodeId,
    skel_node: NodeId,
    pose: &Pose,
) -> Result<Vec<Mat4>> {
    let skel_id = skeleton_of(p, skel_node).ok_or_else(|| anyhow!("not a skeleton instance"))?;
    let skeleton = p
        .library
        .skeletons
        .get(&skel_id)
        .context("skeleton missing")?;
    let mesh_world = p.scene.world_transform(mesh_node).to_mat4().inverse();
    let node_world = p.scene.world_transform(skel_node).to_mat4();
    Ok((0..skeleton.joints.len())
        .map(|j| {
            let posed = joint_global(skeleton, pose, j).to_mat4();
            let bind_inv = skeleton.global_transform(j).to_mat4().inverse();
            mesh_world * node_world * posed * bind_inv
        })
        .collect())
}

/// CPU linear blend skinning: returns deformed positions in the mesh node's
/// local space (apply the mesh node's world transform on top for world space).
/// Used by the viewer; exporters keep the skin data instead.
pub fn skinned_positions(
    p: &Project,
    mesh_id: MeshId,
    mesh_node: NodeId,
    skel_node: NodeId,
    pose: &Pose,
) -> Result<Vec<[f32; 3]>> {
    let mesh = p.library.meshes.get(&mesh_id).context("mesh missing")?.clone();
    let skin = mesh.skin.clone().ok_or_else(|| anyhow!("no skin"))?;
    let mats = skinning_matrices(p, mesh_node, skel_node, pose)?;
    let mut out = Vec::with_capacity(mesh.positions.len());
    for (vi, pos) in mesh.positions.iter().enumerate() {
        let v = Vec3::from_array(*pos);
        let mut acc = Vec3::ZERO;
        let w = skin.weights[vi];
        for k in 0..4 {
            let wk = w[k];
            if wk <= 0.0 {
                continue;
            }
            let ji = skin.joints[vi][k] as usize;
            if ji < mats.len() {
                acc += mats[ji].transform_point3(v) * wk;
            }
        }
        out.push(acc.to_array());
    }
    Ok(out)
}

/// Create an empty reusable animation asset.
pub fn create_animation(p: &mut Project, name: &str) -> AnimationId {
    p.library.add_animation(Animation::new(name))
}

// ---------------------------------------------------------------- tiles

/// Create an empty tileset with the given cell size (X width, Z depth).
pub fn create_tileset(p: &mut Project, name: &str, cell_w: f32, cell_d: f32) -> TileSetId {
    p.library.add_tileset(crate::tileset::TileSet::new(name, cell_w, cell_d))
}

/// Generate a primitive mesh and register it as a tile. Returns the tile index.
pub fn add_tile_primitive(
    p: &mut Project,
    tileset: TileSetId,
    name: &str,
    kind: PrimitiveKind,
    a: f32,
    b: f32,
    segments: u32,
    offset: Transform,
    material: Option<MaterialId>,
) -> Result<u32> {
    if !p.library.tilesets.contains_key(&tileset) {
        bail!("tileset not found");
    }
    let mut mesh = primitives::generate(kind, a, b, segments);
    if mesh.normals.is_none() {
        mesh.compute_normals();
    }
    let mesh_id = p.library.add_mesh(mesh);
    let ts = p.library.tilesets.get_mut(&tileset).unwrap();
    let idx = ts.tiles.len() as u32;
    ts.tiles.push(crate::tileset::Tile {
        name: name.to_string(),
        mesh: mesh_id,
        offset,
        material,
        motion: crate::motion::Motion::None,
    });
    Ok(idx)
}

/// Give one tile a procedural motion (all of its cells animate alike,
/// phase-staggered per cell in the viewer).
pub fn set_tile_motion(
    p: &mut Project,
    tileset: TileSetId,
    tile: u32,
    motion: crate::motion::Motion,
) -> Result<()> {
    let ts = p
        .library
        .tilesets
        .get_mut(&tileset)
        .context("tileset not found")?;
    let t = ts
        .tiles
        .get_mut(tile as usize)
        .with_context(|| format!("tile {tile} not found"))?;
    t.motion = motion;
    Ok(())
}

/// Register an existing mesh asset as a tile. Returns the tile index.
pub fn add_tile_mesh(
    p: &mut Project,
    tileset: TileSetId,
    name: &str,
    mesh: MeshId,
    offset: Transform,
    material: Option<MaterialId>,
) -> Result<u32> {
    if !p.library.meshes.contains_key(&mesh) {
        bail!("mesh not found");
    }
    let ts = p
        .library
        .tilesets
        .get_mut(&tileset)
        .context("tileset not found")?;
    let idx = ts.tiles.len() as u32;
    ts.tiles.push(crate::tileset::Tile {
        name: name.to_string(),
        mesh,
        offset,
        material,
        motion: crate::motion::Motion::None,
    });
    Ok(idx)
}

/// Create a tilemap asset and an instance node for it.
pub fn create_tilemap(
    p: &mut Project,
    name: &str,
    tileset: TileSetId,
    parent: Option<NodeId>,
    transform: Transform,
) -> Result<(NodeId, TileMapId)> {
    if !p.library.tilesets.contains_key(&tileset) {
        bail!("tileset not found");
    }
    let map = p.library.add_tilemap(crate::tileset::TileMap::new(name, tileset));
    let node = add_node(
        p,
        parent,
        name,
        transform,
        NodeKind::TileMapInstance { tilemap: map },
    );
    Ok((node, map))
}

fn validate_tile(p: &Project, map: TileMapId, tile: u32) -> Result<()> {
    let m = p.library.tilemaps.get(&map).context("tilemap not found")?;
    let ts = p.library.tilesets.get(&m.tileset).context("tileset not found")?;
    if tile as usize >= ts.tiles.len() {
        bail!("tile {tile} out of range (tileset has {} tiles)", ts.tiles.len());
    }
    Ok(())
}

/// Paint one cell.
pub fn tilemap_set_cell(p: &mut Project, map: TileMapId, x: i32, z: i32, tile: u32) -> Result<()> {
    validate_tile(p, map, tile)?;
    p.library
        .tilemaps
        .get_mut(&map)
        .context("tilemap not found")?
        .set(x, z, tile);
    Ok(())
}

/// Paint an inclusive rectangle of cells. Returns how many cells were written.
/// Capped at [`MAX_PAINT_CELLS`] so a bogus range from an agent or file
/// cannot allocate without bound.
pub fn tilemap_fill_rect(
    p: &mut Project,
    map: TileMapId,
    x0: i32,
    z0: i32,
    x1: i32,
    z1: i32,
    tile: u32,
) -> Result<usize> {
    validate_tile(p, map, tile)?;
    let (xa, xb) = (x0.min(x1), x0.max(x1));
    let (za, zb) = (z0.min(z1), z0.max(z1));
    let area = (xb as i64 - xa as i64 + 1) * (zb as i64 - za as i64 + 1);
    if area > MAX_PAINT_CELLS as i64 {
        bail!("fill_rect would paint {area} cells (limit {MAX_PAINT_CELLS})");
    }
    let m = p
        .library
        .tilemaps
        .get_mut(&map)
        .context("tilemap not found")?;
    let mut n = 0;
    for x in xa..=xb {
        for z in za..=zb {
            m.set(x, z, tile);
            n += 1;
        }
    }
    Ok(n)
}

/// Remove one cell; returns whether anything was there.
pub fn tilemap_erase(p: &mut Project, map: TileMapId, x: i32, z: i32) -> Result<bool> {
    Ok(p
        .library
        .tilemaps
        .get_mut(&map)
        .context("tilemap not found")?
        .erase(x, z))
}

/// Remove all cells. Returns the count.
pub fn tilemap_clear(p: &mut Project, map: TileMapId) -> Result<usize> {
    Ok(p
        .library
        .tilemaps
        .get_mut(&map)
        .context("tilemap not found")?
        .clear())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{Animation, Channel, KeyValue, Space};
    use glam::Quat;

    fn project_with_rig() -> (Project, NodeId, SkeletonId) {
        let mut p = Project::new("t");
        let mut sk = Skeleton::new("rig");
        sk.add_joint("Hips", None, Transform::from_parts(Vec3::new(0.0, 1.0, 0.0), Quat::IDENTITY, Vec3::ONE));
        let rig = p.library.add_skeleton(sk);
        let node = add_node(&mut p, None, "hero", Transform::default(), NodeKind::SkeletonInstance { skeleton: rig });
        (p, node, rig)
    }

    #[test]
    fn absolute_translation_replaces_bind_offset() {
        let (mut p, node, _) = project_with_rig();
        let mut a = Animation::new("clip");
        let mut ch = Channel::new(Target::Joint("Hips".into()), ChannelPath::Translation);
        ch.set_key(0.0, KeyValue::Vec3([0.0, 0.1, 0.0]));
        a.channels.push(ch);
        let anim = p.library.add_animation(a);
        let pose = sample_pose(&p, node, p.library.animations.get(&anim).unwrap(), 0.0);
        // value replaces the local translation -> joint ends up at 0.1, not 1.1
        assert_eq!(pose.joints[&0].translation, [0.0, 0.1, 0.0]);
    }

    #[test]
    fn relative_translation_offsets_from_bind_pose() {
        let (mut p, node, _) = project_with_rig();
        let mut a = Animation::new("clip");
        let mut ch = Channel::new(Target::Joint("Hips".into()), ChannelPath::Translation);
        ch.space = Space::RelativeToBind;
        ch.set_key(0.0, KeyValue::Vec3([0.0, 0.1, 0.0]));
        a.channels.push(ch);
        let anim = p.library.add_animation(a);
        let pose = sample_pose(&p, node, p.library.animations.get(&anim).unwrap(), 0.0);
        assert!((pose.joints[&0].translation[1] - 1.1).abs() < 1e-5);
    }

    #[test]
    fn relative_clip_retargets_onto_taller_rig() {
        // Same clip, two rigs: the bob is relative, so both keep their height.
        let (mut p, node, rig) = project_with_rig();
        p.library
            .skeletons
            .get_mut(&rig)
            .unwrap()
            .joints
            .iter_mut()
            .for_each(|j| j.local.translation[0] = 0.0);
        let mut a = Animation::new("bob");
        let mut ch = Channel::new(Target::Joint("Hips".into()), ChannelPath::Translation);
        ch.space = Space::RelativeToBind;
        ch.set_key(0.0, KeyValue::Vec3([0.0, 0.0, 0.0]));
        ch.set_key(1.0, KeyValue::Vec3([0.0, 0.2, 0.0]));
        a.channels.push(ch);
        let anim = p.library.add_animation(a);

        let far = add_node(
            &mut p,
            None,
            "hero2",
            Transform::from_parts(Vec3::new(3.0, 0.0, 0.0), Quat::IDENTITY, Vec3::ONE),
            NodeKind::SkeletonInstance { skeleton: rig },
        );
        for (n, expect_x) in [(node, 0.0), (far, 3.0)] {
            let pose = sample_pose(&p, n, p.library.animations.get(&anim).unwrap(), 1.0);
            let g = joint_global(p.library.skeletons.get(&rig).unwrap(), &pose, 0);
            // world position = instance node transform * skeleton-space joint
            let w = p.scene.world_transform(n).to_mat4() * g.to_mat4();
            let pos = w.transform_point3(Vec3::ZERO);
            assert!((pos[1] - 1.2).abs() < 1e-5, "hip height, got {pos:?}");
            assert!((pos[0] - expect_x).abs() < 1e-5, "instance offset, got {pos:?}");
        }
    }

    #[test]
    fn tileset_tilemap_ops_and_validation() {
        let mut p = Project::new("tiles");
        let kit = create_tileset(&mut p, "kit", 2.0, 2.0);
        let floor =
            add_tile_primitive(&mut p, kit, "floor", PrimitiveKind::Plane, 2.0, 2.0, 1, Transform::default(), None)
                .unwrap();
        assert_eq!(floor, 0);
        // referencing an unknown mesh must fail
        let bad = add_tile_mesh(&mut p, kit, "ghost", MeshId(99), Transform::default(), None);
        assert!(bad.is_err());

        let (node, map) = create_tilemap(&mut p, "field", kit, None, Transform::default()).unwrap();
        assert_eq!(tilemap_fill_rect(&mut p, map, -1, -1, 1, 1, floor).unwrap(), 9);
        // out-of-range tile index rejected, map stays untouched
        assert!(tilemap_set_cell(&mut p, map, 5, 5, 7).is_err());
        assert_eq!(p.library.tilemaps[&map].bounds(), Some((-1, -1, 1, 1)));
        assert!(matches!(
            p.scene.get(node).unwrap().kind,
            NodeKind::TileMapInstance { .. }
        ));

        // project survives a JSON roundtrip with tiles intact
        let json = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
}
