// RE M5 — Deferred-Pipeline V2 (ADR-0003, klassische MRT-Quelle).
// Geometrie-Pass schreibt das G-Buffer-Layout deferred_v1 aus
// re-render::gbuffer — Kanal-fuer-Kanal identisch zur CPU-Referenz
// pack()/unpack(); der Pixel-Diff-Test in tests/deferred.rs beweist es.

struct Camera {
    view_proj : mat4x4<f32>,
    light_dir_time : vec4<f32>, // xyz = Lichtrichtung (zum Licht hin), w = Zeit (ungenutzt)
    // Rueckwaertstransformation fuer die Schatten-Rekonstruktion:
    // Clip -> Welt. Der Geometrie-Pass benutzt sie nicht.
    inv_view_proj : mat4x4<f32>,
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

// ── Geclusterte Punktlichter (M5, Box 2) ──────────────────────────────
// Der Lighting-Pass findet zu jedem Pixel seinen Cluster (Kachel x
// Tiefenscheibe) und addiert nur dessen Lichter. Die Cluster-Index-
// Mathematik spiegelt re_render::cluster (slice_of/index) Kanal-fuer-
// Kanal; die Scheibengrenzen kommen fertig gerechnet vom CPU-Kern
// (cull_lights laeuft ebenfalls dort — der GPU-Compute-Cull folgt).
@group(1) @binding(5) var gb_depth : texture_depth_2d;

struct ClusterInfo {
    tiles_x : u32,
    tiles_y : u32,
    slices : u32,
    light_count : u32,
    near : f32,
    far : f32,
    proj_a : f32,   // proj.x_axis.x = cot(fov_x/2)
    proj_b : f32,   // proj.y_axis.y = cot(fov_y/2)
    width : u32,
    height : u32,
    tile_size : u32,
    n_bounds : u32, // = slices + 1
    // Scheibengrenzen als vec4 gepackt (Uniform-Arrays brauchen
    // Stride-Vielfache von 16): bound i = bounds[i/4][i%4], bis 12.
    bounds : array<vec4<f32>, 3>,
};
@group(1) @binding(6) var<uniform> cinfo : ClusterInfo;

fn bound(info : ClusterInfo, i : u32) -> f32 {
    return info.bounds[i / 4u][i % 4u];
}

// Je Cluster CLUSTER_CAP Eintraege: Slot 0 = Anzahl, Slot 1.. = Licht-
// index. Fixed-Stride statt Offset-Tabelle — fuer die Beweis-Kette
// einfach und exakt; echte Szenen spaeter mit compaction.
const CLUSTER_CAP : u32 = 8u;
@group(1) @binding(7) var<storage, read> cluster_lights : array<u32>;

struct LightGpu {
    pos_range : vec4<f32>,        // xyz = Position im View-Raum, w = Reichweite
    color_intensity : vec4<f32>,  // rgb = Farbe, a = Intensitaet
};
@group(1) @binding(8) var<storage, read> plights : array<LightGpu>;

// ── Schatten (M5, Shadow 2.0) ─────────────────────────────────────────
// CSM gegen die Depth-Compare-Map des `d-shadow`-Passes. Die Mathematik
// ist die Zug-fuer-Zug-Spiegelung von re_render::shadow: Kaskaden-Wahl
// (cascade_for_depth), Projektion (project_shadow), Neigungs-Bias
// (depth_bias) und PCF-Box 3x3 (pcf_box3x3). Der Compare-Sampler mit
// linearem Filter vergleicht je Tap die vier Texel und mischt die
// Ergebnisse — exakt die Gewichtung des CPU-Kerns. Die Grenzen kommen
// fertig gerechnet vom CPU-Kern (CascadeSetup::boundaries), die
// Kaskaden-Matrizen vom Light-Space-Fit (fit_cascade).
@group(1) @binding(9) var shadow_map : texture_depth_2d_array;
@group(1) @binding(10) var shadow_cmp : sampler_comparison;

struct ShadowUni {
    light_view_proj : array<mat4x4<f32>, 4>, // je Kaskade (fit_cascade)
    bounds : array<vec4<f32>, 2>,            // Grenzen 0..4, gepackt wie beim Cluster
    params : vec4<f32>, // x = Kaskadenzahl, y = Bias konstant, z = Bias Neigung
};
@group(1) @binding(11) var<uniform> sh : ShadowUni;

fn sbnd(i : u32) -> f32 {
    return sh.bounds[i / 4u][i % 4u];
}

/// Sichttiefe → Kaskade — gespiegeltes `cascade_for_depth`: Bereich
/// [grenze[i], grenze[i+1]), Grenze selbst gehoert zur hoheren Kaskade,
/// außerhalb der Reichweite -1 (kein Schatten).
fn shadow_cascade(d : f32, n : u32) -> u32 {
    var c : u32 = 0u;
    for (var i = 1u; i <= n; i = i + 1u) {
        if (d >= sbnd(i)) {
            c = i;
        }
    }
    return c;
}

/// Schattenfaktor des Welt-Punkts: 0.0 = voll im Schatten, 1.0 = frei.
/// `view_d` ist die Sichttiefe (laengs Blick−Z, dieselbe Groesse wie bei
/// den Cluster-Scheiben), `normal` die G-Buffer-Normale.
fn shadow_visibility(world : vec3<f32>, view_d : f32, normal : vec3<f32>) -> f32 {
    let n = u32(sh.params.x);
    if (n == 0u) {
        return 1.0; // Schatten-Pass deaktiviert
    }
    if (view_d < sbnd(0u) || view_d >= sbnd(n)) {
        return 1.0; // hinter der letzten Grenze — kein Schattenwurf
    }
    let c = shadow_cascade(view_d, n);
    let h = sh.light_view_proj[c] * vec4<f32>(world, 1.0);
    let ndc = h.xyz / h.w;
    // Die hochgeladene Matrix ist in y gespiegelt (Speicherung in der
    // Kern-Konvention: Zeile 0 = v 0) — der UV-Bezug spiegelt zurück:
    // uv.y = 0.5 − ndc.y · 0.5 trifft genau den Texel, den der Kern liest.
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return 1.0; // außerhalb der Map — wie der CPU-Kern: kein Eintrag
    }
    // Neigungs-Bias — gespiegeltes depth_bias mit Fallrichtung −light_dir.
    // Er verschiebt die Referenz Richtung Licht (Subtraktion), damit sich
    // flaechennahe Empfaenger nicht selbst schattieren (Acne).
    let toward = normalize(cam.light_dir_time.xyz);
    let ndotl = clamp(dot(normal, toward), 0.0, 1.0);
    let bias = sh.params.y + sh.params.z * (1.0 - ndotl);
    let reference = ndc.z - bias;
    // PCF-Box 3x3: neun Taps im Texelabstand, gleichmaessig gemittelt —
    // der Compare-Sampler (LessEqual: Referenz <= Texel) uebernimmt
    // die vier Texelvergleiche und die bilineare Mischung je Tap.
    let t = 1.0 / f32(textureDimensions(shadow_map).x);
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let o = vec2<f32>(f32(dx), f32(dy)) * t;
            // Compare-Sampling mit fester Mip-Stufe 0: coords sind die
            // UV, die Schicht ist der Index, die Referenz der letzte
            // Parameter (textureSampleCompareLevel — die Depth-Form des
            // Hardware-Vergleichs).
            sum = sum + textureSampleCompareLevel(shadow_map, shadow_cmp, uv + o, i32(c), reference);
        }
    }
    return clamp(sum / 9.0, 0.0, 1.0);
}

