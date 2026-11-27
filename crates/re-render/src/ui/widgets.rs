//! Immediate-Mode-Widgets auf der Draw-List (P1, Block 2).
//!
//! Widgets sind zwischen Frames zustandslos: Jeder wird pro Frame mit
//! ID und Rechteck aufgerufen, liest die Eingabe aus [`UiInput`] und
//! schreibt Ereignisse als Rückgabe. [`UiState`] hält ausschließlich,
//! was Frames überdauern muss (aktive Taste, Fokus, Scroll-Offset,
//! offene Bäume) — niemals einen Widget-Baum.
//!
//! Doktrin:
//! - **Pixel-Rasterung**: Jedes Widget rastet sein Rechteck auf
//!   ganzzahlige Pixelkanten ein ([`snap`]) — die Konvention des
//!   UI-Kerns, damit CPU-Referenz und Hardware-Rasterizer
//!   nie divergieren.
//! - **Theme allein**: Farben, Abstände, Schriften kommen aus dem
//!   [`Theme`]; Widgets kennen keinen hartcodierten Look
//!   (ADR-0001).
//! - **Kein Engine-Aufruf**: Widgets liefern Ereignisse
//!   (clicked/edited/toggled); die Editor-Schicht bindet sie an
//!   Capabilities (`EditorContext::call`), nie umgekehrt.

use std::collections::BTreeMap;

use super::{DrawList, FontAtlas, Rect, Rgba, Theme};

/// Rechteck auf ganzzahlige Pixelkanten einrasten (x/y floor,
/// rechte/untere Kante ceil) — Kanten treffen nie Pixelzentren.
pub fn snap(r: Rect) -> Rect {
    let x = r.x.floor();
    let y = r.y.floor();
    let right = r.right().ceil();
    let bottom = r.bottom().ceil();
    Rect::new(x, y, (right - x).max(0.0), (bottom - y).max(0.0))
}

/// Ergebnis eines Mausgriffs (`Ui::drag`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DragInfo {
    /// Diese ID hält die Maus (vom Druck- bis zum Loslass-Frame).
    pub claimed: bool,
    /// Seit dem Druck mindestens `min_px` Weg zurückgelegt.
    pub moved: bool,
    /// Feuert genau einmal im Loslass-Frame.
    pub ended: bool,
}

/// Eingabe-Lage eines Frames. Der Editor mappt Plattform-Events
/// (`re-platform`) hierher; die Widgets kennen keine Plattform.
#[derive(Clone, Debug, Default)]
pub struct UiInput {
    /// Zeigerposition in Fenster-Pixeln; `None` = außerhalb.
    pub pointer: Option<[f32; 2]>,
    /// Linke Maustaste in diesem Frame gedrückt worden.
    pub pressed: bool,
    /// Linke Maustaste in diesem Frame losgelassen worden.
    pub released: bool,
    /// Scroll-Rad in Pixeln (positiv = Inhalt nach unten schieben).
    pub scroll: f32,
    /// Getippte Zeichen dieses Frames (bereits decoded).
    pub chars: String,
    pub backspace: bool,
    pub enter: bool,
}

/// Interaktions-Zustand über Frames — ein Exemplar pro Fenster.
#[derive(Clone, Debug, Default)]
pub struct UiState {
    active: Option<u64>,
    focus: Option<u64>,
    scroll: BTreeMap<u64, f32>,
    open: BTreeMap<u64, bool>,
    /// Druckpunkt des laufenden Zieh-Vorgangs (für Weg-Strecke).
    drag_origin: Option<[f32; 2]>,
}

impl UiState {
    /// Hält gerade ein Widget den Tastatur-Fokus (z. B. ein Textfeld)?
    /// Der App-Code nutzt das, um Tastatureingaben umzuleiten.
    pub fn has_focus(&self) -> bool {
        self.focus.is_some()
    }
}

/// Antwort einer Baumzeile.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TreeResponse {
    pub clicked: bool,
    pub toggled: bool,
}

/// FNV-1a über die Widget-ID — stabil über Frames und Prozesse.
fn hash_id(key: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Kürzt Text mit `...`, wenn er in `max_w` nicht passt (Atlas-Messung).
pub fn fit(atlas: &FontAtlas, font: usize, text: &str, max_w: f32) -> String {
    if atlas.measure(font, text, 0.0)[0] <= max_w {
        return text.to_string();
    }
    let mut best = String::new();
    for ch in text.chars() {
        best.push(ch);
        if atlas.measure(font, &format!("{best}..."), 0.0)[0] > max_w {
            best.pop();
            break;
        }
    }
    best.push_str("...");
    best
}

/// Ein Zeichen-Durchgang über eine Fläche (Fenster oder Panel). Der
/// Fluss (`row`) stapelt vertikal; alle Widgets nehmen explizite
/// Rechtecke und rasten sie selbst ein.
pub struct Ui<'a> {
    pub draw: DrawList,
    pub theme: &'a Theme,
    atlas: &'a FontAtlas,
    /// Optional kleineres Atlas für Kleinsttext (Tabs, Fußzeilen);
    /// ohne es fallen kleine Texte auf das normale Atlas zurück.
    small: Option<&'a FontAtlas>,
    state: &'a mut UiState,
    input: &'a UiInput,
    x: f32,
    y: f32,
    w: f32,
    indent: f32,
}

