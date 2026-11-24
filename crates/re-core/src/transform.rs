use glam::{Affine3A, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Translation / Rotation / Scale. Rotation is a quaternion stored as `[x, y, z, w]`.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }
}

impl Transform {
    pub const IDENTITY: Transform = Self {
        translation: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
    };

    pub fn from_parts(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self {
            translation: translation.to_array(),
            rotation: rotation.to_array(),
            scale: scale.to_array(),
        }
    }

    pub fn translation_vec(&self) -> Vec3 {
        Vec3::from_array(self.translation)
    }

    pub fn rotation_quat(&self) -> Quat {
        Quat::from_array(self.rotation)
    }

    pub fn scale_vec(&self) -> Vec3 {
        Vec3::from_array(self.scale)
    }

    pub fn to_mat4(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale_vec(), self.rotation_quat(), self.translation_vec())
    }

    /// Decompose an arbitrary affine matrix back into TRS.
    /// Returns `None` for degenerate (non-invertible) matrices.
    pub fn from_mat4(m: &Mat4) -> Option<Self> {
        let affine = Affine3A::from_mat4(*m);
        let (scale, rotation, translation) = affine.to_scale_rotation_translation();
        if !scale.is_finite() || scale.to_array().iter().any(|s| s.abs() < f32::EPSILON) {
            return None;
        }
        Some(Self::from_parts(translation, rotation, scale))
    }

    /// Concatenate: apply `self` first, then `other` (i.e. `self * other`).
    pub fn then(&self, other: &Transform) -> Transform {
        Transform::from_mat4(&(self.to_mat4() * other.to_mat4())).unwrap_or_default()
    }
}