/// Scheibe einer Sichttiefe — gespiegeltes `ClusterGrid::slice_of`:
/// Grenzen liegen an der *unteren* Scheibe (partition_point), außerhalb
/// der Spanne kein Cluster (-1).
fn slice_of(d : f32) -> i32 {
    let nb = cinfo.n_bounds;
    if (d < bound(cinfo, 0u) || d > bound(cinfo, nb - 1u)) {
        return -1;
    }
    var pp : u32 = 0u;
    for (var i = 0u; i < nb; i = i + 1u) {
        if (bound(cinfo, i) < d) {
            pp = pp + 1u;
        }
    }
    return i32(min(max(pp, 1u) - 1u, cinfo.slices - 1u));
}

/// Summe aller Punktlicht-Beitraege des Pixels (Lambert + quadrati-
/// scher Distanz-Falloff innerhalb der Reichweite).
fn cluster_light_sum(coords : vec2<u32>, normal : vec3<f32>) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    if (cinfo.light_count == 0u) {
        return sum;
    }
    let depth = textureLoad(gb_depth, vec2<i32>(coords), 0);
    if (depth >= 1.0) {
        return sum; // Hintergrund ohne Geometrie — kein rekonstruierbarer Punkt
    }
    // Sichttiefe aus nicht-linearem Depth rekonstruieren (perspective_rh,
    // Depth 0..1): d = n*f / (f - z*(f - n)).
    let d = cinfo.near * cinfo.far / (cinfo.far - depth * (cinfo.far - cinfo.near));
    let ndc_x = 2.0 * (f32(coords.x) + 0.5) / f32(cinfo.width) - 1.0;
    let ndc_y = 1.0 - 2.0 * (f32(coords.y) + 0.5) / f32(cinfo.height);
    let p = vec3<f32>(ndc_x * d / cinfo.proj_a, ndc_y * d / cinfo.proj_b, -d);
    let z = slice_of(d);
    if (z < 0) {
        return sum;
    }
    // Kachel mit Randklemme — gespiegeltes `ClusterGrid::locate`.
    let tx = min(coords.x / cinfo.tile_size, cinfo.tiles_x - 1u);
    let ty = min(coords.y / cinfo.tile_size, cinfo.tiles_y - 1u);
    let cell = (u32(z) * cinfo.tiles_y + ty) * cinfo.tiles_x + tx;
    let base = cell * CLUSTER_CAP;
    let n = min(cluster_lights[base], cinfo.light_count);
    for (var k = 1u; k <= n; k = k + 1u) {
        let li = cluster_lights[base + k];
        let L = plights[li].pos_range;
        let v = L.xyz - p;
        let dist = length(v);
        let t = clamp(1.0 - dist / L.w, 0.0, 1.0);
        let att = t * t;
        if (att > 0.0) {
            let ndl = max(dot(normal, v / max(dist, 1e-7)), 0.0);
            let ci = plights[li].color_intensity;
            sum = sum + ci.rgb * ci.a * att * ndl;
        }
    }
    return sum;
}

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
    let coords = vec2<u32>(u32(frag.x), u32(frag.y));
    let s = surface_at(coords);
    // Lambert + konstanter Ambient fuer die gerichtete Sonne, plus die
    // geclusterten Punktlichter. Das Ziel ist ein Float-Target: Werte
    // ueber 1.0 ueberleben bis zum Tone-Mapping — HDR-Beweis der Kette.
    // Die Sonne ist die einzige geschattete Lichtquelle (Shadow 2.0);
    // der Ambient bleibt ungeschwaecht — dafuer gibt es noch kein
    // Ambient-Occlusion, und ein pauschal beschatteter Ambient waere
    // eine Luege ueber die Qualitaet.
    let depth = textureLoad(gb_depth, vec2<i32>(coords), 0);
    var vis = 1.0;
    if (depth < 1.0) {
        // Welt-Rekonstruktion aus Clip — dieselbe Kette wie die
        // Sichttiefe des Cluster-Passes: d = n*f / (f - z*(f - n)).
        let d = cinfo.near * cinfo.far / (cinfo.far - depth * (cinfo.far - cinfo.near));
        let ndc_x = 2.0 * (f32(coords.x) + 0.5) / f32(cinfo.width) - 1.0;
        let ndc_y = 1.0 - 2.0 * (f32(coords.y) + 0.5) / f32(cinfo.height);
        let world_h = cam.inv_view_proj * vec4<f32>(ndc_x, ndc_y, depth, 1.0);
        vis = shadow_visibility(world_h.xyz / world_h.w, d, s.normal);
    }
    let ndl = max(dot(s.normal, normalize(cam.light_dir_time.xyz)), 0.0);
    let sum = cluster_light_sum(coords, s.normal);
    let c = s.albedo * (vec3<f32>(0.1 + 0.9 * ndl * vis) + sum) * post.light_intensity;
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

