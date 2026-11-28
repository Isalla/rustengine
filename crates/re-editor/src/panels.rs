//! Panel-Inhalte des Widget-Sets (ADR-0001: Widgets sind Code, Layout
//! und Look sind Daten).
//!
//! Jede Funktion malt genau ein Panel in sein Platzierungs-Rechteck.
//! Alle Aktionen laufen über `EditorContext::call` mit Capability-Namen
//! aus dem Manifest — kein Panel greift direkt auf Engine-Interna zu,
//! keine Labels stehen im Code (sie kommen aus `Theme::label`).

use serde_json::{json, Value};

use re_render::ui::widgets::{Ui, UiInput};
use re_render::ui::{DockEdge, FontAtlas, PanelPlacement, Rect, Theme};

use crate::context::EditorContext;
use crate::layout::{Command, Menu};

/// Obergrenze des Konsolen-Puffers.
const LOG_CAP: usize = 200;

/// Panel-übergreifender UI-Zustand über Frames.
#[derive(Default)]
pub struct PanelState {
    /// Ausgewählte Entity (Index); die Selektion im EditorState wird
    /// über die Capability `editor.selection.set` nachgezogen.
    pub selected: Option<u32>,
    /// Konsolen-Puffer (älteste Zeile zuerst).
    pub log: Vec<String>,
    /// Kommandozeile der Konsole.
    pub cmdline: String,
    /// Eingabefeld des Assistant-Docks.
    pub assistant_line: String,
    /// Geöffnetes Aufklapp-Menü (Menü-id) oder None.
    pub open_menu: Option<String>,
    /// Von einem Tab-Knopf angeforderte Aktivierung — die App setzt sie
    /// im Dock-Baum um (`activate_tab`) und leert das Feld.
    pub tab_pick: Option<String>,
    /// Editier-Puffer des Inspektors: "entity/komponente/pfad" → Text.
    pub edits: std::collections::HashMap<String, String>,
    /// Läuft gerade ein Wert-Scrubbing: Tooltip-Text + Zeigerpunkt.
    pub scrub: Option<(String, [f32; 2])>,
    /// Popup-Rechteck des offenen Menüs (für Klick-außerhalb-Test).
    pub popup: Option<Rect>,
    /// Läuft ein Tab-Umlager-Drag (Panel-id)?
    pub drag_panel: Option<String>,
    /// Der Umlager-Drag ist im aktuellen Frame losgelassen worden.
    pub drag_ended: bool,
    /// Aktive Drop-Zone während des Umlager-Drag (Ziel-Panel + Kante).
    pub drop: Option<(String, DockEdge)>,
}

impl PanelState {
    /// Eine Zeile in die Konsole.
    pub fn log(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > LOG_CAP {
            self.log.remove(0);
        }
    }
}

/// Ein Manifest-Command ausführen und das Ergebnis in die Konsole
/// schreiben — der einzige Weg, wie Chrome-Controls die Engine treffen.
pub fn run_command(ctx: &EditorContext, st: &mut PanelState, cmd: &Command) {
    if cmd.capability.is_empty() {
        return;
    }
    let args = if cmd.args.is_null() { json!({}) } else { cmd.args.clone() };
    match ctx.call(&cmd.capability, args.clone()) {
        Ok(v) => st.log(format!("{} → {}", cmd.capability, summarize(&v))),
        Err(e) => st.log(format!("{} FEHLER: {e}", cmd.capability)),
    }
}

/// Ergebnis-JSON kurz anfassbar machen (eine Zeile, gekürzt).
fn summarize(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if s.chars().count() > 120 {
        let cut: String = s.chars().take(117).collect();
        format!("{cut}…")
    } else {
        s
    }
}

// ── Chrome (Menüleiste, Toolbar, Statusleiste) ───────────────────────

/// Breite eines Menütitels: gemessener Text + Innenabstand.
fn menu_width(atlas: &FontAtlas, text: &str, pad: f32) -> f32 {
    atlas.measure(0, text, 0.0)[0].ceil() + pad * 2.0
}

/// Positionen der Menütitel in der Leiste (reiner Layout-Rechner —
/// Popup-Zeichnung und Klickbehandlung nutzen dieselben Rechtecke).
pub fn menu_bar_layout(
    menus: &[Menu],
    atlas: &FontAtlas,
    area: Rect,
    pad: f32,
) -> Vec<(String, Rect)> {
    let mut x = area.x + pad;
    menus
        .iter()
        .map(|m| {
            let w = menu_width(atlas, m.id.as_str(), pad);
            let r = Rect::new(x, area.y, w, area.h);
            x += w + pad;
            (m.id.clone(), r)
        })
        .collect()
}

/// Menüleiste: Titel-Zeile; Klick öffnet/schließt das Menü. Das Popup
/// selbst malt [`draw_menu_popup`] ERST nach den Panels (Overlay).
pub fn draw_menu_bar(
    ui: &mut Ui,
    area: Rect,
    menus: &[Menu],
    theme: &Theme,
    atlas: &FontAtlas,
    st: &mut PanelState,
) {
    ui.draw.rect(area, theme.colors.header);
    let slots = menu_bar_layout(menus, atlas, area, theme.metrics.pad);
    for (id, r) in slots {
        let open = st.open_menu.as_deref() == Some(id.as_str());
        let label = theme.label(&id).to_string();
        if ui.button(&format!("menubar/{id}"), r, &label) {
            st.open_menu = if open { None } else { Some(id.clone()) };
        }
    }
}

