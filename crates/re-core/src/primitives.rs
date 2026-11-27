use crate::mesh::Mesh;
use glam::Vec3;

/// Available generated shapes. 2D shapes lie on the XY plane facing +Z.
#[derive(Copy, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PrimitiveKind {
    Box,
    Sphere,
    Cylinder,
    Cone,
    Capsule,
    Torus,
    Plane,
    Rect,
    Circle,
}

impl PrimitiveKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "box" | "cube" => Self::Box,
            "sphere" => Self::Sphere,
            "cylinder" => Self::Cylinder,
            "cone" => Self::Cone,
            "capsule" => Self::Capsule,
            "torus" => Self::Torus,
            "plane" => Self::Plane,
            "rect" | "rectangle" => Self::Rect,
            "circle" | "disc" => Self::Circle,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Box => "box",
            Self::Sphere => "sphere",
            Self::Cylinder => "cylinder",
            Self::Cone => "cone",
            Self::Capsule => "capsule",
            Self::Torus => "torus",
            Self::Plane => "plane",
            Self::Rect => "rect",
            Self::Circle => "circle",
        }
    }
}

/// Build a primitive mesh. `a`/`b` are size parameters (radius, width, height…),
/// `segments` controls tessellation.
pub fn generate(kind: PrimitiveKind, a: f32, b: f32, segments: u32) -> Mesh {
    let seg = segments.max(3);
    match kind {
        PrimitiveKind::Box => cube(a, b.max(0.001), a), // a=w, b=h; depth=w
        PrimitiveKind::Sphere => uv_sphere(a.max(0.001), seg, seg / 2),
        PrimitiveKind::Cylinder => cylinder(a.max(0.001), b.max(0.001), seg),
        PrimitiveKind::Cone => cone(a.max(0.001), b.max(0.001), seg),
        PrimitiveKind::Capsule => capsule(a.max(0.001), b.max(0.001), seg),
        PrimitiveKind::Torus => torus(a.max(0.001), (b * 0.25).max(0.001), seg, seg),
        PrimitiveKind::Plane => cube(1.0_f32.max(a), 1e-4, 1.0_f32.max(b)),
        PrimitiveKind::Rect => rect_xy(a.max(0.001), b.max(0.001)),
        PrimitiveKind::Circle => circle_xy(a.max(0.001), seg),
    }
}

/// Axis-aligned box centered at origin.
pub fn cube(w: f32, h: f32, d: f32) -> Mesh {
    let (hx, hy, hz) = (w / 2.0, h / 2.0, d / 2.0);
    let mut mesh = Mesh::default();
    // +X
    quad(&mut mesh, [[hx, -hy, -hz], [hx, hy, -hz], [hx, hy, hz], [hx, -hy, hz]], [1.0, 0.0, 0.0]);
    // -X
    quad(&mut mesh, [[-hx, -hy, hz], [-hx, hy, hz], [-hx, hy, -hz], [-hx, -hy, -hz]], [-1.0, 0.0, 0.0]);
    // +Y
    quad(&mut mesh, [[-hx, hy, -hz], [-hx, hy, hz], [hx, hy, hz], [hx, hy, -hz]], [0.0, 1.0, 0.0]);
    // -Y
    quad(&mut mesh, [[-hx, -hy, hz], [-hx, -hy, -hz], [hx, -hy, -hz], [hx, -hy, hz]], [0.0, -1.0, 0.0]);
    // +Z
    quad(&mut mesh, [[-hx, -hy, hz], [hx, -hy, hz], [hx, hy, hz], [-hx, hy, hz]], [0.0, 0.0, 1.0]);
    // -Z
    quad(&mut mesh, [[hx, -hy, -hz], [-hx, -hy, -hz], [-hx, hy, -hz], [hx, hy, -hz]], [0.0, 0.0, -1.0]);
    mesh
}