// ── Licht-Culling als Compute-Pass (M5, Box 2b) ──────────────────────
// Ein Thread pro Cluster: View-Raum-AABB aufbauen (gespiegeltes
// cluster_view_aabb) und jede Licht-Kugel dagegen testen (gespiegelte
// sphere_intersects_aabb). Das ist exakt die Mathematik des CPU-
// Referenzkerns re_render::cluster — der Pixel-Diff- und der
// Listen-Vergleich-Test beweisen die Gleichheit Index-fuer-Index.
// Lichter ueber der Cluster-Kapazitaet fallen deterministisch weg
// (hoechste Index zuerst); die Test-Szene bleibt darunter.
@group(0) @binding(0) var<uniform> cinfo_c : ClusterInfo;
@group(0) @binding(1) var<storage, read> plights_c : array<LightGpu>;
@group(0) @binding(2) var<storage, read_write> cluster_out : array<u32>;

struct ClusterAabb {
    mn : vec3<f32>,
    mx : vec3<f32>,
};

/// View-Raum-AABB des Clusters — konservative Huellzelle ueber beide
/// Scheiben-Tiefen (identisch zur CPU: Ecken bei zn und zf).
fn cluster_aabb(tx : u32, ty : u32, z : u32) -> ClusterAabb {
    let zn = bound(cinfo_c, z);
    let zf = bound(cinfo_c, z + 1u);
    let nx0 = 2.0 * f32(tx) / f32(cinfo_c.tiles_x) - 1.0;
    let nx1 = 2.0 * f32(tx + 1u) / f32(cinfo_c.tiles_x) - 1.0;
    let ny1 = 1.0 - 2.0 * f32(ty) / f32(cinfo_c.tiles_y);
    let ny0 = 1.0 - 2.0 * f32(ty + 1u) / f32(cinfo_c.tiles_y);
    // Startwerte: min sucht ab +Riesig, max sucht ab -Riesig — die
    // echten Eckpunkte (Endlichkeit vorausgesetzt) gewinnen immer,
    // damit ist das identisch zum CPU-splat(f32::MAX/MIN). Literal
    // statt f32.max_value: dieses naga kennt Typ-Konstanten hier nicht.
    var mn = vec3<f32>(1e30);
    var mx = vec3<f32>(-1e30);
    let xs = array<f32, 4>(nx0, nx0, nx1, nx1);
    let ys = array<f32, 4>(ny0, ny1, ny0, ny1);
    let ds = array<f32, 2>(zn, zf);
    for (var e = 0u; e < 4u; e = e + 1u) {
        for (var k = 0u; k < 2u; k = k + 1u) {
            let c = vec3<f32>(
                xs[e] * ds[k] / cinfo_c.proj_a,
                ys[e] * ds[k] / cinfo_c.proj_b,
                -ds[k],
            );
            mn = min(mn, c);
            mx = max(mx, c);
        }
    }
    return ClusterAabb(mn, mx);
}

