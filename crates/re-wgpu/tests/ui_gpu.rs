//! Beweis für M4 P1 (UI-Toolkit, GPU-Pass): der Hardware-UI-Pass in
//! `re-wgpu` malt dieselbe Draw-List wie die CPU-Referenz
//! `re_render::rasterize()` — Pixel für Pixel. Toleranz 1/255 pro Kanal
//! (RGBA16F-Zwischenspeicherung und Rundungsmodus können genau 1 LSB
//! kippen); mehr Divergenz gilt als Fehler. Szenenmix: Flächen, Linien
//! (pixelgesnapst), verschachtelte Scissor-Clips bis über den Rand,
//! halbtransparentes Overlapping-Blending und Glyphentext aus dem
//! R8-Atlas (ASCII + Umlaute + Geviertstrich, zwei Fonts).
//! Läuft headless auf dem Pi und in der CI (Software-Adapter llvmpipe).

use re_render::ui::{rasterize, DrawList, FontAtlas, Rect, Theme};
use re_render::GfxBackend;
use re_wgpu::WgpuBackend;

const FONT_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/inter/Inter-Regular.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../../../assets/fonts/inter/Inter-Bold.ttf");

fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::new(x, y, w, h)
}

/// Repräsentative Editor-Attrappe mit allen Konventions-Fällen.
fn scene() -> (DrawList, FontAtlas, u32, u32, [f32; 4]) {
    let theme = Theme::default();
    let atlas =
        FontAtlas::build(&[FONT_REGULAR, FONT_BOLD], 15.0, "äöüÄÖÜß—", 1024, 512).expect("Atlas");
    let (w, h) = (420u32, 260u32);
    let (wf, hf) = (w as f32, h as f32);
    let mut dl = DrawList::new(r(0.0, 0.0, wf, hf));

    // Flächen + Kopfleiste mit pixelgesnapster Trennlinie.
    dl.rect(r(0.0, 0.0, wf, hf), theme.colors.background);
    dl.rect(r(0.0, 0.0, wf, 28.0), theme.colors.header);
    dl.line(0.0, 28.0, wf, 28.0, 1.0, theme.colors.border);
    dl.text(
        &atlas,
        1,
        "re-editor — GPU-Pass",
        10.0,
        6.0,
        20.0,
        theme.colors.text,
    );

    // Scroll-Region: Inhalt, der über den Clip hinausläuft (beweisender
    // Fall für set_scissor_rect == CPU-Scissor), Clip schneidet rechts
    // und unten NICHT den Rand — der nächste Clip tut es.
    dl.push_clip(r(8.0, 36.0, 180.0, 120.0));
    dl.rect(r(8.0, 36.0, 180.0, 400.0), theme.colors.panel);
    for i in 0..12u32 {
        let y = 40.0 + i as f32 * 22.0;
        if i == 1 {
            dl.rect(r(10.0, y - 2.0, 176.0, 20.0), theme.colors.select);
        } else if i == 4 {
            dl.rect(r(10.0, y - 2.0, 176.0, 20.0), theme.colors.hover);
        }
        dl.text(
            &atlas,
            0,
            &format!("Knoten {i} — äöü—"),
            16.0,
            y,
            18.0,
            theme.colors.text,
        );
    }
    // Verschachtelter Kinder-Clip (Eltern-Clip ∩ Kinder-Clip).
    dl.push_clip(r(20.0, 60.0, 60.0, 40.0));
    dl.rect(r(0.0, 0.0, 400.0, 300.0), [0.9, 0.45, 0.2, 0.35]);
    dl.pop_clip();
    dl.pop_clip();

    // Clip bis an den Zielrand (scissor_rect muss auf Zielgröße klemmen).
    dl.push_clip(r(wf - 60.0, hf - 60.0, 400.0, 400.0));
    dl.rect(
        r(wf - 60.0, hf - 60.0, 400.0, 400.0),
        theme.colors.viewport_bg,
    );
    dl.text(
        &atlas,
        0,
        "Rand",
        wf - 52.0,
        hf - 52.0,
        18.0,
        theme.colors.text,
    );
    dl.pop_clip();

    // Rechts: Overlapping-Blending in mehreren Lagen (Float-Kern der
    // CPU-Referenz gegen ONE/ONE_MINUS_SRC_ALPHA auf RGBA16F).
    dl.rect(r(200.0, 40.0, 200.0, 120.0), theme.colors.panel);
    dl.rect(r(220.0, 60.0, 200.0, 120.0), [0.10, 0.55, 0.85, 0.45]);
    dl.rect(r(240.0, 80.0, 160.0, 60.0), [0.85, 0.25, 0.10, 0.55]);
    dl.stroke(r(200.0, 40.0, 200.0, 120.0), 1.0, theme.colors.border);
    dl.text(
        &atlas,
        1,
        "Anwenden",
        210.0,
        170.0,
        18.0,
        theme.colors.accent,
    );
    dl.text(
        &atlas,
        0,
        "ÄÖÜ ß — Probe 0123",
        210.0,
        192.0,
        18.0,
        theme.colors.text_dim,
    );

    // Dünne Linien in beiden Achsen (Raster-Konvention).
    dl.line(200.0, 220.0, 400.0, 220.0, 1.0, theme.colors.border);
    dl.line(200.5, 220.0, 200.5, 250.0, 1.0, theme.colors.accent);

    (dl, atlas, w, h, theme.colors.background)
}

