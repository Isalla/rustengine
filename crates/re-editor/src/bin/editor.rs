//! re-editor — Szenen-Ansicht (M4.1).
//!
//! Fenster-Modus (Default, Feature `window`): echtes Fenster mit
//! frei fliegender Kamera in die gestreamte Welt — WASD/QE fliegen,
//! Maus ziehen = Blick, Rad = Geschwindigkeit, Esc = Ende. Die Welt
//! wird beim Start wie in `stream-demo` als prozedurales Gelände in
//! Blockdateien gebacken und dann gestreamt (Clipmap → Streamer →
//! Handoff → GPU).
//!
//! Headless-Modus (`--headless <frames> <out.png> [cells_dir]`): läuft
//! ohne Fenster, hält die Kamera am Ursprung und schreibt den letzten
//! Frame als PNG — für Pi/CI und schnelle Smoke-Tests.
//!
//! Preset-Beweis (`--ui-png <out.png> [w h]`): malt einen kompletten
//! Editor-Frame des `default`-Presets (Chrome + Panels) CPU-seitig als
//! PNG — ohne Fenster, ohne GPU, als CI-Gate für M4 P1.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use glam::{Mat4, Vec2, Vec3};
use image::RgbaImage;
use re_core::material::Material;
use re_core::mesh::Mesh;
use re_editor::{activate_tab, EditorCamera, EditorContext, LayoutManifest, PanelState, Preset};
use re_render::ui::widgets::{Ui, UiInput, UiState};
use re_render::ui::{
    dock_layout, dock_put, dock_set_ratio, dock_splitters, dock_take, DockEdge, DockNode, DrawList,
    FontAtlas, Rect, Theme,
};
use re_render::{DrawBatch, GfxBackend};
use re_world::{
    CellHandoff, CellKey, Clipmap, ClipmapConfig, GpuUpload, HandoffConfig, Priority, StreamConfig,
    Streamer,
};

// ── Prozedurales Gelände (wie in stream-demo) ────────────────────────
const BASE_CELL: f64 = 64.0;
const SIDE: usize = 17;
const BLOCK_BYTES: usize = SIDE * SIDE * 4;
const SEED_HALF_Z: f64 = 700.0;
const SEED_AHEAD: f64 = 3_600.0;

fn height_at(x: f64, z: f64) -> f64 {
    let h = (x * 0.007).sin() * 28.0
        + (z * 0.011).cos() * 22.0
        + (x * 0.003 + z * 0.005).sin() * 40.0;
    h + 60.0
}

#[derive(Clone)]
struct Chunk {
    mesh: Mesh,
    band: usize,
}

fn decode_chunk(key: CellKey, bytes: &[u8]) -> Result<Chunk, String> {
    if bytes.len() != BLOCK_BYTES {
        return Err(format!("erwarte {BLOCK_BYTES} Bytes, got {}", bytes.len()));
    }
    let mut heights = vec![0.0f32; SIDE * SIDE];
    for i in 0..SIDE * SIDE {
        let off = i * 4;
        heights[i] = f32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
    }
    let n = SIDE;
    let mut positions = Vec::with_capacity(n * n);
    let mut normals = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            positions.push([i as f32, heights[j * n + i], j as f32]);
            normals.push([0.0, 1.0, 0.0]);
        }
    }
    let mut indices = Vec::with_capacity((n - 1) * (n - 1) * 6);
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            let a = (j * n + i) as u32;
            let b = a + 1;
            let c = a + n as u32;
            let d = c + 1;
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let scale = BASE_CELL as f32 / (n - 1) as f32;
    let positions = positions.iter().map(|p| [p[0] * scale, p[1], p[2] * scale]).collect();
    let mesh = Mesh {
        name: format!("cell_{}_{}", key.x, key.y),
        positions,
        normals: Some(normals),
        indices: Some(indices),
        ..Default::default()
    };
    let avg: f32 = heights.iter().sum::<f32>() / heights.len() as f32;
    let band = (avg / 20.0).clamp(0.0, 4.0) as usize;
    Ok(Chunk { mesh, band })
}

fn seed_blocks(store: &re_world::FileStore, _clip: &re_world::Clipmap, x0: f64, x1: f64, z0: f64, z1: f64) -> usize {
    let mut blocks = 0usize;
    let step = BASE_CELL;
    // Align to the cell grid: the streamer asks for floor_div(world/cell),
    // so blocks must start on multiples of `step` and be keyed by floor,
    // not round — round keys shifted every block by up to half a cell.
    let mut x = (x0 / step).floor() * step;
    while x < x1 {
        let mut z = (z0 / step).floor() * step;
        while z < z1 {
            let mut bytes = vec![0u8; BLOCK_BYTES];
            for j in 0..SIDE {
                for i in 0..SIDE {
                    let wx = x + (i as f64 / (SIDE - 1) as f64) * step;
                    let wz = z + (j as f64 / (SIDE - 1) as f64) * step;
                    let h = height_at(wx, wz) as f32;
                    let off = (j * SIDE + i) * 4;
                    bytes[off..off + 4].copy_from_slice(&h.to_le_bytes());
                }
            }
            let key = re_world::CellKey { level: 0, x: (x / step).floor() as i64, y: (z / step).floor() as i64 };
            store.write_block(key, &bytes).ok();
            blocks += 1;
            z += step;
        }
        x += step;
    }
    blocks
}