@compute @workgroup_size(64)
fn cs_cull(@builtin(global_invocation_id) gid : vec3<u32>) {
    let cell = gid.x;
    if (cell >= cinfo_c.tiles_x * cinfo_c.tiles_y * cinfo_c.slices) {
        return;
    }
    // Koordinaten aus dem linearen Index — gespiegeltes coords():
    // x am schnellsten, dann y, dann Scheibe.
    let tx = cell % cinfo_c.tiles_x;
    let ty = (cell / cinfo_c.tiles_x) % cinfo_c.tiles_y;
    let z = cell / (cinfo_c.tiles_x * cinfo_c.tiles_y);
    let bx = cluster_aabb(tx, ty, z);
    let base = cell * CLUSTER_CAP;
    var n : u32 = 0u;
    for (var li = 0u; li < cinfo_c.light_count; li = li + 1u) {
        let L = plights_c[li].pos_range;
        // Kugel-AABB-Test: Distanz zum naechsten Punkt der Zelle.
        let closest = clamp(L.xyz, bx.mn, bx.mx);
        let d = closest - L.xyz;
        if (dot(d, d) <= L.w * L.w) {
            if (n < CLUSTER_CAP - 1u) {
                cluster_out[base + 1u + n] = li;
            }
            n = n + 1u;
        }
    }
    cluster_out[base] = min(n, CLUSTER_CAP - 1u);
}