impl<'a> Ui<'a> {
    pub fn new(
        area: Rect,
        theme: &'a Theme,
        atlas: &'a FontAtlas,
        small: Option<&'a FontAtlas>,
        state: &'a mut UiState,
        input: &'a UiInput,
    ) -> Self {
        Self {
            draw: DrawList::new(area),
            theme,
            atlas,
            small,
            state,
            input,
            x: area.x,
            y: area.y,
            w: area.w,
            indent: 0.0,
        }
    }

    /// Frame beenden: Loslassen gibt die aktive Taste frei.
    pub fn end(&mut self) {
        if self.input.released {
            self.state.active = None;
            self.state.drag_origin = None;
        }
    }

    // ── Fluss ────────────────────────────────────────────────────────

    /// Nächste Fluss-Zeile über die volle Breite (eingeschnappt).
    pub fn row(&mut self, h: f32) -> Rect {
        let r = snap(Rect::new(self.x + self.indent, self.y, (self.w - self.indent).max(0.0), h));
        self.y = r.bottom();
        r
    }

    /// Fluss um `h` Pixel nach unten schieben.
    pub fn space(&mut self, h: f32) {
        self.y += h;
    }

    /// Einrückung für folgende Fluss-Zeilen (Baumtiefe).
    pub fn push_indent(&mut self, px: f32) {
        self.indent += px;
    }

    pub fn pop_indent(&mut self, px: f32) {
        self.indent = (self.indent - px).max(0.0);
    }

    // ── Eingabe ──────────────────────────────────────────────────────

    /// Treffer-Test eines Frames: (überstrichen, geklickt). Klick =
    /// Loslassen über dem Rechteck, während es die aktive Taste hält.
    /// Wer die Taste schon hält (laufender Zieh-Vorgang), lässt sich
    /// nicht bestehlen — Ansprüche prüfen deshalb `active.is_none()`.
    fn interact(&mut self, id: &str, r: Rect) -> (bool, bool) {
        let hid = hash_id(id);
        let under = self.input.pointer.is_some_and(|p| r.contains(p[0], p[1]));
        if under && self.input.pressed && self.state.active.is_none() && self.state.focus.is_none() {
            self.state.active = Some(hid);
        }
        let clicked = self.input.released && self.state.active == Some(hid) && under;
        let hovered = under || self.state.active == Some(hid);
        (hovered, clicked)
    }

    fn is_active(&self, id: &str) -> bool {
        self.state.active == Some(hash_id(id))
    }

    // ── Ziehen ───────────────────────────────────────────────────────

    /// Zeigerposition dieses Frames (Fenster-Pixel; `None` = außerhalb).
    pub fn pointer(&self) -> Option<[f32; 2]> {
        self.input.pointer
    }

    /// Hält diese ID gerade die Maus (laufender Klick- oder Zieh-Vorgang)?
    pub fn is_held(&self, id: &str) -> bool {
        self.state.active == Some(hash_id(id))
    }

    /// Mausgriff: Druck in `grab` beansprucht die Maus bis zum Loslassen.
    /// `moved` meldet zurückgelegte Wege ab `min_px` (Manhattan-Distanz)
    /// — dasselbe Rechteck kann so Klick-Ziel UND Zieh-Griff sein.
    /// `ended` feuert genau einmal im Loslass-Frame (Zeiger egal wo);
    /// `ui.end()` gibt die Taste danach frei.
    pub fn drag(&mut self, id: &str, grab: Rect, min_px: f32) -> DragInfo {
        let hid = hash_id(id);
        let p = self.input.pointer;
        if self.state.active == Some(hid) {
            let dist = self
                .state
                .drag_origin
                .and_then(|o| p.map(|c| (c[0] - o[0]).abs() + (c[1] - o[1]).abs()))
                .unwrap_or(0.0);
            DragInfo { claimed: true, moved: dist >= min_px.max(0.0), ended: self.input.released }
        } else {
            let under = p.is_some_and(|c| grab.contains(c[0], c[1]));
            let claim = under && self.input.pressed && self.state.active.is_none() && self.state.focus.is_none();
            if claim {
                self.state.active = Some(hid);
                self.state.drag_origin = p;
                DragInfo { claimed: true, moved: min_px <= 0.0, ended: self.input.released }
            } else {
                DragInfo::default()
            }
        }
    }

    // ── Text ─────────────────────────────────────────────────────────

