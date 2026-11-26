//! Deferred-Pipeline V2 (M5, Box 1 + HDR-Aufsatz + Box 2 + Box 3 +
//! Box 5): MRT-G-Buffer + Compute-Licht-Culling + Lighting-Pass
//! (HDR-Float) + Tone-Mapping + FSR1-Upscale. Der Ablauf kommt aus dem
//! Render-Graphen (`re_render::graph`) — der Executor folgt dem
//! kompilierten Plan, der Code schreibt die Reihenfolge nicht mehr vor.
//!
//! Beweis-Strecke für ADR-0003: der Geometrie-Pass schreibt das Layout
//! `GBufferLayout::deferred_v1()` (Albedo+Metallic, Oktaeder-Normalen+
//! Roughness+Material-ID, Velocity RG16F, Tiefe), der Lighting-Pass
//! liest durch die abstrakte `surface_at`-Schnittstelle (hier: Lambert
//! als erster Beleg). Die CPU-Referenz `pack()/unpack()` aus
//! `re-render::gbuffer` ist dieselbe Mathematik — der Pixel-Diff-Test
//! (`tests/deferred.rs`) vergleicht GPU-Ausgabe gegen sie, und
//! [`WgpuBackend::export_deferred_png`](crate::WgpuBackend::export_deferred_png)
//! liefert das PNG als Sichtbeleg.

use re_core::mesh::Mesh;
use re_render::{
    AccessKind, Camera, ClusterGrid, CompiledGraph, GfxError, Layout, RenderGraph, ToneCurve,
};

const DEFERRED_SHADER: &str = include_str!("../deferred.wgsl");

use crate::{pod_bytes, pod_vec_bytes, CamUni, Inner, MeshGpu, Vtx, LIGHT_DIR};

/// IEEE-754-Half (16 Bit) → f32-Bits. Bewusst von Hand statt `f16` —
/// das ist im aktuellen Toolchain noch instabil, und der Readback-Pfad
/// muss auf jeder Rust-Version bauen, auf der die Engine laeuft.
fn half_to_f32_bits(h: u16) -> u32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1F) as u32;
    let man = (h & 0x3FF) as u32;
    match (exp, man) {
        (0, 0) => sign << 31,
        (0, _) => {
            // Subnormal: Mantisse normalisieren (fuehrende 1 herausschieben).
            let mut e = 127 - 15 - 10;
            let mut m = man;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            (sign << 31) | (e << 23) | ((m & 0x3FF) << 13)
        }
        (31, _) => (sign << 31) | (0xFF << 23) | (man << 13), // Inf/NaN
        _ => (sign << 31) | ((exp + 112) << 23) | (man << 13),
    }
}

/// Uniform des Geometrie-Passes: Layout bewusst 16-Byte-orientiert wie
/// in WGSL (vec3-align=16-Falle, siehe BatchUni im Forward-Pfad).
#[repr(C)]
#[derive(Clone, Copy)]
struct DeferredBatchUni {
    world: [[f32; 4]; 4],
    /// rgb = Albedo (linear), a = Metallic.
    albedo: [f32; 4],
    /// x = Roughness, y = Material-ID/255.
    params: [f32; 4],
}

/// Ein Geometrie-Beitrag zum G-Buffer: Mesh + Weltmatrix + Oberfläche.
/// Die Felder sind bewusst keine `re_core::Material`-Felder — der
/// Deferred-Pfad speist die G-Buffer-Kanäle direkt, damit die
/// Pixel-Diff-Referenz exakt dieselben Werte sieht.
pub struct DeferredBatch<'a> {
    pub mesh: &'a Mesh,
    pub world: glam::Mat4,
    pub albedo: [f32; 3],
    pub metallic: f32,
    pub roughness: f32,
    pub material_id: u8,
}

/// HDR-Einstellungen der Deferred-Kette (M5-HDR-Aufsatz). Der
/// Lighting-Pass schreibt ein Float-Target (Werte > 1 ueberleben),
/// der Tone-Mapping-Pass staucht mit derselben Kurve wie die
/// CPU-Referenz `re_render::hdr`.
#[derive(Clone, Copy, Debug)]
pub struct DeferredHdr {
    /// Belichtungs-Multiplikator (siehe `hdr::exposure_from_ev100`).
    pub exposure: f32,
    /// Intensitaet der Umgebungslicht-Formel — > 1 erzeugt echtes HDR.
    pub light_intensity: f32,
    /// Tone-Mapping-Kurve. AgX ist in WGSL noch nicht gespiegelt und
    /// wird mit einem Fehler abgewiesen (ehrlich statt ACES-Fallback).
    pub curve: ToneCurve,
}

impl Default for DeferredHdr {
    fn default() -> Self {
        Self { exposure: 1.0, light_intensity: 1.0, curve: ToneCurve::default() }
    }
}

/// WGSL-Seite des Post-Uniforms (Reihenfolge = `struct Post` im Shader).
#[repr(C)]
#[derive(Clone, Copy)]
struct PostUni {
    exposure: f32,
    light_intensity: f32,
    /// 0 = Reinhard, 1 = ReinhardExtended, 2 = ACES — identisch zur
    /// Kurvenauswahl im Shader.
    curve: f32,
    white: f32,
}

impl DeferredHdr {
    fn to_uni(&self) -> Result<PostUni, GfxError> {
        let (curve, white) = match self.curve {
            ToneCurve::Reinhard => (0.0, 0.0),
            ToneCurve::ReinhardExtended { white } => (1.0, white),
            ToneCurve::Aces => (2.0, 0.0),
            ToneCurve::Agx => {
                return Err(GfxError::Device(
                    "AgX-Kurve in WGSL noch nicht gespiegelt (CPU-Referenz existiert)".into(),
                ))
            }
        };
        Ok(PostUni {
            exposure: self.exposure,
            light_intensity: self.light_intensity,
            curve,
            white,
        })
    }
}