// ── FSR1 (M5, Box 3): EASU + RCAS ──────────────────────────────────────
// Zug-fuer-Zug-Spiegelung von re-render/src/fsr.rs — AMD FidelityFX
// Super Resolution 1.0, Non-Packed 32-Bit (ffx_fsr1.h, MIT). AMDs
// Naeherungs-Instruktionen (rcp/rsqrt) sind auf beiden Seiten durch
// exakte Division ersetzt, die NaN-Pfade der flachen Gebiete durch
// dieselben Epsilon-Klemmen — CPU und GPU rechnen bitidentisch.
// Eingabe ist display-kodiert (Tonemap + sRGB), wie bei AMD Pflicht.

// Gruppe 0, Bindung 0/1 — dieselbe Konvention wie der Tone-Mapping-Pass:
// jede Pipeline traegt ihr eigenes Layout an Position 0 (die
// Modul-Deklarationen teilen sich die Nummern, die Bind-Groups trennen sie).
@group(0) @binding(0) var up_src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> fsr_con: FsrCon;

/// EASU-Konstanten (FsrEasuCon): sk = sw/dw, sh/dh; ofs = 0.5*sk - 0.5;
/// sharp = FsrRcasCon (Blende -> linear). Ein Uniform fuer beide Passes —
/// das haelt die Gruppe-3-Layouts identisch (max_bind_groups=4).
/// (`ofs` statt `of` — das ist in WGSL ein reserviertes Wort.)
struct FsrCon {
    sk: vec2<f32>,
    ofs: vec2<f32>,
    sharp: f32,
    pad: f32,
};

/// Luma mal 2 — AMDs Mehrkanal-Naeherung in derselben Klammereihenfolge.
fn fsr_luma2(c: vec3<f32>) -> f32 {
    return c.b * 0.5 + (c.r * 0.5 + c.g);
}

/// Gespiegeltes FsrEasuSetF: Richtung/Laenge eines Quadranten.
fn easu_set(dir: ptr<function, vec2<f32>>, len: ptr<function, f32>,
            w: f32, lA: f32, lB: f32, lC: f32, lD: f32, lE: f32) {
    var lenX: f32 = abs(lD - lC);
    lenX = max(lenX, abs(lC - lB));
    lenX = 1.0 / max(lenX, 1e-30);
    let dirX: f32 = lD - lB;
    (*dir).x = (*dir).x + dirX * w;
    lenX = clamp(abs(dirX) * lenX, 0.0, 1.0);
    lenX = lenX * lenX;
    (*len) = (*len) + lenX * w;
    var lenY: f32 = abs(lE - lC);
    lenY = max(lenY, abs(lC - lA));
    lenY = 1.0 / max(lenY, 1e-30);
    let dirY: f32 = lE - lA;
    (*dir).y = (*dir).y + dirY * w;
    lenY = clamp(abs(dirY) * lenY, 0.0, 1.0);
    lenY = lenY * lenY;
    (*len) = (*len) + lenY * w;
}

/// Gespiegeltes FsrEasuTapF: modifizierte Lanczos-2-Naeherung,
/// anisotrop gedreht/gestreckt (ohne sin/rcp/sqrt — reine Polynome).
fn easu_tap(aC: ptr<function, vec3<f32>>, aW: ptr<function, f32>,
            off: vec2<f32>, pp: vec2<f32>, dir: vec2<f32>, len2: vec2<f32>,
            lob: f32, clp: f32, c: vec3<f32>) {
    let ox: f32 = off.x - pp.x;
    let oy: f32 = off.y - pp.y;
    let vx: f32 = (ox * dir.x + oy * dir.y) * len2.x;
    let vy: f32 = (ox * (-dir.y) + oy * dir.x) * len2.y;
    let d2: f32 = min(vx * vx + vy * vy, clp);
    var wB: f32 = (2.0 / 5.0) * d2 + (-1.0);
    var wA: f32 = lob * d2 + (-1.0);
    wB = wB * wB;
    wA = wA * wA;
    wB = (25.0 / 16.0) * wB + (-(25.0 / 16.0 - 1.0));
    let w: f32 = wB * wA;
    (*aC) = (*aC) + c * w;
    (*aW) = (*aW) + w;
}

