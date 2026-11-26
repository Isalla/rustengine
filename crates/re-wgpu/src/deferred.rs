//! Deferred-Pipeline V2 (M5, Box 1 + HDR-Aufsatz): MRT-G-Buffer +
//! Lighting-Pass (HDR-Float) + Tone-Mapping.
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
use re_render::{Camera, GfxError, ToneCurve};

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
    #[allow(dead_code)] // bleibt alive, solange depth_view referenziert wird
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
    light_bind: wgpu::BindGroup,
    pipe_geometry: wgpu::RenderPipeline,
    pipe_light: wgpu::RenderPipeline,
    pipe_tonemap: wgpu::RenderPipeline,
    size: (u32, u32),
}

impl Deferred {
    pub(crate) fn new(inner: &Inner) -> Self {
        let device = &inner.device;
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
        let gb0 = Self::tex_rgba8(device, w, h, "gb0");
        let gb1 = Self::tex_rgba8(device, w, h, "gb1");
        let gb2 = Self::tex_rg16f(device, w, h, "gb2");
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("d-depth"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        // Ziel des Tone-Mapping-Passes: sRGB wie der Forward-Offscreen-
        // Pfad, damit read_deferred_rgba() das Endergebnis liefert.
        let final_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("d-final"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        // HDR-Ziel des Lighting-Passes: RGBA32F statt RGBA16F, weil
        // wgpu das Sampeln von 16F-Targets die instabile f16-Feature
        // abverlangt — 32F laeuft ueberall (auch Software-Adapter/CI)
        // und die Testgroesse ist Speicher-egal. Desktop-Tier darf spaeter
        // auf 16F wechseln (Format ist eine Profil-Entscheidung, M13).
        let hdr = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("d-hdr"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
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

        let light_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("d-light-bind"),
            layout: &g1_light,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&gb0_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&gb1_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&gb2_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 4, resource: post_buf.as_entire_binding() },
            ],
        });
        let post_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("d-post-bind"),
            layout: &g_post,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&hdr_view) },
                wgpu::BindGroupEntry { binding: 1, resource: post_buf.as_entire_binding() },
            ],
        });

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
            light_bind,
            pipe_geometry,
            pipe_light,
            pipe_tonemap,
            size: (w.max(1), h.max(1)),
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

    /// Ein Frame: Geometrie-Pass (MRT) → Lighting-Pass (→ HDR-Float) →
    /// Tone-Mapping (→ `final_tex`). Ruft vorher `resize` auf, wenn die
    /// Größe wechselt.
    pub(crate) fn render(
        &mut self,
        inner: &mut Inner,
        camera: &Camera,
        batches: &[DeferredBatch],
        hdr: DeferredHdr,
    ) -> Result<(), GfxError> {
        if self.size != inner.size {
            self.resize(inner);
        }
        let post = hdr.to_uni()?;
        let cam = CamUni {
            view_proj: crate::mat4(camera.view_proj()),
            light_dir_time: [LIGHT_DIR[0], LIGHT_DIR[1], LIGHT_DIR[2], 0.0],
        };
        inner.queue.write_buffer(&self.cam_buf, 0, pod_bytes(&cam));
        inner.queue.write_buffer(&self.post_buf, 0, pod_bytes(&post));

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
        {
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
        {
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
            pass.set_bind_group(1, &self.light_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        {
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
        queue.submit([enc.finish()]);
        Ok(())
    }

    /// Das beleuchtete Endergebnis (`final_tex`) als RGBA8-Bytes — der
    /// Deferred-Seitenarm zum Forward-`readback_rgba`.
    pub(crate) fn read_final(&self, inner: &Inner) -> Result<Vec<u8>, GfxError> {
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
        *self = Self::new(inner);
    }
}
