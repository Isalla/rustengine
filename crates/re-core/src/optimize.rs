//! Mesh and asset optimization for low-budget targets (Raspberry Pi, sprites).
//!
//! - [`cluster_collapse`]: polygon reduction by vertex clustering (snap to a
//!   grid, merge, drop degenerate/duplicate triangles). Robust on any mesh;
//!   stylized results, no topology bookkeeping needed.
//! - [`decimate_to_ratio`]: pick the cluster cell automatically to hit a
//!   target fraction of the original triangles.
//! - [`merge_materials`]: merge near-identical materials so a scene shares
//!   fewer PBR states (WorldBuilder has no bitmap textures, so this is the
//!   equivalent of "texture reduction"; UV channels can be dropped entirely
//!   with [`remove_uvs`]).

use std::collections::{HashMap, HashSet};

use anyhow::{bail, Context, Result};
use glam::Vec3;

use crate::id::{MaterialId, MeshId, NodeId};
use crate::material::Material;
use crate::mesh::Mesh;
use crate::project::Project;
use crate::scene::NodeKind;
use crate::terrain;

/// Snap all vertices onto a grid of `cell` meters, merge each cluster to its
/// centroid and rebuild the triangle list. Degenerate (collapsed) and
/// duplicate triangles are dropped; UVs are averaged per cluster; skin data
/// is removed (callers should refuse skinned meshes explicitly).
///
/// Returns the new triangle count.
pub fn cluster_collapse(mesh: &mut Mesh, cell: f32) -> usize {
    let cell = cell.max(1e-6);
    if mesh.positions.is_empty() {
        return 0;
    }
    let uv_source = mesh.uvs.clone().unwrap_or_default();
    let has_uv = mesh.uvs.as_ref().map(|u| u.len() == mesh.positions.len()).unwrap_or(false);

    let mut cell_of: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut sums: Vec<(Vec3, u32, [f32; 2])> = Vec::new();
    let mut remap = vec![u32::MAX; mesh.positions.len()];
    for (i, p) in mesh.positions.iter().enumerate() {
        let key = (
            (p[0] / cell).floor() as i64,
            (p[1] / cell).floor() as i64,
            (p[2] / cell).floor() as i64,
        );
        let idx = *cell_of.entry(key).or_insert_with(|| {
            sums.push((Vec3::ZERO, 0, [0.0; 2]));
            sums.len() - 1
        });
        remap[i] = idx as u32;
        let s = &mut sums[idx];
        s.0 += Vec3::from_array(*p);
        s.1 += 1;
        if has_uv && i < uv_source.len() {
            s.2[0] += uv_source[i][0];
            s.2[1] += uv_source[i][1];
        }
    }

    let mut out_idx: Vec<u32> = Vec::new();
    let mut seen: HashSet<[u32; 3]> = HashSet::new();
    for tri in mesh.iter_triangles() {
        let t = [
            remap[tri[0] as usize],
            remap[tri[1] as usize],
            remap[tri[2] as usize],
        ];
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            continue; // collapsed
        }
        let mut key = t;
        key.sort_unstable();
        if !seen.insert(key) {
            continue; // duplicate face
        }
        out_idx.extend_from_slice(&t);
    }

    mesh.positions = sums
        .iter()
        .map(|(s, n, _)| (Vec3::from_array([0.0; 3]) + *s)
            .to_array()
            .map(|v| v / *n as f32))
        .collect();
    mesh.uvs = has_uv.then(|| {
        sums.iter()
            .map(|(_, n, uv)| [uv[0] / *n as f32, uv[1] / *n as f32])
            .collect()
    });
    mesh.indices = Some(out_idx);
    mesh.skin = None;
    mesh.compute_normals();
    mesh.triangle_count()
}

/// Cluster with a cell size chosen so the result is close to `ratio` of the
/// original triangle count (bisection search). Returns (before, after).
pub fn decimate_to_ratio(mesh: &mut Mesh, ratio: f32) -> (usize, usize) {
    let before = mesh.triangle_count();
    let ratio = ratio.clamp(0.02, 1.0);
    if before == 0 || ratio >= 1.0 {
        return (before, before);
    }
    let target = ((before as f32 * ratio) as usize).max(1);
    let (min, max) = mesh.aabb();
    let diag = Vec3::new(max[0] - min[0], max[1] - min[1], max[2] - min[2]).length();
    let original = mesh.clone();

    let mut best = original.clone();
    let (mut lo, mut hi) = (0.0f32, diag);
    for _ in 0..10 {
        let mid = (lo + hi) * 0.5;
        let mut trial = original.clone();
        let n = cluster_collapse(&mut trial, mid);
        if n > target {
            lo = mid;
        } else {
            hi = mid;
            best = trial;
        }
    }
    let after = best.triangle_count();
    *mesh = best;
    mesh.touch();
    (before, after)
}