// ── Editor-UI (M4 P1: Preset-getriebenes Frontend) ───────────────────
//
// Der Rahmen baut pro Frame eine Draw-Liste aus dem eingebetteten
// `default`-Preset (Manifest + Theme, ADR-0001). Fenster-Modus legt sie
// als Overlay über die Szene; `--ui-png` malt sie CPU-seitig als PNG —
// derselbe Rahmen, derselbe Beweis ohne Display.

const FONT_REGULAR: &[u8] = include_bytes!("../../../../assets/fonts/inter/Inter-Regular.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../../../../assets/fonts/inter/Inter-Bold.ttf");

/// Startbestand der Welt, damit Outliner und Inspector Inhalte zeigen.
/// Das ist App-Code (Welt bauen), kein Control — Controls rufen nur
/// Capabilities an.
fn seed_entities(ctx: &EditorContext) {
    let Ok(mut w) = ctx.world.lock() else { return };
    for (i, name) in ["Player", "Lager-Halle", "Sendeantenne"].iter().enumerate() {
        let e = w.spawn();
        w.insert(e, re_core::transform::Transform { translation: [i as f32 * 4.0, 0.0, 0.0], ..Default::default() });
        w.insert(e, re_editor::Name::new(*name));
        w.insert(e, re_editor::Visible::default());
    }
}

/// Der Preset-Rahmen: Manifest-Daten + Theme + Widget-Zustand.
struct EditorUi {
    ctx: EditorContext,
    manifest: LayoutManifest,
    /// Arbeitskopie des Dock-Baums (Tab-Aktivierung schreibt hierhin).
    dock: DockNode,
    theme: Theme,
    atlas: FontAtlas,
    state: UiState,
    panels: PanelState,
    /// Viewport-Rechteck des letzten Frames (für Kamera-Input-Gating).
    viewport: Rect,
}

impl EditorUi {
    fn new() -> Result<Self> {
        let preset = Preset::embedded_default();
        let ctx = EditorContext::new(Vec3::new(0.0, 80.0, 0.0));
        preset
            .manifest
            .validate_against(|c| ctx.has(c))
            .context("default-Preset gegen Registry")?;
        let atlas = FontAtlas::build(
            &[FONT_REGULAR, FONT_BOLD],
            preset.theme.metrics.font_px,
            "äöüÄÖÜß—→…",
            1024,
            512,
        )
        .map_err(|e| anyhow::anyhow!("font atlas: {e:?}"))?;
        let mut panels = PanelState::default();
        panels.log(format!("preset '{}' geladen", preset.manifest.name));
        panels.log("willkommen — try: project.info".to_string());
        seed_entities(&ctx);
        Ok(Self {
            ctx,
            dock: preset.manifest.dock.clone(),
            manifest: preset.manifest,
            theme: preset.theme,
            atlas,
            state: UiState::default(),
            panels,
            viewport: Rect::new(0.0, 0.0, 0.0, 0.0),
        })
    }

