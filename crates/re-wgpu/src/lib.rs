//! # re-wgpu — Referenz-Backend für `re_render::GfxBackend`
//!
//! Öffentlicher Standard-Renderer: Vulkan (Linux), DX11/DX12 (Windows),
//! Metal (macOS) und WebGPU (wasm32) laufen über **ein** wgpu-HAL —
//! DX-Versionen werden nie selbst implementiert, naga übersetzt WGSL.
//!
//! M1-Stand: Offscreen-Rendering (Ziel-Textur + Readback zu PNG), damit
//! der Renderer auf dem headless Pi prüfbar ist und in der CI läuft.
//! M2-Stand: mit Feature `window` zusätzlich Fenster-Rendering — die App
//! reicht den winit-Fenster-Handle via [`GfxBackend::set_window_source`]
//! durch, das Backend erzeugt daraus eine Surface und konfiguriert einen
//! Swapchain. Ohne Feature bleibt das Crate headless baubar (Pi/CI).
//!
//! Shader: `shader.wgsl` — Lambert + Albedo-Textur oder Basisfarbe,
//! Transparenz über eine eigene Blend-Pipeline. Culling bleibt in M1
//! aus (Windings aus glTF/Primitives sind gemischt); M1b aktiviert es
//! wieder, sobald die Windings-Regel dokumentiert ist.

use std::collections::HashMap;
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Wake, Waker};

use re_core::mesh::Mesh;
use re_render::{Camera, DrawBatch, GfxBackend, GfxError};

pub mod deferred;
pub mod ui;

pub use deferred::{
    CLUSTER_CAP, CLUSTER_SLICES, CLUSTER_TILE, DeferredBatch, DeferredHdr, DeferredLight,
    DeferredShadow, DeferredUpscale, GBufferReadback, HdrReadback,
};
use deferred::Deferred;

const SHADER: &str = include_str!("../shader.wgsl");
const COPY_ROW_ALIGN: u32 = 256;
/// Lichtrichtung wie im ISO-Baker (iso.rs), damit Bakes und Echtzeitbild matchen.
/// Sonnenrichtung des Standard-Szenarios — zeigt zum Licht (y positiv
/// nach oben). Der Schatten-Kern will die Fallrichtung: negieren.
/// Tests referenzieren denselben Wert, damit CPU- und GPU-Seite
/// dieselbe Sonne sehen.
pub const LIGHT_DIR: [f32; 3] = [0.45, 0.82, 0.35];

#[repr(C)]
#[derive(Clone, Copy)]
struct CamUni {
    view_proj: [[f32; 4]; 4],
    light_dir_time: [f32; 4],
    /// Rueckwaertstransformation fuer die Schatten-Rekonstruktion des
    /// Deferred-Lightings (Clip → Welt). Der Forward-Shader nutzt sie
    /// nicht — das Uniform ist geteilt und darf groesser sein.
    inv_view_proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BatchUni {
    world: [[f32; 4]; 4],
    base_color: [f32; 4],
    /// .x = 1 → Textur sampeln; .y = 1 → Billboard (unlit, aus Sheet).
    /// Absicht: nur 16-Byte-Felder, damit Rust-Layout und WGSL-Uniform-
    /// Layout (vec3-align=16!) identisch sind.
    has_texture: [u32; 4],
    /// normalisiertes UV-Rechteck (u0, v0, du, dv) für Sheet-Ausschnitte.
    uv_rect: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Vtx {
    pos: [f32; 3],
    nrm: [f32; 3],
    uv: [f32; 2],
}

/// Bytes eines #[repr(C)]-POD-Werts (bewusst ohne bytemuck — hält den
/// Abhängigkeitsbaum klein; alle Structs hier sind Copy + repr(C)).
fn pod_bytes<T>(v: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v as *const T as *const u8, std::mem::size_of::<T>()) }
}

/// Bytes eines dichten POD-Vektors (Vtx/u32 — alle repr(C), keine Padding-Lücken).
fn pod_vec_bytes<T: Copy>(v: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * std::mem::size_of::<T>())
    }
}

const VERTEX_ATTRS: [wgpu::VertexAttribute; 3] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 0,
        shader_location: 0,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 12,
        shader_location: 1,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 24,
        shader_location: 2,
    },
];

fn mat4(m: glam::Mat4) -> [[f32; 4]; 4] {
    m.to_cols_array_2d()
}

/// Textur → RGBA-Rohbytes per Copy-Buffer (Zeilen ohne 256er-Padding).
/// `bpp` = Bytes/Pixel des Formats (RGBA8 und RG16F liefern je 4).
pub(crate) fn readback_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    w: u32,
    h: u32,
    bpp: u32,
) -> Result<Vec<u8>, GfxError> {
    readback_texture_layer(device, queue, tex, w, h, bpp, 0)
}