/// Popup des offenen Menüs — als Overlay nach allen Panels gemalt,
/// damit es über dem Inhalt liegt. Klick auf einen Punkt: Command
/// ausführen, Menü schließen.
pub fn draw_menu_popup(
    ui: &mut Ui,
    menus: &[Menu],
    theme: &Theme,
    atlas: &FontAtlas,
    bar: Rect,
    ctx: &EditorContext,
    st: &mut PanelState,
) {
    let Some(open_id) = st.open_menu.clone() else { st.popup = None; return };
    let Some(menu) = menus.iter().find(|m| m.id == open_id) else { st.popup = None; return };
    let slots = menu_bar_layout(menus, atlas, bar, theme.metrics.pad);
    let Some(a) = slots.iter().find(|(id, _)| *id == open_id).map(|(_, r)| *r) else {
        st.popup = None;
        return;
    };

    let row = theme.metrics.row_h;
    let w = {
        let widest = menu
            .items
            .iter()
            .filter(|i| !i.separator)
            .map(|i| atlas.measure(0, theme.label(&i.id), 0.0)[0])
            .fold(0.0f32, f32::max);
        (widest.ceil() + theme.metrics.pad * 4.0).max(160.0)
    };
    let h = menu.items.len() as f32 * row + 2.0;
    let pop = Rect::new(a.x, bar.bottom(), w, h);
    st.popup = Some(pop);

    ui.draw.rect(pop, theme.colors.panel);
    ui.draw.stroke(pop, 1.0, theme.colors.border);
    let mut y = pop.y + 1.0;
    let mut fired: Option<Command> = None;
    for (i, item) in menu.items.iter().enumerate() {
        let r = Rect::new(pop.x + 1.0, y, pop.w - 2.0, row);
        if item.separator {
            ui.separator(r);
        } else {
            let label = theme.label(&item.id).to_string();
            if ui.selectable(&format!("menu/{open_id}/{i}"), r, &label, false) {
                fired = Some(item.clone());
            }
        }
        y += row;
    }
    if let Some(cmd) = fired {
        run_command(ctx, st, &cmd);
        st.open_menu = None;
        st.popup = None;
    }
}

/// Toolbar: Manifest-Commands als Knopfleiste unter der Menüleiste.
pub fn draw_toolbar(
    ui: &mut Ui,
    area: Rect,
    toolbar: &[Command],
    theme: &Theme,
    atlas: &FontAtlas,
    ctx: &EditorContext,
    st: &mut PanelState,
) {
    ui.draw.rect(area, theme.colors.panel);
    let pad = theme.metrics.pad;
    let mut x = area.x + pad;
    for (i, cmd) in toolbar.iter().enumerate() {
        let label = theme.label(&cmd.id).to_string();
        let w = atlas.measure(0, &label, 0.0)[0].ceil() + pad * 2.0;
        let r = Rect::new(x, area.y + 2.0, w, (area.h - 4.0).max(1.0));
        if ui.button(&format!("toolbar/{i}/{}", cmd.capability), r, &label) {
            run_command(ctx, st, cmd);
        }
        x += w + pad * 0.5;
    }
}

/// Statusleiste: Bedienhinweis (Theme-Label) + Projekt-/Selektionsstand.
pub fn draw_status_bar(
    ui: &mut Ui,
    area: Rect,
    theme: &Theme,
    atlas: &FontAtlas,
    ctx: &EditorContext,
    st: &PanelState,
) {
    ui.draw.rect(area, theme.colors.header);
    let p = theme.metrics.pad;
    let line_h = atlas.measure(0, "Ag", 0.0)[1];
    let ty = area.y + ((area.h - line_h) * 0.5).max(0.0);
    ui.draw.text(atlas, 0, theme.label("status.hint"), area.x + p, ty, line_h, theme.colors.text_dim);
    // Rechte Seite: Projekt-Info + Selektion über Capabilities erfragt.
    let info = ctx.call("project.info", json!({})).unwrap_or(Value::Null);
    let mut right = String::new();
    if let Some(name) = info.get("name").and_then(|v| v.as_str()) {
        if !name.is_empty() {
            right.push_str(name);
            if info.get("dirty").and_then(|v| v.as_bool()).unwrap_or(false) {
                right.push_str(" •");
            }
        }
    }
    if let Some(sel) = st.selected {
        right.push_str(&format!("  sel:#{sel}"));
    }
    if !right.is_empty() {
        let w = atlas.measure(0, &right, 0.0)[0];
        ui.draw.text(atlas, 0, &right, (area.right() - w - p).max(area.x), ty, line_h, theme.colors.text);
    }
}

// ── Panels ───────────────────────────────────────────────────────────

