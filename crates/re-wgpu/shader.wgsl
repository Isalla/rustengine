// RE1 M1 — Basis-PBR-Light: Lambert + Albedo-Textur oder Basisfarbe.
// Dieselbe Quelle läuft auf Vulkan, DX12, Metal und WebGPU (naga übersetzt).

struct Camera {
    view_proj : mat4x4<f32>,
    light_dir_time : vec4<f32>, // xyz = Lichtichtung (show), w = Zeit (ungenutzt)
};
@group(0) @binding(0) var<uniform> cam : Camera;

struct Batch {
    world : mat4x4<f32>,
    base_color : vec4<f32>,
    has_texture : vec4<u32>, // .x = 1 → Albedo sampeln, .y = 1 → Billboard (unlit)
    uv_rect : vec4<f32>,     // Sheet-Ausschnitt (u0, v0, du, dv); sonst (0,0,1,1)
};
@group(1) @binding(0) var<uniform> batch : Batch;
@group(1) @binding(1) var albedo : texture_2d<f32>;
@group(1) @binding(2) var samp : sampler;

struct VsIn {
    @location(0) pos : vec3<f32>,
    @location(1) nrm : vec3<f32>,
    @location(2) uv : vec2<f32>,
};

struct VsOut {
    @builtin(position) clip : vec4<f32>,
    @location(0) uv : vec2<f32>,
    @location(1) nrm : vec3<f32>,
    @location(2) world_y : f32,
};

@vertex
fn vs(i : VsIn) -> VsOut {
    var o : VsOut;
    let w = batch.world * vec4<f32>(i.pos, 1.0);
    o.clip = cam.view_proj * w;
    o.uv = batch.uv_rect.xy + i.uv * batch.uv_rect.zw;
    o.nrm = normalize((batch.world * vec4<f32>(i.nrm, 0.0)).xyz);
    o.world_y = w.y;
    return o;
}

@fragment
fn fs(i : VsOut) -> @location(0) vec4<f32> {
    var c = batch.base_color;
    if (batch.has_texture.x != 0u) {
        c = c * textureSampleLevel(albedo, samp, i.uv, 0.0);
    }
    let ndl = max(dot(normalize(i.nrm), normalize(cam.light_dir_time.xyz)), 0.0);
    // Billboard: unlit — die Sprite-Pixel enthalten ihre Schattierung schon.
    let light = select(0.35 + 0.75 * ndl, 1.0, batch.has_texture.y != 0u);
    return vec4<f32>(c.rgb * light, c.a);
}