/// Wie `readback_texture`, aber von einer Schicht einer Array-Textur
/// (Shadow-Map je Kaskade) — der Copy-Origin zeigt in die Tiefe.
pub(crate) fn readback_texture_layer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    w: u32,
    h: u32,
    bpp: u32,
    layer: u32,
) -> Result<Vec<u8>, GfxError> {
    let unpadded = w * bpp;
    let padded = unpadded.div_ceil(COPY_ROW_ALIGN) * COPY_ROW_ALIGN;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback-tex"),
        size: u64::from(padded) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("readback-tex-enc"),
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| GfxError::Device(format!("poll: {e}")))?;
    rx.recv()
        .map_err(|_| GfxError::Device("map-Kanal abgerissen".into()))?
        .map_err(|e| GfxError::Device(format!("map: {e}")))?;
    let view = slice
        .get_mapped_range()
        .map_err(|e| GfxError::Device(format!("mapped range: {e}")))?;
    let mut out = Vec::with_capacity((w * h * bpp) as usize);
    for row in view.chunks_exact(padded as usize) {
        out.extend_from_slice(&row[..unpadded as usize]);
    }
    drop(view);
    staging.unmap();
    Ok(out)
}

/// Bytes eines Storage-Buffers (GPU-Inhalt → CPU) — fuer den
/// Listen-Vergleich des Compute-Cullings. Buffer-zu-Buffer-Kopien
/// kennen keine Zeilen-Ausrichtung, also genügt die exakte Größe.
pub(crate) fn readback_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    src: &wgpu::Buffer,
    size: u64,
) -> Result<Vec<u8>, GfxError> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback-buf"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("readback-buf-enc"),
    });
    enc.copy_buffer_to_buffer(src, 0, &staging, 0, size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| GfxError::Device(format!("poll: {e}")))?;
    rx.recv()
        .map_err(|_| GfxError::Device("map-Kanal abgerissen".into()))?
        .map_err(|e| GfxError::Device(format!("map: {e}")))?;
    let view = slice
        .get_mapped_range()
        .map_err(|e| GfxError::Device(format!("mapped range: {e}")))?;
    let out = view.to_vec();
    drop(view);
    staging.unmap();
    Ok(out)
}

/// wgpu v30 liefert `impl Future` für Adapter/Device-Request. Ohne
/// Async-Runtime im API-Vertrag des Traits: kleiner Spin-poll-Blocker
/// (die Requests sind nach wenigen Polls fertig, kein Langzeit-Blocker).
fn block_on<F: Future>(future: F) -> F::Output {
    struct NoopWake;
    impl Wake for NoopWake {
        fn wake(self: std::sync::Arc<Self>) {}
    }
    let waker = Waker::from(std::sync::Arc::new(NoopWake));
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

/// Fenster-Handle, das die Surface-Erzeugung überleben darf: hält das
/// `Arc<Window>` und delegiert die raw-window-handle-Traits darauf.
/// wgpu box't das Ziel in `SurfaceTarget::DisplayAndWindow` und der
/// Surface-Handle bleibt gültig, solange die Surface lebt — deshalb hier
/// ein owned Arc und kein borrowed Referenz-Handle.
#[cfg(feature = "window")]
#[derive(Clone)]
struct WindowSource(std::sync::Arc<winit::window::Window>);

#[cfg(feature = "window")]
impl wgpu::rwh::HasWindowHandle for WindowSource {
    fn window_handle(&self) -> Result<wgpu::rwh::WindowHandle<'_>, wgpu::rwh::HandleError> {
        self.0.window_handle()
    }
}

#[cfg(feature = "window")]
impl wgpu::rwh::HasDisplayHandle for WindowSource {
    fn display_handle(&self) -> Result<wgpu::rwh::DisplayHandle<'_>, wgpu::rwh::HandleError> {
        self.0.display_handle()
    }
}

#[derive(Default)]
struct MeshGpu {
    vertex: Option<wgpu::Buffer>,
    index: Option<wgpu::Buffer>,
    n_vertices: u32,
    n_indices: u32,
    /// Content stamp of the uploaded CPU mesh (see `re_core::mesh::Mesh::stamp`).
    stamp: u64,
}

struct Inner {
    device: wgpu::Device,
    queue: wgpu::Queue,
    size: (u32, u32),
    /// Bleibt alive, damit Surface/Adapter intern gültige Referenzen behalten.
    #[allow(dead_code)]
    instance: wgpu::Instance,
    /// Format des Farbziels: Surface-Format (Fenster) oder Rgba8UnormSrgb (Offscreen).
    format: wgpu::TextureFormat,
    /// Fenster-Surface; None = Offscreen-Ziel-Textur.
    surface: Option<wgpu::Surface<'static>>,
    surface_cfg: Option<wgpu::SurfaceConfiguration>,
    #[allow(dead_code)] // bleibt alive, solange color_view referenziert wird (nur Offscreen echtes Ziel)
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    #[allow(dead_code)] // dito für depth_view
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    white_view: wgpu::TextureView,
    cam_buf: wgpu::Buffer,
    g1: wgpu::BindGroupLayout,
    cam_bind: wgpu::BindGroup,
    pipe_opaque: wgpu::RenderPipeline,
    pipe_blend: wgpu::RenderPipeline,
    /// TODO(M1c): Key durch stabile Asset-IDs ersetzen; Zeiger-Keys sind
    /// nur für den Offscreen-Demo-Pfad okay.
    mesh_cache: HashMap<usize, MeshGpu>,
    /// Stream-Zellen in GPU-Residenz (M3c-Handoff): explizit vergebene
    /// 64-Bit-Ids → Buffer. `upload_cell` legt an, `release_cell`
    /// nimmt weg; Batches mit `cell`-Id zeichnen nur, was hier liegt.
    cell_cache: HashMap<u64, MeshGpu>,
    tex_cache: HashMap<String, (wgpu::Texture, wgpu::TextureView)>,
    /// Billboard-Sheets unter frei gewählter ID (Upload vom Aufrufer).
    sheet_cache: HashMap<u64, (wgpu::Texture, wgpu::TextureView, u32, u32)>,
    /// UI-Pipelines für den Overlay-Pass aufs Farbziel (M4 P1): Format
    /// ist das des eigentlichen Ziels, Pipeline lazy beim ersten Overlay.
    ui_over: Option<ui::UiGpu>,
}