/// Drop the UV channel (unused without bitmap textures; saves memory).
/// Returns whether anything was removed.
pub fn remove_uvs(mesh: &mut Mesh) -> bool {
    let had = mesh.uvs.is_some();
    if had {
        mesh.uvs = None;
        mesh.touch();
    }
    had
}

/// Material signature used for merging: RGBA, metallic, roughness, emissive, textures.
fn signature(m: &Material) -> ([f32; 8], bool, Option<String>, Option<String>) {
    (
        [
            m.base_color[0],
            m.base_color[1],
            m.base_color[2],
            m.base_color[3],
            m.metallic,
            m.roughness,
            m.emissive[0],
            m.emissive[2],
        ],
        m.double_sided,
        m.albedo_texture_id.clone(),
        m.normal_texture_id.clone(),
    )
}

fn distance(a: &[f32; 8], b: &[f32; 8]) -> f32 {
    let mut s = 0.0;
    for i in 0..8 {
        let d = a[i] - b[i];
        s += d * d;
    }
    s.sqrt()
}

/// Merge materials whose signature distance is <= `tolerance` into the
/// lowest-id one and repoint every node and tile. `double_sided` must match
/// exactly. Returns how many materials were removed.
pub fn merge_materials(p: &mut Project, tolerance: f32) -> usize {
    let tol = tolerance.max(0.0);
    let ids: Vec<MaterialId> = p.library.materials.keys().copied().collect();
    let mut canon: Vec<(MaterialId, [f32; 8], bool, Option<String>, Option<String>)> = Vec::new();
    let mut remap: HashMap<MaterialId, MaterialId> = HashMap::new();
    for id in ids {
        let (sig, ds, albedo, normal) = signature(&p.library.materials[&id]);
        match canon.iter().find(|(_, c, cds, ca, cn)| *cds == ds && *ca == albedo && *cn == normal && distance(c, &sig) <= tol) {
            Some((keep, _, _, _, _)) => {
                remap.insert(id, *keep);
            }
            None => canon.push((id, sig, ds, albedo, normal)),
        }
    }
    if remap.is_empty() {
        return 0;
    }
    for node in p.scene.nodes.values_mut() {
        if let NodeKind::MeshInstance {
            material: Some(mid), ..
        } = &mut node.kind
        {
            if let Some(keep) = remap.get(mid) {
                *mid = *keep;
            }
        }
    }
    for ts in p.library.tilesets.values_mut() {
        for t in &mut ts.tiles {
            if let Some(mid) = &mut t.material {
                if let Some(keep) = remap.get(mid) {
                    *mid = *keep;
                }
            }
        }
    }
    let mut removed = 0;
    for id in remap.keys() {
        if p.library.materials.remove(id).is_some() {
            removed += 1;
        }
    }
    removed
}

fn resolve_mesh(p: &Project, node: NodeId) -> Result<MeshId> {
    match p.scene.get(node).map(|n| &n.kind) {
        Some(NodeKind::MeshInstance { mesh, .. }) => Ok(*mesh),
        Some(_) => bail!("node n{} is not a mesh instance", node.0),
        None => bail!("node n{} not found", node.0),
    }
}

/// Decimate one mesh asset. Terrain grids are resampled (clean lattice);
/// everything else goes through clustering. Skinned meshes are refused:
/// vertex weights would point at merged vertices. Returns (before, after).
pub fn decimate_mesh(p: &mut Project, mesh_id: MeshId, ratio: f32) -> Result<(usize, usize)> {
    let m = p
        .library
        .meshes
        .get(&mesh_id)
        .with_context(|| format!("mesh m{} not found", mesh_id.0))?;
    if m.skin.is_some() {
        bail!("skinntes Mesh kann nicht dezimiert werden (Weights würden kaputtgehen)");
    }
    let before = m.triangle_count();
    let is_grid = terrain::grid_segments(m).is_some();
    if is_grid {
        // grid: new segments from sqrt of the area ratio
        let s = terrain::grid_segments(m).unwrap();
        let new_s = ((s as f32) * ratio.clamp(0.02, 1.0).sqrt()).round() as usize;
        if new_s >= 1 && new_s < s {
            let mut m = p.library.meshes.get_mut(&mesh_id).unwrap();
            if terrain::resample_grid(&mut m, new_s) {
                return Ok((before, m.triangle_count()));
            }
        }
        if new_s >= s {
            return Ok((before, before)); // already at/below target resolution
        }
    }
    let mut m = p.library.meshes.get_mut(&mesh_id).unwrap();
    let (b, a) = decimate_to_ratio(&mut m, ratio);
    Ok((b, a))
}

/// Decimate the mesh instance under `node`. See [`decimate_mesh`].
pub fn decimate_node(p: &mut Project, node: NodeId, ratio: f32) -> Result<(usize, usize)> {
    let mesh_id = resolve_mesh(p, node)?;
    decimate_mesh(p, mesh_id, ratio)
}

