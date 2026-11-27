//! GPU-UI-Pass (M4 P1): führt eine `re_render::ui::DrawList` auf der
//! Hardware aus — Vertex-Buffer aus den Quad-Ecken, Font-Atlas als
//! R8-Textur, Scissor pro Batch, premultipliziertes Source-over.
//!
//! Die CPU-Referenz [`re_render::rasterize`] ist die Wahrheit; dieser
//! Pass spiegelt ihre Konventionen (Pixelzentren, Integer-Scissor nach
//! [`re_render::ui::scissor_rect`], manuelles Bilinear im Shader statt
//! Hardware-Filter, Float-Akkumulation im RGBA16F-Ziel). Der Beweis-Test
//! `tests/ui_gpu.rs` difft beide Pfade Pixel für Pixel.

use crate::GfxError;
use re_render::ui::{scissor_rect, DrawList, FontAtlas, Rgba, UiVertex};

const UI_SHADER: &str = include_str!("../ui.wgsl");

/// Byte-Größe eines UI-Eckpunkts (pos vec2 + uv vec2 + color vec4).
const UI_VERTEX_SIZE: u64 = 32;

/// Hochgeladener Font-Atlas: R8-Textur + Bind-Group (Gruppe 0, Binding 0).
pub struct UiAtlasGpu {
    _tex: wgpu::Texture,
    bind: wgpu::BindGroup,
}

/// Ein UI-Eckpunkt als Bytes — manuell serialisiert, weil
/// `UiVertex` kein `#[repr(C)]` trägt; Reihenfolge muss zu `ui.wgsl`
/// passen: pos, uv, color (je f32, little endian).
fn push_vertex(buf: &mut Vec<u8>, v: &UiVertex) {
    for f in v.pos.iter().chain(v.uv.iter()).chain(v.color.iter()) {
        buf.extend_from_slice(&f.to_le_bytes());
    }
}

impl UiGpu {
    /// Kopiert den Atlas in eine R8Unorm-Textur und bindet sie für
    /// Gruppe 0 (Layout der Pipelines).
    pub fn upload_atlas(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &FontAtlas,
    ) -> UiAtlasGpu {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui-atlas"),
            size: wgpu::Extent3d {
                width: atlas.width,
                height: atlas.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // Copy-Zeilen müssen auf 256 Byte gepolstert sein (R8: 1 Byte
        // je Texel); wgpu übernimmt vom gepolsterten Staging-Buffer.
        let bpr = (atlas.width as usize + 255) & !255;
        let mut padded = vec![0u8; bpr * atlas.height as usize];
        for y in 0..atlas.height as usize {
            padded[y * bpr..y * bpr + atlas.width as usize].copy_from_slice(
                &atlas.pixels[y * atlas.width as usize..(y + 1) * atlas.width as usize],
            );
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &padded,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr as u32),
                rows_per_image: Some(atlas.height),
            },
            wgpu::Extent3d {
                width: atlas.width,
                height: atlas.height,
                depth_or_array_layers: 1,
            },
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui-atlas-bind"),
            layout: &self.bgl_atlas,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        UiAtlasGpu { _tex: tex, bind }
    }
}

/// Pipelines + Bind-Group-Layouts für den UI-Pass auf einem Ziel-Format.
/// Wird lazy pro Backend erzeugt und über Frames hinweg wiederverwendet.
pub struct UiGpu {
    pipe_textured: wgpu::RenderPipeline,
    pipe_solid: wgpu::RenderPipeline,
    bgl_atlas: wgpu::BindGroupLayout,
    bgl_size: wgpu::BindGroupLayout,
}