/// Für den Render-Pass vorbereitete Draw-Ressourcen (alle Handles owned).
struct Prepared {
    blend: bool,
    vbuf: wgpu::Buffer,
    ibuf: Option<wgpu::Buffer>,
    n_vertices: u32,
    n_indices: u32,
    bind: wgpu::BindGroup,
}

/// wgpu-Backend — erst `init()`, dann pro Frame `render()`; `readback_rgba`
/// holt das Ergebnis als RGBA8 (PNG-Export/Tests).
#[derive(Default)]
pub struct WgpuBackend {
    inner: Option<Inner>,
    frames: u64,
    /// Deferred-Zustand (M5): lazy in der aktuellen Zielgröße erzeugt.
    deferred: Option<Deferred>,
    /// UI-Pass-Zustand (M4 P1): Pipelines, lazy für RGBA16F-Ziele.
    ui: Option<ui::UiGpu>,
    /// Vor `init` gesetztes Fenster-Ziel (Feature `window`).
    #[cfg(feature = "window")]
    window_src: Option<std::sync::Arc<winit::window::Window>>,
}

impl WgpuBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fenster-Modus: Handle des `re-platform`-Fensters übernehmen
    /// (`desktop::WindowHandleSource`, als `Arc<dyn Any>` durchgereicht).
    /// Muss vor [`GfxBackend::init`] aufgerufen werden.
    #[cfg(feature = "window")]
    pub fn set_window_source(
        &mut self,
        src: std::sync::Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), GfxError> {
        if self.inner.is_some() {
            return Err(GfxError::Surface("set_window_source nur vor init()".into()));
        }
        let window = src
            .downcast::<winit::window::Window>()
            .map_err(|_| GfxError::Surface("fremder Window-Handle-Typ".into()))?;
        self.window_src = Some(window);
        Ok(())
    }

    /// true, wenn im Fenster-Modus gerendert wird (kein Readback möglich).
    pub fn is_windowed(&self) -> bool {
        self.inner.as_ref().is_some_and(|i| i.surface.is_some())
    }

    pub fn rendered_frames(&self) -> u64 {
        self.frames
    }

    /// Hochgeladene Texturen: Albedo-Dateien + Billboard-Sheets.
    pub fn textures_loaded(&self) -> usize {
        self.inner
            .as_ref()
            .map(|i| i.tex_cache.len() + i.sheet_cache.len())
            .unwrap_or(0)
    }

    // ---------- Setup ----------

    fn init_inner(&mut self) -> Result<(), GfxError> {
        let instance = wgpu::Instance::default();

        // Fenster-Ziel (Feature `window`): Surface aus dem winit-Handle.
        #[cfg(feature = "window")]
        let surface = match &self.window_src {
            Some(window) => Some(
                instance
                    .create_surface(WindowSource(window.clone()))
                    .map_err(|e| GfxError::Surface(e.to_string()))?,
            ),
            None => None,
        };
        #[cfg(not(feature = "window"))]
        let surface: Option<wgpu::Surface<'static>> = None;

        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: surface.as_ref(),
            apply_limit_buckets: false,
        }))
        .map_err(|e| GfxError::Device(format!("adapter: {e}")))?;

        // downlevel_defaults = lowest common denominator (Vulkan…WebGPU).
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("re-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            ..Default::default()
        }))
        .map_err(|e| GfxError::Device(e.to_string()))?;

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("re-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let g0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("g0-cam"),
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
        let g1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("g1-batch"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear-clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // 1×1-Weiß als Fallback für Materialien ohne Textur.
        let white = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255, 255, 255, 255],
            wgpu::TexelCopyBufferLayout::default(),
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_view = white.create_view(&wgpu::TextureViewDescriptor::default());

        let cam_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cam-uni"),
            size: std::mem::size_of::<CamUni>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cam_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cam-bind"),
            layout: &g0,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: cam_buf.as_entire_binding(),
            }],
        });

        // Swapchain für die Fenster-Surface: ihr Format bestimmt die
        // Pipelines. Erstkonfiguration mit der physischen Fenstergröße;
        // der Created-Event ruft anschließend ohnehin `resize` auf.
        let (surface_cfg, format) = match &surface {
            Some(s) => {
                #[cfg(feature = "window")]
                let (cw, ch) = {
                    let w = self.window_src.as_ref().expect("Surface nur mit window_src");
                    let sz = w.inner_size();
                    (sz.width.max(1), sz.height.max(1))
                };
                #[cfg(not(feature = "window"))]
                let (cw, ch) = (1280u32, 720u32);
                let cfg = s
                    .get_default_config(&adapter, cw, ch)
                    .ok_or_else(|| GfxError::Surface("Surface-Konfiguration nicht verfügbar".into()))?;
                s.configure(&device, &cfg);
                let f = cfg.format;
                (Some(cfg), f)
            }
            None => (None, wgpu::TextureFormat::Rgba8UnormSrgb),
        };

        let pipe_opaque = make_pipeline(&device, &module, &g0, &g1, false, format);
        let pipe_blend = make_pipeline(&device, &module, &g0, &g1, true, format);

        let mut inner = Inner {
            device,
            queue,
            size: (0, 0),
            instance,
            format,
            surface,
            surface_cfg,
            color: white.clone(),
            color_view: white_view.clone(),
            depth: white,
            depth_view: white_view.clone(),
            sampler,
            white_view,
            cam_buf,
            g1,
            cam_bind,
            pipe_opaque,
            pipe_blend,
            mesh_cache: HashMap::new(),
            cell_cache: HashMap::new(),
            tex_cache: HashMap::new(),
            sheet_cache: HashMap::new(),
            ui_over: None,
        };
        inner.make_target(1280, 720);
        self.inner = Some(inner);
        Ok(())
    }

    // ---------- UI (M4 P1) ----------

    /// UI-Pass auf ein eigenes RGBA16F-Ziel (linear): malt `dl` über den
    /// Font-Atlas `atlas` und liefert das Ergebnis quantisiert als RGBA8
    /// — Byte-layout identisch zu `re_render::rasterize()`, damit der
    /// Beweis-Test Pixel für Pixel diffen kann. Float-Akkumulation im
    /// 16F-Ziel spiegelt dabei die float-Akkumulation der CPU-Referenz.
    pub fn render_ui_offscreen(
        &mut self,
        dl: &re_render::ui::DrawList,
        atlas: &re_render::ui::FontAtlas,
        w: u32,
        h: u32,
        clear: re_render::ui::Rgba,
    ) -> Result<Vec<u8>, GfxError> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;
        let ui = self
            .ui
            .get_or_insert_with(|| ui::UiGpu::new(&inner.device, wgpu::TextureFormat::Rgba16Float));
        let target = inner.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui-target"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let gpu_atlas = ui.upload_atlas(&inner.device, &inner.queue, atlas);
        let mut encoder = inner
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ui-frame") });
        ui.render(
            &inner.device,
            &inner.queue,
            &mut encoder,
            &view,
            (w.max(1), h.max(1)),
            dl,
            Some(&gpu_atlas),
            clear,
        )?;
        inner.queue.submit(std::iter::once(encoder.finish()));
        // RGBA16F = 8 Byte/Pixel (4 Kanäle à 2 Byte).
        let raw = readback_texture(&inner.device, &inner.queue, &target, w.max(1), h.max(1), 8)?;
        // RGBA16F-Rohbytes → f32 → dieselbe 8-Bit-Quantisierung wie die
        // CPU-Referenz (Klemmen auf 0..1, kaufmännisches Runden).
        let n = (w.max(1) * h.max(1)) as usize;
        let mut out = vec![0u8; n * 4];
        for i in 0..n {
            for k in 0..4 {
                let bits = u16::from_le_bytes([raw[i * 8 + k * 2], raw[i * 8 + k * 2 + 1]]);
                let v = f32::from_bits(deferred::half_to_f32_bits(bits));
                out[i * 4 + k] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
        Ok(out)
    }

    // ---------- Bild ----------

    /// Inhalt des letzten Frames als RGBA8 (Zeilen ohne Copy-Padding).
    /// Nur im Offscreen-Modus — das Fenster präsentiert direkt auf den
    /// Bildschirm, dort gibt es kein auslesbares Ziel.
    pub fn readback_rgba(&mut self) -> Result<(u32, u32, Vec<u8>), GfxError> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;
        if inner.surface.is_some() {
            return Err(GfxError::Surface("Readback im Fenstermodus nicht verfügbar".into()));
        }
        let (w, h) = inner.size;
        let unpadded = w * 4;
        let padded = unpadded.div_ceil(COPY_ROW_ALIGN) * COPY_ROW_ALIGN;
        let staging = inner.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = inner.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("re-readback"),
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &inner.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        inner.queue.submit([enc.finish()]);

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        inner
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| GfxError::Device(format!("poll: {e}")))?;
        rx.recv()
            .map_err(|_| GfxError::Device("map-Kanal abgerissen".into()))?
            .map_err(|e| GfxError::Device(format!("map: {e}")))?;
        let view = slice
            .get_mapped_range()
            .map_err(|e| GfxError::Device(format!("mapped range: {e}")))?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in view.chunks_exact(padded as usize) {
            out.extend_from_slice(&row[..unpadded as usize]);
        }
        drop(view);
        staging.unmap();
        Ok((w, h, out))
    }

    // ---------- Deferred (M5) ----------

    /// Ein Deferred-Frame: Geometrie-Pass (MRT-G-Buffer) → optionaler
    /// Schatten-Werfer (Shadow 2.0) → Lighting-Pass → Tone-Mapping →
    /// optional FSR1-Upscale (M5, Box 3: intern mit 1/`upscale.scale`
    /// rendern, EASU + RCAS aufs Display). Die Pass-Reihenfolge kommt
    /// aus dem Render-Graphen (M5, Box 5); `deferred_pass_order()` liefert
    /// den ausgeführten Plan — mit `Some(shadow)` steckt der `d-shadow`-
    /// Knoten darin. Das Ergebnis liegt im Farb-Ziel —
    /// `read_deferred_rgba()` liefert es in Display-Groesse. Nur headless
    /// sinnvoll (Readback-Proof).
    pub fn render_deferred(
        &mut self,
        camera: &Camera,
        batches: &[DeferredBatch],
        hdr: DeferredHdr,
        lights: &[DeferredLight],
        upscale: Option<DeferredUpscale>,
        shadow: Option<DeferredShadow>,
    ) -> Result<(), GfxError> {
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;
        // Neue Upscale- oder Schatten-Einstellung => andere Ziele bzw.
        // anderer Graph => neu bauen.
        let stale = match &self.deferred {
            Some(d) => d.upscale() != upscale || d.shadow() != shadow,
            None => true,
        };
        if stale {
            self.deferred = Some(Deferred::new(inner, upscale, shadow));
        }
        self.deferred
            .as_mut()
            .expect("gerade erzeugt")
            .render(inner, camera, batches, hdr, lights)?;
        self.frames += 1;
        Ok(())
    }

    /// Roh-Inhalt der G-Buffer-Targets des letzten Deferred-Frames.
    pub fn read_gbuffer(&self) -> Result<GBufferReadback, GfxError> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;
        let d = self
            .deferred
            .as_ref()
            .ok_or_else(|| GfxError::Device("kein Deferred-Frame gerendert".into()))?;
        d.read_gbuffer(inner)
    }

    /// Roh-Inhalt des HDR-Float-Targets (vor Tone-Mapping) des letzten
    /// Deferred-Frames — fuer den Pixel-Diff gegen die CPU-Referenz.
    pub fn read_hdr(&self) -> Result<HdrReadback, GfxError> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;
        let d = self
            .deferred
            .as_ref()
            .ok_or_else(|| GfxError::Device("kein Deferred-Frame gerendert".into()))?;
        d.read_hdr(inner)
    }

    /// Die Depth-Buffer des letzten Deferred-Frames als f32 — der
    /// Test rekonstruiert damit dieselbe Sichttiefe wie der Shader.
    pub fn read_deferred_depth(&self) -> Result<Vec<f32>, GfxError> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;
        let d = self
            .deferred
            .as_ref()
            .ok_or_else(|| GfxError::Device("kein Deferred-Frame gerendert".into()))?;
        d.read_depth(inner)
    }

    /// Die Licht-Listen des GPU-Compute-Cullings (CLUSTER_CAP Eintraege
    /// je Cluster: Slot 0 = Anzahl, Slot 1.. = Indizes).
    pub fn read_cluster_lights(&self) -> Result<Vec<u32>, GfxError> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;
        let d = self
            .deferred
            .as_ref()
            .ok_or_else(|| GfxError::Device("kein Deferred-Frame gerendert".into()))?;
        d.read_cluster_lights(inner)
    }

    /// Das beleuchtete Deferred-Endergebnis als RGBA8 (Zeilen ohne
    /// Copy-Padding) — bei Upscale das FSR1-Bild in Display-Groesse,
    /// sonst das tone-gemappte interne Ziel. Seitenarm zu
    /// `readback_rgba()` für den Forward-Pfad.
    pub fn read_deferred_rgba(&self) -> Result<(u32, u32, Vec<u8>), GfxError> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;
        let d = self
            .deferred
            .as_ref()
            .ok_or_else(|| GfxError::Device("kein Deferred-Frame gerendert".into()))?;
        let (w, h) = inner.size;
        Ok((w.max(1), h.max(1), d.read_final(inner)?))
    }

    /// Die Shadow-Map der Kaskade `cascade` als f32-Tiefen (quadratisch,
    /// Zeilen ohne Padding) — fuer den Beweis gegen die CPU-Referenz.
    /// Groesse (Kantenlaenge) und Tiefen 0..1; 1.0 = nichts gezeichnet.
    pub fn read_deferred_shadow_map(&self, cascade: usize) -> Result<(u32, Vec<f32>), GfxError> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;
        let d = self
            .deferred
            .as_ref()
            .ok_or_else(|| GfxError::Device("kein Deferred-Frame gerendert".into()))?;
        d.read_shadow_map(inner, cascade)
    }

    /// Die Pass-Folge des letzten Deferred-Frames — der Beweis, dass
    /// der Executor dem kompilierten Render-Graphen folgt (M5, Box 5).
    /// Vor dem ersten Deferred-Frame kommt der Plan ohne Ausführung.
    pub fn deferred_pass_order(&self) -> Result<Vec<String>, GfxError> {
        let d = self
            .deferred
            .as_ref()
            .ok_or_else(|| GfxError::Device("kein Deferred-Frame gerendert".into()))?;
        Ok(d.pass_order().to_vec())
    }

    /// Das tone-gemappte interne Ziel in interner Groesse — bei Upscale
    /// die display-kodierte Eingabe der FSR1-Kette (CPU-Referenz-Beweis).
    pub fn read_deferred_internal(&self) -> Result<(u32, u32, Vec<u8>), GfxError> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;
        let d = self
            .deferred
            .as_ref()
            .ok_or_else(|| GfxError::Device("kein Deferred-Frame gerendert".into()))?;
        let (w, h) = d.internal_size();
        Ok((w, h, d.read_internal(inner)?))
    }

    /// Sichtbeleg: das beleuchtete Deferred-Ergebnis als PNG.
    pub fn export_deferred_png(&self, path: &str) -> Result<(), GfxError> {
        let (w, h, rgba) = self.read_deferred_rgba()?;
        image::save_buffer(path, &rgba, w, h, image::ColorType::Rgba8)
            .map_err(|e| GfxError::Device(format!("PNG: {e}")))?;
        Ok(())
    }
}