    fn pick(&self, small: bool, bold: bool) -> (&'a FontAtlas, usize) {
        let atlas = if small { self.small.unwrap_or(self.atlas) } else { self.atlas };
        let font = if bold && atlas.font_count() > 1 { 1 } else { 0 };
        (atlas, font)
    }

    /// Text mittig im Rechteck (schneidet nicht ab — zentrierte Texte
    /// sind kurz und kommen aus dem Label-Set).
    fn text_centered(&mut self, r: Rect, text: &str, color: Rgba, small: bool, bold: bool) {
        let (atlas, font) = self.pick(small, bold);
        let line_h = atlas.measure(font, "Ag", 0.0)[1];
        let [tw, th] = atlas.measure(font, text, line_h);
        let x = r.x + ((r.w - tw) * 0.5).max(0.0);
        let y = r.y + ((r.h - th) * 0.5).max(0.0);
        self.draw.text(atlas, font, text, x, y, line_h, color);
    }

    /// Text linksbündig, vertikal mittig, bei Bedarf mit `...` gekürzt.
    fn text_left(&mut self, r: Rect, text: &str, color: Rgba, small: bool, bold: bool) {
        let (atlas, font) = self.pick(small, bold);
        let line_h = atlas.measure(font, "Ag", 0.0)[1];
        let shown = fit(atlas, font, text, r.w);
        let th = atlas.measure(font, &shown, line_h)[1];
        let y = r.y + ((r.h - th) * 0.5).max(0.0);
        self.draw.text(atlas, font, &shown, r.x, y, line_h, color);
    }

    /// Gefülltes Dreieck aus achsenparallelen Scanlines (der
    /// Rasterizer kennt nur Rechtecke): `down` = ▾, sonst ▸. Alle
    /// Endpunkte werden gerundet — auch Scanlines halten die
    /// Pixelkanten-Konvention ein.
    fn triangle(&mut self, r: Rect, down: bool, color: Rgba) {
        let r = snap(r);
        if down {
            let n = (r.h as i32).max(1);
            for i in 0..n {
                // ▾: oben volle Breite, nach unten zur Spitze.
                let t = 1.0 - i as f32 / n as f32;
                let cx = (r.x + r.w * 0.5).round();
                let hw = (r.w * 0.5 * t).round();
                self.draw.line(cx - hw, r.y + i as f32, cx + hw, r.y + i as f32, 1.0, color);
            }
        } else {
            let n = (r.w as i32).max(1);
            for i in 0..n {
                let t = 1.0 - i as f32 / n as f32;
                let cy = (r.y + r.h * 0.5).round();
                let hh = (r.h * t).round();
                let (y0, y1) = ((cy - hh * 0.5).round(), (cy + hh * 0.5).round());
                self.draw.line(r.x + i as f32, y0, r.x + i as f32, y1, 1.0, color);
            }
        }
    }

    // ── Widgets ──────────────────────────────────────────────────────

    /// Statischer Text, linksbündig, gekürzt — keine Interaktion.
    pub fn label(&mut self, r: Rect, text: &str) {
        let p = self.theme.metrics.pad;
        let c = self.theme.colors.text;
        self.text_left(Rect::new(r.x + p, r.y, (r.w - p * 2.0).max(0.0), r.h), text, c, false, false);
    }

    /// Knopf: Fläche, Rand, mittiger Text; `true` beim Klick.
    pub fn button(&mut self, id: &str, r: Rect, text: &str) -> bool {
        let r = snap(r);
        let (hovered, clicked) = self.interact(id, r);
        let th = self.theme;
        let bg = if self.is_active(id) && hovered {
            th.colors.select
        } else if hovered {
            th.colors.hover
        } else {
            th.colors.header
        };
        self.draw.rect(r, bg);
        self.draw.stroke(r, 1.0, th.colors.border);
        self.text_centered(r, text, th.colors.text, false, false);
        clicked
    }

    /// Listen-/Baumzeile mit Auswahlfarbe; `true` beim Klick.
    pub fn selectable(&mut self, id: &str, r: Rect, text: &str, selected: bool) -> bool {
        let r = snap(r);
        let (hovered, clicked) = self.interact(id, r);
        let th = self.theme;
        if selected {
            self.draw.rect(r, th.colors.select);
        } else if hovered {
            self.draw.rect(r, th.colors.hover);
        }
        let p = th.metrics.pad;
        self.text_left(Rect::new(r.x + p, r.y, (r.w - p * 2.0).max(0.0), r.h), text, th.colors.text, false, false);
        clicked
    }

