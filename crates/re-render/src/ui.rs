//! UI-Kern (M4, P1): Draw-List, Theme, Dock-Layout, Font-Atlas.
//!
//! Das ist der backend-neutrale Teil des Editor-Frontends — reine
//! Mathematik und Rasterung auf der CPU, deterministisch und headless
//! testbar. Der GPU-Pass in `re-wgpu` ist nur der Ausführer: er schiebt
//! [`DrawList`]-Kommandos als Dreiecke mit Scissor-Rechten und die
//! Atlas-Pixel als Textur. Dieselbe Doctrine wie bei Schatten und FSR1:
//! der Kern ist die Wahrheit, die GPU spiegelt ihn.
//!
//! Die Schichten (ADR-0001):
//! - [`Theme`] — Farben, Metriken, Labels als Daten (Serde), je Preset
//!   eine Datei; das Default-Theme zeigt die eigene Identität der
//!   Engine (warme Dunkelgraus, Rust-Orange-Akzent).
//! - [`DockNode`] — der Layout-Baum (Split/Tabs/Leaf) als Daten;
//!   [`dock_layout`] rechnet ihn in Rechtecke um.
//! - [`DrawList`] — gefilterte Zeichen-Befehle (Rechtecke, Linien,
//!   Glyphen) mit Scissor-Stapel, in Batches zerlegt.
//! - [`FontAtlas`] — Glyphen-Rasterung (ab_glyph) in ein R8-Atlas mit
//!   Shelf-Packer; [`FontAtlas::text`] legt einen String als Vertex-
//!   Liste aus.
//! - [`widgets`] — Immediate-Mode-Widgets auf der Draw-List (Button,
//!   Zeile, Tab, Sektionskopf, Scroll, Textfeld): Eingabe als Daten,
//!   Ereignisse als Rückgaben, Look nur aus dem Theme.

pub mod widgets;

use ab_glyph::{Font, ScaleFont};
use serde::{Deserialize, Serialize};

/// Farbe linear RGBA 0..1 — dieselbe Darstellung wie im Theme-JSON.
pub type Rgba = [f32; 4];

// ── Rechtecke ────────────────────────────────────────────────────────

/// Achsenparalleles Rechteck in Fenster-Pixeln (y nach unten).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
    /// Schnittmenge; leer (w/h = 0) wenn auf einer Achse disjunkt.
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        if r <= x || b <= y {
            Rect::new(x, y, 0.0, 0.0)
        } else {
            Rect::new(x, y, r - x, b - y)
        }
    }
}

// ── Theme (Preset-Ebene 3: Look + Labels) ────────────────────────────

/// Farb-Fenster des Themes. Die Namen sind bewusst funktional
/// („panel“, „accent“), nicht per Preset kopiert: ein Preset-Theme
/// belegt sie mit seinen Werten (etwa unser Rust-Orange).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeColors {
    pub background: Rgba,
    pub panel: Rgba,
    pub header: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_dim: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub hover: Rgba,
    pub select: Rgba,
    pub input: Rgba,
    pub viewport_bg: Rgba,
}

impl Default for ThemeColors {
    /// Das `default`-Preset: warme Dunkelgraus, Rust-Orange als Akzent
    /// (die eigene Identität der Engine, ADR-0001).
    fn default() -> Self {
        Self {
            background: [0.086, 0.080, 0.074, 1.0],
            panel: [0.117, 0.110, 0.102, 1.0],
            header: [0.145, 0.137, 0.127, 1.0],
            border: [0.200, 0.190, 0.175, 1.0],
            text: [0.870, 0.850, 0.820, 1.0],
            text_dim: [0.550, 0.530, 0.500, 1.0],
            accent: [0.900, 0.350, 0.150, 1.0],
            accent_hover: [1.000, 0.450, 0.220, 1.0],
            hover: [0.160, 0.150, 0.140, 1.0],
            select: [0.320, 0.160, 0.090, 1.0],
            input: [0.070, 0.065, 0.060, 1.0],
            viewport_bg: [0.020, 0.020, 0.025, 1.0],
        }
    }
}

/// Abstände und Schriftgrößen in Pixeln — Metriken gehören ins Theme,
/// nicht in den Code (ADR-0001 verbietet hartcodierten Look).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeMetrics {
    pub row_h: f32,
    pub tab_h: f32,
    pub pad: f32,
    pub corner: f32,
    pub splitter: f32,
    pub font_px: f32,
    pub font_px_small: f32,
    pub icon_px: f32,
}

impl Default for ThemeMetrics {
    fn default() -> Self {
        Self {
            row_h: 24.0,
            tab_h: 26.0,
            pad: 6.0,
            corner: 3.0,
            splitter: 4.0,
            font_px: 15.0,
            font_px_small: 12.0,
            icon_px: 16.0,
        }
    }
}

/// Ein Preset-Theme: Farben + Metriken + Label-Tabelle (i18n).
/// Labels mappen Control-/Panel-IDs auf sichtbaren Text — der
/// Preset-Ordner ist Daten, Rust-Quellcode enthält keine Labels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub colors: ThemeColors,
    pub metrics: ThemeMetrics,
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "default".into(),
            colors: ThemeColors::default(),
            metrics: ThemeMetrics::default(),
            labels: std::collections::BTreeMap::new(),
        }
    }
}