impl Inner {
    /// Zeichenziele (neu) anlegen. Die Depth-Textur gibt es in beiden
    /// Modi; die Farb-Ziel-Textur nur headless — im Fenstermodus liefert
    /// die Surface pro Frame ihre eigenen Texturen.
    fn make_target(&mut self, w: u32, h: u32) {
        let size = wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        };
        if self.surface.is_none() {
            let color = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("target"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            self.color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
            self.color = color;
        }
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        self.depth = depth;
        self.size = (w.max(1), h.max(1));
    }

    /// Swapchain neu konfigurieren + Depth anpassen. `configure` panickt,
    /// wenn noch eine Surface-Textur lebt — die hält render() nur lokal
    /// und hat sie vor diesem Aufruf längst gepresent.
    fn resize(&mut self, w: u32, h: u32) {
        let (w, h) = (w.max(1), h.max(1));
        if let (Some(surface), Some(cfg)) = (&self.surface, self.surface_cfg.as_mut()) {
            if cfg.width != w || cfg.height != h {
                cfg.width = w;
                cfg.height = h;
                surface.configure(&self.device, cfg);
            }
        }
        self.make_target(w, h);
    }
}

impl Inner {
    fn ensure_mesh(&mut self, mesh: &Mesh) {
        // Key is the address, validity is the content stamp: in-place edits
        // (terrain brush, decimate) refresh the stamp, and a fresh mesh that
        // inherits this address gets a new one — both force a re-upload.
        let key = (mesh as *const Mesh) as usize;
        if self.mesh_cache.get(&key).is_some_and(|g| g.stamp == mesh.stamp) {
            return;
        }
        let gpu = self.upload_mesh_gpu(mesh);
        self.mesh_cache.insert(key, gpu);
    }