    /// Baumzeile: Einrückung je Tiefe, Dreieck zum Auf-/Zuklappen
    /// (nur wenn `has_children`), Text. Der Klick aufs Dreieck zählt
    /// nicht als Zeilen-Klick (Reihenfolge: Zeile erst, Dreieck zuletzt).
    pub fn tree_row(
        &mut self,
        id: &str,
        r: Rect,
        text: &str,
        depth: usize,
        selected: bool,
        has_children: bool,
    ) -> TreeResponse {
        let r = snap(r);
        let th = self.theme;
        let step = th.metrics.pad * 2.0;
        let x0 = r.x + depth as f32 * step;
        // Dreieck ZUERST beanspruchen — sitz der Zeiger drauf, darf die
        // Zeile den Druck nicht mehr wegschnappen (interact claimt nur,
        // solange niemand die Taste hält).
        let mut toggled = false;
        let mut arrow: Option<(Rect, bool, bool)> = None;
        let mut row_x = x0;
        if has_children {
            let a = Rect::new(x0, r.y, step, r.h);
            let (ah, ac) = self.interact(&format!("{id}/pfeil"), a);
            toggled = ac;
            if ac {
                self.toggle(id, true);
            }
            arrow = Some((a, ah, self.is_open(id, true)));
            row_x = a.right();
        }
        let (hovered, clicked) = self.interact(id, Rect::new(row_x, r.y, (r.right() - row_x).max(0.0), r.h));
        let mut tx = x0 + th.metrics.pad;
        if let Some((a, _, _)) = arrow {
            tx = a.right();
        }
        // Fläche zuerst, dann Dreieck, dann Text — sonst übermalt die
        // Auswahl-/Hover-Rechteck das eben erst gemalte Dreieck.
        if selected {
            self.draw.rect(r, th.colors.select);
        } else if hovered {
            self.draw.rect(r, th.colors.hover);
        }
        if let Some((a, ah, open)) = arrow {
            let color = if ah || hovered { th.colors.text } else { th.colors.text_dim };
            self.triangle(Rect::new(a.x + 3.0, a.y + a.h * 0.5 - 4.0, 8.0, 8.0), open, color);
        }
        self.text_left(
            Rect::new(tx, r.y, (r.right() - tx - th.metrics.pad).max(0.0), r.h),
            text,
            th.colors.text,
            false,
            false,
        );
        TreeResponse { clicked, toggled }
    }

    /// Auf-/Zuklapp-Zustand (über Frames gemerkt).
    pub fn is_open(&mut self, id: &str, default: bool) -> bool {
        *self.state.open.entry(hash_id(id)).or_insert(default)
    }

    /// Zustand kippen (der Baum-Widget tut das selbst nach Klick).
    pub fn toggle(&mut self, id: &str, default: bool) {
        let cur = *self.state.open.get(&hash_id(id)).unwrap_or(&default);
        self.state.open.insert(hash_id(id), !cur);
    }

    /// Sektionskopf (fett, Dreieck, Kopfzeilenfläche): gibt den
    /// geöffneten Zustand zurück und kippt ihn beim Klick.
    pub fn header(&mut self, id: &str, r: Rect, text: &str) -> bool {
        let r = snap(r);
        let (hovered, clicked) = self.interact(id, r);
        if clicked {
            self.toggle(id, true);
        }
        let th = self.theme;
        self.draw.rect(r, th.colors.header);
        if hovered {
            self.draw.rect(r, th.colors.hover);
        }
        let open = self.is_open(id, true);
        self.triangle(
            Rect::new(r.x + th.metrics.pad, r.y + r.h * 0.5 - 4.0, 8.0, 8.0),
            open,
            th.colors.text_dim,
        );
        let tx = r.x + th.metrics.pad + 8.0 + th.metrics.pad * 0.5;
        self.text_left(
            Rect::new(tx, r.y, (r.right() - tx - th.metrics.pad).max(0.0), r.h),
            text,
            th.colors.text,
            false,
            true,
        );
        open
    }

    /// Dock-Tab (Rechteck aus `dock_layout`): `true` beim Klick. Das
    /// aktive Tab hebt sich mit Panelfläche + Akzentlinie ab.
    pub fn tab(&mut self, id: &str, r: Rect, text: &str, active: bool) -> bool {
        let r = snap(r);
        let (hovered, clicked) = self.interact(id, r);
        let th = self.theme;
        self.draw.rect(r, if active { th.colors.panel } else { th.colors.background });
        if hovered && !active {
            self.draw.rect(r, th.colors.hover);
        }
        if active {
            self.draw.rect(Rect::new(r.x, r.bottom() - 2.0, r.w, 2.0), th.colors.accent);
        }
        self.text_centered(r, text, if active { th.colors.text } else { th.colors.text_dim }, true, active);
        clicked
    }