/// Tap-Laderand wie ein Clamp-Sampler (CPU: dieselbe Klemme).
fn fsr_load(f: vec2<i32>, ox: i32, oy: i32, grenze: vec2<i32>) -> vec3<f32> {
    let tc = clamp(f + vec2<i32>(ox, oy), vec2<i32>(0, 0), grenze);
    return textureLoad(up_src, tc, 0).rgb;
}

@fragment
fn fs_easu(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let sz = textureDimensions(up_src);
    let grenze = vec2<i32>(i32(sz.x) - 1, i32(sz.y) - 1);
    // Position von 'f'; pp ist der gebrochene Rest in der f-Zelle.
    // Achtung: frag.xy ist der PixelMITTELPUNKT (dx+0.5) — die Kette
    // rechnet wie die CPU-Referenz mit dem ganzzahligen Pixelindex.
    let pix = vec2<f32>(floor(frag.x), floor(frag.y));
    var pp = vec2<f32>(pix.x * fsr_con.sk.x + fsr_con.ofs.x,
                       pix.y * fsr_con.sk.y + fsr_con.ofs.y);
    let fp = vec2<f32>(floor(pp.x), floor(pp.y));
    pp = pp - fp;
    let f = vec2<i32>(i32(fp.x), i32(fp.y));
    // 12-Tap-Kernel relativ zu f (b c / e f g h / i j k l / n o).
    let b = fsr_load(f, 0, -1, grenze);
    let c = fsr_load(f, 1, -1, grenze);
    let e = fsr_load(f, -1, 0, grenze);
    let fc = fsr_load(f, 0, 0, grenze);
    let g = fsr_load(f, 1, 0, grenze);
    let h = fsr_load(f, 2, 0, grenze);
    let i = fsr_load(f, -1, 1, grenze);
    let j = fsr_load(f, 0, 1, grenze);
    let k = fsr_load(f, 1, 1, grenze);
    let l = fsr_load(f, 2, 1, grenze);
    let n = fsr_load(f, 0, 2, grenze);
    let o = fsr_load(f, 1, 2, grenze);
    let bL = fsr_luma2(b);
    let cL = fsr_luma2(c);
    let eL = fsr_luma2(e);
    let fL = fsr_luma2(fc);
    let gL = fsr_luma2(g);
    let hL = fsr_luma2(h);
    let iL = fsr_luma2(i);
    let jL = fsr_luma2(j);
    let kL = fsr_luma2(k);
    let lL = fsr_luma2(l);
    let nL = fsr_luma2(n);
    let oL = fsr_luma2(o);

    // Richtung und Laenge ueber die vier Bilinear-Quadranten.
    var dir = vec2<f32>(0.0, 0.0);
    var len: f32 = 0.0;
    easu_set(&dir, &len, (1.0 - pp.x) * (1.0 - pp.y), bL, eL, fL, gL, jL);
    easu_set(&dir, &len, pp.x * (1.0 - pp.y), cL, fL, gL, hL, kL);
    easu_set(&dir, &len, (1.0 - pp.x) * pp.y, fL, iL, jL, kL, nL);
    easu_set(&dir, &len, pp.x * pp.y, gL, jL, kL, lL, oL);

    // Richtung normalisieren (Nullfall -> horizontal), Laenge 0..1 quadrieren.
    let dirR2 = dir.x * dir.x + dir.y * dir.y;
    let zro = dirR2 < (1.0 / 32768.0);
    var dirR: f32 = 1.0 / sqrt(dirR2);
    if (zro) {
        dirR = 1.0;
        dir.x = 1.0;
    }
    dir.x = dir.x * dirR;
    dir.y = dir.y * dirR;
    len = len * 0.5;
    len = len * len;
    // Fenster strecken und die Kante je nach Kantennaehe verschieben.
    let stretch: f32 = (dir.x * dir.x + dir.y * dir.y)
        * (1.0 / max(abs(dir.x), abs(dir.y)));
    let len2 = vec2<f32>(1.0 + (stretch - 1.0) * len, 1.0 + (-0.5) * len);
    let lob: f32 = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
    let clp: f32 = 1.0 / lob;

    // Akkumulation in der Tap-Reihenfolge des Originals.
    var aC = vec3<f32>(0.0, 0.0, 0.0);
    var aW: f32 = 0.0;
    easu_tap(&aC, &aW, vec2(0.0, -1.0), pp, dir, len2, lob, clp, b);
    easu_tap(&aC, &aW, vec2(1.0, -1.0), pp, dir, len2, lob, clp, c);
    easu_tap(&aC, &aW, vec2(-1.0, 1.0), pp, dir, len2, lob, clp, i);
    easu_tap(&aC, &aW, vec2(0.0, 1.0), pp, dir, len2, lob, clp, j);
    easu_tap(&aC, &aW, vec2(0.0, 0.0), pp, dir, len2, lob, clp, fc);
    easu_tap(&aC, &aW, vec2(-1.0, 0.0), pp, dir, len2, lob, clp, e);
    easu_tap(&aC, &aW, vec2(1.0, 1.0), pp, dir, len2, lob, clp, k);
    easu_tap(&aC, &aW, vec2(2.0, 1.0), pp, dir, len2, lob, clp, l);
    easu_tap(&aC, &aW, vec2(2.0, 0.0), pp, dir, len2, lob, clp, h);
    easu_tap(&aC, &aW, vec2(1.0, 0.0), pp, dir, len2, lob, clp, g);
    easu_tap(&aC, &aW, vec2(1.0, 2.0), pp, dir, len2, lob, clp, o);
    easu_tap(&aC, &aW, vec2(0.0, 2.0), pp, dir, len2, lob, clp, n);

    // Normieren und an die naechsten 2x2 (f, g, j, k) klemmen (Dering).
    let min4 = min(min(min(fc, g), j), k);
    let max4 = max(max(max(fc, g), j), k);
    let cOut = clamp(aC / aW, min4, max4);
    return vec4<f32>(cOut, 1.0);
}