    /// GPU-Buffer (Vertex + Index) fuer ein Mesh anlegen und fuellen —
    /// geteilter Kern von `ensure_mesh` und `upload_cell`.
    fn upload_mesh_gpu(&mut self, mesh: &Mesh) -> MeshGpu {
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
        let vertex = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-vert"),
            size: (verts.len() * std::mem::size_of::<Vtx>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&vertex, 0, pod_vec_bytes(&verts));
        let mut gpu = MeshGpu {
            vertex: Some(vertex),
            index: None,
            n_vertices: mesh.positions.len() as u32,
            n_indices: 0,
            stamp: mesh.stamp,
        };
        if let Some(idx) = mesh.indices.as_ref().filter(|v| !v.is_empty()) {
            let b = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh-idx"),
                size: (idx.len() * 4) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&b, 0, pod_vec_bytes(idx));
            gpu.n_indices = idx.len() as u32;
            gpu.index = Some(b);
        }
        gpu
    }

    fn ensure_texture(&mut self, path: &str) -> Option<wgpu::TextureView> {
        if let Some((_, v)) = self.tex_cache.get(path) {
            return Some(v.clone());
        }
        // Dekodierung zentral in re-io (PNG/JPEG/TGA/BMP/QOI + natives
        // DDS und KTX2). Der Größenwächter bleibt: erst Kopf prüfen,
        // dann decodieren — die Pi-RAM ist kniffelig. Für DDS und KTX2
        // misst re-io selbst (16k-Grenze), `image` kennt diese
        // Formate nicht.
        let native = path
            .rsplit('.')
            .next()
            .is_some_and(|e| e.eq_ignore_ascii_case("dds") || e.eq_ignore_ascii_case("ktx2"));
        if !native {
            let (dw, dh) = image::ImageReader::open(path)
                .ok()?
                .with_guessed_format()
                .ok()?
                .into_dimensions()
                .ok()?;
            if dw > 8192 || dh > 8192 {
                return None;
            }
        }
        let img = re_io::tex::decode(std::path::Path::new(path)).ok()?;
        let (w, h) = (img.width, img.height);
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("albedo"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.tex_cache
            .insert(path.to_string(), (tex, view.clone()));
        Some(view)
    }
}