    /// Ein kompletter UI-Durchgang: Chrome + Panels + Popup. Tastatur-
    /// und Mauskanten kommen als `input`; der Rückgabewert ist die
    /// fertige Draw-Liste für Overlay oder PNG.
    fn frame(&mut self, size: (u32, u32), input: &UiInput) -> DrawList {
        let (w, h) = (size.0 as f32, size.1 as f32);
        let area = Rect::new(0.0, 0.0, w, h);
        // Klick außerhalb von Menüleiste + Popup schließt das Menü.
        if input.pressed && self.panels.open_menu.is_some() {
            let in_popup = self
                .panels
                .popup
                .is_some_and(|p| input.pointer.is_some_and(|pt| p.contains(pt[0], pt[1])));
            let in_bar = input.pointer.is_some_and(|pt| pt[1] < self.theme.metrics.row_h);
            if !in_popup && !in_bar {
                self.panels.open_menu = None;
            }
        }
        let row = self.theme.metrics.row_h;
        let menu_bar = Rect::new(0.0, 0.0, w, row);
        let toolbar = Rect::new(0.0, row, w, row);
        let status = Rect::new(0.0, (h - row).max(0.0), w, row);
        let dock = Rect::new(0.0, row * 2.0, w, (h - row * 3.0).max(0.0));
        let placements =
            dock_layout(&self.dock, dock, self.theme.metrics.tab_h, self.theme.metrics.splitter);

        let mut ui = Ui::new(area, &self.theme, &self.atlas, None, &mut self.state, input);
        // Splitter ZUERST: der Druck gehört dem Greifband, nicht den
        // Panels; das Verhältnis kommt direkt aus Zeiger→Track (absolut
        // — kein Drift über Frames).
        for hit in dock_splitters(&self.dock, dock, self.theme.metrics.tab_h, self.theme.metrics.splitter) {
            let mut id = String::from("split");
            for step in &hit.path {
                id.push('-');
                id.push_str(&step.to_string());
            }
            let d = ui.drag(&id, hit.grab, 0.0);
            if d.claimed {
                if let Some(pt) = ui.pointer() {
                    let ratio = if hit.vertical {
                        (pt[1] - hit.track.y) / hit.track.h.max(1.0)
                    } else {
                        (pt[0] - hit.track.x) / hit.track.w.max(1.0)
                    };
                    dock_set_ratio(&mut self.dock, &hit.path, ratio);
                }
            }
        }
        // Panels zuerst, Chrome darüber, Popup zuletzt (Painter-Order).
        self.panels.drag_ended = false;
        for pl in &placements {
            if pl.panel == "viewport" {
                self.viewport = pl.rect;
            }
            re_editor::panels::draw_panel(&mut ui, pl, &self.theme, &self.atlas, input, &self.ctx, &mut self.panels);
        }
        // Umlager-Drag: Zonen-Rückmeldung übers Dock, Apply im Loslass-Frame.
        if let Some(dragging) = self.panels.drag_panel.clone() {
            if self.panels.drag_ended {
                if let Some((target, edge)) = self.panels.drop.clone() {
                    let mut t = self.dock.clone();
                    if dock_take(&mut t, &dragging) && dock_put(&mut t, &dragging, &target, edge) {
                        self.dock = t;
                        let kante = match edge {
                            DockEdge::Center => "als tab",
                            DockEdge::Left => "links",
                            DockEdge::Right => "rechts",
                            DockEdge::Top => "oben",
                            DockEdge::Bottom => "unten",
                        };
                        self.panels.log(format!("umgelagert: {dragging} → {target} {kante}"));
                    }
                }
                self.panels.drag_panel = None;
                self.panels.drop = None;
            } else if let Some(pt) = input.pointer {
                let zones = re_editor::panels::drop_zones(&placements, &dragging);
                let hit = re_editor::panels::drop_target_at(&zones, pt);
                self.panels.drop = hit.clone().map(|(_, t, e)| (t, e));
                re_editor::panels::draw_drop_zones(&mut ui, &zones, hit.map(|(r, _, _)| r), &self.theme);
                let key = format!("panel.{dragging}");
                let label = self.theme.label(&key).to_string();
                re_editor::panels::draw_drag_ghost(&mut ui, &self.theme, &self.atlas, &label, pt);
            }
        }
        re_editor::panels::draw_menu_bar(&mut ui, menu_bar, &self.manifest.menu_bar, &self.theme, &self.atlas, &mut self.panels);
        re_editor::panels::draw_toolbar(&mut ui, toolbar, &self.manifest.toolbar, &self.theme, &self.atlas, &self.ctx, &mut self.panels);
        re_editor::panels::draw_status_bar(&mut ui, status, &self.theme, &self.atlas, &self.ctx, &self.panels);
        re_editor::panels::draw_menu_popup(&mut ui, &self.manifest.menu_bar, &self.theme, &self.atlas, menu_bar, &self.ctx, &mut self.panels);
        ui.end();
        if let Some(id) = self.panels.tab_pick.take() {
            activate_tab(&mut self.dock, &id);
        }
        ui.draw
    }
}

/// Preset-Beweis ohne Display: einen UI-Frame CPU-seitig rasterisieren
/// und als PNG speichern (CI/Pi — Gate für M4 P1).
fn run_ui_png(out: PathBuf, w: u32, h: u32) -> Result<()> {
    let mut ui = EditorUi::new()?;
    let input = UiInput::default();
    let dl = ui.frame((w, h), &input);
    let rgba = re_render::rasterize(&dl, Some(&ui.atlas), w, h, ui.theme.colors.background);
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).ok();
    }
    let img = RgbaImage::from_raw(w, h, rgba).context("puffer")?;
    img.save(&out).with_context(|| format!("PNG: {}", out.display()))?;
    eprintln!("preset-frame ({}x{}) -> {}", w, h, out.display());
    Ok(())
}