    /// Dock-Tab als Klick-ZIEL **und** Zieh-Griff: gibt
    /// (geklickt, drag) zurück. Klick = Loslassen ohne vorherige Weg-
    /// strecke ab `min_px` → Tab aktivieren. Weiterer Weg → Umlager-
    /// Drag; dann bleibt `clicked` false und der Aufrufer hält die
    /// Drag-Info in den Händen.
    pub fn tab_drag(&mut self, id: &str, r: Rect, text: &str, active: bool, min_px: f32) -> (bool, DragInfo) {
        let r = snap(r);
        let d = self.drag(id, r, min_px);
        let under = self.input.pointer.is_some_and(|p| r.contains(p[0], p[1]));
        let clicked = d.claimed && d.ended && !d.moved && under;
        let th = self.theme;
        self.draw.rect(r, if active { th.colors.panel } else { th.colors.background });
        if under && !d.moved && !active {
            self.draw.rect(r, th.colors.hover);
        }
        if active {
            self.draw.rect(Rect::new(r.x, r.bottom() - 2.0, r.w, 2.0), th.colors.accent);
        }
        self.text_centered(r, text, if active { th.colors.text } else { th.colors.text_dim }, true, active);
        (clicked, d)
    }

    /// Checkbox mit Text; `true` beim Umschalten. Das Häkchen ist ein
    /// gefüllter Akzent-Block (der Rasterizer kennt keine Diagonalen).
    pub fn checkbox(&mut self, id: &str, r: Rect, checked: bool, text: &str) -> bool {
        let r = snap(r);
        let (hovered, clicked) = self.interact(id, r);
        let th = self.theme;
        if hovered {
            self.draw.rect(r, th.colors.hover);
        }
        let s = th.metrics.icon_px.min(r.h - 4.0).max(4.0);
        let bx = snap(Rect::new(r.x + th.metrics.pad, r.y + (r.h - s) * 0.5, s, s));
        self.draw.rect(bx, th.colors.input);
        self.draw.stroke(bx, 1.0, th.colors.border);
        if checked {
            self.draw.rect(Rect::new(bx.x + 2.0, bx.y + 2.0, (bx.w - 4.0).max(1.0), (bx.h - 4.0).max(1.0)), th.colors.accent);
        }
        let tx = bx.right() + th.metrics.pad * 0.5;
        self.text_left(Rect::new(tx, r.y, (r.right() - tx - th.metrics.pad).max(0.0), r.h), text, th.colors.text, false, false);
        clicked
    }

    /// Textfeld: Klick setzt Fokus, Zeichen landen in `value`,
    /// Backspace löscht, Enter verliert den Fokus. `true` bei Änderung.
    pub fn text_field(&mut self, id: &str, r: Rect, value: &mut String) -> bool {
        let r = snap(r);
        let hid = hash_id(id);
        let under = self.input.pointer.is_some_and(|p| r.contains(p[0], p[1]));
        if under && self.input.pressed {
            self.state.focus = Some(hid);
        } else if self.input.pressed && self.state.focus == Some(hid) {
            self.state.focus = None;
        }
        let focused = self.state.focus == Some(hid);
        let mut edited = false;
        if focused {
            if self.input.backspace && !value.is_empty() {
                value.pop();
                edited = true;
            }
            for ch in self.input.chars.chars() {
                if !ch.is_control() {
                    value.push(ch);
                    edited = true;
                }
            }
            if self.input.enter {
                self.state.focus = None;
            }
        }
        let th = self.theme;
        self.draw.rect(r, th.colors.input);
        self.draw.stroke(r, 1.0, if focused { th.colors.accent } else { th.colors.border });
        let p = th.metrics.pad;
        let inner = Rect::new(r.x + p, r.y, (r.w - p * 2.0).max(0.0), r.h);
        let (atlas, font) = self.pick(false, false);
        let shown = fit(atlas, font, value, inner.w);
        let line_h = atlas.measure(font, "Ag", 0.0)[1];
        let ty = r.y + ((r.h - line_h) * 0.5).max(0.0);
        self.draw.text(atlas, font, &shown, inner.x, ty, line_h, th.colors.text);
        if focused {
            let w = atlas.measure(font, &shown, line_h)[0];
            let cx = (inner.x + w + 1.0).round();
            self.draw.rect(Rect::new(cx, r.y + 3.0, 1.0, (r.h - 6.0).max(1.0)), th.colors.text);
        }
        edited
    }

    /// Scroll-Fläche: das Scroll-Rad über `r` verschiebt den gemerkten
    /// Offset (geklemmt auf `0..=content_h - r.h`); bei Überlauf zeichnet
    /// sie einen dünnen Scrollbalken. Der Aufrufer clippt den Inhalt
    /// selbst (`draw.push_clip`) und schiebt ihn um `-Offset` nach oben.
    pub fn scroll(&mut self, id: &str, r: Rect, content_h: f32) -> f32 {
        let hid = hash_id(id);
        let th = self.theme;
        let max = (content_h - r.h).max(0.0);
        let under = self.input.pointer.is_some_and(|p| r.contains(p[0], p[1]));
        let off = self.state.scroll.entry(hid).or_insert(0.0);
        if under {
            *off = (*off + self.input.scroll).clamp(0.0, max);
        } else {
            *off = off.clamp(0.0, max);
        }
        if max > 0.0 {
            let hh = (r.h * r.h / content_h).max(20.0);
            let ty = r.y + (r.h - hh) * (*off / max);
            self.draw.rect(snap(Rect::new(r.right() - 5.0, ty, 4.0, hh)), th.colors.border);
        }
        *off
    }