fn make_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    g0: &wgpu::BindGroupLayout,
    g1: &wgpu::BindGroupLayout,
    blend: bool,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("re-layout"),
        bind_group_layouts: &[Some(g0), Some(g1)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if blend { "re-blend" } else { "re-opaque" }),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vtx>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRS,
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None, // M1: Windings gemischt, siehe Moduldoku
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(!blend),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: blend.then(|| wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

impl GfxBackend for WgpuBackend {
    fn name(&self) -> &'static str {
        "wgpu"
    }

    fn init(&mut self) -> Result<(), GfxError> {
        if self.inner.is_some() {
            return Ok(());
        }
        self.init_inner()
    }

    fn resize(&mut self, width: u32, height: u32) {
        if let Some(inner) = self.inner.as_mut() {
            inner.resize(width, height);
            // Deferred-Targets passen nicht mehr — beim naechsten
            // render_deferred in neuer Groesse neu anlegen.
            self.deferred = None;
        }
    }

    /// Fenster-Handle übernehmen (nur mit Feature `window` baubar).
    #[cfg(feature = "window")]
    fn set_window_source(
        &mut self,
        src: std::sync::Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), GfxError> {
        WgpuBackend::set_window_source(self, src)
    }

    fn upload_texture(&mut self, id: u64, rgba: &[u8], width: u32, height: u32) -> Result<(), GfxError> {
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;
        let need = (width as usize) * (height as usize) * 4;
        if width == 0 || height == 0 || rgba.len() < need {
            return Err(GfxError::Device(format!(
                "upload_texture({id}): {width}x{height} braucht {need} RGBA-Bytes, hat {}",
                rgba.len()
            )));
        }
        let tex = inner.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sheet"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // SC1-Palettenwerte sind sRGB — wie bei den Albedo-Texturen.
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        inner.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        inner.sheet_cache.insert(id, (tex, view, width, height));
        Ok(())
    }

    /// Stream-Zelle in die GPU-Residenz (M3c-Handoff): Buffer anlegen,
    /// ab jetzt zeichnen Batches mit `cell == Some(id)`. Doppelte
    /// Anmeldung wird ignoriert — die Pipeline liefert nur einmal.
    fn upload_cell(&mut self, id: u64, mesh: &Mesh) -> Result<(), GfxError> {
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;
        if !inner.cell_cache.contains_key(&id) {
            let gpu = inner.upload_mesh_gpu(mesh);
            inner.cell_cache.insert(id, gpu);
        }
        Ok(())
    }

    /// GPU-Ressourcen einer Zelle freigeben (Schonfrist ist vorbei).
    fn release_cell(&mut self, id: u64) {
        if let Some(inner) = self.inner.as_mut() {
            inner.cell_cache.remove(&id);
        }
    }

    fn render(&mut self, camera: &Camera, batches: &[DrawBatch]) -> Result<(), GfxError> {
        self.render_with_ui(camera, batches, None)
    }
}

