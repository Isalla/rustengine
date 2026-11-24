use crate::id::SkeletonId;
use serde::{Deserialize, Serialize};

/// Per-vertex skinning influences referencing joints of a skeleton asset.
/// Joint indices point into `Skeleton::joints` of `skeleton`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Skin {
    pub skeleton: SkeletonId,
    /// Up to 4 joint indices per vertex (u16 to match glTF JOINTS_0).
    pub joints: Vec<[u16; 4]>,
    /// Corresponding weights per vertex, normalized to sum to 1.
    pub weights: Vec<[f32; 4]>,
}

/// A triangle-list mesh with optional vertex attributes and skin weights.
/// 2D elements are meshes lying on the XY plane (normals +Z).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mesh {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normals: Option<Vec<[f32; 3]>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uvs: Option<Vec<[f32; 2]>>,
    /// Triangle indices; `None` means a flat, non-indexed triangle list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indices: Option<Vec<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin: Option<Skin>,
}

impl Mesh {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        match &self.indices {
            Some(idx) => idx.len() / 3,
            None => self.positions.len() / 3,
        }
    }

    /// Axis-aligned bounding box: (min, max).
    pub fn aabb(&self) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for p in &self.positions {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        if self.positions.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        (min, max)
    }

    /// Recompute smooth vertex normals from triangle faces.
    pub fn compute_normals(&mut self) {
        let n = self.positions.len();
        let mut acc = vec![glam::Vec3::ZERO; n];
        let tri_count = self.triangle_count();
        for t in 0..tri_count {
            let [a, b, c] = self.triangle_indices(t);
            let pa = glam::Vec3::from_array(self.positions[a as usize]);
            let pb = glam::Vec3::from_array(self.positions[b as usize]);
            let pc = glam::Vec3::from_array(self.positions[c as usize]);
            let face = (pb - pa).cross(pc - pa);
            for idx in [a, b, c] {
                acc[idx as usize] += face;
            }
        }
        self.normals = Some(
            acc.iter()
                .map(|v| {
                    if v.length_squared() > 0.0 {
                        v.normalize().to_array()
                    } else {
                        [0.0, 1.0, 0.0]
                    }
                })
                .collect(),
        );
    }

    pub fn iter_triangles(&self) -> impl Iterator<Item = [u32; 3]> + '_ {
        (0..self.triangle_count()).map(move |t| self.triangle_indices(t))
    }

    fn triangle_indices(&self, t: usize) -> [u32; 3] {
        match &self.indices {
            Some(idx) => [idx[t * 3], idx[t * 3 + 1], idx[t * 3 + 2]],
            None => [(t * 3) as u32, (t * 3 + 1) as u32, (t * 3 + 2) as u32],
        }
    }
}
