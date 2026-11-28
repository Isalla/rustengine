//! glTF 2.0 / GLB reader built on the `gltf` crate.
//!
//! - Node hierarchy → `Scene`; primitives → `Mesh` assets; materials → `Material`.
//! - Skins become `Skeleton` assets + hidden original joint nodes plus a
//!   `SkeletonInstance` node carrying the bind pose.
//! - Joint-targeted animation channels become `Target::Joint(name)` so clips
//!   stay reusable; other channels keep concrete node targets.

use anyhow::{anyhow, bail, Context, Result};
use glam::Mat4;
use std::collections::HashMap;
use std::path::Path;

use re_core::animation::{Animation, Channel, ChannelPath, Interpolation, KeyValue, Target};
use re_core::id::{MaterialId, MeshId, NodeId, SkeletonId};
use re_core::mesh::{Mesh as WMesh, Skin};
use re_core::project::Project;
use re_core::scene::{Node as WNode, NodeKind};
use re_core::skeleton::Skeleton;
use re_core::transform::Transform;

pub(crate) fn import_gltf(path: &Path) -> Result<Project> {
    let (document, buffers, _images) = gltf::import(path)
        .with_context(|| format!("open {}", path.display()))?;
    let blob = |buffer: gltf::Buffer| buffers.get(buffer.index()).map(|b| &b.0[..]);

    let mut p = Project::new(
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("imported"),
    );
    let mut imp = Importer::default();

    // Pass 1: node tree + mesh/material assets.
    if let Some(scene) = document.default_scene().or_else(|| document.scenes().next()) {
        for node in scene.nodes() {
            imp.convert_node(blob, node, None, &mut p, 0)?;
        }
    } else {
        for node in document.nodes() {
            if node_is_root(&document, node.index()) {
                imp.convert_node(blob, node, None, &mut p, 0)?;
            }
        }
    }

    // Pass 2: skins -> skeleton assets + instances.
    imp.build_skins(&document, &mut p)?;
    // Pass 3: attach skin weights to imported meshes.
    imp.attach_skins(&mut p);
    // Pass 4: animations.
    imp.build_animations(&document, blob, &mut p)?;

    Ok(p)
}

fn node_is_root(document: &gltf::Document, idx: usize) -> bool {
    !document
        .nodes()
        .any(|n| n.children().any(|c| c.index() == idx))
}

#[derive(Default)]
struct Importer {
    our_node: HashMap<usize, NodeId>,
    world_of: HashMap<usize, Transform>,
    parent_gltf: HashMap<usize, usize>,
    /// gltf mesh index -> MeshIds created for its triangle primitives
    mesh_assets: HashMap<usize, Vec<MeshId>>,
    /// MeshId -> raw JOINTS_0/WEIGHTS_0 (indices into skin.joints[])
    raw_skin_data: HashMap<MeshId, (Vec<[u16; 4]>, Vec<[f32; 4]>)>,
    /// (our mesh node, gltf skin index) for nodes referencing a skin
    node_skins: Vec<(MeshId, NodeId, usize)>,
    /// gltf skin index -> (skeleton asset, instance node, gltf joint idx -> our joint idx)
    skin_map: HashMap<usize, (SkeletonId, NodeId, HashMap<usize, usize>)>,
    /// gltf joint node index -> (skin index, our joint index)
    joints_of_node: HashMap<usize, (usize, usize)>,
    material_of: HashMap<usize, MaterialId>,
}

impl Importer {
    fn material(&mut self, p: &mut Project, m: gltf::Material) -> MaterialId {
        let key = m.index();
        if let Some(i) = key {
            if let Some(id) = self.material_of.get(&i) {
                return *id;
            }
        }
        let pbr = m.pbr_metallic_roughness();
        // Read texture references (by URI if available).
        let albedo_tex = pbr.base_color_texture().and_then(|t| {
            let img = t.texture().source();
            match img.source() {
                gltf::image::Source::Uri { uri, .. } => Some(uri.to_string()),
                _ => None,
            }
        });
        let normal_tex = m.normal_texture().and_then(|t| {
            let img = t.texture().source();
            match img.source() {
                gltf::image::Source::Uri { uri, .. } => Some(uri.to_string()),
                _ => None,
            }
        });
        let mat = re_core::material::Material {
            name: m.name().unwrap_or("material").to_string(),
            base_color: pbr.base_color_factor(),
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            emissive: m.emissive_factor(),
            double_sided: m.double_sided(),
            albedo_texture_id: albedo_tex,
            normal_texture_id: normal_tex,
        };
        let id = p.library.add_material(mat);
        if let Some(i) = key {
            self.material_of.insert(i, id);
        }
        id
    }