impl WgpuBackend {
    /// Szenen-Bild mit optionalem UI-Overlay auf demselben Farbziel
    /// (M4 P1): erst Szenen-Pass, dann UI-Pass (LoadOp::Load), dann
    /// wie üblich present. `ui_dl` = Draw-List + Font-Atlas.
    pub fn render_with_ui(
        &mut self,
        camera: &Camera,
        batches: &[DrawBatch],
        ui_dl: Option<(&re_render::ui::DrawList, &re_render::ui::FontAtlas)>,
    ) -> Result<(), GfxError> {
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| GfxError::Device("init nicht ausgeführt".into()))?;

        let cam = CamUni {
            view_proj: mat4(camera.view_proj()),
            light_dir_time: [LIGHT_DIR[0], LIGHT_DIR[1], LIGHT_DIR[2], 0.0],
            inv_view_proj: mat4(camera.view_proj().inverse()),
        };
        inner.queue.write_buffer(&inner.cam_buf, 0, pod_bytes(&cam));

        // Phase 1: Buffers/Textures/Bind-Groups vorbereiten (inner &mut).
        // Opaque zuerst, Transparent danach (Konvention des Traits).
        let mut prepared: Vec<Prepared> = Vec::with_capacity(batches.len());
        for pass_kind in [false, true] {
            for b in batches {
                // Billboard-Sprites gehören immer in den Blend-Pass.
                let transparent = b.sprite.is_some() || b.material.base_color[3] < 0.999;
                if transparent != pass_kind || b.mesh.positions.is_empty() {
                    continue;
                }
                // Zell-Batches zeichnen aus der GPU-Residenz (M3c):
                // fehlt die Zelle, wird uebersprungen, nie nachgeladen.
                let mg = match b.cell {
                    Some(id) => match inner.cell_cache.get(&id) {
                        Some(mg) => mg,
                        None => continue,
                    },
                    None => {
                        inner.ensure_mesh(b.mesh);
                        let key = (b.mesh as *const Mesh) as usize;
                        match inner.mesh_cache.get(&key) {
                            Some(mg) => mg,
                            None => continue,
                        }
                    }
                };
                let (Some(vbuf), ibuf) = (mg.vertex.clone(), mg.index.clone()) else {
                    continue;
                };
                let (n_vertices, n_indices) = (mg.n_vertices, mg.n_indices);

                let (tex_view, has_texture, uv_rect) = match &b.sprite {
                    Some(sp) => match inner.sheet_cache.get(&sp.texture) {
                        Some((_, v, w, h)) => {
                            (v.clone(), [1u32, 1, 0, 0], re_render::uv_rect(sp, *w, *h))
                        }
                        None => (inner.white_view.clone(), [0u32; 4], [0.0, 0.0, 1.0, 1.0]),
                    },
                    None => {
                        let has_tex = match &b.material.albedo_texture_id {
                            Some(p) => inner.ensure_texture(p).is_some(),
                            None => false,
                        };
                        let v = b
                            .material
                            .albedo_texture_id
                            .as_ref()
                            .and_then(|p| inner.tex_cache.get(p).map(|(_, v)| v.clone()))
                            .unwrap_or_else(|| inner.white_view.clone());
                        (v, [has_tex as u32, 0, 0, 0], [0.0, 0.0, 1.0, 1.0])
                    }
                };

                let uni = BatchUni {
                    world: mat4(b.world),
                    base_color: b.material.base_color,
                    has_texture,
                    uv_rect,
                };
                let uni_buf = inner
                    .device
                    .create_buffer(&wgpu::BufferDescriptor {
                        label: Some("batch-uni"),
                        size: std::mem::size_of::<BatchUni>() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                inner.queue.write_buffer(&uni_buf, 0, pod_bytes(&uni));
                let sampler = inner.sampler.clone();
                let bind = inner.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("batch-bind"),
                    layout: &inner.g1,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uni_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&tex_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                    ],
                });

                prepared.push(Prepared {
                    blend: transparent,
                    vbuf,
                    ibuf,
                    n_vertices,
                    n_indices,
                    bind,
                });
            }
        }

        // Phase 2: ein Render-Pass mit geklonten Handles (inner wird frei).
        let device = inner.device.clone();
        let queue = inner.queue.clone();
        let depth_view = inner.depth_view.clone();
        // Farbziel: im Fenstermodus die aktuelle Swapchain-Textur (der
        // SurfaceTexture-Handle muss bis zum present() alive bleiben),
        // headless die feste Ziel-Textur.
        let mut frame: Option<wgpu::SurfaceTexture> = None;
        let color_view = match &inner.surface {
            Some(surface) => match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(st)
                | wgpu::CurrentSurfaceTexture::Suboptimal(st) => {
                    let v = st.texture.create_view(&wgpu::TextureViewDescriptor::default());
                    frame = Some(st);
                    v
                }
                // Verdeckt/minimiert oder Acquire-Knappheit: Frame folgenlos überspringen.
                wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                    return Ok(())
                }
                // Outdated/Lost: Swapchain mit gespeicherter Konfiguration
                // neu anlegen, diesen Frame auslassen.
                _ => {
                    if let Some(cfg) = inner.surface_cfg.clone() {
                        surface.configure(&inner.device, &cfg);
                    }
                    return Ok(());
                }
            },
            None => inner.color_view.clone(),
        };
        let cam_bind = inner.cam_bind.clone();
        let pipe_opaque = inner.pipe_opaque.clone();
        let pipe_blend = inner.pipe_blend.clone();

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("re-frame"),
        });
        {
            let c = camera.clear_color;
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("re-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: c[0] as f64,
                            g: c[1] as f64,
                            b: c[2] as f64,
                            a: c[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            let mut current_blend: Option<bool> = None;
            for p in &prepared {
                if current_blend != Some(p.blend) {
                    pass.set_pipeline(if p.blend { &pipe_blend } else { &pipe_opaque });
                    pass.set_bind_group(0, &cam_bind, &[]);
                    current_blend = Some(p.blend);
                }
                pass.set_bind_group(1, &p.bind, &[]);
                pass.set_vertex_buffer(0, p.vbuf.slice(..));
                if let Some(ib) = &p.ibuf {
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..p.n_indices, 0, 0..1);
                } else {
                    pass.draw(0..p.n_vertices, 0..1);
                }
            }
        }
        // UI-Overlay (M4 P1): Panels über dasselbe Farbziel malen, ohne
        // es zu löschen; Pipeline lazy für das Ziel-Format angelegt.
        if let Some((dl, atlas)) = ui_dl {
            let (fmt, size) = (inner.format, inner.size);
            let gpu = inner.ui_over.get_or_insert_with(|| ui::UiGpu::new(&device, fmt));
            let gpu_atlas = gpu.upload_atlas(&device, &queue, atlas);
            gpu.render_overlay(&device, &queue, &mut enc, &color_view, size, dl, Some(&gpu_atlas))?;
        }
        queue.submit([enc.finish()]);
        if let Some(st) = frame {
            // wgpu 30: Presenten ist ein Queue-Schritt NACH dem Submit.
            queue.present(st);
        }
        self.frames += 1;
        Ok(())
    }
}