/// Panel-Rahmen: Fläche, Rand, Titelstreifen, Tab-Streifen; gibt das
/// Inhalts-Rechteck unter dem Titel zurück.
fn panel_frame(
    ui: &mut Ui,
    pl: &PanelPlacement,
    theme: &Theme,
    atlas: &FontAtlas,
    st: &mut PanelState,
) -> Rect {
    let r = pl.rect;
    ui.draw.rect(r, theme.colors.panel);
    ui.draw.stroke(r, 1.0, theme.colors.border);
    let title = theme.label(&format!("panel.{}", pl.panel)).to_string();
    let th = theme.metrics.row_h;
    let head = Rect::new(r.x, r.y, r.w, th);
    ui.draw.rect(head, theme.colors.header);
    let line_h = atlas.measure(0, "Ag", 0.0)[1];
    ui.draw.text(
        atlas,
        1,
        &title,
        r.x + theme.metrics.pad,
        head.y + ((head.h - line_h) * 0.5).max(0.0),
        line_h,
        theme.colors.text,
    );
    // Tabs der Gruppe (falls dieses Panel in einer Tab-Gruppe aktiv ist).
    // Klick aktiviert; Weg ab 6 px startet den Umlager-Drag (die App
    // wertet `st.drag_panel`/`st.drag_ended` nach dem Panels-Durchgang aus).
    for (id, tr) in &pl.tabs {
        let label = theme.label(&format!("panel.{id}")).to_string();
        let active = id == &pl.panel;
        let (clicked, d) = ui.tab_drag(&format!("tab/{id}"), *tr, &label, active, 6.0);
        if clicked {
            st.tab_pick = Some(id.clone());
        }
        if d.claimed && d.moved && st.drag_panel.is_none() {
            st.drag_panel = Some(id.clone());
        }
        if st.drag_panel.as_deref() == Some(id.as_str()) {
            st.drag_ended = d.ended;
        }
    }
    Rect::new(r.x, r.y + th, r.w, (r.h - th).max(0.0))
}

// ── Umlager-Drag (Drop-Zonen + Geist) ────────────────────────────────

/// Drop-Zonen über den übrigen Panels: erst die vier Randstreifen
/// (dorthin splitten), dann die ganze Fläche (Center = als Tab andocken).
/// Die Suchreihenfolge ist die Priorität — Kante schlägt Mitte.
pub fn drop_zones(placements: &[PanelPlacement], dragging: &str) -> Vec<(Rect, String, DockEdge)> {
    let mut out = Vec::new();
    for pl in placements {
        if pl.panel == dragging {
            continue;
        }
        let r = pl.rect;
        let d = (r.w.min(r.h) * 0.28).clamp(24.0, 140.0);
        let bands = [
            (Rect::new(r.x, r.y, d, r.h), DockEdge::Left),
            (Rect::new(r.right() - d, r.y, d, r.h), DockEdge::Right),
            (Rect::new(r.x, r.y, r.w, d), DockEdge::Top),
            (Rect::new(r.x, r.bottom() - d, r.w, d), DockEdge::Bottom),
        ];
        for (z, e) in bands {
            out.push((z, pl.panel.clone(), e));
        }
        out.push((r, pl.panel.clone(), DockEdge::Center));
    }
    out
}

/// Zone unter dem Zeiger (Kanten zuerst — Center wird zuletzt gesucht).
pub fn drop_target_at(zones: &[(Rect, String, DockEdge)], p: [f32; 2]) -> Option<(Rect, String, DockEdge)> {
    zones.iter().find(|(r, _, _)| r.contains(p[0], p[1])).cloned()
}

/// Rückmeldung während des Drags: feine Akzentkonturen über den
/// Kantenstreifen, Leuchtfüllung fürs aktive Ziel — dieselbe Sprache
/// wie UE5s Kompass, im eigenen Theme.
pub fn draw_drop_zones(ui: &mut Ui, zones: &[(Rect, String, DockEdge)], active: Option<Rect>, theme: &Theme) {
    let mut faint = theme.colors.accent;
    faint[3] = 0.45;
    let mut glow = theme.colors.accent;
    glow[3] = 0.28;
    for (r, _, e) in zones {
        if *e != DockEdge::Center {
            ui.draw.stroke(*r, 1.0, faint);
        }
    }
    if let Some(a) = active {
        ui.draw.rect(a, glow);
        ui.draw.stroke(a, 2.0, theme.colors.accent);
    }
}

/// Der Geister-Anhänger, der dem Zeiger folgt.
pub fn draw_drag_ghost(ui: &mut Ui, theme: &Theme, atlas: &FontAtlas, label: &str, p: [f32; 2]) {
    let line_h = atlas.measure(0, "Ag", 0.0)[1];
    let w = atlas.measure(0, label, 0.0)[0].ceil() + theme.metrics.pad * 2.0;
    let r = Rect::new(p[0] + 10.0, p[1] + 6.0, w, line_h + theme.metrics.pad);
    ui.draw.rect(r, theme.colors.panel);
    ui.draw.stroke(r, 1.0, theme.colors.accent);
    ui.draw.text(atlas, 0, label, r.x + theme.metrics.pad, r.y + theme.metrics.pad * 0.5, line_h, theme.colors.text);
}

