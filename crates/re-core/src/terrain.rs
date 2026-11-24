//! Heightfield-style terrain: a subdivided grid mesh in the XZ plane whose
//! vertex heights (local Y) are edited by radial brush operations.
//!
//! A terrain is a plain `MeshInstance` node, so rendering, undo, saving and
//! glTF export work without any special cases. The brush functions operate
//! on mesh-local positions; callers with transformed nodes must convert the
//! brush center into local space first.

use crate::id::{MaterialId, MeshId, NodeId};
use crate::material::Material;
use crate::mesh::Mesh;
use crate::ops;
use crate::project::Project;
use crate::transform::Transform;

/// Smooth falloff 1.0 at the brush center, 0.0 at (and beyond) the radius.
pub fn falloff(dist: f32, radius: f32) -> f32 {
    let t = (dist / radius.max(1e-6)).clamp(0.0, 1.0);
    let w = 1.0 - t;
    w * w * (3.0 - 2.0 * w)
}

/// How a brush stroke changes the heights inside its radius.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BrushMode {
    /// Push heights up along +Y (hills, mountains).
    Raise,
    /// Pull heights down along -Y (trenches, river beds).
    Lower,
    /// Pull heights towards the disk average (smoothing).
    Smooth,
    /// Move heights towards a fixed target plane (terraces, plateaus).
    /// The target is the Y of the brush center.
    Flatten,
}

impl BrushMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "raise" | "anheben" | "up" => Self::Raise,
            "lower" | "absenken" | "down" | "dig" => Self::Lower,
            "smooth" | "glaetten" | "glätten" => Self::Smooth,
            "flatten" | "abflachen" | "planieren" => Self::Flatten,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Raise => "raise",
            Self::Lower => "lower",
            Self::Smooth => "smooth",
            Self::Flatten => "flatten",
        }
    }
}