    /// 1px-Trennlinie (Endpunkte gerundet, von `line()` pixelgesnapst).
    pub fn separator(&mut self, r: Rect) {
        let c = self.theme.colors.border;
        let y = (r.y + r.h * 0.5).floor();
        self.draw.line(r.x.floor(), y, r.right().ceil(), y, 1.0, c);
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const REGULAR: &[u8] = include_bytes!("../../../../assets/fonts/inter/Inter-Regular.ttf");
    const BOLD: &[u8] = include_bytes!("../../../../assets/fonts/inter/Inter-Bold.ttf");

    fn atlas() -> FontAtlas {
        FontAtlas::build(&[REGULAR, BOLD], 15.0, "äöüß", 512, 512).expect("Atlas")
    }

    fn inp(pointer: Option<[f32; 2]>, pressed: bool, released: bool) -> UiInput {
        UiInput { pointer, pressed, released, ..Default::default() }
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 200.0, 120.0)
    }

    #[test]
    fn button_klickt_erst_beim_release_innerhalb() {
        let a = atlas();
        let t = Theme::default();
        let mut st = UiState::default();
        let r = Rect::new(0.0, 0.0, 50.0, 20.0);
        {
            let i = inp(Some([10.0, 10.0]), true, false);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            assert!(!ui.button("ok", r, "OK"), "Press allein klickt nicht");
            ui.end();
        }
        {
            let i = inp(Some([10.0, 10.0]), false, true);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            assert!(ui.button("ok", r, "OK"), "Release innerhalb klickt");
            ui.end();
        }
        {
            // Nochmal: Press innen, Release außen → kein Klick.
            let i = inp(Some([10.0, 10.0]), true, false);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            ui.button("ok", r, "OK");
            ui.end();
            let i = inp(Some([150.0, 100.0]), false, true);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            assert!(!ui.button("ok", r, "OK"), "Release außerhalb bricht ab");
            ui.end();
        }
    }

    #[test]
    fn tab_drag_klick_ohne_weg_aktiviert_mit_weg_dragt() {
        let a = atlas();
        let t = Theme::default();
        let mut st = UiState::default();
        let r = Rect::new(0.0, 0.0, 90.0, 24.0);
        // Klick: Druck und Release ohne Wegstrecke → aktivieren, kein Drag.
        {
            let i = inp(Some([10.0, 10.0]), true, false);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            let (clicked, d) = ui.tab_drag("tab/x", r, "X", false, 6.0);
            assert!(d.claimed && !d.moved && !d.ended && !clicked, "Press allein noch nicht");
            ui.end();
        }
        {
            let i = inp(Some([12.0, 11.0]), false, true);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            let (clicked, d) = ui.tab_drag("tab/x", r, "X", false, 6.0);
            assert!(d.ended && !d.moved && clicked, "Klick mit Mini-Wackeln");
            ui.end();
        }
        // Drag: press, ≥ 6 px ziehen, anderswo loslassen → kein Klick.
        {
            let i = inp(Some([10.0, 10.0]), true, false);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            ui.tab_drag("tab/y", r, "Y", false, 6.0);
            ui.end();
        }
        {
            let i = inp(Some([90.0, 60.0]), false, false);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            let (clicked, d) = ui.tab_drag("tab/y", r, "Y", false, 6.0);
            assert!(d.claimed && d.moved && !d.ended && !clicked, "Drag läuft");
            ui.end();
        }
        {
            let i = inp(Some([90.0, 60.0]), false, true);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            let (clicked, d) = ui.tab_drag("tab/y", r, "Y", false, 6.0);
            assert!(d.ended && d.moved && !clicked, "Drag endet ohne Klick");
            ui.end();
        }
    }

    #[test]
    fn laufender_drag_wehrt_andere_widgets_am_klick_ab() {
        let a = atlas();
        let t = Theme::default();
        let mut st = UiState::default();
        let grab = Rect::new(0.0, 0.0, 6.0, 100.0);
        let btn = Rect::new(100.0, 0.0, 60.0, 24.0);
        {
            let i = inp(Some([2.0, 50.0]), true, false);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            assert!(ui.drag("split", grab, 0.0).claimed, "Splitter greift");
            assert!(!ui.button("ok", btn, "OK"));
            ui.end();
        }
        {
            // Release über einem Button: der darf nicht durch den Drag
            // hindurch klicken.
            let i = inp(Some([120.0, 10.0]), false, true);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            let d = ui.drag("split", grab, 0.0);
            assert!(d.claimed && d.moved && d.ended);
            assert!(!ui.button("ok", btn, "OK"), "Release klickt nicht durch");
            ui.end();
        }
    }