/// Anzeigename einer Entity: Name-Component, sonst rohe Index-Form.
fn entity_name(ctx: &EditorContext, e: u32) -> String {
    ctx.call("ecs.entity.get", json!({ "entity": e, "component": "Name" }))
        .ok()
        .and_then(|v| v.get("wert")?.get("value")?.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("Entity #{e}"))
}

/// Outliner: Entity-Liste über `ecs.entities`, Klick selektiert über
/// `editor.selection.set`.
pub fn draw_outliner(
    ui: &mut Ui,
    pl: &PanelPlacement,
    theme: &Theme,
    atlas: &FontAtlas,
    ctx: &EditorContext,
    st: &mut PanelState,
) {
    let body = panel_frame(ui, pl, theme, atlas, st);
    let entities: Vec<u32> = ctx
        .call("ecs.entities", json!({}))
        .ok()
        .and_then(|v| v.get("entities")?.as_array().cloned())
        .map(|a| a.iter().filter_map(|e| e.as_u64().map(|x| x as u32)).collect())
        .unwrap_or_default();
    if entities.is_empty() {
        let line_h = atlas.measure(0, "Ag", 0.0)[1];
        ui.draw.text(atlas, 0, "—", body.x + theme.metrics.pad, body.y + theme.metrics.pad, line_h, theme.colors.text_dim);
        return;
    }
    let row = theme.metrics.row_h;
    let content_h = entities.len() as f32 * row;
    let off = ui.scroll("outliner/scroll", body, content_h);
    ui.draw.push_clip(body);
    for (i, e) in entities.iter().enumerate() {
        let name = entity_name(ctx, *e);
        let r = Rect::new(body.x, body.y + i as f32 * row - off, (body.w - 5.0).max(0.0), row);
        if ui.selectable(&format!("outliner/{e}"), r, &name, st.selected == Some(*e)) {
            st.selected = Some(*e);
            st.edits.clear();
            let _ = ctx.call("editor.selection.set", json!({ "ids": [e] }));
        }
    }
    ui.draw.pop_clip();
}

/// Inspector: Komponenten der selektierten Entity aus
/// `ecs.entity.components` + `ecs.entity.get`; das Name-Feld ist über
/// `ecs.entity.set` editierbar.
pub fn draw_inspector(
    ui: &mut Ui,
    pl: &PanelPlacement,
    theme: &Theme,
    atlas: &FontAtlas,
    ctx: &EditorContext,
    st: &mut PanelState,
) {
    let body = panel_frame(ui, pl, theme, atlas, st);
    let Some(e) = st.selected else {
        let line_h = atlas.measure(0, "Ag", 0.0)[1];
        ui.draw.text(atlas, 0, "—", body.x + theme.metrics.pad, body.y + theme.metrics.pad, line_h, theme.colors.text_dim);
        return;
    };
    let comps: Vec<String> = ctx
        .call("ecs.entity.components", json!({ "entity": e }))
        .ok()
        .and_then(|v| v.get("components")?.as_array().cloned())
        .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let pad = theme.metrics.pad;
    let row = theme.metrics.row_h;
    let line_h = atlas.measure(0, "Ag", 0.0)[1];
    let mut y = body.y + pad * 0.5;
    ui.draw.push_clip(body);
    for comp in &comps {
        if y > body.bottom() {
            break;
        }
        let hr = Rect::new(body.x + 1.0, y, (body.w - 2.0).max(0.0), row);
        let open = ui.header(&format!("insp/{e}/{comp}"), hr, comp);
        y += row;
        if !open {
            continue;
        }
        let wert = ctx
            .call("ecs.entity.get", json!({ "entity": e, "component": comp }))
            .ok()
            .and_then(|v| v.get("wert").cloned());
        let Some(wert) = wert else { continue };
        if comp == "Name" {
            // Editierbares Namensfeld — Commit pro Änderung über die
            // Capability (kein direkter Welt-Zugriff).
            let key = format!("{e}/{comp}/value");
            let cur = wert.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let buf = st.edits.entry(key).or_insert(cur);
            let fr = Rect::new(body.x + pad * 2.0, y, (body.w - pad * 4.0).max(40.0), row);
            if ui.text_field(&format!("insp/{e}/name/value"), fr, buf) {
                let _ = ctx.call(
                    "ecs.entity.set",
                    json!({ "entity": e, "component": "Name", "wert": { "value": buf } }),
                );
            }
            y += row + pad * 0.5;
        } else {
            // Zahlenblätter sind direkt editierbar: Klick ins Feld
            // schreibt Text, Ziehen am Namen scrubbt den Wert (Shift =
            // feiner). Alles andere erscheint als JSON-Zeile. Geschrieben
            // wird ausschließlich über `ecs.entity.set`.
            match &wert {
                Value::Object(map) => {
                    let label_w = (body.w * 0.45).max(70.0);
                    for (k, v) in map {
                        if y > body.bottom() {
                            break;
                        }
                        match v {
                            Value::Number(n) => {
                                y = number_row(ui, body, y, label_w, row, pad, e, comp, k, &Value::Number(n.clone()), &wert, ctx, st);
                            }
                            Value::Array(a) if !a.is_empty() && a.iter().all(Value::is_number) => {
                                for (i, n) in a.iter().enumerate() {
                                    let path = format!("{k}[{i}]");
                                    y = number_row(ui, body, y, label_w, row, pad, e, comp, &path, n, &wert, ctx, st);
                                }
                            }
                            other => {
                                let line = format!("{k}: {}", summarize(other));
                                ui.draw.text(atlas, 0, &line, body.x + pad * 2.0, y, line_h, theme.colors.text_dim);
                                y += line_h + 2.0;
                            }
                        }
                    }
                    y += pad * 0.5;
                }
                Value::Number(n) => {
                    let label_w = (body.w * 0.45).max(70.0);
                    y = number_row(ui, body, y, label_w, row, pad, e, comp, "value", &Value::Number(n.clone()), &wert, ctx, st);
                    y += pad * 0.5;
                }
                other => {
                    let line = summarize(other);
                    ui.draw.text(atlas, 0, &line, body.x + pad * 2.0, y, line_h, theme.colors.text_dim);
                    y += line_h + 2.0;
                    y += pad * 0.5;
                }
            }
        }
    }
    ui.draw.pop_clip();
}

