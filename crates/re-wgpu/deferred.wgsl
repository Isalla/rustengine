// RE M5 — Deferred-Pipeline V2 (ADR-0003, klassische MRT-Quelle).
// Geometrie-Pass schreibt das G-Buffer-Layout deferred_v1 aus
// re-render::gbuffer — Kanal-fuer-Kanal identisch zur CPU-Referenz
// pack()/unpack(); der Pixel-Diff-Test in tests/deferred.rs beweist es.

struct Camera {
    view_proj : mat4x4<f32>,
    light_dir_time : vec4<f32>, // xyz = Lichtrichtung, w = Zeit (ungenutzt)
};
@group(0) @binding(0) var<uniform> cam : Camera;

struct Batch {
    world : mat4x4<f32>,
    albedo : vec4<f32>,   // rgb = Albedo (linear), a = Metallic
    params : vec4<f32>,   // x = Roughness, y = Material-ID/255
};
@group(1) @binding(0) var<uniform> batch : Batch;

// ── Oktaeder-Kodierung: gespiegelte re-render::gbuffer-Mathe ─────────
fn oct_encode(n : vec3<f32>) -> vec2<f32> {
    let p = n / max(abs(n.x) + abs(n.y) + abs(n.z), 1e-7);
    var e = p.xy;
    if (p.z < 0.0) {
        let sx = select(-1.0, 1.0, e.x >= 0.0);
        let sy = select(-1.0, 1.0, e.y >= 0.0);
        e = vec2<f32>((1.0 - abs(e.y)) * sx, (1.0 - abs(e.x)) * sy);
    }
    return e * 0.5 + vec2<f32>(0.5);
}

fn oct_decode(uv : vec2<f32>) -> vec3<f32> {
    let e = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)) * 2.0 - vec2<f32>(1.0);
    let z = 1.0 - abs(e.x) - abs(e.y);
    var v : vec3<f32>;
    if (z >= 0.0) {
        v = vec3<f32>(e.x, e.y, z);
    } else {
        let sx = select(-1.0, 1.0, e.x >= 0.0);
        let sy = select(-1.0, 1.0, e.y >= 0.0);
        v = vec3<f32>((1.0 - abs(e.y)) * sx, (1.0 - abs(e.x)) * sy, z);
    }
    let len = length(v);
    if (len <= 1e-7) {
        return vec3<f32>(0.0, 0.0, 1.0);
    }
    return v / len;
}

// ── Geometrie-Pass (MRT) ──────────────────────────────────────────────
struct VsIn {
    @location(0) pos : vec3<f32>,
    @location(1) nrm : vec3<f32>,
    @location(2) uv : vec2<f32>,
};

struct GpOut {
    @builtin(position) clip : vec4<f32>,
    @location(0) nrm : vec3<f32>,
};

@vertex
fn vs_geometry(i : VsIn) -> GpOut {
    var o : GpOut;
    o.clip = cam.view_proj * batch.world * vec4<f32>(i.pos, 1.0);
    o.nrm = normalize((batch.world * vec4<f32>(i.nrm, 0.0)).xyz);
    return o;
}

struct GBufferOut {
    @location(0) albedo_metallic : vec4<f32>, // RGBA8: rgb + Metallic
    @location(1) normal_params : vec4<f32>,   // RGBA8: Oktaeder + Roughness + ID
    @location(2) velocity : vec2<f32>,        // RG16F; statisch = 0
};

@fragment
fn fs_geometry(i : GpOut) -> GBufferOut {
    var o : GBufferOut;
    o.albedo_metallic = vec4<f32>(batch.albedo.rgb, batch.albedo.a);
    let enc = oct_encode(normalize(i.nrm));
    o.normal_params = vec4<f32>(enc.x, enc.y, batch.params.x, batch.params.y);
    o.velocity = vec2<f32>(0.0);
    return o;
}

// ── Lighting-Pass (Fullscreen-Dreieck, liest durch surface_at) ──────
@group(1) @binding(0) var gb0 : texture_2d<f32>;
@group(1) @binding(1) var gb1 : texture_2d<f32>;
@group(1) @binding(2) var gb2 : texture_2d<f32>;
@group(1) @binding(3) var samp : sampler;

// Post-Uniform: Belichtung + Kurvenwahl. Dieselben Felder nutzt der
// Tone-Mapping-Pass; der Lighting-Pass braucht nur light_intensity.
struct Post {
    exposure : f32,
    light_intensity : f32,
    curve : f32,   // 0 = Reinhard, 1 = ReinhardExtended, 2 = ACES
    white : f32,   // Weißpunkt für ReinhardExtended
};
@group(1) @binding(4) var<uniform> post : Post;