    #[test]
    fn auswahl_und_hover_malen_theme_farben() {
        let a = atlas();
        let t = Theme::default();
        let mut st = UiState::default();
        let i = inp(Some([10.0, 30.0]), false, false);
        let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
        ui.selectable("z", Rect::new(0.0, 0.0, 100.0, 24.0), "Zeile", true);
        ui.selectable("x", Rect::new(0.0, 24.0, 100.0, 24.0), "Andere", false);
        let farben: Vec<Rgba> = ui
            .draw
            .cmds()
            .iter()
            .filter(|c| !c.textured)
            .flat_map(|c| c.vertices.iter())
            .map(|v| v.color)
            .collect();
        assert!(farben.contains(&t.colors.select), "Auswahl nutzt select-Farbe");
        assert!(farben.contains(&t.colors.hover), "Hover nutzt hover-Farbe");
    }

    #[test]
    fn baum_pfeil_klappt_um_ohne_zeilenklick() {
        let a = atlas();
        let t = Theme::default();
        let mut st = UiState::default();
        let r = Rect::new(0.0, 0.0, 150.0, 24.0);
        // Press ins Dreieck (x < pad*2) kippt noch nicht …
        {
            let i = inp(Some([6.0, 12.0]), true, false);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            let ant = ui.tree_row("ordner", r, "Content", 0, false, true);
            assert!(!ant.toggled, "Press allein kippt nicht");
            ui.end();
        }
        // … der Release darüber kippt, ohne die Zeile zu klicken.
        let ant = {
            let i = inp(Some([6.0, 12.0]), false, true);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            let ant = ui.tree_row("ordner", r, "Content", 0, false, true);
            ui.end();
            ant
        };
        assert!(ant.toggled && !ant.clicked, "Dreieck kippt nur");
        assert!(!st.open[&hash_id("ordner")], "offen → zu");
        // Klick auf den Text: Zeile klickt, Zustand bleibt.
        {
            let i = inp(Some([80.0, 12.0]), true, false);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            ui.tree_row("ordner", r, "Content", 0, false, true);
            ui.end();
        }
        let ant = {
            let i = inp(Some([80.0, 12.0]), false, true);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            let ant = ui.tree_row("ordner", r, "Content", 0, false, true);
            ui.end();
            ant
        };
        assert!(ant.clicked && !ant.toggled, "Zeile klickt nur");
        assert!(!st.open[&hash_id("ordner")], "Zustand bleibt");
    }

    #[test]
    fn scroll_klemmt_und_reagiert_nur_unter_der_flaeche() {
        let a = atlas();
        let t = Theme::default();
        let mut st = UiState::default();
        let r = Rect::new(0.0, 0.0, 100.0, 100.0);
        {
            // Rad über der Fläche: 350 nach, geklemmt auf 300.
            let mut i = inp(Some([50.0, 50.0]), false, false);
            i.scroll = 350.0;
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            assert_eq!(ui.scroll("liste", r, 400.0), 300.0);
            ui.end();
        }
        {
            // Rad daneben (Zeiger außerhalb): keine Änderung.
            let mut i = inp(Some([150.0, 110.0]), false, false);
            i.scroll = -100.0;
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            assert_eq!(ui.scroll("liste", r, 400.0), 300.0);
            ui.end();
        }
        {
            // Über der Fläche zurück: unter 0 geklemmt.
            let mut i = inp(Some([50.0, 50.0]), false, false);
            i.scroll = -500.0;
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            assert_eq!(ui.scroll("liste", r, 400.0), 0.0);
            ui.end();
        }
    }

    #[test]
    fn textfeld_fokus_eingabe_enter() {
        let a = atlas();
        let t = Theme::default();
        let mut st = UiState::default();
        let mut wert = String::new();
        let r = Rect::new(0.0, 0.0, 120.0, 24.0);
        {
            let i = inp(Some([10.0, 10.0]), true, true);
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            ui.text_field("pfad", r, &mut wert);
            ui.end();
        }
        {
            let mut i = inp(Some([10.0, 10.0]), false, false);
            i.chars = "hi".into();
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            assert!(ui.text_field("pfad", r, &mut wert));
            ui.end();
        }
        assert_eq!(wert, "hi");
        {
            let mut i = inp(Some([10.0, 10.0]), false, false);
            i.backspace = true;
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            assert!(ui.text_field("pfad", r, &mut wert));
            ui.end();
        }
        assert_eq!(wert, "h");
        {
            let mut i = inp(Some([10.0, 10.0]), false, false);
            i.enter = true;
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            ui.text_field("pfad", r, &mut wert);
            ui.end();
        }
        assert_eq!(wert, "h");
        assert_eq!(st.focus, None, "Enter verliert den Fokus");
        {
            // Ohne Fokus landen Zeichen nicht mehr im Wert.
            let mut i = inp(Some([10.0, 10.0]), false, false);
            i.chars = "x".into();
            let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
            assert!(!ui.text_field("pfad", r, &mut wert));
            ui.end();
        }
        assert_eq!(wert, "h", "fokussloses Tippen ändert nichts");
    }