/// Eine editierbare Zahlenzeile: Name links (Zieh-Fläche), Eingabefeld
/// rechts. Gibt die neue y-Position zurück.
#[allow(clippy::too_many_arguments)]
fn number_row(
    ui: &mut Ui,
    body: Rect,
    y: f32,
    label_w: f32,
    row: f32,
    pad: f32,
    e: u32,
    comp: &str,
    path: &str,
    leaf: &Value,
    wert: &Value,
    ctx: &EditorContext,
    st: &mut PanelState,
) -> f32 {
    let is_int = leaf.is_i64() || leaf.is_u64();
    let step = if is_int { 0.05 } else { scrub_step(comp, path) };
    let lr = Rect::new(body.x + 4.0, y, (label_w - 6.0).max(30.0), row);
    let s = ui.scrub(&format!("scrub/{e}/{comp}/{path}"), lr, step);
    let key = format!("{e}/{comp}/{path}");
    if !st.edits.contains_key(&key) {
        st.edits.insert(key.clone(), leaf_text(leaf));
    }
    let mut buf = st.edits.get(&key).cloned().unwrap_or_default();
    let mut touched = false;
    if s.dragging || s.ended {
        let base = buf.trim().parse::<f64>().unwrap_or_else(|_| leaf.as_f64().unwrap_or(0.0));
        buf = fmt_leaf(is_int, base + f64::from(s.delta));
        touched = true;
    }
    if s.dragging {
        if let Some(p) = ui.pointer() {
            st.scrub = Some((buf.clone(), p));
        }
    } else if s.ended {
        st.scrub = None;
    }
    let fr = Rect::new(lr.right() + 2.0, y, (body.right() - pad - lr.right()).max(40.0), row);
    if ui.text_field(&format!("insp/{e}/{comp}/{path}"), fr, &mut buf) {
        touched = true;
    }
    if touched && buf.trim().parse::<f64>().is_ok() {
        let _ = ctx.call(
            "ecs.entity.set",
            json!({ "entity": e, "component": comp, "wert": patched_value(wert, path, &buf, is_int) }),
        );
    }
    st.edits.insert(key, buf);
    y + row + 2.0
}

/// Scrub-Tempo je Eigenschaftsfamilie (Welt-Einheiten pro Pixel).
fn scrub_step(comp: &str, path: &str) -> f32 {
    if comp == "Transform" {
        if path.starts_with("rotation") {
            0.25
        } else {
            0.01
        }
    } else {
        0.01
    }
}

/// Anzeigetext eines Zahlenblatts („9" statt „9.0").
fn leaf_text(v: &Value) -> String {
    if v.is_i64() || v.is_u64() {
        v.as_i64().map(|i| i.to_string()).unwrap_or_default()
    } else {
        fmt_num(v.as_f64().unwrap_or(0.0))
    }
}

/// Kürzeste lesbare Darstellung: ganze Zahlen ohne Nachkomma-Null,
/// auf f32 gerundet (die Welt speichert float32 — kein „1.2000000476").
fn fmt_num(v: f64) -> String {
    let f = v as f32;
    if f.abs() < 1e15 && (f as f64 - f.round() as f64).abs() < 1e-9 {
        (f.round() as i64).to_string()
    } else {
        f.to_string()
    }
}

fn fmt_leaf(is_int: bool, v: f64) -> String {
    if is_int {
        (v.round() as i64).to_string()
    } else {
        fmt_num(v)
    }
}

/// Zerlegt „translation[1]" in („translation", Some(1)) bzw. „k" in („k", None).
fn split_leaf(path: &str) -> (&str, Option<usize>) {
    if let Some(rest) = path.strip_suffix(']') {
        if let Some(b) = rest.rfind('[') {
            if let Ok(i) = rest[b + 1..].parse() {
                return (&rest[..b], Some(i));
            }
        }
    }
    (path, None)
}