/// Ein Punktlicht der geclusterten Beleuchtung (M5, Box 2). Position in
/// Weltkoordinaten; `range` ist die Reichweite der Culling-Kugel und
/// zugleich die Grenze des Distanz-Falloffs (quadratisch, hart bei
/// `range`). Farbe linear, Intensitaet multipliziert — Werte ueber 1
/// sind ausdruecklich erlaubt (HDR).
#[derive(Clone, Copy, Debug)]
pub struct DeferredLight {
    pub position: [f32; 3],
    pub range: f32,
    pub color: [f32; 3],
    pub intensity: f32,
}

/// Kachelgroesse in Pixeln, Tiefenscheiben und Cluster-Kapazitaet —
/// dieselben Werte wie in `deferred.wgsl` (CLUSTER_CAP) und im Test.
pub const CLUSTER_TILE: u32 = 16;
pub const CLUSTER_SLICES: u32 = 4;
pub const CLUSTER_CAP: u32 = 8;

/// WGSL-Seite von `struct ClusterInfo` (Reihenfolge = Shader).
#[repr(C)]
#[derive(Clone, Copy)]
struct ClusterUni {
    tiles_x: u32,
    tiles_y: u32,
    slices: u32,
    light_count: u32,
    near: f32,
    far: f32,
    proj_a: f32,
    proj_b: f32,
    width: u32,
    height: u32,
    tile_size: u32,
    n_bounds: u32,
    /// Scheibengrenzen, 4 pro vec4 (Uniform-Stride-Regel im Shader):
    /// bound i = bounds[i/4][i%4], belegt bis n_bounds <= 12.
    bounds: [[f32; 4]; 3],
}

/// WGSL-Seite von `struct LightGpu`: Position im View-Raum (die
/// CPU rechnet sie um — dieselbe Transformation wie im Culling).
#[repr(C)]
#[derive(Clone, Copy)]
struct LightGpu {
    pos_range: [f32; 4],
    color_intensity: [f32; 4],
}

/// Upscale-Stufe hinter dem Tone-Mapping (M5, Box 3 / M13-Pi-Profil):
/// intern mit 1/`scale` rendern, dann FSR1 (EASU + RCAS) aufs Display.
/// `scale` ist ganzzahlig (2 = halbe interne Auflösung), wie es die
/// Pi-Formel der M13-Doktrin vorsieht.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeferredUpscale {
    /// Ganzzahliger Skala-Faktor interner Render-Pixel pro Display-Pixel.
    pub scale: u32,
    /// RCAS-Schärfe in AMD-Blenden (0 = schärfste, 2 = typisch weich).
    pub sharpness_stops: f32,
}

/// WGSL-Seite von `struct FsrCon` (EASU-Konstanten + RCAS-Blende,
/// FsrEasuCon/FsrRcasCon in einem Upload): sk = interne/display Größe,
/// ofs = 0.5*sk − 0.5 (`ofs`, weil `of` in WGSL reserviert ist),
/// sharp = 2^-stops. Dieselben Werte nutzt die CPU-Referenz in
/// `re_render::fsr` — bitidentisch.
#[repr(C)]
#[derive(Clone, Copy)]
struct FsrCon {
    sk: [f32; 2],
    ofs: [f32; 2],
    sharp: f32,
    _pad: f32,
}

/// Roh-Inhalt des HDR-Float-Targets (RGBA32F) nach einem Frame.
/// 16 Byte/Pixel; der Beweis, dass Werte ueber 1.0 die Kette
/// durchlaufen — ein 8-Bit-Ziel koennte das nicht.
pub struct HdrReadback {
    pub width: u32,
    pub height: u32,
    data: Vec<u8>,
}

impl HdrReadback {
    /// Pixel als RGBA-f32 (Rohwert des Float-Targets, ungetoont).
    pub fn pixel(&self, x: u32, y: u32) -> [f32; 4] {
        let i = ((y * self.width + x) * 16) as usize;
        let f = |k: usize| {
            f32::from_bits(u32::from_le_bytes([
                self.data[i + k * 4],
                self.data[i + k * 4 + 1],
                self.data[i + k * 4 + 2],
                self.data[i + k * 4 + 3],
            ]))
        };
        [f(0), f(1), f(2), f(3)]
    }
}

/// Roh-Inhalt der G-Buffer-Targets nach einem Frame (Readback).
pub struct GBufferReadback {
    pub width: u32,
    pub height: u32,
    /// Target 0: RGBA8-Bytes (Albedo rgb + Metallic a).
    pub t0: Vec<u8>,
    /// Target 1: RGBA8-Bytes (Oktaeder xy + Roughness + Material-ID).
    pub t1: Vec<u8>,
    /// Target 2: RG16F-Rohbytes (Velocity) — 4 Byte/Pixel.
    pub t2: Vec<u8>,
}

impl GBufferReadback {
    /// Target 0 als normierte Kanalwerte (Byte / 255).
    pub fn t0_f32(&self, x: u32, y: u32) -> [f32; 4] {
        Self::rgba8(&self.t0, self.width, x, y)
    }

    /// Target 1 als normierte Kanalwerte (Byte / 255).
    pub fn t1_f32(&self, x: u32, y: u32) -> [f32; 4] {
        Self::rgba8(&self.t1, self.width, x, y)
    }

    /// Velocity-Kanal (RG16F) als f32.
    pub fn velocity(&self, x: u32, y: u32) -> [f32; 2] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            f32::from_bits(half_to_f32_bits(u16::from_le_bytes([
                self.t2[i],
                self.t2[i + 1],
            ]))),
            f32::from_bits(half_to_f32_bits(u16::from_le_bytes([
                self.t2[i + 2],
                self.t2[i + 3],
            ]))),
        ]
    }

    fn rgba8(buf: &[u8], width: u32, x: u32, y: u32) -> [f32; 4] {
        let i = ((y * width + x) * 4) as usize;
        [
            buf[i] as f32 / 255.0,
            buf[i + 1] as f32 / 255.0,
            buf[i + 2] as f32 / 255.0,
            buf[i + 3] as f32 / 255.0,
        ]
    }
}

