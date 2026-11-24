use serde::{Deserialize, Serialize};

/// Simple PBR-style material, maps directly onto glTF `pbrMetallicRoughness`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub name: String,
    /// Linear RGBA base color.
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// Linear RGB emissive factor.
    pub emissive: [f32; 3],
    #[serde(default)]
    pub double_sided: bool,
    /// Optional albedo/base-color texture path (relative to asset dir).
    #[serde(default)]
    pub albedo_texture_id: Option<String>,
    /// Optional normal map texture path (relative to asset dir).
    #[serde(default)]
    pub normal_texture_id: Option<String>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: "default".into(),
            base_color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            emissive: [0.0, 0.0, 0.0],
            double_sided: false,
            albedo_texture_id: None,
            normal_texture_id: None,
        }
    }
}

impl Material {
    pub fn colored(name: &str, rgba: [f32; 4]) -> Self {
        Self {
            name: name.into(),
            base_color: rgba,
            ..Default::default()
        }
    }
}