// ── Fenster-Modus ─────────────────────────────────────────────────────
#[cfg(feature = "window")]
mod live {
    use super::*;
    use re_platform::{run_app, App, AppResponse, InputEvent, Key, MouseButton, PlatformEvent, WindowHandleSource, WindowOptions};
    use re_wgpu::WgpuBackend;

    const MATERIALS: [Material; 5] = [
        Material { name: String::new(), base_color: [0.35, 0.55, 0.30, 1.0], metallic: 0.0, roughness: 0.9, emissive: [0.0; 3], double_sided: false, albedo_texture_id: None, normal_texture_id: None },
        Material { name: String::new(), base_color: [0.45, 0.60, 0.32, 1.0], metallic: 0.0, roughness: 0.9, emissive: [0.0; 3], double_sided: false, albedo_texture_id: None, normal_texture_id: None },
        Material { name: String::new(), base_color: [0.55, 0.62, 0.38, 1.0], metallic: 0.0, roughness: 0.9, emissive: [0.0; 3], double_sided: false, albedo_texture_id: None, normal_texture_id: None },
        Material { name: String::new(), base_color: [0.62, 0.66, 0.50, 1.0], metallic: 0.0, roughness: 0.9, emissive: [0.0; 3], double_sided: false, albedo_texture_id: None, normal_texture_id: None },
        Material { name: String::new(), base_color: [0.70, 0.72, 0.60, 1.0], metallic: 0.0, roughness: 0.9, emissive: [0.0; 3], double_sided: false, albedo_texture_id: None, normal_texture_id: None },
    ];
    const HLOD_MATERIAL: Material = Material { name: String::new(), base_color: [0.62, 0.66, 0.58, 1.0], metallic: 0.0, roughness: 0.9, emissive: [0.0; 3], double_sided: true, albedo_texture_id: None, normal_texture_id: None };

    fn hlod_quad() -> Mesh {
        let mut mesh = Mesh::default();
        mesh.name = "hlod_cluster".into();
        mesh.positions = vec![[-0.5, 0.0, -0.5], [0.5, 0.0, -0.5], [0.5, 0.0, 0.5], [-0.5, 0.0, 0.5]];
        mesh.normals = Some(vec![[0.0, 1.0, 0.0]; 4]);
        mesh.indices = Some(vec![0, 1, 2, 0, 2, 3]);
        mesh
    }

    struct ExecState {
        next_id: u64,
        ids: std::collections::BTreeMap<CellKey, Vec<u64>>,
        chunks: std::collections::BTreeMap<CellKey, Chunk>,
        backend: WgpuBackend,
        releases: usize,
    }
    impl ExecState {
        fn new(backend: WgpuBackend) -> Self {
            Self { next_id: 1, ids: Default::default(), chunks: Default::default(), backend, releases: 0 }
        }
    }
    impl GpuUpload<Chunk> for ExecState {
        fn submit(&mut self, _frame: u64, jobs: Vec<(CellKey, Chunk, u64)>) {
            for (cell, chunk, _bytes) in jobs {
                let id = self.next_id;
                self.next_id += 1;
                if self.backend.upload_cell(id, &chunk.mesh).is_err() { continue; }
                self.ids.entry(cell).or_default().push(id);
                self.chunks.insert(cell, chunk);
            }
        }
        fn release(&mut self, cell: CellKey) {
            if let Some(ids) = self.ids.remove(&cell) {
                for id in ids {
                    self.backend.release_cell(id);
                    self.releases += 1;
                }
            }
            self.chunks.remove(&cell);
        }
    }

    pub struct EditorApp {
        window_src: Option<WindowHandleSource>,
        size: (u32, u32),
        clip: Clipmap,
        streamer: Streamer<re_world::AsyncSource<Chunk>>,
        handoff: CellHandoff<Chunk>,
        exec: ExecState,
        seeded: Box<dyn Fn(CellKey) -> bool + Send>,
        camera: EditorCamera,
        input: re_platform::InputState,
        keys: Vec<Key>,
        dragging: bool,
        last_mouse: Option<(f64, f64)>,
        speed: f32,
        frames: u64,
        now: f64,
        hlod: re_world::HlodSet,
        outer: f64,
        /// Preset-Rahmen (Manifest + Theme + Widget-Zustand, M4 P1).
        ui: EditorUi,
        /// Eingabe-Kanten für den nächsten UI-Frame.
        ui_pointer: Option<[f32; 2]>,
        ui_pressed: bool,
        ui_released: bool,
        ui_wheel: f32,
        ui_chars: String,
        ui_backspace: bool,
        ui_enter: bool,
    }