impl Theme {
    /// Label-Lookup mit Rückfall auf die ID selbst (fehlende Labels
    /// crashen nie — sie zeigen den rohen Namen statt einer Lücke).
    pub fn label<'a>(&'a self, id: &'a str) -> &'a str {
        self.labels.get(id).map(String::as_str).unwrap_or(id)
    }
}

// ── Dock-Baum (Preset-Ebene 1: Layout-Manifest) ──────────────────────

/// Knoten des Dock-Baums. Ein Preset-Manifest ist genau dieser Baum,
/// serialisiert; Panels sind nach ID referenziert und werden vom
/// Widget-Set gefüllt (welches Panel wo lebt, entscheidet nur das
/// Manifest).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DockNode {
    /// Split: `vertical = true` trennt oben/unten, sonst links/rechts.
    /// `ratio` ist der Anteil des `first`-Zweigs (0..1, geklemmt).
    Split {
        vertical: bool,
        ratio: f32,
        first: Box<DockNode>,
        second: Box<DockNode>,
    },
    /// Tab-Gruppe: Streifen oben, das aktive Panel füllt den Rest.
    Tabs { active: usize, panels: Vec<String> },
    /// Einzelnes Panel.
    Leaf { panel: String },
}

/// Wo ein Panel landet: Inhalts-Rechteck, plus Tab-Rechtecke wenn es in
/// einer Tab-Gruppe lebt (nur für das aktive Panel gefüllt).
#[derive(Clone, Debug, PartialEq)]
pub struct PanelPlacement {
    pub panel: String,
    pub rect: Rect,
    pub tabs: Vec<(String, Rect)>,
}

/// Rechnet den Dock-Baum in Panel-Platzierungen für eine Fläche.
/// Splitter werden aus der Fläche herausgenommen (je `splitter` px
/// zwischen den Zweigen); Tab-Streifen kosten `tab_h` oben.
pub fn dock_layout(node: &DockNode, area: Rect, tab_h: f32, splitter: f32) -> Vec<PanelPlacement> {
    let mut out = Vec::new();
    place(node, area, tab_h, splitter, &mut out);
    out
}

fn place(node: &DockNode, area: Rect, tab_h: f32, sp: f32, out: &mut Vec<PanelPlacement>) {
    match node {
        DockNode::Leaf { panel } => out.push(PanelPlacement {
            panel: panel.clone(),
            rect: area,
            tabs: Vec::new(),
        }),
        DockNode::Tabs { active, panels } => {
            if panels.is_empty() {
                return;
            }
            let tw = area.w / panels.len() as f32;
            let tabs: Vec<(String, Rect)> = panels
                .iter()
                .enumerate()
                .map(|(i, p)| (p.clone(), Rect::new(area.x + i as f32 * tw, area.y, tw, tab_h)))
                .collect();
            let body = Rect::new(area.x, area.y + tab_h, area.w, (area.h - tab_h).max(0.0));
            // Nur das aktive Panel (Index geklemmt) bekommt den Inhalt.
            let idx = (*active).min(panels.len() - 1);
            out.push(PanelPlacement {
                panel: panels[idx].clone(),
                rect: body,
                tabs,
            });
        }
        DockNode::Split { vertical, ratio, first, second } => {
            let r = ratio.clamp(0.05, 0.95);
            let (fa, sa) = if *vertical {
                let h1 = (area.h * r - sp * 0.5).max(0.0);
                let y2 = area.y + h1 + sp;
                (
                    Rect::new(area.x, area.y, area.w, h1),
                    Rect::new(area.x, y2, area.w, (area.bottom() - y2).max(0.0)),
                )
            } else {
                let w1 = (area.w * r - sp * 0.5).max(0.0);
                let x2 = area.x + w1 + sp;
                (
                    Rect::new(area.x, area.y, w1, area.h),
                    Rect::new(x2, area.y, (area.right() - x2).max(0.0), area.h),
                )
            };
            place(first, fa, tab_h, sp, out);
            place(second, sa, tab_h, sp, out);
        }
    }
}

// ── Draw-List ────────────────────────────────────────────────────────

/// Ein UI-Eckpunkt: Position, Atlas-UV (0 für untexturierte Farbe),
/// Farbe linear RGBA. Identisches Layout für den GPU-Pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: Rgba,
}

/// Ein Batch: zusammenhängende Dreiecke (je 6 Vertices) unter einem
/// Scissor-Rechteck, alle untexturiert oder alle texturiert.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawCmd {
    pub clip: Rect,
    pub textured: bool,
    pub vertices: Vec<UiVertex>,
}

/// Zeichen-Sammlung mit Scissor-Stapel. Alles Landet in Batches —
/// der Stapel ist wie bei GPU-Scissoring verschachtelt (Kinder
/// clippen gegen den Eltern-Clip).
#[derive(Clone, Debug, Default)]
pub struct DrawList {
    cmds: Vec<DrawCmd>,
    clips: Vec<Rect>,
}

impl DrawList {
    pub fn new(clip: Rect) -> Self {
        Self { cmds: Vec::new(), clips: vec![clip] }
    }

    pub fn cmds(&self) -> &[DrawCmd] {
        &self.cmds
    }

    pub fn push_clip(&mut self, r: Rect) {
        let parent = *self.clips.last().unwrap_or(&Rect::default());
        let c = parent.intersect(&r);
        self.clips.push(c);
    }

    pub fn pop_clip(&mut self) {
        if self.clips.len() > 1 {
            self.clips.pop();
        }
    }