/// Remove the UV channel of one mesh asset; true when it had one.
pub fn remove_uvs_mesh(p: &mut Project, mesh_id: MeshId) -> Result<bool> {
    let m = p
        .library
        .meshes
        .get_mut(&mesh_id)
        .with_context(|| format!("mesh m{} not found", mesh_id.0))?;
    Ok(remove_uvs(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{self, PrimitiveKind};
    use crate::transform::Transform;

    fn sphere(r: f32, seg: u32) -> Mesh {
        let mut m = primitives::generate(PrimitiveKind::Sphere, r, r, seg);
        m.compute_normals();
        m
    }

    #[test]
    fn clustering_reduces_triangles_without_degenerates() {
        let mut m = sphere(1.0, 16);
        let before = m.triangle_count();
        let after = cluster_collapse(&mut m, 0.35);
        assert!(after > 0 && after < before, "{after} of {before}");
        let n = m.positions.len();
        for t in m.iter_triangles() {
            assert!(
                t[0] != t[1] && t[1] != t[2] && t[0] != t[2],
                "degenerate triangle"
            );
            assert!(t.iter().all(|i| (*i as usize) < n));
        }
        // normals were recomputed and are unit length
        let ns = m.normals.clone().unwrap();
        assert_eq!(ns.len(), n);
        for nn in &ns {
            let l = Vec3::from_array(*nn).length();
            assert!((l - 1.0).abs() < 1e-3, "normal not unit: {nn:?}");
        }
    }

    #[test]
    fn ratio_search_hits_the_target() {
        let mut m = sphere(1.0, 24);
        let before = m.triangle_count();
        let (b, a) = decimate_to_ratio(&mut m, 0.25);
        assert_eq!(b, before);
        assert!(
            a <= before / 4 && a > before / 20,
            "target {} got {a}",
            before / 4
        );
    }

    #[test]
    fn uv_removal_works() {
        let mut m = primitives::generate(PrimitiveKind::Box, 1.0, 1.0, 4);
        m.uvs = Some(vec![[0.0, 0.0]; m.positions.len()]);
        assert!(remove_uvs(&mut m));
        assert!(m.uvs.is_none());
        assert!(!remove_uvs(&mut m));
    }

    #[test]
    fn merge_materials_repoints_nodes_and_tiles() {
        let mut p = Project::new("opt");
        let a = p.library.add_material(Material::colored("rot", [0.9, 0.1, 0.1, 1.0]));
        let b = p.library.add_material(Material::colored("rot2", [0.9, 0.1, 0.09, 1.0]));
        let c = p.library.add_material(Material::colored("blau", [0.1, 0.1, 0.9, 1.0]));
        let mesh = p.library.add_mesh(primitives::generate(PrimitiveKind::Box, 1.0, 1.0, 4));
        let n1 = crate::ops::add_node(
            &mut p,
            None,
            "n1",
            Transform::default(),
            NodeKind::MeshInstance {
                mesh,
                material: Some(b),
                skeleton_node: None,
            },
        );
        let ts = crate::ops::create_tileset(&mut p, "kit", 1.0, 1.0);
        crate::ops::add_tile_mesh(&mut p, ts, "t", mesh, Transform::default(), Some(b)).unwrap();

        let removed = merge_materials(&mut p, 0.05);
        assert_eq!(removed, 1, "only rot2 merges into rot");
        assert!(p.library.materials.contains_key(&a));
        assert!(p.library.materials.contains_key(&c));
        let NodeKind::MeshInstance {
            material: Some(mid), ..
        } = &p.scene.get(n1).unwrap().kind
        else {
            panic!("kind")
        };
        assert_eq!(*mid, a, "node repointed to kept material");
        assert_eq!(p.library.tilesets[&ts].tiles[0].material, Some(a));
    }

    #[test]
    fn skinned_meshes_are_refused() {
        let mut p = Project::new("opt");
        let (rig, _skel) = crate::ops::add_humanoid(&mut p, None, "rig");
        let mesh_id = match &p.scene.get(rig).unwrap().kind {
            _ => {
                // find a skinned capsule from the preset: build one manually
                let mut m = primitives::generate(PrimitiveKind::Capsule, 0.2, 1.0, 6);
                m.skin = Some(crate::mesh::Skin {
                    skeleton: crate::id::SkeletonId(0),
                    joints: vec![[0, 0, 0, 0]; m.positions.len()],
                    weights: vec![[1.0, 0.0, 0.0, 0.0]; m.positions.len()],
                });
                p.library.add_mesh(m)
            }
        };
        let err = decimate_mesh(&mut p, mesh_id, 0.5).unwrap_err();
        assert!(err.to_string().contains("skin"));
    }
}