// ── Render-Graph (M5, Box 5): die Pass-Kette als Daten ───────────────
// Die Indizes verweisen auf die Deklaration in [`build_graph`]; der
// Executor in `Deferred::render` folgt dem kompilierten Ablauf statt
// einem Code-Zettel. Ein neuer Pass (Shadow 2.0) ist ein Knoten mehr:
// Ressource registrieren, Zugriffe deklarieren, Match-Arm ergänzen.
const P_CULL: usize = 0;
const P_GEOMETRY: usize = 1;
const P_LIGHT: usize = 2;
const P_TONE: usize = 3;
const P_EASU: usize = 4;
const P_RCAS: usize = 5;

/// Die Deferred-Kette als Graph deklariert — dieselbe Deklaration in
/// zwei Konfigurationen: mit Upscale hängen EASU und RCAS hinter dem
/// Tone-Mapping, sonst endet die Kette dort. Der kompilierte Ablauf
/// entscheidet die Reihenfolge (deterministisch, alphabetischer
/// Tie-Break — deshalb läuft `d-cull` vor `d-geometry`, obwohl beide
/// unabhängig sind); der Executor führt nur noch aus.
///
/// Was der Graph hier wirklich steuert: die Reihenfolge und die
/// Lese-/Schreib-Rechte je Pass. Die Layout-Übergänge berechnet er
/// mit — auf wgpu sind sie implizit (das Backend stellt sie aus
/// Attachment- und Bind-Nutzung selbst her), für spätere explizite
/// Backends liegen sie im [`CompiledGraph`] bereit. Speicher-Aliasing
/// folgt mit einem eigenen Target-Pool: die Lebensspannen (`spans`)
/// sind berechnet, wgpu allokiert die Targets noch selbst.
fn build_graph(upscale: bool) -> (RenderGraph, CompiledGraph) {
    let mut g = RenderGraph::new();
    let gb0 = g.add_resource("gb0", Layout::Undefined);
    let gb1 = g.add_resource("gb1", Layout::Undefined);
    let gb2 = g.add_resource("gb2", Layout::Undefined);
    let depth = g.add_resource("depth", Layout::Undefined);
    let lists = g.add_resource("cluster_lights", Layout::Undefined);
    let hdr = g.add_resource("hdr", Layout::Undefined);
    let final_img = g.add_resource("final", Layout::Undefined);
    let easu = g.add_resource("easu", Layout::Undefined);
    let rcas = g.add_resource("rcas", Layout::Undefined);
    // Kamera-, Post- und Licht-Buffer sind Frame-Eingaben der CPU —
    // kein Pass-zu-Pass-Fluss, sie gehören nicht in den Graphen.
    g.add_pass("d-cull", vec![RenderGraph::access(lists, AccessKind::Write)]);
    g.add_pass(
        "d-geometry",
        vec![
            RenderGraph::access(gb0, AccessKind::Write),
            RenderGraph::access(gb1, AccessKind::Write),
            RenderGraph::access(gb2, AccessKind::Write),
            RenderGraph::access(depth, AccessKind::Write),
        ],
    );
    g.add_pass(
        "d-light",
        vec![
            RenderGraph::access(gb0, AccessKind::Read),
            RenderGraph::access(gb1, AccessKind::Read),
            RenderGraph::access(gb2, AccessKind::Read),
            RenderGraph::access(depth, AccessKind::Read),
            RenderGraph::access(lists, AccessKind::Read),
            RenderGraph::access(hdr, AccessKind::Write),
        ],
    );
    g.add_pass(
        "d-tonemap",
        vec![
            RenderGraph::access(hdr, AccessKind::Read),
            RenderGraph::access(final_img, AccessKind::Write),
        ],
    );
    if upscale {
        g.add_pass(
            "d-easu",
            vec![
                RenderGraph::access(final_img, AccessKind::Read),
                RenderGraph::access(easu, AccessKind::Write),
            ],
        );
        g.add_pass(
            "d-rcas",
            vec![
                RenderGraph::access(easu, AccessKind::Read),
                RenderGraph::access(rcas, AccessKind::Write),
            ],
        );
    }
    // Die Deklaration ist statisch und bewusst fehlerfrei gebaut —
    // ein Compile-Fehler hier ist ein Programmierfehler, kein Szenario.
    let compiled = g.compile().expect("Deferred-Deklaration kompiliert fehlerfrei");
    (g, compiled)
}