    /// Öffnet einen Batch, falls der letzte nicht passt (Clip oder
    /// Textur-Modus anders); sonst wird an den letzten angehängt.
    fn open_batch(&mut self, textured: bool) {
        let clip = *self.clips.last().unwrap_or(&Rect::default());
        let reuse = self.cmds.last().is_some_and(|c| c.textured == textured && c.clip == clip);
        if !reuse {
            self.cmds.push(DrawCmd { clip, textured, vertices: Vec::new() });
        }
    }

    fn quad(&mut self, r: Rect, uv: Rect, color: Rgba, textured: bool) {
        let v = |x: f32, y: f32, u: f32, vv: f32| UiVertex { pos: [x, y], uv: [u, vv], color };
        let a = v(r.x, r.y, uv.x, uv.y);
        let b = v(r.right(), r.y, uv.right(), uv.y);
        let c = v(r.right(), r.bottom(), uv.right(), uv.bottom());
        let d = v(r.x, r.bottom(), uv.x, uv.bottom());
        self.open_batch(textured);
        self.cmds
            .last_mut()
            .expect("Batch geöffnet")
            .vertices
            .extend_from_slice(&[a, b, c, a, c, d]);
    }

    /// Gefülltes Rechteck.
    pub fn rect(&mut self, r: Rect, color: Rgba) {
        self.quad(r, Rect::default(), color, false);
    }

    /// Rand als vier dünne Rechtecke (eckig; gerundete Ränder später).
    pub fn stroke(&mut self, r: Rect, w: f32, color: Rgba) {
        self.rect(Rect::new(r.x, r.y, r.w, w), color);
        self.rect(Rect::new(r.x, r.bottom() - w, r.w, w), color);
        self.rect(Rect::new(r.x, r.y, w, r.h), color);
        self.rect(Rect::new(r.right() - w, r.y, w, r.h), color);
    }

    /// Horizontale oder vertikale Linie mit Dicke — auf das Pixelraster
    /// gerastert (Kanten ganzzahlig), damit CPU-Referenz und
    /// Hardware-Rasterizer nie um eine Zeile divergieren. Eine 1px-Linie
    /// bei y=28 trifft die Zeile 28.
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, color: Rgba) {
        if (y0 - y1).abs() <= w {
            let top = (y0 - w * 0.5).round();
            self.rect(Rect::new(x0.min(x1), top, (x1 - x0).abs(), w.max(1.0)), color);
        } else {
            let left = (x0 - w * 0.5).round();
            self.rect(Rect::new(left, y0.min(y1), w.max(1.0), (y1 - y0).abs()), color);
        }
    }

    /// Texturiertes Rechteck (Glyph oder Bild) mit Atlas-UV.
    pub fn image(&mut self, r: Rect, uv: Rect, color: Rgba) {
        self.quad(r, uv, color, true);
    }

    /// Text in die Liste legen (texturiert); gibt die bbox (w, h) zurück.
    pub fn text(
        &mut self,
        atlas: &FontAtlas,
        font: usize,
        text: &str,
        x: f32,
        y: f32,
        line_h: f32,
        color: Rgba,
    ) -> [f32; 2] {
        let (verts, size) = atlas.text(font, text, x, y, line_h, color);
        if !verts.is_empty() {
            self.open_batch(true);
            self.cmds
                .last_mut()
                .expect("Batch geöffnet")
                .vertices
                .extend_from_slice(&verts);
        }
        size
    }
}

// ── CPU-Rasterizer (Referenz für den GPU-UI-Pass) ────────────────────