impl UiGpu {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui-shader"),
            source: wgpu::ShaderSource::Wgsl(UI_SHADER.into()),
        });
        let bgl_atlas = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui-g0-atlas"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    // filterable: false — der Shader liest Texel per
                    // textureLoad, ein Hardware-Filter wäre Verschwendung.
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let bgl_size = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui-g1-size"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout_textured = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui-layout-textured"),
            bind_group_layouts: &[Some(&bgl_atlas), Some(&bgl_size)],
            immediate_size: 0,
        });
        // Der Solid-Shader liest keinen Atlas — Gruppe 0 bleibt frei.
        let layout_solid = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui-layout-solid"),
            bind_group_layouts: &[None, Some(&bgl_size)],
            immediate_size: 0,
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: UI_VERTEX_SIZE,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 8,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 2,
                },
            ],
        };
        let blend = Some(wgpu::BlendState {
            // premultipliziertes Source-over — Ziel ist linear (kein sRGB),
            // damit die Blend-Arithmetik bitgleich zur CPU-Referenz bleibt.
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        });
        let pipe_textured = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui-textured"),
            layout: Some(&layout_textured),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_ui"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout.clone())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_ui_textured"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: blend.clone(),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let pipe_solid = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui-solid"),
            layout: Some(&layout_solid),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_ui"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout.clone())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_ui_solid"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipe_textured,
            pipe_solid,
            bgl_atlas,
            bgl_size,
        }
    }

    /// Malt die Draw-List in `target` (Viewport = ganze Zieltextur).
    /// `clear` ist linear RGBA — dieselbe Startfläche, die die CPU-
    /// Referenz `rasterize()` bekommt.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        target_size: (u32, u32),
        dl: &DrawList,
        atlas: Option<&UiAtlasGpu>,
        clear: Rgba,
    ) -> Result<(), GfxError> {
        // Alle Batches in einen Vertex-Buffer; je Batch (first, count).
        let mut verts: Vec<u8> = Vec::new();
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for cmd in dl.cmds() {
            if cmd.vertices.is_empty() {
                ranges.push((0, 0));
                continue;
            }
            let first = (verts.len() / UI_VERTEX_SIZE as usize) as u32;
            for v in &cmd.vertices {
                push_vertex(&mut verts, v);
            }
            ranges.push((first, cmd.vertices.len() as u32));
        }
        let vbuf = if verts.is_empty() {
            None
        } else {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ui-verts"),
                size: verts.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buf, 0, &verts);
            Some(buf)
        };
        let size_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui-size"),
            size: 8,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &size_buf,
            0,
            &[
                &(target_size.0 as f32).to_le_bytes()[..],
                &(target_size.1 as f32).to_le_bytes()[..],
            ]
            .concat(),
        );
        let bind_size = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui-size-bind"),
            layout: &self.bgl_size,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: size_buf.as_entire_binding(),
            }],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0] as f64,
                        g: clear[1] as f64,
                        b: clear[2] as f64,
                        a: clear[3] as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(1, &bind_size, &[]);
        let mut last_textured: Option<bool> = None;
        for (cmd, (first, count)) in dl.cmds().iter().zip(ranges.iter()) {
            if *count == 0 {
                continue;
            }
            let (sx, sy, sw, sh) = scissor_rect(&cmd.clip);
            if sw == 0 || sh == 0 {
                continue;
            }
            pass.set_scissor_rect(sx, sy, sw, sh);
            if last_textured != Some(cmd.textured) {
                if cmd.textured {
                    let atlas = atlas
                        .ok_or_else(|| GfxError::Surface("texturierter Batch ohne Atlas".into()))?;
                    pass.set_bind_group(0, &atlas.bind, &[]);
                    pass.set_pipeline(&self.pipe_textured);
                } else {
                    pass.set_pipeline(&self.pipe_solid);
                }
                last_textured = Some(cmd.textured);
            }
            if let Some(buf) = &vbuf {
                let lo = *first as u64 * UI_VERTEX_SIZE;
                let hi = (*first + *count) as u64 * UI_VERTEX_SIZE;
                pass.set_vertex_buffer(0, buf.slice(lo..hi));
                pass.draw(0..*count, 0..1);
            }
        }
        Ok(())
    }
}
