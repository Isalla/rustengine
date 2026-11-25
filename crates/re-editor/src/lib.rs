//! # re-editor — Editor-Kern (M4)
//!
//! Der Editor ist keine zweite Anwendung neben der Engine, sondern ein
//! Konsument derselben Capabilities wie der Agent: Szenen-Ansicht,
//! Outliner und Inspector lesen und schreiben über die
//! `CapabilityRegistry`. Diese Crate enthält den **kopflosen Kern** —
//! Kamera-Rig, Selektion, Zustand — und die Capabilities, die ihn
//! adressierbar machen. Das Fenster und der Render-Loop leben im
//! Binary `re-editor` (Feature `window`).
//!
//! Doppelte Adressierbarkeit (M11): alles, was hier als Capability
//! registriert ist, kann Editor, KI und (später) MCP gleichermaßen.

use std::sync::{Arc, Mutex, MutexGuard};

use glam::{Mat4, Vec3};
use re_agent::capability::{CapabilityRegistry, ToolSpec};
use re_render::Camera;
use serde_json::json;

/// Frei fliegende Editor-Kamera: Position + Gier/Nickwinkel.
/// Bewegung und Blick sind reine Vektorrechnung, kein Zustand außerhalb
/// — deterministisch und headless testbar.
#[derive(Clone, Debug)]
pub struct EditorCamera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

impl EditorCamera {
    pub fn new(position: Vec3) -> Self {
        Self { position, yaw: 0.0, pitch: 0.0 }
    }

    /// Blickrichtung (Einheitsvektor) aus Gier/Nick.
    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        )
    }

    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Y).normalize_or_zero()
    }

    pub fn up(&self) -> Vec3 {
        Vec3::Y
    }

    /// Kamera relativ bewegen (Weltkoordinaten).
    pub fn translate(&mut self, delta: Vec3) {
        self.position += delta;
    }

    /// Blick drehen (Radian). Nick wird auf ±89° begrenzt.
    pub fn look(&mut self, dyaw: f32, dpitch: f32) {
        self.yaw += dyaw;
        self.pitch = (self.pitch + dpitch).clamp(-1.55, 1.55);
    }

    /// Render-Kamera mit gegebenem Seitenverhältnis.
    pub fn camera(&self, width: u32, height: u32) -> Camera {
        let mut cam = Camera::perspective(
            self.position,
            self.position + self.forward(),
            Vec3::Y,
            60f32.to_radians(),
            width.max(1) as f32 / height.max(1) as f32,
        );
        cam.clear_color = [0.52, 0.68, 0.85, 1.0];
        cam
    }

    /// Sichtmatrix (für Ray-Casting / Gizmos später).
    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.position, self.position + self.forward(), Vec3::Y)
    }
}

/// Selektion: Menge von Instanz-IDs. Ordnung egal, daher ein Set.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    ids: std::collections::BTreeSet<usize>,
}

impl Selection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, id: usize) {
        self.ids.insert(id);
    }

    pub fn remove(&mut self, id: usize) {
        self.ids.remove(&id);
    }

    pub fn toggle(&mut self, id: usize) {
        if self.ids.contains(&id) {
            self.ids.remove(&id);
        } else {
            self.ids.insert(id);
        }
    }

    pub fn clear(&mut self) {
        self.ids.clear();
    }

    pub fn contains(&self, id: usize) -> bool {
        self.ids.contains(&id)
    }

    pub fn ids(&self) -> Vec<usize> {
        self.ids.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// Editor-Modus der Szenen-Ansicht.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    /// Freies Fliegen (WASD + Maus).
    Fly,
    /// Strategie-Übersicht (Orbit).
    Orbit,
}

impl Default for ViewMode {
    fn default() -> Self {
        Self::Fly
    }
}

/// Kopfloser Editor-Zustand: alles, was die Capabilities adressieren.
#[derive(Clone, Debug)]
pub struct EditorState {
    pub camera: EditorCamera,
    pub selection: Selection,
    pub view_mode: ViewMode,
    pub frame_count: u64,
}

impl EditorState {
    pub fn new(start: Vec3) -> Self {
        Self {
            camera: EditorCamera::new(start),
            selection: Selection::new(),
            view_mode: ViewMode::default(),
            frame_count: 0,
        }
    }

    pub fn tick(&mut self) {
        self.frame_count += 1;
    }
}

/// Shared-Handle für die Registry-Runner (Editor, Agent, MCP).
pub type SharedEditorState = Arc<Mutex<EditorState>>;

fn lock(state: &SharedEditorState) -> Result<MutexGuard<'_, EditorState>, String> {
    state.lock().map_err(|_| "Editor-Sperre vergiftet".to_string())
}