/// Setzt ein gescrubbeltes oder getipptes Zahlenblatt in eine Kopie des
/// Component-JSON — so bleibt `ecs.entity.set` der einzige Schreibweg.
fn patched_value(wert: &Value, path: &str, text: &str, is_int: bool) -> Value {
    let n: f64 = text.trim().parse().unwrap_or(0.0);
    let leaf = if is_int { json!(n.round() as i64) } else { json!(n) };
    let (k, idx) = split_leaf(path);
    let mut out = wert.clone();
    match (&mut out, idx) {
        (Value::Object(o), None) => {
            o.insert(k.to_string(), leaf);
        }
        (Value::Object(o), Some(i)) => {
            if let Some(a) = o.get_mut(k).and_then(Value::as_array_mut) {
                if i < a.len() {
                    a[i] = leaf;
                }
            }
        }
        (Value::Number(_), _) => out = leaf,
        _ => {}
    }
    out
}

/// Konsole: Log-Puffer + Kommandozeile. Enter (über `input.enter`)
/// führt die Zeile als `capability {json}` aus.
pub fn draw_console(
    ui: &mut Ui,
    pl: &PanelPlacement,
    theme: &Theme,
    atlas: &FontAtlas,
    input: &UiInput,
    ctx: &EditorContext,
    st: &mut PanelState,
) {
    let body = panel_frame(ui, pl, theme, atlas, st);
    let pad = theme.metrics.pad;
    let row = theme.metrics.row_h;
    let line_h = atlas.measure(0, "Ag", 0.0)[1];
    // Kommandozeile unten, Log darüber.
    let cmd_r = Rect::new(body.x + pad, body.bottom() - row - pad, (body.w - pad * 2.0).max(10.0), row);
    let log_r = Rect::new(body.x, body.y, body.w, (cmd_r.y - body.y - pad * 0.5).max(0.0));

    let content_h = st.log.len() as f32 * (line_h + 2.0);
    let off = ui.scroll("console/scroll", log_r, content_h);
    ui.draw.push_clip(log_r);
    for (i, line) in st.log.iter().rev().enumerate() {
        // Neueste oben — Rollende-Ansicht ohne Auto-Scroll-Zustand.
        let y = log_r.y + i as f32 * (line_h + 2.0) - off;
        if y > log_r.bottom() {
            continue;
        }
        ui.draw.text(atlas, 0, line, log_r.x + pad, y, line_h, theme.colors.text_dim);
    }
    ui.draw.pop_clip();

    let mut buf = st.cmdline.clone();
    ui.text_field("console/cmd", cmd_r, &mut buf);
    if input.enter && !buf.is_empty() {
        exec_line(ctx, st, &buf);
        buf.clear();
    }
    st.cmdline = buf;
}

/// Eine Kommandozeile ausführen: `capability [json]`.
fn exec_line(ctx: &EditorContext, st: &mut PanelState, line: &str) {
    let (name, rest) = match line.find(char::is_whitespace) {
        Some(i) => (&line[..i], line[i..].trim()),
        None => (line, ""),
    };
    let args: Value = if rest.is_empty() {
        json!({})
    } else {
        match serde_json::from_str(rest) {
            Ok(v) => v,
            Err(e) => {
                st.log(format!("argumente: {e}"));
                return;
            }
        }
    };
    run_command(
        ctx,
        st,
        &Command { id: name.to_string(), capability: name.to_string(), args, separator: false },
    );
}

/// Assistant-Dock: Einstieg für den KI-Assistenten (M4 P2). In P1 ein
/// ehrlicher Platzhalter mit Eingabefeld — die Anfrage landet in der
/// Konsole, ein Provider ist noch nicht verbunden.
pub fn draw_assistant(
    ui: &mut Ui,
    pl: &PanelPlacement,
    theme: &Theme,
    atlas: &FontAtlas,
    input: &UiInput,
    st: &mut PanelState,
) {
    let body = panel_frame(ui, pl, theme, atlas, st);
    let pad = theme.metrics.pad;
    let row = theme.metrics.row_h;
    let line_h = atlas.measure(0, "Ag", 0.0)[1];
    ui.draw.text(
        atlas,
        0,
        "KI-Assistent: noch kein Provider verbunden (M4 P2).",
        body.x + pad,
        body.y + pad,
        line_h,
        theme.colors.text_dim,
    );
    let mut buf = st.assistant_line.clone();
    let fr = Rect::new(body.x + pad, body.bottom() - row - pad, (body.w - pad * 2.0).max(10.0), row);
    ui.text_field("assistant/input", fr, &mut buf);
    if input.enter && !buf.is_empty() {
        st.log(format!("assistant → {buf}"));
        buf.clear();
    }
    st.assistant_line = buf;
}