#[test]
fn gpu_ui_pass_spiegelt_die_cpu_referenz() {
    let mut be = WgpuBackend::new();
    if be.init().is_err() {
        eprintln!("kein Adapter — Test uebersprungen");
        return;
    }
    let (dl, atlas, w, h, clear) = scene();
    let cpu = rasterize(&dl, Some(&atlas), w, h, clear);
    let gpu = be
        .render_ui_offscreen(&dl, &atlas, w, h, clear)
        .expect("GPU-UI-Pass");
    assert_eq!(cpu.len(), gpu.len(), "gleiches Byte-Layout");

    // Die Szene muss nicht-trivial sein: sonst wäre der Diff faul.
    let mut content = 0usize;
    for px in cpu.chunks_exact(4) {
        if px
            != [
                (clear[0] * 255.0).round() as u8,
                (clear[1] * 255.0).round() as u8,
                (clear[2] * 255.0).round() as u8,
                255,
            ]
        {
            content += 1;
        }
    }
    assert!(
        content > (w * h) as usize / 4,
        "Szene füllt das Bild ({content} Pixel)"
    );

    let mut max_diff = 0u8;
    let mut over_tol = 0usize;
    let mut sum = 0u64;
    let mut worst = (0u32, 0u32);
    for (idx, (a, b)) in cpu.iter().zip(gpu.iter()).enumerate() {
        let d = a.abs_diff(*b);
        if d > max_diff {
            max_diff = d;
            worst = (idx as u32 / 4 % w, idx as u32 / 4 / w);
        }
        sum += d as u64;
        if d > 1 {
            if over_tol < 12 {
                let px = idx as u32 / 4 % w;
                let py = idx as u32 / 4 / w;
                eprintln!("über-Tol ({px},{py}) kanal {} cpu={a} gpu={b}", idx % 4);
            }
            over_tol += 1;
        }
    }
    let i = ((worst.1 * w + worst.0) as usize) * 4;
    eprintln!(
        "schlimmster Pixel ({},{}) cpu={:?} gpu={:?}",
        worst.0,
        worst.1,
        &cpu[i..i + 4],
        &gpu[i..i + 4]
    );
    // Diagnose: Quads in der Region des schlimmsten Pixels.
    if std::env::var("RE_UI_GPU_DEBUG").is_ok() {
        for (bi, cmd) in dl.cmds().iter().enumerate() {
            for q in cmd.vertices.chunks_exact(6) {
                let (a, b) = (q[0], q[2]);
                let (x0, x1) = (a.pos[0].min(b.pos[0]), a.pos[0].max(b.pos[0]));
                let (y0, y1) = (a.pos[1].min(b.pos[1]), a.pos[1].max(b.pos[1]));
                if x1 > 330.0 && x0 < 336.0 && y1 > 195.0 && y0 < 205.0 {
                    eprintln!(
                        "batch{bi} tex={} quad x[{x0}..{x1}] y[{y0}..{y1}] uv[{},{},{},{}] c={:?}",
                        cmd.textured, a.uv[0], a.uv[1], b.uv[0], b.uv[1], a.color
                    );
                }
            }
        }
    }
    // Diagnose: Region um den schlimmsten Pixel als Diff-Karte.
    if std::env::var("RE_UI_GPU_DEBUG").is_ok() {
        let (wx, wy) = worst;
        for y in wy.saturating_sub(6)..=(wy + 6).min(h - 1) {
            let mut row = String::new();
            for x in wx.saturating_sub(10)..=(wx + 10).min(w - 1) {
                let j = ((y * w + x) as usize) * 4;
                let d = cpu[j]
                    .abs_diff(gpu[j])
                    .max(cpu[j + 1].abs_diff(gpu[j + 1]))
                    .max(cpu[j + 2].abs_diff(gpu[j + 2]));
                row.push(match d {
                    0 => '.',
                    1 => ':',
                    2..=5 => '+',
                    6..=30 => '*',
                    _ => '#',
                });
            }
            eprintln!(
                "y{y:>3} {row}   cpu{:>3} gpu{:>3}",
                cpu[((y * w + wx) as usize) * 4],
                gpu[((y * w + wx) as usize) * 4]
            );
        }
    }
    let n = cpu.len() as u64;
    eprintln!(
        "ui-diff: max {max_diff}/255, über-Toleranz {over_tol}, mittel {}e-6",
        sum * 1_000_000 / n
    );
    assert_eq!(
        over_tol, 0,
        "kein Pixel darf um mehr als 1/255 divergieren (max {max_diff})"
    );

    // Opt-in: RE_UI_GPU_PNG=/pfad/ui.png schreibt CPU-|GPU-Bildseite.
    if let Ok(path) = std::env::var("RE_UI_GPU_PNG") {
        let mut combo = image::RgbaImage::new(w * 2 + 8, h);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) as usize) * 4;
                combo.put_pixel(x, y, image::Rgba([cpu[i], cpu[i + 1], cpu[i + 2], 255]));
                combo.put_pixel(
                    w + 8 + x,
                    y,
                    image::Rgba([gpu[i], gpu[i + 1], gpu[i + 2], 255]),
                );
            }
        }
        combo.save(&path).expect("png");
    }
}
