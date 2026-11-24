//! re-io: file format import/export for WorldBuilder projects.
//!
//! Currently supported: glTF 2.0 (`.gltf`) and binary GLB (`.glb`) with
//! meshes, materials, skins (skeletons) and animations.

mod export;
mod import;
pub mod png;

pub use export::export;
pub use import::import;
pub use png::write_png_rgba8;

/// File extensions this backend recognizes.
pub const EXTENSIONS: &[&str] = &["gltf", "glb"];
