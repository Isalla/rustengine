// UI-Pass (M4 P1): malt die Draw-List aus `re_render::ui` als Quads.
// CPU-Referenz ist `re_render::ui::rasterize()` — dieselbe Doctrine wie
// beim Schatten: der Kern ist die Wahrheit, die Hardware spiegelt sie.
// Deshalb bewusst OHNE Hardware-Sampler gerastert: die bilineare
// Atlas-Abdeckung wird per `textureLoad` von Hand gerechnet, exakt in
// derselben Operationsfolge wie der CPU-Kern (Hardware-Filter hätten
// festkomma-quantisierte Texel-Gewichte und würden um LSBs divergieren).
// Ziel ist lineares RGBA16F; die 8-Bit-Quantisierung passiert erst nach
// dem Readback — wie beim CPU-Pfad, der ebenfalls float-akkumuliert.

struct In {
    @location(0) pos: vec2f,
    @location(1) uv: vec2f,
    @location(2) color: vec4f,
};

struct Out {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
};

@group(0) @binding(0) var atlas: texture_2d<f32>;

// Zielgröße in Pixeln — setzt die Pixelkoordinaten der Draw-List (y nach
// unten) in Clip-Raum (y nach oben) um. Der Viewport deckt das Ziel
// vollständig ab; affine Abbildung, die UV-Interpolation bleibt
// pixel-zentrisch identisch zur CPU-Referenz.
@group(1) @binding(0) var<uniform> view_size: vec2f;

// Positions sind Ziel-Pixelkoordinaten (y nach unten, wie in der
// Draw-List); die Uniform rechnet sie in Clip-Raum.
@vertex
fn vs_ui(i: In) -> Out {
    var o: Out;
    let ndc = vec2f(
        i.pos.x / view_size.x * 2.0 - 1.0,
        1.0 - i.pos.y / view_size.y * 2.0,
    );
    o.clip = vec4f(ndc, 0.0, 1.0);
    o.uv = i.uv;
    o.color = i.color;
    return o;
}

// Premultipliziertes Source-over (Blend: ONE, ONE_MINUS_SRC_ALPHA) —
// identisch zur CPU-Formel `dst = color * sa + dst * (1 - sa)`.
@fragment
fn fs_ui_solid(i: Out) -> @location(0) vec4f {
    let a = i.color.a;
    return vec4f(i.color.rgb * a, a);
}

fn tex_at(x: i32, y: i32) -> f32 {
    return textureLoad(atlas, vec2i(x, y), 0).r;
}

@fragment
fn fs_ui_textured(i: Out) -> @location(0) vec4f {
    let size = vec2f(textureDimensions(atlas));
    // Dasselbe Bilinear wie FontAtlas::sample: Koordinate ins Texel-
    // Raster rechnen, auf den Texelzentrums-Bereich [0, size-1] klemmen,
    // vier Texel gewichten. R8Unorm liefert .r bereits als Wert/255.
    let fx = clamp(i.uv.x * size.x - 0.5, 0.0, size.x - 1.0);
    let fy = clamp(i.uv.y * size.y - 0.5, 0.0, size.y - 1.0);
    let x0 = i32(floor(fx));
    let y0 = i32(floor(fy));
    let x1 = min(x0 + 1i, i32(size.x) - 1i);
    let y1 = min(y0 + 1i, i32(size.y) - 1i);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let top = mix(tex_at(x0, y0), tex_at(x1, y0), tx);
    let bot = mix(tex_at(x0, y1), tex_at(x1, y1), tx);
    let cov = mix(top, bot, ty);
    let a = i.color.a * cov;
    return vec4f(i.color.rgb * a, a);
}