    #[test]
    fn fit_kuerzt_mit_drei_punkten() {
        let a = atlas();
        let lang = "Ein sehr langer Panelname fuer die Kuerzung";
        let kurz = fit(&a, 0, lang, 60.0);
        assert!(kurz.ends_with("..."), "KI: {kurz}");
        assert!(a.measure(0, &kurz, 0.0)[0] <= 60.0, "muss in 60 px passen");
        assert_eq!(fit(&a, 0, "kurz", 60.0), "kurz");
    }

    #[test]
    fn alle_widget_kanten_rasten_auf_pixel() {
        let a = atlas();
        let t = Theme::default();
        let mut st = UiState::default();
        let mut wert = String::from("Test");
        let i = inp(Some([7.3, 9.7]), false, false);
        let mut ui = Ui::new(area(), &t, &a, None, &mut st, &i);
        // Absichtlich fraktionale Rechtecke — die Widgets müssen snappen.
        ui.header("k", Rect::new(0.4, 0.2, 199.1, 23.7), "Kategorie");
        ui.button("b", Rect::new(10.5, 30.1, 44.3, 19.8), "OK");
        ui.tab("t", Rect::new(60.9, 0.5, 50.2, 25.4), "Tab", true);
        ui.checkbox("c", Rect::new(10.2, 55.6, 90.4, 22.3), true, "Aktiv");
        ui.text_field("f", Rect::new(10.8, 80.9, 100.1, 23.2), &mut wert);
        ui.scroll("s", Rect::new(0.0, 0.0, 100.0, 50.0), 400.0);
        ui.separator(Rect::new(0.3, 110.7, 199.4, 0.0));
        for cmd in ui.draw.cmds() {
            if cmd.textured {
                continue; // Glyphen snappt der Atlas selbst.
            }
            for v in &cmd.vertices {
                assert_eq!(v.pos[0].fract(), 0.0, "x-Kante {} nicht pixelgesnapst", v.pos[0]);
                assert_eq!(v.pos[1].fract(), 0.0, "y-Kante {} nicht pixelgesnapst", v.pos[1]);
            }
        }
    }

    #[test]
    fn frame_mit_widgets_rasterisiert_sichtbar() {
        let a = atlas();
        let t = Theme::default();
        let mut st = UiState::default();
        let i = inp(Some([30.0, 34.0]), false, false);
        let (w, h) = (320u32, 200u32);
        let mut ui = Ui::new(Rect::new(0.0, 0.0, w as f32, h as f32), &t, &a, None, &mut st, &i);
        let kopf = ui.row(24.0);
        ui.header("k", kopf, "Outliner");
        let z1 = ui.row(24.0);
        ui.tree_row("a", z1, "Welt", 0, false, true);
        let z2 = ui.row(24.0);
        ui.tree_row("b", z2, "Licht", 1, true, false);
        let z3 = ui.row(24.0);
        ui.tree_row("c", z3, "Ein sehr langer Szenenobjektname", 1, false, false);
        let tab = ui.row(26.0);
        ui.tab("t", Rect::new(0.0, tab.y, 90.0, tab.h), "Details", true);
        ui.tab("t2", Rect::new(90.0, tab.y, 90.0, tab.h), "Konsole", false);
        let mut wert = String::from("r.ScreenPercentage 50");
        ui.text_field("f", Rect::new(6.0, 140.0, 150.0, 24.0), &mut wert);
        ui.button("ok", Rect::new(162.0, 140.0, 60.0, 24.0), "Ausführen");
        ui.separator(Rect::new(0.0, 175.0, w as f32, 1.0));
        let px = |c: Rgba| [
            (c[0] * 255.0).round() as u8,
            (c[1] * 255.0).round() as u8,
            (c[2] * 255.0).round() as u8,
            255,
        ];
        let bild = super::super::rasterize(&ui.draw, Some(&a), w, h, t.colors.background);
        let hat = |c: Rgba| bild.chunks_exact(4).any(|p| p == px(c));
        assert!(hat(t.colors.header), "Kopfzeile gemalt");
        assert!(hat(t.colors.select), "Auswahl gemalt");
        assert!(hat(t.colors.text), "Text gemalt");
        assert!(hat(t.colors.accent), "Tab-Akzent gemalt");
        if let Ok(p) = std::env::var("RE_WIDGETS_PNG") {
            use image::RgbaImage;
            RgbaImage::from_raw(w, h, bild).expect("puffer").save(&p).expect("png");
        }
    }
}