@vertex
fn vs_fullscreen(@builtin(vertex_index) vi : u32) -> @builtin(position) vec4<f32> {
    // Spannt das Viewport-Rechteck in einem Dreieck auf (Standard-Trick).
    let p = vec2<f32>(f32(i32(vi & 1u) * 4 - 1), f32(i32(vi >> 1u) * 4 - 1));
    return vec4<f32>(p, 0.0, 1.0);
}

/// Die abstrakte surface_at()-Schnittstelle aus ADR-0003: der Lighting-
/// Pass kennt nur diese Abfrage, nicht die Target-Belegung. Heute die
/// MRT-Quelle; eine Visibility-Buffer-Quelle liefert später dieselbe
/// Struktur aus einem ID-Fetch.
struct SurfaceGpu {
    albedo : vec3<f32>,
    metallic : f32,
    roughness : f32,
    normal : vec3<f32>,
    velocity : vec2<f32>,
};

fn surface_at(coords : vec2<u32>) -> SurfaceGpu {
    let t0 = textureLoad(gb0, coords, 0);
    let t1 = textureLoad(gb1, coords, 0);
    let t2 = textureLoad(gb2, coords, 0);
    var s : SurfaceGpu;
    s.albedo = t0.rgb;
    s.metallic = t0.a;
    s.normal = oct_decode(vec2<f32>(t1.x, t1.y));
    s.roughness = t1.z;
    s.velocity = t2.xy;
    return s;
}

@fragment
fn fs_light(@builtin(position) frag : vec4<f32>) -> @location(0) vec4<f32> {
    let s = surface_at(vec2<u32>(u32(frag.x), u32(frag.y)));
    // Erster Beleuchtungs-Beweis: Lambert + konstanter Ambient. Die
    // volle BRDF (Cluster-Lichter, Specular) folgt mit dem Licht-Pass.
    // Das Ziel ist ein Float-Target: Werte ueber 1.0 ueberleben bis
    // zum Tone-Mapping — das ist der HDR-Beweis dieser Kette.
    let ndl = max(dot(s.normal, normalize(cam.light_dir_time.xyz)), 0.0);
    let c = s.albedo * (0.1 + 0.9 * ndl) * post.light_intensity;
    return vec4<f32>(c, 1.0);
}

// ── Tone-Mapping-Pass (HDR-float → Display-linear → sRGB-Target) ────
// Die Kurven sind Kanal-fuer-Kanal die gespiegelte CPU-Referenz aus
// re_render::hdr (ACES = Narkowicz; AgX folgt, wenn gespiegelt). Das
// Ziel-Format ist Rgba8UnormSrgb — die Hardware uebernimmt das
// sRGB-Encoding, der Shader liefert display-lineare Werte.
@group(0) @binding(0) var hdr_tex : texture_2d<f32>;
@group(0) @binding(1) var<uniform> post_t : Post;

fn tm_reinhard(x : f32) -> f32 {
    return x / (1.0 + x);
}

fn tm_reinhard_ext(x : f32, white : f32) -> f32 {
    let w = max(white, 1e-3);
    return (x * (1.0 + x / (w * w))) / (1.0 + x);
}

fn tm_aces(x : f32) -> f32 {
    // (x*(2.51x+0.03)) / (x*(2.43x+0.59)+0.14) — hdr.rs, Konstanten identisch.
    return (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14);
}

fn tonemap_channel(x : f32) -> f32 {
    if (x <= 0.0) {
        return 0.0;
    }
    var y : f32;
    if (post_t.curve < 0.5) {
        y = tm_reinhard(x);
    } else if (post_t.curve < 1.5) {
        y = tm_reinhard_ext(x, post_t.white);
    } else {
        y = tm_aces(x);
    }
    return clamp(y, 0.0, 1.0);
}

@fragment
fn fs_tonemap(@builtin(position) frag : vec4<f32>) -> @location(0) vec4<f32> {
    let coords = vec2<u32>(u32(frag.x), u32(frag.y));
    let c = textureLoad(hdr_tex, coords, 0);
    let exposed = c.rgb * post_t.exposure;
    let mapped = vec3<f32>(
        tonemap_channel(exposed.x),
        tonemap_channel(exposed.y),
        tonemap_channel(exposed.z),
    );
    return vec4<f32>(mapped, c.a);
}