    impl EditorApp {
        pub fn new(cells_dir: PathBuf, seed_x0: f64, seed_x1: f64) -> Result<Self> {
            let clip_cfg = ClipmapConfig { base_cell: BASE_CELL, radius: 2, levels: 3 };
            let clip = Clipmap::new(clip_cfg);
            let store = re_world::FileStore::new(&cells_dir);
            let codec: re_world::CellCodec<Chunk> = Arc::new(|key, bytes| decode_chunk(key, bytes));
            let source = re_world::AsyncSource::new(store, codec, 4);
            let streamer = Streamer::new(source, StreamConfig { budget_bytes: 100_000, max_inflight: 64 });
            let handoff = CellHandoff::<Chunk>::new(HandoffConfig { slots: 3, upload_limit_bytes: 4 * 1024 * 1024, retire_frames: 2 });
            let outer = clip_cfg.cell_size(clip_cfg.levels - 1);
            let hlod = re_world::HlodSet::new(re_world::HlodConfig { cell_size: outer, take_over: clip_cfg.outer_radius(), fade_out: clip_cfg.outer_radius() * 8.0, hysteresis: 0.05 });
            let seeded = move |key: CellKey| -> bool {
                let s = clip_cfg.cell_size(key.level);
                let gx0 = (seed_x0 / s).floor() as i64;
                let gx1 = (seed_x1 / s).floor() as i64;
                let gz0 = (-SEED_HALF_Z / s).floor() as i64;
                let gz1 = (SEED_HALF_Z / s).floor() as i64;
                key.x >= gx0 && key.x <= gx1 && key.y >= gz0 && key.y <= gz1
            };
            Ok(Self {
                window_src: None,
                size: (1280, 720),
                clip,
                streamer,
                handoff,
                exec: ExecState::new(WgpuBackend::new()),
                seeded: Box::new(seeded),
                camera: EditorCamera::new(Vec3::new(0.0, 80.0, 0.0)),
                input: Default::default(),
                keys: Vec::new(),
                dragging: false,
                last_mouse: None,
                speed: 40.0,
                frames: 0,
                now: 0.0,
                hlod,
                outer,
                ui: EditorUi::new()?,
                ui_pointer: None,
                ui_pressed: false,
                ui_released: false,
                ui_wheel: 0.0,
                ui_chars: String::new(),
                ui_backspace: false,
                ui_enter: false,
            })
        }

        fn held(&self, k: Key) -> bool { self.keys.contains(&k) }

        /// Passt ein gerade gedrückter Key zu einem Manifest-Shortcut?
        /// Liefert das zugehörige Command (Modifikatoren aus dem
        /// gedrückten Zustand, Namen aus dem Manifest — ADR-0001).
        fn match_shortcut(&self, key: Key) -> Option<re_editor::LayoutCommand> {
            let name = format!("{key:?}");
            let ctrl = self.held(Key::ControlLeft) || self.held(Key::ControlRight);
            let shift = self.held(Key::ShiftLeft) || self.held(Key::ShiftRight);
            let alt = self.held(Key::AltLeft) || self.held(Key::AltRight);
            self.ui.manifest.shortcuts.iter().find(|s| {
                s.key == name && s.ctrl == ctrl && s.shift == shift && s.alt == alt
            }).map(|s| re_editor::LayoutCommand {
                id: s.capability.clone(),
                capability: s.capability.clone(),
                args: s.args.clone(),
                separator: false,
            })
        }

        /// Zeiger liegt über dem Viewport-Fenster (Szene, kein Panel)?
        fn over_viewport(&self) -> bool {
            self.ui_pointer.is_some_and(|p| self.ui.viewport.contains(p[0], p[1]))
        }
    }

    impl App for EditorApp {
        fn attach_window(&mut self, window: WindowHandleSource) { self.window_src = Some(window); }