    fn node_transform(node: &gltf::scene::Node) -> Transform {
        match node.transform() {
            gltf::scene::Transform::Matrix { matrix } => {
                let m = Mat4::from_cols_array_2d(&matrix);
                Transform::from_mat4(&m).unwrap_or_default()
            }
            gltf::scene::Transform::Decomposed {
                translation,
                rotation,
                scale,
            } => Transform {
                translation,
                rotation,
                scale,
            },
        }
    }

    /// Convert one glTF node and its subtree; returns our node id.
    /// Depth-capped: a crafted file may contain a node cycle, which would
    /// recurse forever here (the `gltf` crate does not check acyclicity).
    fn convert_node<'s, F>(
        &mut self,
        blob: F,
        node: gltf::scene::Node,
        parent: Option<NodeId>,
        p: &mut Project,
        depth: u32,
    ) -> Result<NodeId>
    where
        F: Copy + for<'a> Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
    {
        if depth > 128 {
            bail!("node hierarchy deeper than 128 (cycle in file?)");
        }
        let gidx = node.index();
        let local = Self::node_transform(&node);
        let world = match parent {
            Some(pn) => p.scene.world_transform(pn).then(&local),
            None => local,
        };
        self.world_of.insert(gidx, world);
        let name = node.name().unwrap_or("node").to_string();

        // Mesh assets (cached per glTF mesh, since meshes can be instanced).
        let mut prim_assets: Vec<(MeshId, Option<MaterialId>)> = Vec::new();
        if let Some(mesh) = node.mesh() {
            let midx = mesh.index();
            if !self.mesh_assets.contains_key(&midx) {
                let mut created = Vec::new();
                for primitive in mesh.primitives() {
                    if primitive.mode() != gltf::mesh::Mode::Triangles {
                        continue; // lines/points later
                    }
                    let reader = primitive.reader(blob);
                    let positions: Vec<[f32; 3]> = reader
                        .read_positions()
                        .ok_or_else(|| anyhow!("primitive without POSITION"))?
                        .collect();
                    if positions.is_empty() {
                        continue;
                    }
                    let normals: Option<Vec<[f32; 3]>> = reader.read_normals().map(|i| i.collect());
                    let uvs: Option<Vec<[f32; 2]>> =
                        reader.read_tex_coords(0).map(|i| i.into_f32().collect());
                    let indices: Option<Vec<u32>> =
                        reader.read_indices().map(|i| i.into_u32().collect());
                    let mut wmesh = WMesh {
                        name: mesh.name().unwrap_or("mesh").to_string(),
                        positions,
                        normals,
                        uvs,
                        indices,
                        ..Default::default()
                    };
                    if wmesh.normals.is_none() {
                        wmesh.compute_normals();
                    }
                    let mid = p.library.add_mesh(wmesh);
                    if let (Some(j), Some(w)) = (
                        reader.read_joints(0).map(|i| i.into_u16().collect::<Vec<[u16; 4]>>()),
                        reader.read_weights(0).map(|i| i.into_f32().collect::<Vec<[f32; 4]>>()),
                    ) {
                        if j.len() == w.len() && !j.is_empty() {
                            self.raw_skin_data.insert(mid, (j, w));
                        }
                    }
                    created.push(mid);
                }
                self.mesh_assets.insert(midx, created);
            }
            let cached = self.mesh_assets[&midx].clone();
            // materials per triangle primitive (same filter order as creation)
            let mats: Vec<MaterialId> = mesh
                .primitives()
                .filter(|prm| prm.mode() == gltf::mesh::Mode::Triangles)
                .map(|prm| self.material(p, prm.material()))
                .collect();
            for (i, mid) in cached.iter().enumerate() {
                prim_assets.push((*mid, mats.get(i).copied()));
            }
        }

        // Our node: first primitive here, additional primitives as children.
        let kind = match prim_assets.first() {
            Some((mid, mat)) => NodeKind::MeshInstance {
                mesh: *mid,
                material: *mat,
                skeleton_node: None,
            },
            None => NodeKind::Empty,
        };
        let node_id = p.scene.insert(
            parent,
            WNode {
                name: name.clone(),
                transform: local,
                kind,
                ..Default::default()
            },
        );
        self.our_node.insert(gidx, node_id);

        for (mid, mat) in prim_assets.iter().skip(1) {
            p.scene.insert(
                Some(node_id),
                WNode {
                    name: format!("{name} (prim)"),
                    kind: NodeKind::MeshInstance {
                        mesh: *mid,
                        material: *mat,
                        skeleton_node: None,
                    },
                    ..Default::default()
                },
            );
        }

        if let Some(skin) = node.skin() {
            if let Some((mid, _)) = prim_assets.first() {
                self.node_skins.push((*mid, node_id, skin.index()));
            }
        }

        for child in node.children() {
            self.parent_gltf.insert(child.index(), gidx);
            self.convert_node(blob, child, Some(node_id), p, depth + 1)?;
        }
        Ok(node_id)
    }

    // -------------------------------------------------------------- skins

    fn build_skins(&mut self, document: &gltf::Document, p: &mut Project) -> Result<()> {
        for skin in document.skins() {
            let si = skin.index();
            let joint_nodes: Vec<(usize, Option<String>)> = skin
                .joints()
                .map(|n| (n.index(), n.name().map(|s| s.to_string())))
                .collect();
            if joint_nodes.is_empty() {
                continue;
            }

            // instance attachment: parent of the skin root node
            let base = match skin.skeleton() {
                Some(n) => (n.index(), n.name().map(|s| s.to_string())),
                None => joint_nodes[0].clone(),
            };
            let base_parent = self.parent_gltf.get(&base.0).copied();
            let instance_parent = base_parent.and_then(|bp| self.our_node.get(&bp).copied());
            let base_world_inv = base_parent
                .and_then(|bp| self.world_of.get(&bp).cloned())
                .map(|t| t.to_mat4())
                .unwrap_or(Mat4::IDENTITY)
                .inverse();

            let gid_to_ji: HashMap<usize, usize> = joint_nodes
                .iter()
                .enumerate()
                .map(|(i, (gidx, _))| (*gidx, i))
                .collect();

            let mut skel = Skeleton::new(skin.name().unwrap_or("imported skin").to_string());
            for (ji, (gidx, jname)) in joint_nodes.iter().enumerate() {
                let gidx = *gidx;
                // nearest joint ancestor defines the hierarchy parent
                let mut anc = self.parent_gltf.get(&gidx).copied();
                let mut parent_ji = None;
                let mut anc_world = base_world_inv;
                while let Some(a) = anc {
                    if a != gidx {
                        if let Some(&pai) = gid_to_ji.get(&a) {
                            parent_ji = Some(pai);
                            anc_world = self
                                .world_of
                                .get(&a)
                                .map(|t| t.to_mat4())
                                .unwrap_or(Mat4::IDENTITY)
                                .inverse();
                            break;
                        }
                        anc = self.parent_gltf.get(&a).copied();
                    } else {
                        break;
                    }
                }
                let world = self
                    .world_of
                    .get(&gidx)
                    .map(|t| t.to_mat4())
                    .unwrap_or(Mat4::IDENTITY);
                let local = Transform::from_mat4(&(anc_world * world)).unwrap_or_default();
                let jname = jname
                    .clone()
                    .unwrap_or_else(|| format!("joint_{ji}"));
                skel.add_joint(jname, parent_ji, local);
                self.joints_of_node.insert(gidx, (si, ji));
            }

            let skel_id = p.library.add_skeleton(skel);
            let inst = p.scene.insert(
                instance_parent,
                WNode {
                    name: format!("{} (rig)", base.1.as_deref().unwrap_or("skin")),
                    kind: NodeKind::SkeletonInstance { skeleton: skel_id },
                    ..Default::default()
                },
            );

            // hide original joint nodes; their transform now lives in the skeleton
            for (gidx, _) in &joint_nodes {
                if let Some(&nid) = self.our_node.get(gidx) {
                    if let Some(n) = p.scene.get_mut(nid) {
                        n.visible = false;
                    }
                }
            }

            self.skin_map.insert(si, (skel_id, inst, gid_to_ji));
        }
        Ok(())
    }

    fn attach_skins(&mut self, p: &mut Project) {
        let entries = std::mem::take(&mut self.node_skins);
        for (mid, node_id, si) in entries {
            let Some((skel_id, inst, _)) = self.skin_map.get(&si).cloned() else {
                continue;
            };
            // raw skin data may live on a shared mesh asset (several nodes with
            // different skins): give this node a private copy in that case.
            let raw = match self.raw_skin_data.get(&mid) {
                Some(r) => r.clone(),
                None => continue,
            };
            let mut target_mesh = mid;
            let already_used = p
                .library
                .meshes
                .get(&mid)
                .and_then(|m| m.skin.as_ref())
                .is_some();
            if already_used {
                match p.library.duplicate_mesh(mid) {
                    Ok(new_id) => {
                        target_mesh = new_id;
                        if let Some(NodeKind::MeshInstance { mesh, .. }) =
                            p.scene.get_mut(node_id).map(|n| &mut n.kind)
                        {
                            *mesh = new_id;
                        }
                    }
                    Err(_) => continue,
                }
            }
            let (joints, weights) = raw;
            // JOINTS_0 indexes skin.joints[]; our joints were created in that
            // same order, so indices map 1:1. Normalize weights for safety.
            let w: Vec<[f32; 4]> = weights
                .iter()
                .map(|w| {
                    let sum: f32 = w.iter().sum();
                    if sum > 1e-9 {
                        [w[0] / sum, w[1] / sum, w[2] / sum, w[3] / sum]
                    } else {
                        [1.0, 0.0, 0.0, 0.0]
                    }
                })
                .collect();
            if let Some(m) = p.library.meshes.get_mut(&target_mesh) {
                m.skin = Some(Skin {
                    skeleton: skel_id,
                    joints,
                    weights: w,
                });
            }
            if let Some(NodeKind::MeshInstance { skeleton_node, .. }) =
                p.scene.get_mut(node_id).map(|n| &mut n.kind)
            {
                *skeleton_node = Some(inst);
            }
        }
    }

    // --------------------------------------------------------- animations

    fn build_animations<'s, F>(
        &mut self,
        document: &gltf::Document,
        blob: F,
        p: &mut Project,
    ) -> Result<()>
    where
        F: Copy + for<'a> Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
    {
        for anim in document.animations() {
            let mut clip = Animation::new(anim.name().unwrap_or("animation"));
            for channel in anim.channels() {
                let t = channel.target();
                let path = match t.property() {
                    gltf::animation::Property::Translation => ChannelPath::Translation,
                    gltf::animation::Property::Rotation => ChannelPath::Rotation,
                    gltf::animation::Property::Scale => ChannelPath::Scale,
                    gltf::animation::Property::MorphTargetWeights => continue,
                };
                let interp = match channel.sampler().interpolation() {
                    gltf::animation::Interpolation::Step => Interpolation::Step,
                    gltf::animation::Interpolation::CubicSpline => Interpolation::Cubic,
                    _ => Interpolation::Linear,
                };
                let cubic = matches!(interp, Interpolation::Cubic);

                let reader = channel.reader(blob);
                let Some(inputs) = reader.read_inputs() else {
                    continue;
                };
                let Some(outputs) = reader.read_outputs() else {
                    continue;
                };
                let times: Vec<f32> = inputs.collect();
                if times.is_empty() {
                    continue;
                }
                let t0 = times.iter().cloned().fold(f32::INFINITY, f32::min);
                // CUBICSPLINE: [in-tangent, value, out-tangent] per keyframe.
                let stride = if cubic { 3 } else { 1 };
                let offset = if cubic { 1 } else { 0 };

                let mut ch = Channel::new(Target::Node(NodeId(0)), path);
                ch.interpolation = interp;
                match outputs {
                    gltf::animation::util::ReadOutputs::Translations(it) => {
                        let vals: Vec<[f32; 3]> = it.collect();
                        collect_keys(&mut ch, &times, t0, stride, offset, vals, KeyValue::Vec3);
                    }
                    gltf::animation::util::ReadOutputs::Scales(it) => {
                        let vals: Vec<[f32; 3]> = it.collect();
                        collect_keys(&mut ch, &times, t0, stride, offset, vals, KeyValue::Vec3);
                    }
                    gltf::animation::util::ReadOutputs::Rotations(it) => {
                        let vals: Vec<[f32; 4]> = it.into_f32().collect();
                        collect_keys(&mut ch, &times, t0, stride, offset, vals, KeyValue::Quat);
                    }
                    gltf::animation::util::ReadOutputs::MorphTargetWeights(_) => continue,
                }
                if ch.keys.is_empty() {
                    continue;
                }

                ch.target = match self.joints_of_node.get(&t.node().index()) {
                    Some((si, ji)) => {
                        let (skel_id, _, _) = self.skin_map[si];
                        let name = p
                            .library
                            .skeletons
                            .get(&skel_id)
                            .and_then(|s| s.joints.get(*ji))
                            .map(|j| j.name.clone())
                            .unwrap_or_else(|| format!("joint_{ji}"));
                        Target::Joint(name)
                    }
                    None => match self.our_node.get(&t.node().index()) {
                        Some(n) => Target::Node(*n),
                        None => continue,
                    },
                };
                clip.channels.push(ch);
            }
            if !clip.channels.is_empty() {
                p.library.add_animation(clip);
            }
        }
        Ok(())
    }
}

fn collect_keys<T>(
    ch: &mut Channel,
    times: &[f32],
    t0: f32,
    stride: usize,
    offset: usize,
    values: Vec<T>,
    wrap: fn(T) -> KeyValue,
) {
    for (i, v) in values.into_iter().enumerate() {
        if i % stride != offset {
            continue;
        }
        let ti = i / stride;
        if let Some(&t) = times.get(ti) {
            ch.set_key((t - t0).max(0.0), wrap(v));
        }
    }
}
