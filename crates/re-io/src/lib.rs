//! re-io: file format import/export for WorldBuilder projects.
//!
//! Scene formats (dispatched by extension in [`import`]/[`export`]):
//! glTF 2.0 (`.gltf`), binary GLB (`.glb`) with meshes, materials,
//! skins (skeletons) and animations, and Wavefront OBJ (`.obj`) with
//! MTL materials on the legacy path. Texture files (`.png`, `.jpg`,
//! `.tga`, `.bmp`, `.qoi`, `.dds`, `.ktx2`, `.exr`) decode through
//! [`tex`]; DDS and KTX2 are decoded natively (uncompressed plus
//! BC1–BC5 and BC7; Basis-supercompressed KTX2 arrives with the
//! transcoder), OpenEXR through the pure-Rust `exr` crate.

mod bc7;
mod etc2;
mod export;
mod import;
pub mod obj;
pub mod png;
pub mod tex;

pub use png::write_png_rgba8;
pub use tex::{decode_bytes as decode_texture, Decoded as DecodedTexture, TEXTURE_EXTENSIONS};

/// File extensions this backend recognizes.
pub const EXTENSIONS: &[&str] = &["gltf", "glb", "obj"];

fn ext_of(path: &std::path::Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Szene importieren — das Format ergibt sich aus der Endung.
///
/// `re_core::project::Project` ist das Ziel aller Formatpfade; neue
/// Formate hängen hier nur noch an einem `match`-Arm.
pub fn import(path: &std::path::Path) -> anyhow::Result<re_core::project::Project> {
    match ext_of(path).as_str() {
        "gltf" | "glb" => import::import_gltf(path),
        "obj" => obj::import(path),
        "" => anyhow::bail!("ohne Endung — bekannte Formate: {}", EXTENSIONS.join(", ")),
        other => anyhow::bail!("unbekanntes Format: .{other} (bekannt: {})", EXTENSIONS.join(", ")),
    }
}

/// Szene exportieren; ohne Endung gilt das frühere Standardziel glTF.
pub fn export(p: &re_core::project::Project, path: &std::path::Path) -> anyhow::Result<()> {
    match ext_of(path).as_str() {
        "gltf" | "glb" | "" => export::export_gltf(p, path),
        "obj" => obj::export(p, path),
        other => anyhow::bail!("unbekanntes Format: .{other} (bekannt: {})", EXTENSIONS.join(", ")),
    }
}