        fn on_event(&mut self, event: PlatformEvent) -> AppResponse {
            match event {
                PlatformEvent::Created { width, height, .. } => {
                    self.size = (width.max(1), height.max(1));
                    let mut backend = WgpuBackend::new();
                    if let Some(src) = &self.window_src { backend.set_window_source(src.as_any()).ok(); }
                    if backend.init().is_err() { return AppResponse::Exit; }
                    backend.resize(self.size.0, self.size.1);
                    self.exec.backend = backend;
                    eprintln!("re-editor: {}x{} — WASD/QE fliegen, Maus ziehen = Blick, Rad = Tempo, Esc = Ende",
                        self.size.0, self.size.1);
                    AppResponse::Continue
                }
                PlatformEvent::Resized { width, height, .. } => {
                    self.size = (width.max(1), height.max(1));
                    self.exec.backend.resize(self.size.0, self.size.1);
                    AppResponse::Continue
                }
                PlatformEvent::Input(ev) => {
                    self.input.apply(&ev);
                    match ev {
                        InputEvent::Key { key, pressed, repeat } => {
                            if pressed && !repeat {
                                // Manifest-Shortcuts haben Vorrang (Ctrl+S/Z/Y).
                                if let Some(cmd) = self.match_shortcut(key) {
                                    re_editor::panels::run_command(&self.ui.ctx, &mut self.ui.panels, &cmd);
                                } else if key == Key::Escape {
                                    return AppResponse::Exit;
                                }
                                if key == Key::Enter { self.ui_enter = true; }
                                if key == Key::Backspace { self.ui_backspace = true; }
                            }
                            if pressed {
                                if !self.keys.contains(&key) { self.keys.push(key); }
                            } else { self.keys.retain(|k| *k != key); }
                        }
                        InputEvent::Char(c) => {
                            if !c.is_control() { self.ui_chars.push(c); }
                        }
                        InputEvent::MouseMove { x, y } => {
                            self.ui_pointer = Some([x as f32, y as f32]);
                            let delta = self.last_mouse.map(|(px, py)| (x - px, y - py));
                            self.last_mouse = Some((x, y));
                            if self.dragging {
                                if let Some((dx, dy)) = delta {
                                    self.camera.look(-(dx as f32) * 0.003, -(dy as f32) * 0.003);
                                }
                            }
                        }
                        InputEvent::MouseButton { button: MouseButton::Left, pressed } => {
                            if pressed { self.ui_pressed = true; } else { self.ui_released = true; }
                            // Ziehen startet nur über dem Viewport — Panels
                            // beanspruchen Klicks zuerst (Widget-Regel).
                            self.dragging = pressed && self.over_viewport();
                        }
                        InputEvent::Wheel { dy, .. } => {
                            self.ui_wheel += dy as f32 * 24.0;
                            if self.over_viewport() {
                                let f = (1.0 - dy as f32 * 0.1).clamp(0.5, 2.0);
                                self.speed = (self.speed * f).clamp(5.0, 400.0);
                            }
                        }
                        _ => {}
                    }
                    AppResponse::Continue
                }
                PlatformEvent::Tick { dt, .. } => {
                    self.now += dt as f64;
                    // Flugsteuerung nur ohne Textfeld-Fokus — getippter
                    // Text gehört dem Widget, nicht der Kamera.
                    if !self.ui.state.has_focus() {
                        let speed = self.speed * dt;
                        let boost = if self.held(Key::ShiftLeft) || self.held(Key::ShiftRight) { 4.0 } else { 1.0 };
                        let fwd = self.camera.forward();
                        let right = self.camera.right();
                        let mut move_by = Vec3::ZERO;
                        if self.held(Key::W) { move_by += fwd * speed * boost; }
                        if self.held(Key::S) { move_by -= fwd * speed * boost; }
                        if self.held(Key::D) { move_by += right * speed * boost; }
                        if self.held(Key::A) { move_by -= right * speed * boost; }
                        if self.held(Key::E) { move_by += Vec3::Y * speed * boost; }
                        if self.held(Key::Q) { move_by -= Vec3::Y * speed * boost; }
                        self.camera.translate(move_by);
                    }
                    self.input.frame_start();

                    let cam2 = Vec2::new(self.camera.position.x, self.camera.position.z);
                    self.clip.set_center(cam2);
                    if let Some(_) = self.hlod.update(cam2) {}
                    self.streamer.set_now(self.now);
                    self.streamer.sync_demand(
                        self.clip.cells().iter().filter(|a| (self.seeded)(a.key))
                            .map(|a| (a.key, Priority(a.key.level, a.dist_base))),
                    );
                    let delta = self.streamer.drive();
                    for (cell, chunk) in delta.loaded { self.handoff.enqueue(cell, chunk, BLOCK_BYTES as u64); }
                    for cell in delta.evicted { self.handoff.evict(cell); }
                    self.handoff.tick(&mut self.exec);

                    let materials = &MATERIALS;
                    let hlod_quads = hlod_quad();
                    let hlod_material = &HLOD_MATERIAL;
                    let camera = self.camera.camera(self.size.0, self.size.1);
                    let mut batches: Vec<DrawBatch> = Vec::new();
                    for cell in self.handoff.resident_cells() {
                        let (Some(ids), Some(chunk)) = (self.exec.ids.get(&cell), self.exec.chunks.get(&cell)) else { continue; };
                        let Some(&id) = ids.last() else { continue; };
                        let size = BASE_CELL as f32;
                        let world = glam::Mat4::from_translation(Vec3::new(cell.x as f32 * size, 0.0, cell.y as f32 * size));
                        batches.push(DrawBatch { mesh: &chunk.mesh, material: &materials[chunk.band], world, sprite: None, cell: Some(id) });
                    }
                    for c in self.hlod.shown() {
                        let ctr = self.hlod.config().cluster_center(c);
                        let h = height_at(ctr.x as f64, ctr.y as f64);
                        let world = Mat4::from_scale_rotation_translation(
                            Vec3::new(self.outer as f32, 1.0, self.outer as f32),
                            glam::Quat::IDENTITY,
                            Vec3::new(ctr.x, h as f32 + 0.6, ctr.y),
                        );
                        batches.push(DrawBatch { mesh: &hlod_quads, material: hlod_material, world, sprite: None, cell: None });
                    }
                    // UI-Frame: Eingabe-Kanten → Draw-Liste; Szene mit
                    // Overlay rendern, Kanten für nächsten Frame leeren.
                    let ui_input = UiInput {
                        pointer: self.ui_pointer,
                        pressed: std::mem::replace(&mut self.ui_pressed, false),
                        released: std::mem::replace(&mut self.ui_released, false),
                        scroll: std::mem::replace(&mut self.ui_wheel, 0.0),
                        chars: std::mem::take(&mut self.ui_chars),
                        backspace: std::mem::replace(&mut self.ui_backspace, false),
                        enter: std::mem::replace(&mut self.ui_enter, false),
                    };
                    let dl = self.ui.frame(self.size, &ui_input);
                    if let Err(e) = self.exec.backend.render_with_ui(&camera, &batches, Some((&dl, &self.ui.atlas))) {
                        eprintln!("render: {e}");
                        return AppResponse::Exit;
                    }
                    self.frames += 1;
                    if self.frames % 120 == 0 {
                        eprintln!("editor: {} frames, {} resident, speed {:.0}", self.frames, self.handoff.stats().resident, self.speed);
                    }
                    AppResponse::Continue
                }
                PlatformEvent::Closing => AppResponse::Exit,
            }
        }
    }