/// Hängt die Editor-Capabilities an die Registry. `call` ist `&self`, also
/// kapseln die Runner ihre Innere Mutabilität selbst (Mutex).
pub fn register_editor_tools(reg: &mut CapabilityRegistry, state: SharedEditorState) {
    {
        let s = Arc::clone(&state);
        reg.register(
            ToolSpec::new(
                "editor.camera.get",
                "Aktuelle Editor-Kameraposition und Blickrichtung",
                json!({ "typ": "object", "felder": {} }),
            ),
            move |_| {
                let st = lock(&s)?;
                Ok(json!({
                    "position": { "x": st.camera.position.x, "y": st.camera.position.y, "z": st.camera.position.z },
                    "yaw": st.camera.yaw,
                    "pitch": st.camera.pitch,
                    "forward": { "x": st.camera.forward().x, "y": st.camera.forward().y, "z": st.camera.forward().z },
                }))
            },
        );
    }
    {
        let s = Arc::clone(&state);
        reg.register(
            ToolSpec::new(
                "editor.camera.set",
                "Editor-Kamera setzen (Position + Blickwinkel)",
                json!({ "typ": "object", "felder": {
                    "x": { "typ": "number" }, "y": { "typ": "number" }, "z": { "typ": "number" },
                    "yaw": { "typ": "number" }, "pitch": { "typ": "number" }
                } }),
            ),
            move |args| {
                let mut st = lock(&s)?;
                if let Some(v) = args.get("x").and_then(|v| v.as_f64()) {
                    st.camera.position.x = v as f32;
                }
                if let Some(v) = args.get("y").and_then(|v| v.as_f64()) {
                    st.camera.position.y = v as f32;
                }
                if let Some(v) = args.get("z").and_then(|v| v.as_f64()) {
                    st.camera.position.z = v as f32;
                }
                if let Some(v) = args.get("yaw").and_then(|v| v.as_f64()) {
                    st.camera.yaw = v as f32;
                }
                if let Some(v) = args.get("pitch").and_then(|v| v.as_f64()) {
                    st.camera.pitch = v as f32;
                }
                Ok(json!({ "ok": true }))
            },
        );
    }
    {
        let s = Arc::clone(&state);
        reg.register(
            ToolSpec::new(
                "editor.selection.set",
                "Selektion ersetzen (Liste von Instanz-IDs)",
                json!({ "typ": "object", "felder": { "ids": { "typ": "array" } } }),
            ),
            move |args| {
                let mut st = lock(&s)?;
                st.selection.clear();
                if let Some(arr) = args.get("ids").and_then(|v| v.as_array()) {
                    for v in arr {
                        if let Some(id) = v.as_u64() {
                            st.selection.add(id as usize);
                        }
                    }
                }
                Ok(json!({ "anzahl": st.selection.len() }))
            },
        );
    }
    {
        let s = Arc::clone(&state);
        reg.register(
            ToolSpec::simple("editor.selection.clear", "Selektion leeren"),
            move |_| {
                let mut st = lock(&s)?;
                st.selection.clear();
                Ok(json!({ "ok": true }))
            },
        );
    }
    {
        let s = Arc::clone(&state);
        reg.register(
            ToolSpec::simple("editor.stats", "Editor-Zustand: Frames, Kamera, Selektion"),
            move |_| {
                let st = lock(&s)?;
                Ok(json!({
                    "frames": st.frame_count,
                    "kamera": { "x": st.camera.position.x, "y": st.camera.position.y, "z": st.camera.position.z },
                    "selektion": st.selection.ids(),
                    "modus": format!("{:?}", st.view_mode),
                }))
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use re_agent::CapabilityRegistry;
    use serde_json::json;

    #[test]
    fn kamera_vorwaerts_und_blick_sind_deterministisch() {
        let mut c = EditorCamera::new(Vec3::ZERO);
        c.look(0.5, 0.2);
        let f1 = c.forward();
        c.translate(c.forward() * 3.0);
        assert_eq!(c.position, f1 * 3.0);
        // Blickrichtung unverändert nach Translation.
        assert_eq!(c.forward(), f1);
    }

    #[test]
    fn pitch_wird_begrenzt() {
        let mut c = EditorCamera::new(Vec3::ZERO);
        c.look(0.0, 10.0);
        assert!(c.pitch <= 1.55);
        c.look(0.0, -10.0);
        assert!(c.pitch >= -1.55);
    }

    #[test]
    fn selektion_toggle_und_clear() {
        let mut s = Selection::new();
        s.toggle(3);
        assert!(s.contains(3));
        s.toggle(3);
        assert!(!s.contains(3));
        s.add(1);
        s.add(2);
        assert_eq!(s.ids(), vec![1, 2]);
        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    fn capabilities_runden_trip() {
        let state = Arc::new(Mutex::new(EditorState::new(Vec3::new(1.0, 2.0, 3.0))));
        let mut reg = CapabilityRegistry::new();
        register_editor_tools(&mut reg, Arc::clone(&state));
        for name in ["editor.camera.get", "editor.camera.set", "editor.selection.set",
                     "editor.selection.clear", "editor.stats"] {
            assert!(reg.has(name), "fehlt: {name}");
        }
        let got = reg.call("editor.camera.get", json!({})).unwrap();
        assert_eq!(got["position"]["x"], json!(1.0));
        reg.call("editor.camera.set", json!({ "x": 5.0, "yaw": 0.7 })).unwrap();
        let got = reg.call("editor.camera.get", json!({})).unwrap();
        assert_eq!(got["position"]["x"], json!(5.0));
        assert!((got["yaw"].as_f64().unwrap() - 0.7).abs() < 1e-5);
        reg.call("editor.selection.set", json!({ "ids": [1, 2, 2] })).unwrap();
        let st = reg.call("editor.stats", json!({})).unwrap();
        assert_eq!(st["selektion"], json!([1, 2]));
        reg.call("editor.selection.clear", json!({})).unwrap();
        assert_eq!(reg.call("editor.stats", json!({})).unwrap()["selektion"], json!([]));
    }

    #[test]
    fn view_matrix_ist_endlich() {
        let c = EditorCamera::new(Vec3::new(1.0, 2.0, 3.0));
        let m = c.view_matrix();
        for col in 0..4 {
            for row in 0..4 {
                assert!(m.col(col)[row].is_finite());
            }
        }
    }
}
