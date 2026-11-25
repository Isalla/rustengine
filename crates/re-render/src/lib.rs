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
//! M5 (Render-Pipeline V2) bringt die deterministische Mathe-Schicht,
//! die jedes Backend teilt und die headless testbar ist — die GPU-Paesse
//! spiegeln diese Kurven und Indexrechnungen Kanal-fuer-Kanal:
//! - [`hdr`]     — Belichtung (EV100, Augen-Adaption), Tone-Kurven
//!   (Reinhard, ACES, AgX), sRGB-Transfer.
//! - [`shadow`]  — CSM-Kaskaden-Aufteilung (practical split).
//! - [`cluster`] — Cluster-Grid (Kacheln x log-Tiefscheiben) fuer
//!   Licht-Culling und Clustered Shadows.
//! - [`jitter`]  — Subpixel-Jitter (Halton) fuer TAA und Upscaler.
//! - [`gbuffer`] — oktaedrische Normalen-Kodierung und das
//!   G-Buffer-Kanal-Layout als Daten (ADR-0003).
//! - [`cull`]    — Frustum-Ebenen, Kugel/AABB-Tests, Hi-Z-Occlusion.
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
pub mod cluster;
pub mod cull;
pub mod gbuffer;
pub mod hdr;
pub mod jitter;
pub mod shadow;

pub use backend::{DrawBatch, GfxBackend, GfxError, NullBackend, Sprite};
pub use billboard::{billboard_matrix, unit_quad, uv_rect};
pub use camera::Camera;
pub use cluster::{
    cluster_view_aabb, cull_lights, log_z_bounds, sphere_intersects_aabb, ClusterGrid, SphereLight,
};
pub use cull::{Frustum, HiZPyramid, Plane};
pub use gbuffer::{
    octahedral_decode, octahedral_encode, pack_visibility_id, quantize_uv,
    unpack_visibility_id, ChannelSemantic, GBufferLayout, GBufferTarget, GfxFormat,
    Surface,
};
pub use hdr::{
    ev100_from_luminance, exposure_from_ev100, linear_to_srgb, luminance, mean_luminance,
    srgb_to_linear, AdaptiveExposure, HdrPipeline, ToneCurve, MIDDLE_GRAY,
};
pub use jitter::{halton, JitterPattern};
pub use shadow::{fit_cascade, CascadeFit, CascadeSetup};