/// Integer-Scissor (x, y, w, h) eines Clip-Rechtecks — dieselbe Regel,
/// die der GPU-Pass als `set_scissor_rect` verwendet: ein Pixel `p`
/// gehört dazu, wenn `floor(clip) <= p < ceil(clip-Kante)` (Achse für
/// Achse). CPU-Referenz und Hardware teilen sich diese Konvention,
/// damit der Pixel-Diff exakt wird.
pub fn scissor_rect(clip: &Rect) -> (u32, u32, u32, u32) {
    let x0 = clip.x.max(0.0).floor() as u32;
    let y0 = clip.y.max(0.0).floor() as u32;
    let x1 = clip.right().max(0.0).ceil() as u32;
    let y1 = clip.bottom().max(0.0).ceil() as u32;
    (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

/// Malt eine Draw-List in ein RGBA8-Bild (linear gerechnet, am Ende
/// quantisiert). Achsenparallele Quads, Scissor nach `scissor_rect`
/// (Pixel-Ganzzahl-Mitgliedschaft — identisch zum GPU-`set_scissor_rect`),
/// Alpha-Blending in linearer RGBA, Atlas-Abdeckung bilinear — exakt
/// die Konvention, die der Hardware-Filter des GPU-Passes spiegeln
/// muss. Deterministisch; dieselbe Doctrine wie beim Schatten-Kern:
/// der CPU-Pfad ist die Wahrheit, die GPU difft dagegen.
pub fn rasterize(dl: &DrawList, atlas: Option<&FontAtlas>, w: u32, h: u32, clear: Rgba) -> Vec<u8> {
    let n = (w * h) as usize;
    let mut buf: Vec<Rgba> = vec![clear; n];
    for cmd in dl.cmds() {
        let (sx, sy, sw, sh) = scissor_rect(&cmd.clip);
        if sw == 0 || sh == 0 {
            continue;
        }
        for quad in cmd.vertices.chunks_exact(6) {
            // quad[0] = linke obere Ecke, quad[2] = rechte untere.
            let (x0, y0) = (quad[0].pos[0], quad[0].pos[1]);
            let (x1, y1) = (quad[2].pos[0], quad[2].pos[1]);
            let (rx0, rx1) = (x0.min(x1), x0.max(x1));
            let (ry0, ry1) = (y0.min(y1), y0.max(y1));
            if rx1 <= rx0 || ry1 <= ry0 {
                continue;
            }
            let (u0, v0) = (quad[0].uv[0], quad[0].uv[1]);
            let (u1, v1) = (quad[2].uv[0], quad[2].uv[1]);
            let color = quad[0].color;
            let px0 = (rx0.floor().max(sx as f32)) as u32;
            let py0 = (ry0.floor().max(sy as f32)) as u32;
            let px1 = (rx1.ceil().min((sx + sw) as f32).min(w as f32) as u32).max(px0);
            let py1 = (ry1.ceil().min((sy + sh) as f32).min(h as f32) as u32).max(py0);
            for py in py0..py1 {
                for px in px0..px1 {
                    let pcx = px as f32 + 0.5;
                    let pcy = py as f32 + 0.5;
                    if pcx < rx0 || pcx >= rx1 || pcy < ry0 || pcy >= ry1 {
                        continue;
                    }
                    let cov = if cmd.textured {
                        match atlas {
                            Some(a) => {
                                let tu = (pcx - rx0) / (rx1 - rx0);
                                let tv = (pcy - ry0) / (ry1 - ry0);
                                a.sample(u0 + (u1 - u0) * tu, v0 + (v1 - v0) * tv)
                            }
                            None => 1.0,
                        }
                    } else {
                        1.0
                    };
                    let sa = color[3] * cov;
                    if sa <= 0.0 {
                        continue;
                    }
                    let dst = &mut buf[(py * w + px) as usize];
                    let inv = 1.0 - sa;
                    for k in 0..3 {
                        dst[k] = color[k] * sa + dst[k] * inv;
                    }
                    dst[3] = sa + dst[3] * inv;
                }
            }
        }
    }
    let mut out = vec![0u8; n * 4];
    for (i, rgba) in buf.iter().enumerate() {
        for k in 0..4 {
            out[i * 4 + k] = (rgba[k].clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    out
}

// ── Font-Atlas ───────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error("Font konnte nicht gelesen werden: {0}")]
    Font(String),
    #[error("Atlas reicht nicht — {0} Glyphen passen nicht in {1}x{2}")]
    AtlasFull(usize, u32, u32),
}

/// Eine Glyph im Atlas: UV-Rechteck (normalisiert, exakt die Texel
/// der Glyphenfläche), Offset von der Grundlinie zur linken Oberkante
/// der Umrissfläche (y nach unten), Breite/Höhe in px, Vorschub.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub uv: Rect,
    pub offset: [f32; 2],
    pub size: [f32; 2],
    pub advance: f32,
}

/// R8-Glyphen-Atlas mehrerer Schriften, Shelf-Packer mit 1px Rand
/// (verhindert UV-Bluten beim Linearen Filtern). Deterministisch:
/// Glyphen werden sortiert nach (Font, Codepoint) gelegt.
#[derive(Clone, Debug)]
pub struct FontAtlas {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    ascents: Vec<f32>,
    descents: Vec<f32>,
    entries: std::collections::BTreeMap<(usize, char), Glyph>,
}

/// Zeichen, die der Atlas immer enthält: druckbares ASCII. Der Rest
/// kommt aus dem Text, der zum Atlas gebaut wird (`extra`).
pub const ASCII_CHARS: &str =
    " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

impl FontAtlas {
    /// Baut das Atlas aus TTF-Bytes (Font-Reihenfolge = Font-Index).
    /// `extra` ergänzt ASCII um weitere Zeichen (z. B. Umlaute).
    pub fn build(ttf_fonts: &[&[u8]], px: f32, extra: &str, width: u32, height: u32) -> Result<Self, UiError> {
        let mut fonts = Vec::with_capacity(ttf_fonts.len());
        for bytes in ttf_fonts {
            fonts.push(
                ab_glyph::FontVec::try_from_vec(bytes.to_vec())
                    .map_err(|e| UiError::Font(e.to_string()))?,
            );
        }
        let mut chars: Vec<char> = ASCII_CHARS.chars().collect();
        chars.extend(extra.chars());
        chars.sort_unstable();
        chars.dedup();

        let mut pixels = vec![0u8; (width * height) as usize];
        let mut entries: std::collections::BTreeMap<(usize, char), Glyph> =
            std::collections::BTreeMap::new();
        let mut ascents = Vec::with_capacity(fonts.len());
        let mut descents = Vec::with_capacity(fonts.len());

        let mut shelf_x = 1u32;
        let mut shelf_y = 1u32;
        let mut shelf_h = 0u32;
        for (fi, font) in fonts.iter().enumerate() {
            let scaled = font.as_scaled(px);
            ascents.push(scaled.ascent());
            descents.push(scaled.descent());
            for &ch in &chars {
                let id = scaled.glyph_id(ch);
                let adv = scaled.h_advance(id);
                // Umriss in Bildschirmkoordinaten (y nach unten, y = 0 ist
                // die Grundlinie): px_bounds.min.y ist negativ (Oberkante).
                let og = font.outline_glyph(id.with_scale(px));
                let bounds = og.as_ref().map(|o| o.px_bounds());
                let gw = bounds.map_or(0, |b| b.width().ceil().max(0.0) as u32);
                let gh = bounds.map_or(0, |b| b.height().ceil().max(0.0) as u32);
                if og.is_none() || gw == 0 || gh == 0 {
                    // Leerer Glyph (z. B. Leerzeichen): nur Vorschub.
                    entries.insert((fi, ch), Glyph {
                        uv: Rect::default(),
                        offset: [0.0, 0.0],
                        size: [0.0, 0.0],
                        advance: adv,
                    });
                    continue;
                }
                if shelf_x + gw + 1 > width {
                    shelf_x = 1;
                    shelf_y += shelf_h + 1;
                    shelf_h = 0;
                }
                if shelf_y + gh + 1 > height {
                    return Err(UiError::AtlasFull(entries.len(), width, height));
                }
                let og = og.expect("geprüft");
                og.draw(|x, y, alpha| {
                    let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                    let ix = shelf_x + 1 + x;
                    let iy = shelf_y + 1 + y;
                    if ix < width && iy < height {
                        let idx = (iy * width + ix) as usize;
                        pixels[idx] = pixels[idx].max(a);
                    }
                });
                let b = bounds.expect("geprüft");
                entries.insert(
                    (fi, ch),
                    Glyph {
                        uv: Rect::new(
                            (shelf_x + 1) as f32 / width as f32,
                            (shelf_y + 1) as f32 / height as f32,
                            gw as f32 / width as f32,
                            gh as f32 / height as f32,
                        ),
                        offset: [b.min.x, b.min.y],
                        size: [gw as f32, gh as f32],
                        advance: adv,
                    },
                );
                shelf_x += gw + 2;
                shelf_h = shelf_h.max(gh);
            }
        }
        Ok(Self {
            width,
            height,
            pixels,
            ascents,
            descents,
            entries,
        })
    }

    pub fn glyph(&self, font: usize, ch: char) -> Option<Glyph> {
        self.entries.get(&(font, ch)).copied()
    }

    /// Anzahl der Schriften im Atlas (Font-Index = Reihenfolge in
    /// [`Self::build`]; 0 = Regular, 1 = Bold, wenn mitgeliefert).
    pub fn font_count(&self) -> usize {
        self.ascents.len()
    }

    /// Misst einen Text ohne zu zeichnen — dieselbe Pen-Logik wie
    /// [`Self::text`] (unbekannte Zeichen übersprungen, `\n` bricht die
    /// Zeile), Rückgabe (w, h).
    pub fn measure(&self, font: usize, text: &str, line_h: f32) -> [f32; 2] {
        let ascent = self.ascents[font];
        let descent = self.descents[font];
        let mut pen_x = 0.0f32;
        let mut pen_y = 0.0f32;
        let mut max_w = 0.0f32;
        for ch in text.chars() {
            if ch == '\n' {
                max_w = max_w.max(pen_x);
                pen_x = 0.0;
                pen_y += line_h;
                continue;
            }
            let Some(g) = self.glyph(font, ch) else { continue };
            pen_x += g.advance;
        }
        max_w = max_w.max(pen_x);
        // descent ist negativ → Höhe = ascent - descent.
        [max_w, pen_y + ascent - descent]
    }

    /// Bilinear abgetastete Abdeckung (0..1) bei normalisierten
    /// Atlas-UV — Texel-Mitten-Konvention wie der Hardware-Filter.
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        let fx = (u * self.width as f32 - 0.5).clamp(0.0, self.width as f32 - 1.0);
        let fy = (v * self.height as f32 - 0.5).clamp(0.0, self.height as f32 - 1.0);
        let x0 = fx.floor() as u32;
        let y0 = fy.floor() as u32;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let at = |x: u32, y: u32| self.pixels[(y * self.width + x) as usize] as f32 / 255.0;
        let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
        let bot = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
        top * (1.0 - ty) + bot * ty
    }

    /// Legt einen Text als Vertex-Liste aus (ein Batch, texturiert).
    /// `x`/`y` = Schreibpunkt (y = Oberkante der ersten Zeile; die
    /// Grundlinie liegt bei `y + ascent`), `color` färbt die Glyphen.
    /// Rückgabe: Vertices + bbox (w, h). Unbekannte Zeichen werden
    /// übersprungen, `\n` bricht die Zeile (`line_h`).
    pub fn text(&self, font: usize, text: &str, x: f32, y: f32, line_h: f32, color: Rgba) -> (Vec<UiVertex>, [f32; 2]) {
        let ascent = self.ascents[font];
        let descent = self.descents[font];
        let mut verts = Vec::new();
        let mut pen_x = x;
        let mut pen_y = y;
        let mut max_w = 0.0f32;
        for ch in text.chars() {
            if ch == '\n' {
                max_w = max_w.max(pen_x - x);
                pen_x = x;
                pen_y += line_h;
                continue;
            }
            let Some(g) = self.glyph(font, ch) else { continue };
            if g.size[0] > 0.0 && g.size[1] > 0.0 {
                let baseline = pen_y + ascent;
                // Pixel-Snapping: die Glyphenfläche rastet auf das
                // Ganzzahl-Pixelraster ein. Damit liegen alle Quad-Kanten
                // auf ganzzahligen Koordinaten (Pixelzentren .5 treffen
                // nie eine Kante — CPU-Fill-Rule und Hardware-Rasterizer
                // können nicht divergieren), und die UV-Abtastung wird
                // 1:1: jedes Ziel-Pixel trifft exakt ein Atlas-Texel
                // (fx ganzzahlig → bilinear degeneriert zum Texelwert).
                // Der Pen bleibt fraktional, die Schrift läuft nicht weg.
                let gx = (pen_x + g.offset[0]).round();
                let gy = (baseline + g.offset[1]).round();
                let r = Rect::new(gx, gy, g.size[0], g.size[1]);
                // UV = exakt die Texel der Glyphenfläche; der 1px-Rand
                // im Atlas schützt beim linearen Filtern vor Bluten.
                let uv = g.uv;
                let v = |px: f32, py: f32, u: f32, vv: f32| UiVertex { pos: [px, py], uv: [u, vv], color };
                verts.extend_from_slice(&[
                    v(r.x, r.y, uv.x, uv.y),
                    v(r.right(), r.y, uv.right(), uv.y),
                    v(r.right(), r.bottom(), uv.right(), uv.bottom()),
                    v(r.x, r.y, uv.x, uv.y),
                    v(r.right(), r.bottom(), uv.right(), uv.bottom()),
                    v(r.x, r.bottom(), uv.x, uv.bottom()),
                ]);
            }
            pen_x += g.advance;
        }
        max_w = max_w.max(pen_x - x);
        // descent ist negativ (unter der Grundlinie) → Höhe = ascent - descent.
        (verts, [max_w, pen_y - y + ascent - descent])
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const FONT_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/inter/Inter-Regular.ttf");

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn rechteck_schnitt_und_enthalt() {
        let a = r(0.0, 0.0, 100.0, 50.0);
        assert_eq!(a.intersect(&r(50.0, 25.0, 100.0, 100.0)), r(50.0, 25.0, 50.0, 25.0));
        assert_eq!(a.intersect(&r(200.0, 0.0, 10.0, 10.0)), r(200.0, 0.0, 0.0, 0.0));
        assert!(a.contains(0.0, 0.0) && a.contains(99.9, 49.9) && !a.contains(100.0, 25.0));
    }

    #[test]
    fn theme_serde_roundtrip_und_default_identitaet() {
        let t = Theme::default();
        // Rust-Orange-Akzent: mehr Rot als Grün als Blau, gesättigt.
        let [rr, gg, bb, _] = t.colors.accent;
        assert!(rr > 0.7 && gg < rr && bb < gg, "Akzent muss warm-orange sein");
        let json = serde_json::to_string(&t).expect("serialisiert");
        let back: Theme = serde_json::from_str(&json).expect("deserialisiert");
        assert_eq!(back, t);
        assert_eq!(t.label("unbekannt.panel"), "unbekannt.panel");
    }

    #[test]
    fn dock_split_füllt_fläche_mit_splittern() {
        let tree = DockNode::Split {
            vertical: false,
            ratio: 0.25,
            first: Box::new(DockNode::Leaf { panel: "outliner".into() }),
            second: Box::new(DockNode::Leaf { panel: "viewport".into() }),
        };
        let p = dock_layout(&tree, r(0.0, 0.0, 1000.0, 600.0), 26.0, 4.0);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].rect, r(0.0, 0.0, 248.0, 600.0)); // 250 - halber Splitter
        assert_eq!(p[1].rect, r(252.0, 0.0, 748.0, 600.0));
    }

    #[test]
    fn dock_tabs_reservieren_streifen_und_zeigen_aktives() {
        let tree = DockNode::Tabs {
            active: 1,
            panels: vec!["a".into(), "b".into()],
        };
        let p = dock_layout(&tree, r(10.0, 20.0, 200.0, 180.0), 26.0, 4.0);
        assert_eq!(p.len(), 1, "nur das aktive Panel bekommt Inhalt");
        assert_eq!(p[0].panel, "b");
        assert_eq!(p[0].rect, r(10.0, 46.0, 200.0, 154.0));
        assert_eq!(p[0].tabs.len(), 2);
        assert_eq!(p[0].tabs[0].1, r(10.0, 20.0, 100.0, 26.0));
    }

    #[test]
    fn dock_verschachtelt_ohne_luecken() {
        let tree = DockNode::Split {
            vertical: true,
            ratio: 0.7,
            first: Box::new(DockNode::Split {
                vertical: false,
                ratio: 0.5,
                first: Box::new(DockNode::Leaf { panel: "a".into() }),
                second: Box::new(DockNode::Leaf { panel: "b".into() }),
            }),
            second: Box::new(DockNode::Leaf { panel: "c".into() }),
        };
        let area = r(0.0, 0.0, 400.0, 400.0);
        let p = dock_layout(&tree, area, 26.0, 4.0);
        assert_eq!(p.len(), 3);
        // Kein Blatt ragt über die Fläche hinaus, keine Überlappung.
        for x in &p {
            assert!(x.rect.x >= 0.0 && x.rect.right() <= 400.0);
            assert!(x.rect.y >= 0.0 && x.rect.bottom() <= 400.0);
        }
        for i in 0..p.len() {
            for j in i + 1..p.len() {
                assert_eq!(p[i].rect.intersect(&p[j].rect).w * p[i].rect.intersect(&p[j].rect).h, 0.0, "Überlappung {} vs {}", p[i].panel, p[j].panel);
            }
        }
    }

    #[test]
    fn drawlist_batcht_nach_clip_und_textur() {
        let mut dl = DrawList::new(r(0.0, 0.0, 100.0, 100.0));
        dl.rect(r(0.0, 0.0, 10.0, 10.0), [1.0, 0.0, 0.0, 1.0]);
        dl.rect(r(20.0, 0.0, 10.0, 10.0), [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(dl.cmds().len(), 1, "gleicher Clip + untexturiert => ein Batch");
        assert_eq!(dl.cmds()[0].vertices.len(), 12);
        dl.push_clip(r(50.0, 0.0, 50.0, 100.0));
        dl.rect(r(60.0, 0.0, 10.0, 10.0), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(dl.cmds().len(), 2, "anderer Clip => neuer Batch");
        assert_eq!(dl.cmds()[1].clip, r(50.0, 0.0, 50.0, 100.0));
        dl.pop_clip();
        dl.rect(r(0.0, 0.0, 5.0, 5.0), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(dl.cmds().len(), 3, "zurück: eigener Batch (Batch-Reihenfolge zählt)");
    }

    #[test]
    fn scissor_stapel_verschachtelt_klemmend() {
        let mut dl = DrawList::new(r(0.0, 0.0, 100.0, 100.0));
        dl.push_clip(r(10.0, 10.0, 80.0, 80.0));
        dl.push_clip(r(50.0, 50.0, 80.0, 80.0));
        dl.rect(r(60.0, 60.0, 5.0, 5.0), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(dl.cmds()[0].clip, r(50.0, 50.0, 40.0, 40.0), "Kind-Clip = Schnitt");
        dl.pop_clip();
        dl.rect(r(20.0, 20.0, 5.0, 5.0), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(dl.cmds()[1].clip, r(10.0, 10.0, 80.0, 80.0));
    }

    #[test]
    fn stroke_sind_vier_rechtecke() {
        let mut dl = DrawList::new(r(0.0, 0.0, 100.0, 100.0));
        dl.stroke(r(0.0, 0.0, 20.0, 20.0), 2.0, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(dl.cmds()[0].vertices.len(), 4 * 6);
    }

    #[test]
    fn atlas_rastert_inter_und_legt_text_aus() {
        let atlas = FontAtlas::build(&[FONT_REGULAR], 15.0, "äöüÄÖÜß", 512, 512).expect("Atlas");
        assert!(atlas.pixels.iter().any(|&p| p > 0), "Atlas muss Abdeckungen haben");
        let a = atlas.glyph(0, 'A').expect("A im Atlas");
        let sp = atlas.glyph(0, ' ').expect("Leerzeichen im Atlas");
        assert!(a.size[0] > 0.0 && a.advance > 0.0);
        assert_eq!(sp.size[0], 0.0, "Leerzeichen hat keine Fläche");
        assert!(sp.advance > 0.0, "aber Vorschub");
        let uml = atlas.glyph(0, 'ö').expect("ö im Atlas");
        assert!(uml.size[0] > 0.0, "extra-Zeichen im Atlas");
        // UV-Rechtecke dürfen sich nicht überlappen (1px Rand inklusive).
        let mut rects: Vec<(usize, char, Rect)> =
            atlas.entries.iter().map(|((f, c), g)| (*f, *c, g.uv)).filter(|(_, _, uv)| uv.w > 0.0).collect();
        rects.sort_by_key(|(f, c, _)| (*f, *c as u32));
        for w in rects.windows(2) {
            let (a, b) = (w[0].2, w[1].2);
            let ix = a.intersect(&b);
            assert!(ix.w <= 0.0 || ix.h <= 0.0, "UV-Überlappung {:?} vs {:?}", w[0], w[1]);
        }
        // Text-Auslage: zwei Zeichen => 12 Vertices, Breite > 0.
        let (verts, size) = atlas.text(0, "Aö", 10.0, 20.0, 18.0, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(verts.len(), 12);
        assert!(size[0] > 0.0 && size[1] >= 15.0);
        assert!(verts.iter().all(|v| v.uv[0] >= 0.0 && v.uv[0] <= 1.0 && v.uv[1] >= 0.0 && v.uv[1] <= 1.0));
        // Mehrzeilig: Höhe wächst um line_h.
        let (_, size2) = atlas.text(0, "A\nA", 0.0, 0.0, 18.0, [1.0, 1.0, 1.0, 1.0]);
        assert!(size2[1] > size[1]);
    }

    #[test]
    fn atlas_bau_ist_deterministisch() {
        let a = FontAtlas::build(&[FONT_REGULAR], 15.0, "", 512, 512).expect("a");
        let b = FontAtlas::build(&[FONT_REGULAR], 15.0, "", 512, 512).expect("b");
        assert_eq!(a.pixels, b.pixels);
        assert_eq!(a.entries.keys().count(), b.entries.keys().count());
        for (k, ga) in &a.entries {
            assert_eq!(*ga, b.entries[k], "Glyph {k:?} instabil");
        }
    }

    #[test]
    fn rasterizer_blendet_und_scissort() {
        let mut dl = DrawList::new(r(0.0, 0.0, 20.0, 20.0));
        dl.rect(r(2.0, 2.0, 8.0, 8.0), [1.0, 0.0, 0.0, 1.0]);
        dl.push_clip(r(12.0, 0.0, 8.0, 20.0));
        dl.rect(r(10.0, 4.0, 8.0, 8.0), [0.0, 1.0, 0.0, 0.5]);
        dl.pop_clip();
        let img = rasterize(&dl, None, 20, 20, [0.0, 0.0, 0.0, 0.0]);
        let at = |x: u32, y: u32, k: usize| img[((y * 20 + x) as usize) * 4 + k];
        assert_eq!((at(5, 5, 0), at(5, 5, 3)), (255, 255), "ndeckendes Rot");
        assert_eq!((at(0, 0, 0), at(0, 0, 3)), (0, 0), "Clear bleibt leer");
        assert_eq!((at(14, 8, 1), at(14, 8, 3)), (128, 128), "50% gemischt");
        assert_eq!(at(10, 8, 1), 0, "Scissor schneidet links ab");
        assert_eq!(at(14, 2, 1), 0, "oberhalb des Rechtecks nichts");
    }

    #[test]
    fn rasterizer_malt_text_lesbar() {
        let atlas = FontAtlas::build(&[FONT_REGULAR], 24.0, "", 512, 512).expect("Atlas");
        let mut dl = DrawList::new(r(0.0, 0.0, 160.0, 48.0));
        let size = dl.text(&atlas, 0, "Hallo", 8.0, 8.0, 28.0, [1.0, 1.0, 1.0, 1.0]);
        assert!(size[0] > 20.0 && size[1] > 20.0, "bbox plausibel: {size:?}");
        let img = rasterize(&dl, Some(&atlas), 160, 48, [0.0, 0.0, 0.0, 1.0]);
        let lit = |x: u32, y: u32| img[((y * 160 + x) as usize) * 4] > 40;
        let inside = (8..8 + size[0] as u32 + 2).flat_map(|x| (6..40).map(move |y| (x, y)));
        assert!(inside.filter(|&(x, y)| lit(x, y)).count() > 100, "Glyphenfläche");
        // Außerhalb der Textbbox (mit Rand) darf nichts leuchten.
        let outside = (0..160u32).flat_map(|x| (0..48u32).map(move |y| (x, y))).filter(
            |(x, y)| *x < 6 || *x > 8 + size[0] as u32 + 2 || *y < 4 || *y > 42,
        );
        assert!(outside.filter(|&(x, y)| lit(x, y)).count() == 0, "kein Ausbluten");
    }

    #[test]
    fn rasterizer_png_beweis() {
        // Opt-in: RE_UI_PNG=/pfad/mock.png schreibt ein zusammengesetztes
        // Panel-Bild — Augenbeweis für Theme, Atlas und Rasterizer.
        let Ok(path) = std::env::var("RE_UI_PNG") else { return };
        const FONT_BOLD: &[u8] = include_bytes!("../../../assets/fonts/inter/Inter-Bold.ttf");
        let theme = Theme::default();
        let atlas = FontAtlas::build(&[FONT_REGULAR, FONT_BOLD], 15.0, "äöüÄÖÜß—", 1024, 512)
            .expect("Atlas");
        let (w, h) = (480u32, 280u32);
        let (wf, hf) = (w as f32, h as f32);
        let mut dl = DrawList::new(r(0.0, 0.0, wf, hf));
        dl.rect(r(0.0, 0.0, wf, hf), theme.colors.background);
        // Kopfleiste
        dl.rect(r(0.0, 0.0, wf, 28.0), theme.colors.header);
        dl.line(0.0, 28.0, wf, 28.0, 1.0, theme.colors.border);
        dl.text(&atlas, 1, "re-editor — P1", 10.0, 6.0, 20.0, theme.colors.text);
        // Outliner-Attrappe links
        dl.rect(r(6.0, 36.0, 170.0, hf - 42.0), theme.colors.panel);
        dl.stroke(r(6.0, 36.0, 170.0, hf - 42.0), 1.0, theme.colors.border);
        dl.text(&atlas, 1, "Outliner", 14.0, 42.0, 18.0, theme.colors.text_dim);
        for i in 0..6u32 {
            let y = 66.0 + i as f32 * 22.0;
            if i == 1 {
                dl.rect(r(8.0, y - 2.0, 166.0, 20.0), theme.colors.select);
            } else if i == 3 {
                dl.rect(r(8.0, y - 2.0, 166.0, 20.0), theme.colors.hover);
            }
            dl.text(
                &atlas,
                0,
                &format!("Entity {i} — Würfel äöü"),
                16.0,
                y,
                18.0,
                theme.colors.text,
            );
        }
        // Viewport rechts
        let vx = 182.0;
        dl.rect(r(vx, 36.0, wf - vx - 6.0, hf - 42.0), theme.colors.viewport_bg);
        dl.stroke(r(vx, 36.0, wf - vx - 6.0, hf - 42.0), 1.0, theme.colors.border);
        dl.text(&atlas, 0, "Perspektive", vx + 10.0, 44.0, 18.0, theme.colors.text_dim);
        // Akzent-Button (Rust-Orange)
        let btn = r(vx + 12.0, hf - 46.0, 120.0, 26.0);
        dl.rect(btn, theme.colors.accent);
        dl.text(&atlas, 1, "Anwenden", btn.x + 22.0, btn.y + 5.0, 18.0, [1.0, 1.0, 1.0, 1.0]);
        let img = rasterize(&dl, Some(&atlas), w, h, theme.colors.background);
        image::RgbaImage::from_raw(w, h, img)
            .expect("bild")
            .save(&path)
            .expect("png");
    }
}