/// Viewport-Rahmen: die Szene malt durch dieses Panel durch — hier gibt
/// es nur eine feine Kontur und ein kleines Label.
pub fn draw_viewport_frame(ui: &mut Ui, pl: &PanelPlacement, theme: &Theme, atlas: &FontAtlas) {
    let r = pl.rect;
    ui.draw.stroke(r, 1.0, theme.colors.border);
    let label = theme.label("panel.viewport").to_string();
    let line_h = atlas.measure(0, "Ag", 0.0)[1];
    let pad = theme.metrics.pad;
    let w = atlas.measure(0, &label, 0.0)[0] + pad;
    ui.draw.rect(Rect::new(r.x + pad, r.y + pad, w, line_h + pad * 0.5), [0.0, 0.0, 0.0, 0.45]);
    ui.draw.text(atlas, 0, &label, r.x + pad * 1.5, r.y + pad * 0.75, line_h, theme.colors.text_dim);
}

/// Unbekanntes Panel: sichtbarer Hinweis statt Stille (Manifest-Fehler
/// soll im Fenster auffallen, nicht crashen).
fn draw_unknown(ui: &mut Ui, pl: &PanelPlacement, theme: &Theme, atlas: &FontAtlas, st: &mut PanelState) {
    let body = panel_frame(ui, pl, theme, atlas, st);
    let line_h = atlas.measure(0, "Ag", 0.0)[1];
    let msg = format!("unbekanntes Panel: {}", pl.panel);
    ui.draw.text(atlas, 0, &msg, body.x + theme.metrics.pad, body.y + theme.metrics.pad, line_h, theme.colors.accent);
}

