//! # re-render — GPU-Renderer der Rust Engine (RE)
//!
//! Meilenstein M1: realzeitfaehiger Renderer, der die Szenen aus
//! [`re_core`] mit den Materialien und Albedo-Texturen aus `re-io`
//! (glTF 2.0 inkl. `baseColorTexture`) darstellt.
//!
//! Dieses Crate enthaelt nur plattformneutrale Konzepte:
//! - [`backend`] — das [`backend::GfxBackend`]-Trait. Wer rendern will,
//!   implementiert dieses Trait; der RE-Kern kennt keine GPU-API.
//! - [`camera`]  — Kamera-Mathematik (View/Proj), abstrakt, WebGL-tauglich.
//!
//! Geplante Backend-Module (je ein eigener Crate, ggf. eigenes Repo):
//! - `re-wgpu`     (oeffentlich): Vulkan (Linux), DX11/DX12 (Windows),
//!   Metal (macOS), WebGPU (wasm32) — alles ueber ein einziges wgpu-HAL.
//! - `re-software` (oeffentlich): Rasterer fuer sehr alte Ziele
//!   (GBA/NDS/PSP) und headless Tests.
//! - `re-<konsole>-nda` (privat): SDK-Kleber hinter demselben Trait,
//!   per git-submodule eingeklinkt — der oeffentliche Kern bleibt
//!   NDA-frei.

pub mod backend;
pub mod billboard;
pub mod camera;
pub mod hdr;
pub mod shadow;

pub use backend::{DrawBatch, GfxBackend, GfxError, NullBackend, Sprite};
pub use billboard::{billboard_matrix, unit_quad, uv_rect};
pub use camera::Camera;
pub use hdr::{
    ev100_from_luminance, exposure_from_ev100, linear_to_srgb, luminance, mean_luminance,
    srgb_to_linear, AdaptiveExposure, HdrPipeline, ToneCurve, MIDDLE_GRAY,
};
pub use shadow::CascadeSetup;