/// Die Deferred-Zustände des Backends: eigene Targets, eigene Pipelines.
/// Entsteht lazy beim ersten `render_deferred` in der aktuellen Größe.
pub(crate) struct Deferred {
    cam_buf: wgpu::Buffer,
    cam_bind: wgpu::BindGroup,
    g1_geo: wgpu::BindGroupLayout,
    gb0: wgpu::Texture,
    gb0_view: wgpu::TextureView,
    gb1: wgpu::Texture,
    gb1_view: wgpu::TextureView,
    gb2: wgpu::Texture,
    gb2_view: wgpu::TextureView,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    #[allow(dead_code)] // dito: final_tex naehrt final_view
    final_tex: wgpu::Texture,
    final_view: wgpu::TextureView,
    /// HDR-Ziel des Lighting-Passes (RGBA32F, linear, Werte > 1).
    #[allow(dead_code)] // bleibt alive, solange hdr_view referenziert wird
    hdr: wgpu::Texture,
    hdr_view: wgpu::TextureView,
    post_buf: wgpu::Buffer,
    post_bind: wgpu::BindGroup,
    /// Layout des Lighting-Passes — der Bind-Group-Wechsel (Lichtzahl
    /// aendert die Buffer-Groesse) passiert pro Frame in `render`.
    g1_light: wgpu::BindGroupLayout,
    /// Point-Sampler fuer die G-Buffer-Reads des Lighting-Passes.
    sampler: wgpu::Sampler,
    cluster_uni_buf: wgpu::Buffer,
    cluster_lights_buf: wgpu::Buffer,
    lights_buf: wgpu::Buffer,
    /// Kapazitaet von `lights_buf` in Lichtern (wachst bei Bedarf).
    lights_cap: usize,
    g_cull: wgpu::BindGroupLayout,
    pipe_cull: wgpu::ComputePipeline,
    pipe_geometry: wgpu::RenderPipeline,
    pipe_light: wgpu::RenderPipeline,
    pipe_tonemap: wgpu::RenderPipeline,
    /// FSR1 (Box 3): Layout, Pipelines und — nur bei Upscale — die
    /// Display-Ziele (EASU-Zwischenablage, RCAS-Endbild) samt Bindings.
    /// Das Layout haellt die Bind-Groups nicht lesbar, aber wgpu
    /// verlangt ihr Weiterleben bis zum letzten Submit — Halter unten.
    #[allow(dead_code)] // naehrt fsr_bind und rcas_bind
    g_fsr: wgpu::BindGroupLayout,
    pipe_fsr: wgpu::RenderPipeline,
    pipe_rcas: wgpu::RenderPipeline,
    #[allow(dead_code)] // dito: easu_tex naehrt easu_view
    easu_tex: Option<wgpu::Texture>,
    #[allow(dead_code)] // naehrt rcas_bind
    easu_view: Option<wgpu::TextureView>,
    #[allow(dead_code)] // rcas_view ist das Display-Endergebnis
    rcas_tex: Option<wgpu::Texture>,
    rcas_view: Option<wgpu::TextureView>,
    fsr_bind: Option<wgpu::BindGroup>,
    rcas_bind: Option<wgpu::BindGroup>,
    /// Upscale-Einstellung, fuer die diese Zustaende gebaut wurden.
    upscale: Option<DeferredUpscale>,
    /// Render-Graph (Box 5): die Deklaration der Kette und ihr
    /// kompilierter Ablauf — der Executor in `render` folgt ihm.
    graph: RenderGraph,
    compiled: CompiledGraph,
    /// Die Pass-Namen des zuletzt ausgefuehrten Ablaufs — Beweiskanal
    /// fuer den Graph-Test: der Executor folgt dem Plan, nicht dem Code.
    last_order: Vec<String>,
    /// Display-Groesse (RCAS-Ziel); `size` ist die interne Groesse.
    display: (u32, u32),
    /// Interne Ziel-Groesse (bei Upscale = display / scale).
    size: (u32, u32),
}

