//! re-io: file format import/export for WorldBuilder projects.
//!
//! Scene formats (dispatched by extension in [`import`]/[`export`]):
//! glTF 2.0 (`.gltf`), binary GLB (`.glb`) with meshes, materials,
//! skins (skeletons) and animations, Wavefront OBJ (`.obj`) with
//! MTL materials on the legacy path, binary FBX ≥ 7.4 (`.fbx`,
//! import only) via the pure-Rust `fbxcel` crate, and the ASCII subset
//! of OpenUSD (`.usda`/`.usd`, import only, no references or variants)
//! through the own `usd` reader. Texture files
//! (`.png`, `.jpg`, `.tga`, `.bmp`, `.qoi`, `.dds`, `.ktx2`, `.exr`)
//! decode through [`tex`]; DDS and KTX2 are decoded natively
//! (uncompressed plus BC1–BC5, BC7, ETC2/EAC and ASTC in all fourteen
//! 2D block sizes; Basis-supercompressed KTX2 arrives with the
//! transcoder), OpenEXR through the pure-Rust `exr` crate.

pub mod astc;
mod bc7;
mod etc2;
mod export;
mod fbx;
mod import;
pub mod obj;
pub mod png;
pub mod tex;
mod usd;

pub use png::write_png_rgba8;
pub use tex::{decode_bytes as decode_texture, Decoded as DecodedTexture, TEXTURE_EXTENSIONS};

/// File extensions this backend recognizes.
pub const EXTENSIONS: &[&str] = &["gltf", "glb", "obj", "fbx", "usda", "usd", "usdc"];

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
        "fbx" => fbx::import_fbx(path),
        "usda" | "usd" | "usdc" => usd::import(path),
        "" => anyhow::bail!("ohne Endung — bekannte Formate: {}", EXTENSIONS.join(", ")),
        other => anyhow::bail!("unbekanntes Format: .{other} (bekannt: {})", EXTENSIONS.join(", ")),
    }
}

/// Szene exportieren; ohne Endung gilt das frühere Standardziel glTF.
pub fn export(p: &re_core::project::Project, path: &std::path::Path) -> anyhow::Result<()> {
    match ext_of(path).as_str() {
        "gltf" | "glb" | "" => export::export_gltf(p, path),
        "obj" => obj::export(p, path),
        "fbx" => anyhow::bail!("FBX ist nur zum Importieren da — exportiere nach glTF oder OBJ"),
        "usda" | "usd" | "usdc" => {
            anyhow::bail!("USD ist nur zum Importieren da — exportiere nach glTF oder OBJ")
        }
        other => anyhow::bail!("unbekanntes Format: .{other} (bekannt: {})", EXTENSIONS.join(", ")),
    }
}