    pub fn run_windowed(cells_dir: PathBuf, seed_x0: f64, seed_x1: f64) -> Result<()> {
        // Welt backen.
        std::fs::remove_dir_all(&cells_dir).ok();
        std::fs::create_dir_all(&cells_dir).context("Zellen-Ordner")?;
        let clip = Clipmap::new(ClipmapConfig { base_cell: BASE_CELL, radius: 2, levels: 3 });
        let store = re_world::FileStore::new(&cells_dir);
        let blocks = seed_blocks(&store, &clip, seed_x0, seed_x1, -SEED_HALF_Z, SEED_HALF_Z);
        eprintln!("Welt erzeugt: {blocks} Blöcke in {} — Flugstrecke {:.0} m", cells_dir.display(), (seed_x1 - seed_x0));
        let mut app = EditorApp::new(cells_dir, seed_x0, seed_x1)?;
        let options = WindowOptions { title: "RE — Editor (scene view)".into(), width: 1280, height: 720 };
        run_app(&mut app, options).map_err(anyhow::Error::msg)
    }

    pub fn run_headless(cells_dir: PathBuf, frames: u32, out_path: PathBuf, seed_x0: f64, seed_x1: f64) -> Result<()> {
        std::fs::remove_dir_all(&cells_dir).ok();
        std::fs::create_dir_all(&cells_dir).context("Zellen-Ordner")?;
        let clip_cfg = ClipmapConfig { base_cell: BASE_CELL, radius: 2, levels: 3 };
        let mut clip = Clipmap::new(clip_cfg);
        let store = re_world::FileStore::new(&cells_dir);
        let blocks = seed_blocks(&store, &clip, seed_x0, seed_x1, -SEED_HALF_Z, SEED_HALF_Z);
        eprintln!("Welt erzeugt: {blocks} Blöcke in {} — Flugstrecke {:.0} m", cells_dir.display(), (seed_x1 - seed_x0));
        let codec: re_world::CellCodec<Chunk> = Arc::new(|key, bytes| decode_chunk(key, bytes));
        let source = re_world::AsyncSource::new(re_world::FileStore::new(&cells_dir), codec, 4);
        let mut streamer = Streamer::new(source, StreamConfig { budget_bytes: 100_000, max_inflight: 64 });
        let mut handoff = CellHandoff::<Chunk>::new(HandoffConfig { slots: 3, upload_limit_bytes: 4 * 1024 * 1024, retire_frames: 2 });
        let mut exec = ExecState::new(WgpuBackend::new());
        let seeded = move |key: CellKey| -> bool {
            let s = clip_cfg.cell_size(key.level);
            let gx0 = (seed_x0 / s).floor() as i64;
            let gx1 = (seed_x1 / s).floor() as i64;
            let gz0 = (-SEED_HALF_Z / s).floor() as i64;
            let gz1 = (SEED_HALF_Z / s).floor() as i64;
            key.x >= gx0 && key.x <= gx1 && key.y >= gz0 && key.y <= gz1
        };
        let outer = clip_cfg.cell_size(clip_cfg.levels - 1);
        let mut hlod = re_world::HlodSet::new(re_world::HlodConfig { cell_size: outer, take_over: clip_cfg.outer_radius(), fade_out: clip_cfg.outer_radius() * 8.0, hysteresis: 0.05 });
        let mut backend = WgpuBackend::new();
        backend.init().context("Backend-Init")?;
        let (w, h) = (960u32, 540u32);
        backend.resize(w, h);

        let mut now = 0.0f64;
        let camera = EditorCamera::new(Vec3::new(0.0, 80.0, 0.0));
        for _ in 0..frames {
            now += 1.0 / 60.0;
            let cam2 = Vec2::new(camera.position.x, camera.position.z);
            clip.set_center(cam2);
            if let Some(_) = hlod.update(cam2) {}
            streamer.set_now(now);
            streamer.sync_demand(clip.cells().iter().filter(|a| seeded(a.key)).map(|a| (a.key, Priority(a.key.level, a.dist_base))));
            let delta = streamer.drive();
            for (cell, chunk) in delta.loaded { handoff.enqueue(cell, chunk, BLOCK_BYTES as u64); }
            for cell in delta.evicted { handoff.evict(cell); }
            handoff.tick(&mut exec);
        }
        let materials = &MATERIALS;
        let hlod_quads = hlod_quad();
        let hlod_material = &HLOD_MATERIAL;
        let cam = camera.camera(w, h);
        let mut batches: Vec<DrawBatch> = Vec::new();
        for cell in handoff.resident_cells() {
            let (Some(ids), Some(chunk)) = (exec.ids.get(&cell), exec.chunks.get(&cell)) else { continue; };
            let Some(&id) = ids.last() else { continue; };
            let size = BASE_CELL as f32;
            let world = glam::Mat4::from_translation(Vec3::new(cell.x as f32 * size, 0.0, cell.y as f32 * size));
            batches.push(DrawBatch { mesh: &chunk.mesh, material: &materials[chunk.band], world, sprite: None, cell: Some(id) });
        }
        for c in hlod.shown() {
            let ctr = hlod.config().cluster_center(c);
            let hh = height_at(ctr.x as f64, ctr.y as f64);
            let world = Mat4::from_scale_rotation_translation(
                Vec3::new(outer as f32, 1.0, outer as f32),
                glam::Quat::IDENTITY,
                Vec3::new(ctr.x, hh as f32 + 0.6, ctr.y),
            );
            batches.push(DrawBatch { mesh: &hlod_quads, material: hlod_material, world, sprite: None, cell: None });
        }
        backend.render(&cam, &batches).context("render")?;
        let (rw, rh, rgba) = backend.readback_rgba().context("readback")?;
        if let Some(p) = out_path.parent() { std::fs::create_dir_all(p).ok(); }
        let img = RgbaImage::from_raw(rw, rh, rgba).context("puffer")?;
        img.save(&out_path).with_context(|| format!("PNG: {}", out_path.display()))?;
        eprintln!("letzter Frame -> {}", out_path.display());
        Ok(())
    }
}