fn quad(mesh: &mut Mesh, corners: [[f32; 3]; 4], normal: [f32; 3]) {
    let base = mesh.positions.len() as u32;
    let uvs = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
    for i in 0..4 {
        mesh.positions.push(corners[i]);
        mesh.normals.get_or_insert_with(Vec::new).push(normal);
        mesh.uvs.get_or_insert_with(Vec::new).push(uvs[i]);
    }
    mesh.indices.get_or_insert_with(Vec::new).extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// UV sphere, poles on +Y/-Y.
pub fn uv_sphere(radius: f32, segments: u32, rings: u32) -> Mesh {
    let segments = segments.max(3) as usize;
    let rings = rings.max(2) as usize;
    let mut mesh = Mesh::default();
    for r in 0..=rings {
        let phi = std::f32::consts::PI * r as f32 / rings as f32; // 0..PI
        for s in 0..=segments {
            let theta = 2.0 * std::f32::consts::PI * s as f32 / segments as f32;
            let n = Vec3::new(phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin());
            mesh.push_v((n * radius).to_array(), n.to_array(), [s as f32 / segments as f32, r as f32 / rings as f32]);
        }
    }
    let stride = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let a = (r * stride + s) as u32;
            let b = a + stride as u32;
            mesh.indices.as_mut().unwrap().extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    mesh
}

/// Cylinder along Y, centered.
pub fn cylinder(radius: f32, height: f32, segments: u32) -> Mesh {
    let seg = segments.max(3) as usize;
    let hy = height / 2.0;
    let mut mesh = Mesh::default();
    // side
    for s in 0..=seg {
        let t = 2.0 * std::f32::consts::PI * s as f32 / seg as f32;
        let (x, z) = (t.cos(), t.sin());
        let bottom = mesh.positions.len() as u32;
        mesh.push_v([x * radius, -hy, z * radius], [x, 0.0, z], [s as f32 / seg as f32, 0.0]);
        mesh.push_v([x * radius, hy, z * radius], [x, 0.0, z], [s as f32 / seg as f32, 1.0]);
        if s > 0 {
            let i = mesh.indices.as_mut().unwrap();
            i.extend_from_slice(&[bottom - 2, bottom - 1, bottom, bottom, bottom - 1, bottom + 1]);
        }
    }
    // caps
    for (y, ny, flip) in [(hy, 1.0, false), (-hy, -1.0, true)] {
        let center = mesh.positions.len() as u32;
        mesh.push_v([0.0, y, 0.0], [0.0, ny, 0.0], [0.5, 0.5]);
        for s in 0..=seg {
            let t = 2.0 * std::f32::consts::PI * s as f32 / seg as f32;
            let ring = mesh.positions.len() as u32;
            mesh.push_v([t.cos() * radius, y, t.sin() * radius], [0.0, ny, 0.0], [t.cos() * 0.5 + 0.5, t.sin() * 0.5 + 0.5]);
            if s > 0 {
                let i = mesh.indices.as_mut().unwrap();
                if flip {
                    i.extend_from_slice(&[center, ring, ring - 1]);
                } else {
                    i.extend_from_slice(&[center, ring - 1, ring]);
                }
            }
        }
    }
    mesh
}

/// Cone along Y, base at -height/2.
pub fn cone(radius: f32, height: f32, segments: u32) -> Mesh {
    let seg = segments.max(3) as usize;
    let hy = height / 2.0;
    let mut mesh = Mesh::default();
    for s in 0..=seg {
        let t = 2.0 * std::f32::consts::PI * s as f32 / seg as f32;
        let (x, z) = (t.cos(), t.sin());
        let bottom = mesh.positions.len() as u32;
        // side normal for a cone with apex at +Y
        let n = Vec3::new(x * height, radius, z * height).normalize_or_zero();
        let n = if n.length_squared() > 0.0 { n } else { Vec3::Y };
        mesh.push_v([x * radius, -hy, z * radius], n.to_array(), [s as f32 / seg as f32, 0.0]);
        mesh.push_v([0.0, hy, 0.0], n.to_array(), [s as f32 / seg as f32, 1.0]);
        if s > 0 {
            let i = mesh.indices.as_mut().unwrap();
            i.extend_from_slice(&[bottom - 2, bottom - 1, bottom, bottom, bottom - 1, bottom + 1]);
        }
    }
    // base cap
    let center = mesh.positions.len() as u32;
    mesh.push_v([0.0, -hy, 0.0], [0.0, -1.0, 0.0], [0.5, 0.5]);
    for s in 0..=seg {
        let t = 2.0 * std::f32::consts::PI * s as f32 / seg as f32;
        let ring = mesh.positions.len() as u32;
        mesh.push_v([t.cos() * radius, -hy, t.sin() * radius], [0.0, -1.0, 0.0], [0.0, 0.0]);
        if s > 0 {
            mesh.indices.as_mut().unwrap().extend_from_slice(&[center, ring, ring - 1]);
        }
    }
    mesh
}

/// Capsule along Y: `height` = total height, radius `radius`.
pub fn capsule(radius: f32, height: f32, segments: u32) -> Mesh {
    let height = height.max(radius * 2.0);
    let cyl_h = height - radius * 2.0;
    let mut mesh = uv_sphere(radius, segments, segments / 2 + 1);
    // shift top hemisphere up, bottom down
    for p in &mut mesh.positions {
        if p[1] > 0.0 {
            p[1] += cyl_h / 2.0;
        } else {
            p[1] -= cyl_h / 2.0;
        }
    }
    // insert a cylinder band between the hemispheres
    let seg = segments.max(3) as usize;
    let mut band = Mesh::default();
    for s in 0..=seg {
        let t = 2.0 * std::f32::consts::PI * s as f32 / seg as f32;
        band.push_v([t.cos() * radius, cyl_h / 2.0, t.sin() * radius], [t.cos(), 0.0, t.sin()], [0.0, 0.0]);
        band.push_v([t.cos() * radius, -cyl_h / 2.0, t.sin() * radius], [t.cos(), 0.0, t.sin()], [0.0, 0.0]);
    }
    // quad strip around the band: ring s = (2s top, 2s+1 bottom)
    {
        let idx = band.indices.as_mut().unwrap();
        for s in 0..seg {
            let (t0, b0) = (2 * s as u32, 2 * s as u32 + 1);
            let (t1, b1) = (t0 + 2, b0 + 2);
            idx.extend_from_slice(&[t0, b0, t1, t1, b0, b1]);
        }
    }
    merge(&mut mesh, band);
    mesh
}

/// Torus in the XZ plane.
pub fn torus(radius: f32, tube: f32, segments: u32, sides: u32) -> Mesh {
    let seg = segments.max(3) as usize;
    let side = sides.max(3) as usize;
    let mut mesh = Mesh::default();
    for i in 0..=seg {
        let u = 2.0 * std::f32::consts::PI * i as f32 / seg as f32;
        for j in 0..=side {
            let v = 2.0 * std::f32::consts::PI * j as f32 / side as f32;
            let cx = radius * u.cos();
            let cz = radius * u.sin();
            let nx = u.cos() * v.cos();
            let ny = v.sin();
            let nz = u.sin() * v.cos();
            mesh.push_v([cx + tube * nx, tube * ny, cz + tube * nz], [nx, ny, nz], [i as f32 / seg as f32, j as f32 / side as f32]);
        }
    }
    let stride = side + 1;
    for i in 0..seg {
        for j in 0..side {
            let a = (i * stride + j) as u32;
            let b = a + stride as u32;
            mesh.indices.as_mut().unwrap().extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    mesh
}

/// 2D rectangle on the XY plane, normal +Z.
pub fn rect_xy(w: f32, h: f32) -> Mesh {
    let (hx, hy) = (w / 2.0, h / 2.0);
    let mut mesh = Mesh::default();
    quad(&mut mesh, [[-hx, -hy, 0.0], [hx, -hy, 0.0], [hx, hy, 0.0], [-hx, hy, 0.0]], [0.0, 0.0, 1.0]);
    mesh
}

/// 2D filled circle on the XY plane, normal +Z.
pub fn circle_xy(radius: f32, segments: u32) -> Mesh {
    let seg = segments.max(3) as usize;
    let mut mesh = Mesh::default();
    let center = mesh.positions.len() as u32;
    mesh.push_v([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.5, 0.5]);
    for s in 0..=seg {
        let t = 2.0 * std::f32::consts::PI * s as f32 / seg as f32;
        let ring = mesh.positions.len() as u32;
        mesh.push_v([t.cos() * radius, t.sin() * radius, 0.0], [0.0, 0.0, 1.0], [t.cos() * 0.5 + 0.5, t.sin() * 0.5 + 0.5]);
        if s > 0 {
            mesh.indices.as_mut().unwrap().extend_from_slice(&[center, ring - 1, ring]);
        }
    }
    mesh
}

impl Mesh {
    fn push_v(&mut self, p: [f32; 3], n: [f32; 3], uv: [f32; 2]) {
        self.positions.push(p);
        self.normals.get_or_insert_with(Vec::new).push(n);
        self.uvs.get_or_insert_with(Vec::new).push(uv);
        self.indices.get_or_insert_with(Vec::new);
    }
}

/// Append `other` to `base` (reindexing). Used for capsule band.
fn merge(base: &mut Mesh, other: Mesh) {
    let off = base.positions.len() as u32;
    base.positions.extend(other.positions);
    if let Some(n) = other.normals {
        base.normals.get_or_insert_with(Vec::new).extend(n);
    }
    if let Some(u) = other.uvs {
        base.uvs.get_or_insert_with(Vec::new).extend(u);
    }
    if let Some(i) = other.indices {
        let bi = base.indices.get_or_insert_with(Vec::new);
        bi.reserve(i.len());
        bi.extend(i.into_iter().map(|v| v + off));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn capsule_band_is_triangulated() {
        let m = capsule(0.5, 3.0, 8);
        let idx = m.indices.clone().unwrap();
        assert_eq!(idx.len() % 3, 0);
        assert!(idx.iter().all(|&i| (i as usize) < m.positions.len()), "Index außerhalb");
        // Die Band-Vertizes (letztes 2*(seg+1)) müssen von Dreiecken
        // referenziert werden — sonst rendert die Kapsel mit Loch.
        let first_band = m.positions.len() - 2 * (8 + 1);
        let referenced: BTreeSet<u32> = idx.iter().copied().collect();
        for v in first_band..m.positions.len() {
            assert!(referenced.contains(&(v as u32)), "Band-Vertiz {v} ohne Dreieck");
        }
        let (min, max) = m.aabb();
        assert!((max[1] - min[1] - 3.0).abs() < 1e-3, "Kapsel überspannt nicht die Höhe");
    }
}