impl Deferred {
    pub(crate) fn new(inner: &Inner, upscale: Option<DeferredUpscale>) -> Self {
        let device = &inner.device;
        // Der Graph haengt nur an der Upscale-Entscheidung: gleiche
        // Deklaration => gleicher kompilierter Ablauf (Box 5).
        let (graph, compiled) = build_graph(upscale.is_some());
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("re-deferred"),
            source: wgpu::ShaderSource::Wgsl(DEFERRED_SHADER.into()),
        });

        // g0: Kamera-Uniform (beide Passes teilen sich das Layout).
        let g0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("d-g0-cam"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        // g1 (Geometrie): Batch-Uniform.
        let g1_geo = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("d-g1-batch"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        // g1 (Lighting): drei G-Buffer-Textures (textureLoad, nicht
        // gefiltert) + Sampler-Platzhalter.
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let g1_light = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("d-g1-light"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                tex_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Cluster-Lichter: Depth-Textur, Cluster-Uniform,
                // Licht-Listen und Licht-Daten (Storage, nur lesend).
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        // g0 (Tonemap): HDR-Textur + Post-Uniform (eigene Pipeline-Layout-
        // Gruppe, der Pass braucht keine Kamera).
        let g_post = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("d-g0-post"),
            entries: &[
                tex_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // g_cull (Compute): Cluster-Uniform + Licht-Daten + Listen-Ziel.
        let g_cull = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("d-g-cull"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let cam_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("d-cam-uni"),
            size: std::mem::size_of::<CamUni>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cam_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("d-cam-bind"),
            layout: &g0,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: cam_buf.as_entire_binding(),
            }],
        });
        let post_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("d-post-uni"),
            size: std::mem::size_of::<PostUni>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("d-point"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let (w, h) = inner.size;
        // Pi-Formel (M13): intern mit 1/scale rendern, FSR1 füllt danach
        // das Display-Ziel. Ohne Upscale sind beide Größen gleich.
        let (iw, ih) = match upscale {
            Some(u) => ((w / u.scale.max(1)).max(1), (h / u.scale.max(1)).max(1)),
            None => (w, h),
        };
        let gb0 = Self::tex_rgba8(device, iw, ih, "gb0");
        let gb1 = Self::tex_rgba8(device, iw, ih, "gb1");
        let gb2 = Self::tex_rg16f(device, iw, ih, "gb2");
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("d-depth"),
            size: wgpu::Extent3d { width: iw.max(1), height: ih.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            // RENDER_ATTACHMENT (Geometrie-Pass) + TEXTURE_BINDING
            // (Lighting-Pass rekonstruiert die Sichttiefe) + COPY_SRC
            // (Readback fuer den Pixel-Diff-Test).
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        // Ziel des Tone-Mapping-Passes: sRGB wie der Forward-Offscreen-
        // Pfad, damit read_deferred_rgba() das Endergebnis liefert. Bei
        // Upscale naehrt es zusaetzlich den EASU-Pass — AMD verlangt
        // display-kodierte Eingabe, also die rohen (sRGB-dekodierten)
        // Bytes ueber eine Rgba8Unorm-Rohansicht (view_formats).
        let final_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("d-final"),
            size: wgpu::Extent3d { width: iw.max(1), height: ih.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
        });
        // HDR-Ziel des Lighting-Passes: RGBA32F statt RGBA16F, weil
        // wgpu das Sampeln von 16F-Targets die instabile f16-Feature
        // abverlangt — 32F laeuft ueberall (auch Software-Adapter/CI)
        // und die Testgroesse ist Speicher-egal. Desktop-Tier darf spaeter
        // auf 16F wechseln (Format ist eine Profil-Entscheidung, M13).
        let hdr = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("d-hdr"),
            size: wgpu::Extent3d { width: iw.max(1), height: ih.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let gb0_view = gb0.create_view(&wgpu::TextureViewDescriptor::default());
        let gb1_view = gb1.create_view(&wgpu::TextureViewDescriptor::default());
        let gb2_view = gb2.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let final_view = final_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let hdr_view = hdr.create_view(&wgpu::TextureViewDescriptor::default());

        // Cluster-Buffer fuer die Lighting-Pipeline (M5, Box 2). Die
        // Licht-Listen haben Fixed-Stride je Cluster; die Licht-Daten
        // wachsen bei Bedarf ( lights_cap ). Groessen hier minimal —
        // render() schreibt nur so viel, wie die Frame-Lichter fuellen.
        let cluster_count =
            iw.max(1).div_ceil(CLUSTER_TILE) * ih.max(1).div_ceil(CLUSTER_TILE) * CLUSTER_SLICES;
        let cluster_uni_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("d-cluster-uni"),
            size: std::mem::size_of::<ClusterUni>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cluster_lights_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("d-cluster-lights"),
            size: (cluster_count * CLUSTER_CAP as u32 * 4) as u64,
            // Der Compute-Pass schreibt die Listen, der Test liest sie.
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let lights_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("d-lights"),
            size: std::mem::size_of::<LightGpu>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let post_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("d-post-bind"),
            layout: &g_post,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&hdr_view) },
                wgpu::BindGroupEntry { binding: 1, resource: post_buf.as_entire_binding() },
            ],
        });

        // ── FSR1 (M5, Box 3) ───────────────────────────────────────────
        // EASU und RCAS hängen hinters Tone-Mapping; beide lesen per
        // textureLoad (nicht gefiltert) und brauchen je ein Uniform.
        let fsr_tex_entry = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let fsr_uni_entry = wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let g_fsr = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("d-g-fsr"),
            entries: &[fsr_tex_entry, fsr_uni_entry],
        });
        // Die Display-Ziele und Bindings entstehen nur bei Upscale —
        // ohne Skala gibt es keine Kette (final_tex ist das Endergebnis).
        // EASU und RCAS teilen sich Layout und Uniform (max_bind_groups=4):
        // die Bind-Groups unterscheiden sich nur in der Quelle an binding 0.
        let (easu_tex, easu_view, rcas_tex, rcas_view, fsr_bind, rcas_bind) =
            if let Some(u) = upscale {
                let con = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("d-fsr-con"),
                    size: std::mem::size_of::<FsrCon>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let sk = [iw as f32 / w.max(1) as f32, ih as f32 / h.max(1) as f32];
                // FsrEasuCon + FsrRcasCon in einem Upload — die Blende
                // -> linear-Zahl ist dieselbe wie in der CPU-Referenz.
                inner.queue.write_buffer(&con, 0, pod_bytes(&FsrCon {
                    sk,
                    ofs: [0.5 * sk[0] - 0.5, 0.5 * sk[1] - 0.5],
                    sharp: (-u.sharpness_stops).exp2(),
                    _pad: 0.0,
                }));
                let mk = |label: &str| {
                    let t = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    });
                    let v = t.create_view(&wgpu::TextureViewDescriptor::default());
                    (t, v)
                };
                let (easu_tex, easu_view) = mk("d-easu");
                let (rcas_tex, rcas_view) = mk("d-rcas");
                // EASU liest das interne Ziel als ROH-Bytes-Ansicht: die
                // sRGB-Kurve bleibt stehen, die AMD-Kette verlangt sie.
                let final_raw = final_tex.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(wgpu::TextureFormat::Rgba8Unorm),
                    ..Default::default()
                });
                let fsr_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("d-fsr-bind"),
                    layout: &g_fsr,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&final_raw) },
                        wgpu::BindGroupEntry { binding: 1, resource: con.as_entire_binding() },
                    ],
                });
                let rcas_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("d-rcas-bind"),
                    layout: &g_fsr,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&easu_view) },
                        wgpu::BindGroupEntry { binding: 1, resource: con.as_entire_binding() },
                    ],
                });
                (Some(easu_tex), Some(easu_view), Some(rcas_tex), Some(rcas_view), Some(fsr_bind), Some(rcas_bind))
            } else {
                (None, None, None, None, None, None)
            };

        let layout_geo = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("d-layout-geo"),
            bind_group_layouts: &[Some(&g0), Some(&g1_geo)],
            immediate_size: 0,
        });
        let layout_light = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("d-layout-light"),
            bind_group_layouts: &[Some(&g0), Some(&g1_light)],
            immediate_size: 0,
        });
        let layout_tonemap = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("d-layout-tonemap"),
            bind_group_layouts: &[Some(&g_post)],
            immediate_size: 0,
        });

        let pipe_geometry = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("d-geometry"),
            layout: Some(&layout_geo),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_geometry"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vtx>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &crate::VERTEX_ATTRS,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_geometry"),
                compilation_options: Default::default(),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rg16Float,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                ],
            }),
            multiview_mask: None,
            cache: None,
        });

        let pipe_light = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("d-light"),
            layout: Some(&layout_light),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_fullscreen"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_light"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba32Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let pipe_tonemap = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("d-tonemap"),
            layout: Some(&layout_tonemap),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_fullscreen"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_tonemap"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let layout_cull = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("d-layout-cull"),
            bind_group_layouts: &[Some(&g_cull)],
            immediate_size: 0,
        });
        let pipe_cull = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("d-cull"),
            layout: Some(&layout_cull),
            module: &module,
            entry_point: Some("cs_cull"),
            compilation_options: Default::default(),
            cache: None,
        });

        // FSR1-Pipelines: Vollbild-Dreiecke, ein Ziel, kein Blend —
        // dieselbe Bauform wie der Tone-Mapping-Pass.
        let layout_fsr = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("d-layout-fsr"),
            bind_group_layouts: &[Some(&g_fsr)],
            immediate_size: 0,
        });
        let pipe_fsr = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("d-easu"),
            layout: Some(&layout_fsr),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_fullscreen"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_easu"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let layout_rcas = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("d-layout-rcas"),
            bind_group_layouts: &[Some(&g_fsr)],
            immediate_size: 0,
        });
        let pipe_rcas = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("d-rcas"),
            layout: Some(&layout_rcas),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_fullscreen"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_rcas"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            cam_buf,
            cam_bind,
            g1_geo,
            gb0,
            gb0_view,
            gb1,
            gb1_view,
            gb2,
            gb2_view,
            depth,
            depth_view,
            final_tex,
            final_view,
            hdr,
            hdr_view,
            post_buf,
            post_bind,
            g1_light,
            sampler,
            cluster_uni_buf,
            cluster_lights_buf,
            lights_buf,
            lights_cap: 1,
            g_cull,
            pipe_cull,
            pipe_geometry,
            pipe_light,
            pipe_tonemap,
            g_fsr,
            pipe_fsr,
            pipe_rcas,
            easu_tex,
            easu_view,
            rcas_tex,
            rcas_view,
            fsr_bind,
            rcas_bind,
            upscale,
            graph,
            compiled,
            last_order: Vec::new(),
            display: (w.max(1), h.max(1)),
            size: (iw.max(1), ih.max(1)),
        }
    }

    fn tex_rgba8(device: &wgpu::Device, w: u32, h: u32, label: &str) -> wgpu::Texture {
        Self::tex(device, w, h, label, wgpu::TextureFormat::Rgba8Unorm)
    }

    fn tex_rg16f(device: &wgpu::Device, w: u32, h: u32, label: &str) -> wgpu::Texture {
        Self::tex(device, w, h, label, wgpu::TextureFormat::Rg16Float)
    }

    fn tex(
        device: &wgpu::Device,
        w: u32,
        h: u32,
        label: &str,
        format: wgpu::TextureFormat,
    ) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    /// Ein Frame: der kompilierte Graph plant die Pässe (Cull,
    /// Geometrie, Lighting, Tone-Mapping, FSR1 bei Upscale), der
    /// Executor baut sie in genau dieser Reihenfolge auf dem Encoder.
    /// Ruft vorher `resize` auf, wenn die Display-Größe wechselt.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        &mut self,
        inner: &mut Inner,
        camera: &Camera,
        batches: &[DeferredBatch],
        hdr: DeferredHdr,
        lights: &[DeferredLight],
    ) -> Result<(), GfxError> {
        if self.display != inner.size {
            self.resize(inner);
        }
        let post = hdr.to_uni()?;
        let cam = CamUni {
            view_proj: crate::mat4(camera.view_proj()),
            light_dir_time: [LIGHT_DIR[0], LIGHT_DIR[1], LIGHT_DIR[2], 0.0],
        };
        inner.queue.write_buffer(&self.cam_buf, 0, pod_bytes(&cam));
        inner.queue.write_buffer(&self.post_buf, 0, pod_bytes(&post));

        // ── Geclusterte Lichter: Das Culling uebernimmt der GPU-Compute-
        // Pass `cs_cull` (gespiegeltes cluster_view_aabb +
        // sphere_intersects_aabb aus re_render::cluster). Der CPU-Kern
        // `cull_lights` bleibt als Referenz im Test — der
        // Listen-Vergleich beweist die Gleichheit Index-fuer-Index. ──────
        let (w, h) = self.size;
        let dr = camera.depth_range();
        if !lights.is_empty() && dr.is_none() {
            return Err(GfxError::Device("geclusterte Lichter brauchen eine Perspektiv-Kamera".into()));
        }
        let (near, far) = dr.unwrap_or((0.1, 4000.0));
        let grid = ClusterGrid::for_render_size([w, h], CLUSTER_TILE, CLUSTER_SLICES, near, far);
        let mut bounds = [[0.0f32; 4]; 3];
        for s in 0..grid.slice_count() {
            let (a, b) = grid.slice_bounds(s).unwrap_or((near, far));
            bounds[s as usize / 4][s as usize % 4] = a;
            bounds[(s as usize + 1) / 4][(s as usize + 1) % 4] = b;
        }
        let cuni = ClusterUni {
            tiles_x: grid.tiles_x,
            tiles_y: grid.tiles_y,
            slices: grid.slice_count(),
            light_count: lights.len() as u32,
            near,
            far,
            proj_a: camera.proj.x_axis.x,
            proj_b: camera.proj.y_axis.y,
            width: w,
            height: h,
            tile_size: CLUSTER_TILE,
            n_bounds: grid.slice_count() + 1,
            bounds,
        };
        // Licht-Daten in den View-Raum — dieselbe Transformation wie im
        // CPU-Referenzculling, die GPU prueft nur noch Kugel gegen Zelle.
        let lgs: Vec<LightGpu> = lights
            .iter()
            .map(|l| {
                let vp = camera.view.transform_point3(glam::Vec3::new(
                    l.position[0],
                    l.position[1],
                    l.position[2],
                ));
                LightGpu {
                    pos_range: [vp.x, vp.y, vp.z, l.range],
                    color_intensity: [l.color[0], l.color[1], l.color[2], l.intensity],
                }
            })
            .collect();
        if lights.len() > self.lights_cap {
            self.lights_cap = lights.len();
            self.lights_buf = inner.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("d-lights"),
                size: (self.lights_cap * std::mem::size_of::<LightGpu>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        inner.queue.write_buffer(&self.cluster_uni_buf, 0, pod_bytes(&cuni));
        if !lgs.is_empty() {
            inner.queue.write_buffer(&self.lights_buf, 0, pod_vec_bytes(&lgs));
        }
        let cull_bind = inner.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("d-cull-bind"),
            layout: &self.g_cull,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.cluster_uni_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.lights_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.cluster_lights_buf.as_entire_binding() },
            ],
        });
        let cluster_count = grid.cluster_count();
        // Bind-Group pro Frame: Groessen der Licht-Buffer wechseln mit
        // der Szenen-Lichtzahl; das Anlegen kostet Mikrosekunden.
        let light_bind = inner.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("d-light-bind"),
            layout: &self.g1_light,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.gb0_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.gb1_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.gb2_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 4, resource: self.post_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&self.depth_view) },
                wgpu::BindGroupEntry { binding: 6, resource: self.cluster_uni_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: self.cluster_lights_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 8, resource: self.lights_buf.as_entire_binding() },
            ],
        });

        // Batches vorbereiten (Mesh-Upload + Uniform + Bind-Group).
        struct Prep {
            vbuf: wgpu::Buffer,
            ibuf: wgpu::Buffer,
            n_indices: u32,
            bind: wgpu::BindGroup,
        }
        let mut prep: Vec<Prep> = Vec::with_capacity(batches.len());
        for b in batches {
            if b.mesh.positions.is_empty() {
                continue;
            }
            let gpu = self.upload_mesh(inner, b.mesh);
            let (Some(vbuf), Some(ibuf)) = (gpu.vertex.clone(), gpu.index.clone()) else {
                continue;
            };
            let uni = DeferredBatchUni {
                world: crate::mat4(b.world),
                albedo: [b.albedo[0], b.albedo[1], b.albedo[2], b.metallic],
                params: [b.roughness, b.material_id as f32 / 255.0, 0.0, 0.0],
            };
            let uni_buf = inner.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("d-batch-uni"),
                size: std::mem::size_of::<DeferredBatchUni>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            inner.queue.write_buffer(&uni_buf, 0, pod_bytes(&uni));
            let bind = inner.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("d-batch-bind"),
                layout: &self.g1_geo,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uni_buf.as_entire_binding(),
                }],
            });
            prep.push(Prep { vbuf, ibuf, n_indices: gpu.n_indices, bind });
        }

        let device = inner.device.clone();
        let queue = inner.queue.clone();
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("d-frame"),
        });
        // Der Graph plant, der Executor baut (Box 5): jeder kompilierte
        // Pass bekommt seinen Befehl auf dem Encoder — die Reihenfolge
        // kommt aus dem CompiledGraph, nicht aus dem Code. Dass `d-cull`
        // vor `d-geometry` laeuft, entscheidet der Sortierer (beide
        // unabhaengig, alphabetischer Tie-Break), nicht die Schreibweise.
        let mut order = Vec::with_capacity(self.compiled.order.len());
        for &pi in &self.compiled.order {
            match pi {
                P_CULL => {
                    // Licht-Culling auf der GPU — die Listen liest erst
                    // `d-light`; die Ordnung sichert der Graph.
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("d-cull"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&self.pipe_cull);
                    pass.set_bind_group(0, &cull_bind, &[]);
                    pass.dispatch_workgroups(cluster_count.div_ceil(64), 1, 1);
                }
                P_GEOMETRY => {
                    let c = camera.clear_color;
                    let clear = wgpu::Color {
                        r: c[0] as f64,
                        g: c[1] as f64,
                        b: c[2] as f64,
                        a: c[3] as f64,
                    };
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("d-geometry"),
                        color_attachments: &[
                            Some(wgpu::RenderPassColorAttachment {
                                view: &self.gb0_view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                            }),
                            Some(wgpu::RenderPassColorAttachment {
                                view: &self.gb1_view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                            }),
                            Some(wgpu::RenderPassColorAttachment {
                                view: &self.gb2_view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                            }),
                        ],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &self.depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        ..Default::default()
                    });
                    pass.set_pipeline(&self.pipe_geometry);
                    pass.set_bind_group(0, &self.cam_bind, &[]);
                    for p in &prep {
                        pass.set_bind_group(1, &p.bind, &[]);
                        pass.set_vertex_buffer(0, p.vbuf.slice(..));
                        pass.set_index_buffer(p.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..p.n_indices, 0, 0..1);
                    }
                }
                P_LIGHT => {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("d-light"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.hdr_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        ..Default::default()
                    });
                    pass.set_pipeline(&self.pipe_light);
                    pass.set_bind_group(0, &self.cam_bind, &[]);
                    pass.set_bind_group(1, &light_bind, &[]);
                    pass.draw(0..3, 0..1);
                }
                P_TONE => {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("d-tonemap"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.final_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        ..Default::default()
                    });
                    pass.set_pipeline(&self.pipe_tonemap);
                    pass.set_bind_group(0, &self.post_bind, &[]);
                    pass.draw(0..3, 0..1);
                }
                // FSR1-Kette (nur im Plan, wenn Upscale an ist): EASU
                // skaliert das interne Ziel aufs Display, RCAS schaerft
                // nach — der Pi-Pfad (M13). Beide Passes lesen
                // display-kodierte 8-Bit-Werte, wie AMD es verlangt.
                P_EASU => {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("d-easu"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: self.easu_view.as_ref().expect("Pass im Plan => easu_view"),
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        ..Default::default()
                    });
                    pass.set_pipeline(&self.pipe_fsr);
                    pass.set_bind_group(0, self.fsr_bind.as_ref().expect("Pass im Plan => fsr_bind"), &[]);
                    pass.draw(0..3, 0..1);
                }
                P_RCAS => {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("d-rcas"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: self.rcas_view.as_ref().expect("Pass im Plan => rcas_view"),
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        ..Default::default()
                    });
                    pass.set_pipeline(&self.pipe_rcas);
                    pass.set_bind_group(0, self.rcas_bind.as_ref().expect("Pass im Plan => rcas_bind"), &[]);
                    pass.draw(0..3, 0..1);
                }
                other => unreachable!("Pass {other} ist nicht deklariert"),
            }
            order.push(self.graph.passes()[pi].name.clone());
        }
        self.last_order = order;
        queue.submit([enc.finish()]);
        Ok(())
    }

    /// Die Pass-Folge des letzten Frames, so wie der Graph sie geplant
    /// und der Executor ausgefuehrt hat (Beweiskanal fuer Box 5).
    pub(crate) fn pass_order(&self) -> &[String] {
        &self.last_order
    }

    /// Das beleuchtete Endergebnis als RGBA8-Bytes — bei Upscale der
    /// RCAS-Ausgang in Display-Groesse, sonst `final_tex`. Der
    /// Deferred-Seitenarm zum Forward-`readback_rgba`.
    pub(crate) fn read_final(&self, inner: &Inner) -> Result<Vec<u8>, GfxError> {
        if let Some(tex) = &self.rcas_tex {
            let (w, h) = self.display;
            return crate::readback_texture(&inner.device, &inner.queue, tex, w, h, 4);
        }
        let (w, h) = self.size;
        crate::readback_texture(&inner.device, &inner.queue, &self.final_tex, w, h, 4)
    }

    /// Das tone-gemappte interne Ziel (`final_tex`) in interner Groesse —
    /// bei Upscale die display-kodierte Eingabe der FSR1-Kette, gegen
    /// die der CPU-Referenztest antritt.
    pub(crate) fn read_internal(&self, inner: &Inner) -> Result<Vec<u8>, GfxError> {
        let (w, h) = self.size;
        crate::readback_texture(&inner.device, &inner.queue, &self.final_tex, w, h, 4)
    }

    /// Roh-Inhalt des HDR-Float-Targets vor dem Tone-Mapping — der
    /// Beweis-Kanal fuer Werte ueber 1.0.
    pub(crate) fn read_hdr(&self, inner: &Inner) -> Result<HdrReadback, GfxError> {
        let (w, h) = self.size;
        Ok(HdrReadback {
            width: w,
            height: h,
            data: crate::readback_texture(&inner.device, &inner.queue, &self.hdr, w, h, 16)?,
        })
    }

    /// Die Depth-Buffer des Geometrie-Passes als f32 — der Test
    /// rekonstruiert damit dieselbe Sichttiefe wie der Shader, ohne
    /// dass Rundungsunterschiede die Cluster-Wahl verschieben.
    pub(crate) fn read_depth(&self, inner: &Inner) -> Result<Vec<f32>, GfxError> {
        let (w, h) = self.size;
        let raw = crate::readback_texture(&inner.device, &inner.queue, &self.depth, w, h, 4)?;
        Ok(raw
            .chunks_exact(4)
            .map(|c| f32::from_bits(u32::from_le_bytes([c[0], c[1], c[2], c[3]])))
            .collect())
    }

    /// Die Licht-Listen des Compute-Cullings, roh (CLUSTER_CAP Eintraege
    /// je Cluster: Slot 0 = Anzahl, Slot 1.. = Indizes) — fuer den
    /// Listen-Vergleich gegen die CPU-Referenz.
    pub(crate) fn read_cluster_lights(&self, inner: &Inner) -> Result<Vec<u32>, GfxError> {
        let (w, h) = self.size;
        let clusters = w.div_ceil(CLUSTER_TILE) * h.div_ceil(CLUSTER_TILE) * CLUSTER_SLICES;
        let bytes = crate::readback_buffer(
            &inner.device,
            &inner.queue,
            &self.cluster_lights_buf,
            u64::from(clusters * CLUSTER_CAP * 4),
        )?;
        Ok(bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect())
    }

    /// G-Buffer-Targets auslesen (für den Pixel-Diff gegen die
    /// CPU-Referenz). Nur headless — dasselbe wie `readback_rgba`.
    pub(crate) fn read_gbuffer(&self, inner: &Inner) -> Result<GBufferReadback, GfxError> {
        let (w, h) = self.size;
        Ok(GBufferReadback {
            width: w,
            height: h,
            t0: crate::readback_texture(&inner.device, &inner.queue, &self.gb0, w, h, 4)?,
            t1: crate::readback_texture(&inner.device, &inner.queue, &self.gb1, w, h, 4)?,
            t2: crate::readback_texture(&inner.device, &inner.queue, &self.gb2, w, h, 4)?,
        })
    }

    /// Mesh in die GPU-Residenz legen (eigener Cache über Zeiger-Key wie
    /// im Forward-Pfad; TODO(M1c): stabile Asset-IDs).
    fn upload_mesh(&mut self, inner: &mut Inner, mesh: &Mesh) -> MeshGpu {
        let key = (mesh as *const Mesh) as usize;
        if let Some(gpu) = inner.mesh_cache.get(&key) {
            return MeshGpu {
                vertex: gpu.vertex.clone(),
                index: gpu.index.clone(),
                n_vertices: gpu.n_vertices,
                n_indices: gpu.n_indices,
            };
        }
        let mut verts = Vec::with_capacity(mesh.positions.len());
        for i in 0..mesh.positions.len() {
            verts.push(Vtx {
                pos: mesh.positions[i],
                nrm: mesh
                    .normals
                    .as_ref()
                    .and_then(|ns| ns.get(i).copied())
                    .unwrap_or([0.0, 1.0, 0.0]),
                uv: mesh
                    .uvs
                    .as_ref()
                    .and_then(|us| us.get(i).copied())
                    .unwrap_or([0.0, 0.0]),
            });
        }
        let vertex = inner.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("d-mesh-vert"),
            size: (verts.len() * std::mem::size_of::<Vtx>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        inner.queue.write_buffer(&vertex, 0, pod_vec_bytes(&verts));
        let mut gpu = MeshGpu {
            vertex: Some(vertex),
            index: None,
            n_vertices: mesh.positions.len() as u32,
            n_indices: 0,
        };
        if let Some(idx) = mesh.indices.as_ref().filter(|v| !v.is_empty()) {
            let b = inner.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("d-mesh-idx"),
                size: (idx.len() * 4) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            inner.queue.write_buffer(&b, 0, pod_vec_bytes(idx));
            gpu.n_indices = idx.len() as u32;
            gpu.index = Some(b);
        }
        inner.mesh_cache.insert(key, MeshGpu {
            vertex: gpu.vertex.clone(),
            index: gpu.index.clone(),
            n_vertices: gpu.n_vertices,
            n_indices: gpu.n_indices,
        });
        gpu
    }

    /// Groessenwechsel: alle Targets und abgeleiteten Views/Bind-Groups
    /// neu anlegen. Die neue Größe kommt aus `inner.size`.
    fn resize(&mut self, inner: &mut Inner) {
        let upscale = self.upscale;
        *self = Self::new(inner, upscale);
    }

    /// Die Upscale-Einstellung, fuer die diese Zustaende gebaut wurden —
    /// das Backend vergleicht sie gegen den Wunsch des Aufrufers.
    pub(crate) fn upscale(&self) -> Option<DeferredUpscale> {
        self.upscale
    }

    /// Interne Ziel-Groesse (bei Upscale = Display / Skala).
    pub(crate) fn internal_size(&self) -> (u32, u32) {
        self.size
    }
}