// RCAS teilt sich Gruppe 3 und dasselbe Layout mit EASU (Quelle +
// FsrCon): das Bind-Group-Objekt des RCAS-Passes tauscht nur die
// Quelle — binding 0 haelt dann das EASU-Zwischenbild.

/// Randklemme wie ein Clamp-Sampler (CPU: dieselbe Klemme).
fn rcas_load(p: vec2<i32>, grenze: vec2<i32>) -> vec3<f32> {
    return textureLoad(up_src, clamp(p, vec2<i32>(0, 0), grenze), 0).rgb;
}

@fragment
fn fs_rcas(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let sz = textureDimensions(up_src);
    let grenze = vec2<i32>(i32(sz.x) - 1, i32(sz.y) - 1);
    let q = vec2<i32>(i32(frag.x), i32(frag.y));
    // 5-Punkt-Kreuz: b oben, d links, e Mitte, f rechts, h unten.
    let b = rcas_load(q + vec2(0, -1), grenze);
    let d = rcas_load(q + vec2(-1, 0), grenze);
    let e = rcas_load(q, grenze);
    let f = rcas_load(q + vec2(1, 0), grenze);
    let hh = rcas_load(q + vec2(0, 1), grenze);
    let mn4 = min(min(min(b, d), f), hh);
    let mx4 = max(max(max(b, d), f), hh);
    // Begrenzer: wie weit darf der Lobe, ohne 0..1 zu verlassen?
    // (Epsilon-Klemmen spiegeln die CPU-Seite; trifft nur exakt 0/1.)
    let hitMin = min(mn4, e) * (1.0 / max(4.0 * mx4, vec3(1e-30)));
    let hitMax = (1.0 - max(mx4, e)) * (1.0 / min(4.0 * mn4 + (-4.0), vec3(-1e-30)));
    let lobe = max(-hitMin, hitMax);
    // FSR_RCAS_LIMIT = 0.25 - 1/16 = 0.1875, mal Blende (FsrRcasCon).
    let lobeF: f32 = max(-0.1875, min(max(max(lobe.x, lobe.y), lobe.z), 0.0)) * fsr_con.sharp;
    let rcpL: f32 = 1.0 / (4.0 * lobeF + 1.0);
    let o = (lobeF * b + lobeF * d + lobeF * hh + lobeF * f + e) * rcpL;
    return vec4<f32>(o, 1.0);
}