/// Subdivided terrain grid centred on the origin, lying on the XZ plane at
/// y = 0 with normals pointing up. Shared vertices (indexed), so brushes
/// deform the surface without seams.
pub fn grid_mesh(name: &str, size_x: f32, size_z: f32, segments: u32) -> Mesh {
    let seg = segments.clamp(1, 512) as usize;
    let (hx, hz) = (size_x.max(0.01) / 2.0, size_z.max(0.01) / 2.0);
    let mut mesh = Mesh {
        name: name.to_string(),
        ..Default::default()
    };
    for z in 0..=seg {
        for x in 0..=seg {
            let fx = x as f32 / seg as f32;
            let fz = z as f32 / seg as f32;
            mesh.positions.push([-hx + fx * 2.0 * hx, 0.0, -hz + fz * 2.0 * hz]);
        }
    }
    let stride = seg + 1;
    let mut idx = Vec::with_capacity(seg * seg * 6);
    for z in 0..seg {
        for x in 0..seg {
            let a = (z * stride + x) as u32;
            let b = a + 1;
            let c = a + stride as u32;
            let d = c + 1;
            // winding chosen so the face normal points +Y
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    mesh.indices = Some(idx);
    mesh.compute_normals();
    mesh
}

/// Flat quad on the XZ plane at the given height, for a water surface.
pub fn water_mesh(name: &str, size_x: f32, size_z: f32) -> Mesh {
    let (hx, hz) = (size_x.max(0.01) / 2.0, size_z.max(0.01) / 2.0);
    let mut mesh = Mesh {
        name: name.to_string(),
        positions: vec![
            [-hx, 0.0, -hz],
            [hx, 0.0, -hz],
            [hx, 0.0, hz],
            [-hx, 0.0, hz],
        ],
        normals: Some(vec![[0.0, 1.0, 0.0]; 4]),
        uvs: Some(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
        indices: Some(vec![0, 2, 1, 0, 3, 2]), // +Y facing, but double-sided anyway
        skin: None,
    };
    mesh.compute_normals();
    mesh
}

/// Apply one brush step to a terrain grid mesh in mesh-local space.
///
/// `center` is the local-space brush position (its Y is the flatten target,
/// distances are measured in XZ). `amount` is the displacement in meters at
/// full falloff (raise/lower) or the blend factor 0..1 (smooth/flatten).
/// Returns the number of vertices touched; normals are recomputed on change.
pub fn brush(
    mesh: &mut Mesh,
    center: [f32; 3],
    radius: f32,
    amount: f32,
    mode: BrushMode,
) -> usize {
    let r = radius.max(0.001);
    let touched: Vec<(usize, f32)> = mesh
        .positions
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let dx = p[0] - center[0];
            let dz = p[2] - center[2];
            let d = (dx * dx + dz * dz).sqrt();
            let w = falloff(d, r);
            (w > 0.0).then_some((i, w))
        })
        .collect();
    if touched.is_empty() {
        return 0;
    }
    match mode {
        BrushMode::Raise | BrushMode::Lower => {
            let sign = if mode == BrushMode::Raise { 1.0 } else { -1.0 };
            for (i, w) in &touched {
                mesh.positions[*i][1] += sign * amount * w;
            }
        }
        BrushMode::Smooth => {
            let mut acc = 0.0;
            let mut wsum = 0.0;
            for (i, w) in &touched {
                acc += mesh.positions[*i][1] * w;
                wsum += w;
            }
            let avg = if wsum > 1e-6 { acc / wsum } else { 0.0 };
            let k = amount.clamp(0.0, 1.0);
            for (i, w) in &touched {
                let y = mesh.positions[*i][1];
                mesh.positions[*i][1] = y + (avg - y) * k * w;
            }
        }
        BrushMode::Flatten => {
            let target = center[1];
            let k = amount.clamp(0.0, 1.0);
            for (i, w) in &touched {
                let y = mesh.positions[*i][1];
                mesh.positions[*i][1] = y + (target - y) * k * w;
            }
        }
    }
    mesh.compute_normals();
    touched.len()
}

/// Find or create a named material so repeated calls don't pile up duplicates.
pub fn material_by_name(p: &mut Project, name: &str, make: impl FnOnce() -> Material) -> MaterialId {
    if let Some((id, _)) = p.library.materials.iter().find(|(_, m)| m.name == name) {
        return *id;
    }
    p.library.add_material(make())
}

/// Ground material for freshly created maps.
pub fn soil_material() -> Material {
    let mut m = Material::colored("Boden", [0.36, 0.45, 0.26, 1.0]);
    m.roughness = 0.95;
    m
}

/// Translucent blue material for water surfaces.
pub fn water_material() -> Material {
    let mut m = Material::colored("Wasser", [0.10, 0.35, 0.60, 0.45]);
    m.roughness = 0.15;
    m.double_sided = true;
    m
}

/// Create a terrain grid node with the soil material attached.
pub fn add_terrain(
    p: &mut Project,
    parent: Option<NodeId>,
    name: &str,
    size_x: f32,
    size_z: f32,
    segments: u32,
    transform: Transform,
) -> (NodeId, MeshId) {
    let mesh = grid_mesh(name, size_x, size_z, segments);
    let mesh_id = p.library.add_mesh(mesh);
    let soil = material_by_name(p, "Boden", soil_material);
    let node = ops::add_node(
        p,
        parent,
        name,
        transform,
        crate::scene::NodeKind::MeshInstance {
            mesh: mesh_id,
            material: Some(soil),
            skeleton_node: None,
        },
    );
    (node, mesh_id)
}

/// Create a translucent water surface node at world height `height`.
pub fn add_water(
    p: &mut Project,
    parent: Option<NodeId>,
    name: &str,
    size_x: f32,
    size_z: f32,
    height: f32,
) -> (NodeId, MeshId, MaterialId) {
    let mesh = water_mesh(name, size_x, size_z);
    let mesh_id = p.library.add_mesh(mesh);
    let mat = material_by_name(p, "Wasser", water_material);
    let node = ops::add_node(
        p,
        parent,
        name,
        Transform {
            translation: [0.0, height, 0.0],
            ..Default::default()
        },
        crate::scene::NodeKind::MeshInstance {
            mesh: mesh_id,
            material: Some(mat),
            skeleton_node: None,
        },
    );
    // a still lake looks frozen — give the surface a gentle ripple by default
    if let Some(n) = p.scene.get_mut(node) {
        n.motion = crate::motion::Motion::Waves {
            amp: 0.05,
            speed: 1.1,
        };
    }
    (node, mesh_id, mat)
}

/// Detect a grid produced by [`grid_mesh`]: `(s+1)²` shared vertices on a
/// regular XZ lattice plus `s²·6` indices in the builder's winding pattern.
/// Returns the segment count `s`.
pub fn grid_segments(mesh: &Mesh) -> Option<usize> {
    let n = mesh.positions.len();
    if n < 4 {
        return None;
    }
    let side = (n as f64).sqrt();
    let s = side.round() as usize;
    if s * s != n || s < 2 {
        return None;
    }
    let seg = s - 1;
    let idx = mesh.indices.as_ref()?;
    if idx.len() != seg * seg * 6 {
        return None;
    }
    let stride = s as u32;
    // first two triangles of row 0: [0, stride, 1] then [1, stride, stride+1]
    if idx[0..3] != [0, stride, 1] {
        return None;
    }
    let (min, max) = mesh.aabb();
    let dx = (max[0] - min[0]) / seg as f32;
    let dz = (max[2] - min[2]) / seg as f32;
    if dx <= 1e-6 || dz <= 1e-6 {
        return None;
    }
    for z in 0..s {
        for x in 0..s {
            let p = &mesh.positions[z * s + x];
            if (p[0] - (min[0] + x as f32 * dx)).abs() > dx * 0.02 {
                return None;
            }
            if (p[2] - (min[2] + z as f32 * dz)).abs() > dz * 0.02 {
                return None;
            }
        }
    }
    Some(seg)
}

/// Resample a terrain grid to `new_segments` per axis by bilinearly sampling
/// the height field; XZ extents are preserved, normals recomputed.
/// Returns false when the mesh is not a [`grid_mesh`] lattice.
pub fn resample_grid(mesh: &mut Mesh, new_segments: usize) -> bool {
    let Some(s_old) = grid_segments(mesh) else {
        return false;
    };
    if new_segments < 1 || new_segments > 512 {
        return false;
    }
    let (min, max) = mesh.aabb();
    let (sx, sz) = (max[0] - min[0], max[2] - min[2]);
    let stride = s_old + 1;
    let h = |x: usize, z: usize| -> f32 { mesh.positions[z * stride + x][1] };
    let mut out = grid_mesh(&mesh.name, sx, sz, new_segments as u32);
    let n = new_segments;
    for z in 0..=n {
        for x in 0..=n {
            let gx = x as f32 * s_old as f32 / n as f32;
            let gz = z as f32 * s_old as f32 / n as f32;
            let x0 = gx.floor() as usize;
            let z0 = gz.floor() as usize;
            let x1 = (x0 + 1).min(s_old);
            let z1 = (z0 + 1).min(s_old);
            let fx = gx - x0 as f32;
            let fz = gz - z0 as f32;
            let y = h(x0, z0) * (1.0 - fx) * (1.0 - fz)
                + h(x1, z0) * fx * (1.0 - fz)
                + h(x0, z1) * (1.0 - fx) * fz
                + h(x1, z1) * fx * fz;
            out.positions[z * (n + 1) + x][1] = y;
        }
    }
    out.compute_normals();
    *mesh = out;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_topology_is_a_closed_seamless_surface() {
        let m = grid_mesh("t", 10.0, 10.0, 8);
        assert_eq!(m.positions.len(), 9 * 9);
        assert_eq!(m.triangle_count(), 8 * 8 * 2);
        // all vertices start on y = 0, normals point up
        assert!(m.positions.iter().all(|p| p[1].abs() < 1e-6));
        let n = m.normals.clone().unwrap();
        assert!(n.iter().all(|v| v[1] > 0.99));
    }

    #[test]
    fn raise_and_lower_respect_the_brush_radius() {
        let mut m = grid_mesh("t", 10.0, 10.0, 10);
        let hit = brush(&mut m, [0.0, 0.0, 0.0], 2.0, 1.0, BrushMode::Raise);
        assert!(hit > 0);
        // center pushed up the most, far corners untouched
        let y_at = |m: &Mesh, x: usize, z: usize| m.positions[z * 11 + x][1];
        assert!(y_at(&m, 5, 5) > 0.99);
        assert!(y_at(&m, 0, 0).abs() < 1e-6);
        // a monotonic profile from center to edge
        assert!(y_at(&m, 4, 5) > y_at(&m, 3, 5));
        brush(&mut m, [0.0, 0.0, 0.0], 2.0, 0.5, BrushMode::Lower);
        assert!(y_at(&m, 5, 5) < 0.6 && y_at(&m, 5, 5) > 0.0);
    }

    #[test]
    fn flatten_moves_heights_to_the_target_plane() {
        let mut m = grid_mesh("t", 10.0, 10.0, 8);
        brush(&mut m, [0.0, 0.0, 0.0], 8.0, 2.0, BrushMode::Raise);
        let center = 4 * 9 + 4; // stride is segments+1
        assert!(m.positions[center][1] > 1.9, "dome under the brush");
        let far_before = m.positions[center + 4][1]; // world x = 5 m, outside r = 4
        assert!(far_before > 0.0);
        brush(&mut m, [0.0, 0.25, 0.0], 4.0, 1.0, BrushMode::Flatten);
        // dead center (falloff 1, blend 1) lands exactly on the target plane
        assert!((m.positions[center][1] - 0.25).abs() < 1e-5);
        // towards the rim the pull is only partial
        let mid = &m.positions[center + 2];
        assert!(mid[1] > 0.25 && mid[1] < 1.9, "partial blend, got {}", mid[1]);
        // outside the brush radius nothing changes
        assert!((m.positions[center + 4][1] - far_before).abs() < 1e-6);
    }

    #[test]
    fn smooth_reduces_variance() {
        let mut m = grid_mesh("t", 10.0, 10.0, 8);
        brush(&mut m, [0.0, 0.0, 0.0], 2.0, 2.0, BrushMode::Raise);
        let variance = |m: &Mesh| {
            let ys: Vec<f32> = m.positions.iter().map(|p| p[1]).collect();
            let mean = ys.iter().sum::<f32>() / ys.len() as f32;
            ys.iter().map(|y| (y - mean).powi(2)).sum::<f32>()
        };
        let before = variance(&m);
        brush(&mut m, [0.0, 0.0, 0.0], 4.0, 1.0, BrushMode::Smooth);
        assert!(variance(&m) < before);
    }

    #[test]
    fn terrain_and_water_nodes_roundtrip_through_json() {
        let mut p = Project::new("welt");
        let (node, mesh) = add_terrain(&mut p, None, "Karte", 30.0, 30.0, 16, Transform::default());
        assert!(matches!(
            p.scene.get(node).unwrap().kind,
            crate::scene::NodeKind::MeshInstance { .. }
        ));
        // brush through the asset, like the GUI does
        brush(p.library.meshes.get_mut(&mesh).unwrap(), [1.0, 0.0, 1.0], 3.0, 0.5, BrushMode::Raise);
        let (wnode, _, wmat) = add_water(&mut p, None, "Wasser", 30.0, 30.0, -0.2);
        assert!(p.library.materials[&wmat].base_color[3] < 1.0, "water must be translucent");
        assert_eq!(p.scene.get(wnode).unwrap().transform.translation[1], -0.2);

        let json = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn materials_are_reused_not_duplicated() {
        let mut p = Project::new("welt");
        add_terrain(&mut p, None, "a", 10.0, 10.0, 4, Transform::default());
        add_terrain(&mut p, None, "b", 10.0, 10.0, 4, Transform::default());
        add_water(&mut p, None, "w", 10.0, 10.0, 0.0);
        assert_eq!(p.library.materials.len(), 2, "one Boden + one Wasser");
    }

    #[test]
    fn grid_detection_and_resample_keep_the_heights() {
        let mut m = grid_mesh("t", 10.0, 10.0, 16);
        assert_eq!(grid_segments(&m), Some(16));
        brush(&mut m, [0.0, 0.0, 0.0], 3.0, 2.0, BrushMode::Raise);
        let peak_before = m.positions.iter().map(|p| p[1]).fold(0.0f32, f32::max);

        assert!(resample_grid(&mut m, 8));
        assert_eq!(grid_segments(&m), Some(8));
        assert_eq!(m.triangle_count(), 8 * 8 * 2);
        let peak_after = m.positions.iter().map(|p| p[1]).fold(0.0f32, f32::max);
        // bilinear coarsening keeps most of the dome
        assert!(
            peak_after > peak_before * 0.7,
            "{peak_after} vs {peak_before}"
        );
        // extents preserved
        let (min, max) = m.aabb();
        assert!((max[0] - min[0] - 10.0).abs() < 1e-4);

        // a non-grid mesh is refused
        let mut boxy = crate::primitives::generate(crate::primitives::PrimitiveKind::Box, 1.0, 1.0, 4);
        boxy.compute_normals();
        assert_eq!(grid_segments(&boxy), None);
        assert!(!resample_grid(&mut boxy, 4));
    }
}