/// Dispatch: Platzierung → Panel-Funktion.
pub fn draw_panel(
    ui: &mut Ui,
    pl: &PanelPlacement,
    theme: &Theme,
    atlas: &FontAtlas,
    input: &UiInput,
    ctx: &EditorContext,
    st: &mut PanelState,
) {
    match pl.panel.as_str() {
        "outliner" => draw_outliner(ui, pl, theme, atlas, ctx, st),
        "inspector" => draw_inspector(ui, pl, theme, atlas, ctx, st),
        "console" => draw_console(ui, pl, theme, atlas, input, ctx, st),
        "assistant" => draw_assistant(ui, pl, theme, atlas, input, st),
        "viewport" => draw_viewport_frame(ui, pl, theme, atlas),
        _ => draw_unknown(ui, pl, theme, atlas, st),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Preset;
    use glam::Vec3;
    use re_render::ui::widgets::UiState;
    use re_render::ui::dock_layout;

    const FONT: &[u8] = include_bytes!("../../../assets/fonts/inter/Inter-Regular.ttf");
    const FONT_BOLD: &[u8] = include_bytes!("../../../assets/fonts/inter/Inter-Bold.ttf");

    fn atlas() -> FontAtlas {
        FontAtlas::build(&[FONT, FONT_BOLD], 15.0, "äöüÄÖÜß—→…", 1024, 512).expect("Atlas")
    }

    fn input() -> UiInput {
        UiInput {
            pointer: None,
            pressed: false,
            released: false,
            scroll: 0.0,
            chars: String::new(),
            backspace: false,
            enter: false,
            shift: false,
        }
    }

    /// Vollständiger Preset-Aufbau: Manifest → Platzierungen → alle
    /// Panels gemalt (CPU-Referenz). Beweist, dass das default-Preset
    /// rendert, ohne ein Fenster zu öffnen.
    #[test]
    fn default_preset_malt_alle_panels() {
        let preset = Preset::embedded_default();
        let ctx = EditorContext::new(Vec3::ZERO);
        preset.manifest.validate_against(|c| ctx.has(c)).expect("Manifest-Bindings");
        let atlas = atlas();
        let theme = &preset.theme;
        let area = Rect::new(0.0, 0.0, 1280.0, 720.0);
        let mut state = UiState::default();
        let mut pst = PanelState::default();
        let inp = input();

        let chrome = theme.metrics.row_h * 2.0 + 4.0;
        let dock = Rect::new(0.0, chrome, 1280.0, 720.0 - chrome - theme.metrics.row_h);
        let placements = dock_layout(&preset.manifest.dock, dock, theme.metrics.tab_h, theme.metrics.splitter);
        assert_eq!(placements.len(), 4, "vier Dock-Zellen (Tab-Gruppe zählt eine aktive)");

        let mut ui = Ui::new(area, theme, &atlas, None, &mut state, &inp);
        for pl in &placements {
            draw_panel(&mut ui, pl, theme, &atlas, &inp, &ctx, &mut pst);
        }
        ui.end();
        assert!(!ui.draw.cmds().is_empty(), "Draw-List ist leer");
        // Nichts darf außerhalb des Fensters malen: alle Pfade durch
        // push_clip abgesichert — hier genügt der Befund nicht-leer.
    }

    #[test]
    fn konsole_fuehrt_kommandozeile_als_capability_call() {
        let ctx = EditorContext::new(Vec3::ZERO);
        let mut st = PanelState::default();
        exec_line(&ctx, &mut st, "project.new {\"name\":\"Konsole\"}");
        assert!(st.log.iter().any(|l| l.contains("project.new")), "{:?}", st.log);
        let info = ctx.call("project.info", json!({})).unwrap();
        assert_eq!(info["name"], json!("Konsole"));
        // Müll wird abgewiesen, ohne zu paniken.
        exec_line(&ctx, &mut st, "project.new {kein json");
        assert!(st.log.iter().any(|l| l.contains("argumente")));
    }

    #[test]
    fn run_command_loggt_fehler_stattzu_crashen() {
        let ctx = EditorContext::new(Vec3::ZERO);
        let mut st = PanelState::default();
        run_command(
            &ctx,
            &mut st,
            &Command { id: "x".into(), capability: "gibt.es.nicht".into(), args: json!({}), separator: false },
        );
        assert!(st.log.iter().any(|l| l.contains("FEHLER")));
    }

    #[test]
    fn dropzonen_kanten_schlagen_mitte() {
        let ps = vec![PanelPlacement {
            panel: "viewport".into(),
            rect: Rect::new(100.0, 0.0, 400.0, 300.0),
            tabs: vec![],
        }];
        let zones = drop_zones(&ps, "console");
        assert!(!zones.iter().any(|(_, t, _)| t == "console"), "eigenes Panel hat keine Zonen");
        let hit = drop_target_at(&zones, [110.0, 150.0]).expect("linker Randstreifen");
        assert_eq!(hit.2, DockEdge::Left);
        let hit = drop_target_at(&zones, [300.0, 150.0]).expect("Flächenmitte");
        assert_eq!(hit.2, DockEdge::Center);
        assert!(drop_target_at(&zones, [10.0, 10.0]).is_none(), "außerhalb ist kein Ziel");
    }

    #[test]
    fn patched_value_setzt_blatt_und_laesst_rest() {
        let wert = json!({ "translation": [1.0, 2.0, 3.0], "rotation": [0.0, 0.0, 0.0, 1.0] });
        let p = patched_value(&wert, "translation[1]", "2.5", false);
        assert_eq!(p["translation"][1], json!(2.5));
        assert_eq!(p["translation"][2], json!(3.0), "Nachbarn bleiben");
        assert_eq!(p["rotation"][3], json!(1.0), "andere Schlüssel bleiben");
        let i = patched_value(&json!({ "energie": 40 }), "energie", "40.7", true);
        assert_eq!(i["energie"], json!(41), "integer-Blatt rundet");
    }

    /// Transform-Header aufklappen, dann das erste Zahlenblatt am Namen
    /// scrubben — die Welt muss sich über die Capability geändert haben.
    #[test]
    fn inspektor_scrubbt_zahlenblatt_ueber_capability() {
        use re_core::transform::Transform;
        let ctx = EditorContext::new(Vec3::ZERO);
        let id = {
            let mut w = ctx.world.lock().unwrap();
            let ent = w.spawn();
            w.insert(ent, Transform { translation: [1.0, 0.0, 0.0], ..Default::default() });
            ent.index()
        };
        let vorher = ctx.call("ecs.entity.get", json!({ "entity": id, "component": "Transform" })).unwrap()["wert"].clone();

        let a = atlas();
        let t = Theme::default();
        let row = t.metrics.row_h;
        let pad = t.metrics.pad;
        let pl = PanelPlacement { panel: "inspector".into(), rect: Rect::new(0.0, 0.0, 300.0, 400.0), tabs: vec![] };
        let mut st = UiState::default();
        let mut pst = PanelState { selected: Some(id), ..Default::default() };
        // body = unten am Titelstreifen; erste Wertzeile sitzt eine
        // Kopfzeile unter body-Anfang.
        let first_leaf_y = row + pad * 0.5 + row + row * 0.5; // Mitte der ersten Blattzeile
        let mut frame = |pointer: Option<[f32; 2]>, pressed: bool, released: bool, st: &mut UiState, pst: &mut PanelState| {
            let i = UiInput { pointer, pressed, released, ..Default::default() };
            let mut ui = Ui::new(Rect::new(0.0, 0.0, 300.0, 400.0), &t, &a, None, st, &i);
            draw_inspector(&mut ui, &pl, &t, &a, &ctx, pst);
            ui.end();
        };
        // Komponenten-Header sind offen per Vorgabe — direkt ans Blatt.
        // Ziehen am Namen des ersten Zahlenblatts: 30 px nach rechts.
        frame(Some([40.0, first_leaf_y]), true, false, &mut st, &mut pst);
        frame(Some([60.0, first_leaf_y]), false, false, &mut st, &mut pst);
        frame(Some([70.0, first_leaf_y]), false, false, &mut st, &mut pst);
        assert!(pst.scrub.is_some(), "Scrub-Chip hängt am Zeiger");
        frame(Some([70.0, first_leaf_y]), false, true, &mut st, &mut pst);

        let nachher = ctx.call("ecs.entity.get", json!({ "entity": id, "component": "Transform" })).unwrap()["wert"].clone();
        assert_ne!(vorher, nachher, "Scrubben muss die Welt über die Capability ändern");
        assert!(pst.scrub.is_none(), "nach dem Loslass kein Chip mehr");
    }
}