// ── main ──────────────────────────────────────────────────────────────
fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(|s| s.as_str()) == Some("--ui-png") {
        let out = PathBuf::from(argv.get(1).cloned().unwrap_or_else(|| "editor_ui.png".into()));
        let w: u32 = argv.get(2).and_then(|s| s.parse().ok()).unwrap_or(1280);
        let h: u32 = argv.get(3).and_then(|s| s.parse().ok()).unwrap_or(720);
        return run_ui_png(out, w, h);
    }
    if argv.first().map(|s| s.as_str()) == Some("--headless") {
        let frames: u32 = argv.get(1).and_then(|s| s.parse().ok()).unwrap_or(60);
        let out = PathBuf::from(argv.get(2).cloned().unwrap_or_else(|| "editor.png".into()));
        let cells = PathBuf::from(argv.get(3).cloned().unwrap_or_else(|| "editor_cells".into()));
        #[cfg(feature = "window")]
        { return live::run_headless(cells, frames, out, -SEED_AHEAD, SEED_AHEAD); }
        #[cfg(not(feature = "window"))]
        { let _ = (cells, frames, out); anyhow::bail!("window feature deaktiviert"); }
    }
    let cells = PathBuf::from(argv.first().cloned().unwrap_or_else(|| "editor_cells".into()));
    #[cfg(feature = "window")]
    { return live::run_windowed(cells, -SEED_AHEAD, SEED_AHEAD); }
    #[cfg(not(feature = "window"))]
    { let _ = cells; anyhow::bail!("window feature deaktiviert"); }
}
