//! Undo/Redo als reversible Systeme (M4c).
//!
//! Jede Editor-Aktion ist ein [`Command`]: `apply` führt sie aus, `undo`
//! dreht sie zurück. Der [`CommandStack`] hält die Historie mit einem
//! Cursor — Undo bewegt den Cursor zurück und ruft `undo`, Redo bewegt
//! ihn vor und ruft `apply`. Eine neue Aktion nach einem Undo verwirft
//! den Redo-Zweig (Standard-Semantik).
//!
//! Die Aktionen sind generisch über das Ziel (`T`): derselbe Stack
//! dient dem Editor-Zustand, der Szene oder einem World-Partition-Handle.
//! Die Built-in-Commands unten decken den `EditorState` ab; Szenen-
//! Befehle (Spawn/Move/Remove) kommen mit dem Outliner.

use std::sync::{Arc, Mutex};

use crate::EditorState;

/// Eine umkehrbare Aktion.
pub trait Command<T>: Send {
    fn apply(&mut self, target: &mut T) -> Result<(), String>;
    fn undo(&mut self, target: &mut T) -> Result<(), String>;
    fn description(&self) -> &str;
}

/// Historie mit Cursor. `limit` begrenzt die Zahl gehaltener Befehle.
pub struct CommandStack<T> {
    history: Vec<Box<dyn Command<T>>>,
    cursor: usize,
    limit: usize,
}

impl<T> CommandStack<T> {
    pub fn new(limit: usize) -> Self {
        Self { history: Vec::new(), cursor: 0, limit: limit.max(1) }
    }

    pub fn limit(&self) -> usize { self.limit }
    pub fn len(&self) -> usize { self.history.len() }
    pub fn cursor(&self) -> usize { self.cursor }
    pub fn is_empty(&self) -> bool { self.history.is_empty() }
    pub fn can_undo(&self) -> bool { self.cursor > 0 }
    pub fn can_redo(&self) -> bool { self.cursor < self.history.len() }

    pub fn description_at(&self, idx: usize) -> Option<&str> {
        self.history.get(idx).map(|c| c.description())
    }

    /// Letzte angewandte Aktion (für UI-Hinweise).
    pub fn last_applied(&self) -> Option<&str> {
        if self.cursor == 0 { None } else { self.history.get(self.cursor - 1).map(|c| c.description()) }
    }

    pub fn next_redo(&self) -> Option<&str> {
        self.history.get(self.cursor).map(|c| c.description())
    }

    /// Aktion ausführen und in die Historie eintragen.
    pub fn execute(&mut self, mut cmd: Box<dyn Command<T>>, target: &mut T) -> Result<(), String> {
        cmd.apply(target)?;
        // Redo-Zweig verwerfen.
        self.history.truncate(self.cursor);
        self.history.push(cmd);
        self.cursor += 1;
        // Limit: älteste Aktion herausfallen lassen.
        if self.history.len() > self.limit {
            self.history.remove(0);
            self.cursor = self.cursor.saturating_sub(1);
        }
        Ok(())
    }

    /// Letzte Aktion rückgängig machen.
    pub fn undo(&mut self, target: &mut T) -> Result<(), String> {
        if !self.can_undo() {
            return Err("nichts zum Rückgängigmachen".into());
        }
        self.cursor -= 1;
        self.history[self.cursor].undo(target)
    }

    /// Zuletzt rückgängig gemachte Aktion wiederholen.
    pub fn redo(&mut self, target: &mut T) -> Result<(), String> {
        if !self.can_redo() {
            return Err("nichts zum Wiederholen".into());
        }
        self.history[self.cursor].apply(target)?;
        self.cursor += 1;
        Ok(())
    }

    /// Historie leeren (z. B. beim Laden eines neuen Dokuments).
    pub fn clear(&mut self) {
        self.history.clear();
        self.cursor = 0;
    }
}

pub type SharedCommandStack<T> = Arc<Mutex<CommandStack<T>>>;

// ── Built-in Commands für EditorState ─────────────────────────────────

/// Kamera ersetzen.
pub struct SetCameraCommand {
    pub old: crate::EditorCamera,
    pub new: crate::EditorCamera,
}

impl Command<EditorState> for SetCameraCommand {
    fn apply(&mut self, target: &mut EditorState) -> Result<(), String> {
        target.camera = self.new.clone();
        Ok(())
    }
    fn undo(&mut self, target: &mut EditorState) -> Result<(), String> {
        target.camera = self.old.clone();
        Ok(())
    }
    fn description(&self) -> &str { "Kamera setzen" }
}

/// Selektion ersetzen.
pub struct SetSelectionCommand {
    pub old: Vec<usize>,
    pub new: Vec<usize>,
}

impl Command<EditorState> for SetSelectionCommand {
    fn apply(&mut self, target: &mut EditorState) -> Result<(), String> {
        target.selection.clear();
        for id in &self.new { target.selection.add(*id); }
        Ok(())
    }
    fn undo(&mut self, target: &mut EditorState) -> Result<(), String> {
        target.selection.clear();
        for id in &self.old { target.selection.add(*id); }
        Ok(())
    }
    fn description(&self) -> &str { "Selektion setzen" }
}

/// View-Modus wechseln.
pub struct SetViewModeCommand {
    pub old: crate::ViewMode,
    pub new: crate::ViewMode,
}

impl Command<EditorState> for SetViewModeCommand {
    fn apply(&mut self, target: &mut EditorState) -> Result<(), String> {
        target.view_mode = self.new;
        Ok(())
    }
    fn undo(&mut self, target: &mut EditorState) -> Result<(), String> {
        target.view_mode = self.old;
        Ok(())
    }
    fn description(&self) -> &str { "Ansichtsmodus wechseln" }
}

// ── Capabilities ─────────────────────────────────────────────────────

use re_agent::capability::{CapabilityRegistry, ToolSpec};
use serde_json::json;

/// Hängt Undo/Redo-Capabilities an die Registry. Der Stack und der
/// Editor-Zustand werden geteilt (Arc<Mutex<…>>) — doppelte
/// Adressierbarkeit.
pub fn register_undo_tools(
    reg: &mut CapabilityRegistry,
    state: crate::SharedEditorState,
    stack: SharedCommandStack<EditorState>,
) {
    {
        let st = state.clone();
        let sk = stack.clone();
        reg.register(
            ToolSpec::simple("editor.undo", "Letzte Aktion rückgängig machen"),
            move |_| {
                let mut s = st.lock().map_err(|_| "Editor-Sperre vergiftet".to_string())?;
                let mut k = sk.lock().map_err(|_| "Undo-Sperre vergiftet".to_string())?;
                let desc = k.last_applied().map(|d| d.to_string());
                k.undo(&mut *s)?;
                Ok(json!({ "ok": true, "rueckgaengig": desc }))
            },
        );
    }
    {
        let st = state.clone();
        let sk = stack.clone();
        reg.register(
            ToolSpec::simple("editor.redo", "Zuletzt rückgängig gemachte Aktion wiederholen"),
            move |_| {
                let mut s = st.lock().map_err(|_| "Editor-Sperre vergiftet".to_string())?;
                let mut k = sk.lock().map_err(|_| "Undo-Sperre vergiftet".to_string())?;
                let desc = k.next_redo().map(|d| d.to_string());
                k.redo(&mut *s)?;
                Ok(json!({ "ok": true, "wiederholt": desc }))
            },
        );
    }
    {
        let sk = stack.clone();
        reg.register(
            ToolSpec::simple("editor.history", "Undo/Redo-Status und Historie"),
            move |_| {
                let k = sk.lock().map_err(|_| "Undo-Sperre vergiftet".to_string())?;
                let mut cmds = Vec::new();
                for i in 0..k.len() {
                    cmds.push(json!({ "index": i, "beschreibung": k.description_at(i), "angewandt": i < k.cursor() }));
                }
                Ok(json!({
                    "cursor": k.cursor(),
                    "anzahl": k.len(),
                    "kann_undo": k.can_undo(),
                    "kann_redo": k.can_redo(),
                    "befehle": cmds,
                }))
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use crate::{EditorCamera, EditorState};

    fn state() -> EditorState { EditorState::new(Vec3::ZERO) }

    #[test]
    fn execute_und_undo_roundtrip() {
        let mut st = state();
        let mut stack = CommandStack::new(10);
        stack.execute(Box::new(SetCameraCommand {
            old: st.camera.clone(),
            new: EditorCamera::new(Vec3::new(5.0, 0.0, 0.0)),
        }), &mut st).unwrap();
        assert_eq!(st.camera.position, Vec3::new(5.0, 0.0, 0.0));
        assert!(stack.can_undo());
        stack.undo(&mut st).unwrap();
        assert_eq!(st.camera.position, Vec3::ZERO);
        assert!(stack.can_redo());
        stack.redo(&mut st).unwrap();
        assert_eq!(st.camera.position, Vec3::new(5.0, 0.0, 0.0));
    }

    #[test]
    fn neuer_befehl_verwirft_redo_zweig() {
        let mut st = state();
        let mut stack = CommandStack::new(10);
        stack.execute(Box::new(SetCameraCommand { old: st.camera.clone(), new: EditorCamera::new(Vec3::new(1.0, 0.0, 0.0)) }), &mut st).unwrap();
        stack.undo(&mut st).unwrap();
        assert!(stack.can_redo());
        stack.execute(Box::new(SetCameraCommand { old: st.camera.clone(), new: EditorCamera::new(Vec3::new(2.0, 0.0, 0.0)) }), &mut st).unwrap();
        assert!(!stack.can_redo());
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn limit_verwirft_aelteste() {
        let mut st = state();
        let mut stack = CommandStack::new(2);
        for i in 1..=3 {
            let old = st.camera.clone();
            let new = EditorCamera::new(Vec3::new(i as f32, 0.0, 0.0));
            stack.execute(Box::new(SetCameraCommand { old, new }), &mut st).unwrap();
        }
        assert_eq!(stack.len(), 2);
        assert_eq!(stack.cursor(), 2);
        // Ältester (i=1) ist weg.
        assert!(stack.description_at(0).is_some());
    }

    #[test]
    fn selektion_command() {
        let mut st = state();
        st.selection.add(7);
        let mut stack = CommandStack::new(10);
        stack.execute(Box::new(SetSelectionCommand { old: vec![7], new: vec![1, 2] }), &mut st).unwrap();
        assert_eq!(st.selection.ids(), vec![1, 2]);
        stack.undo(&mut st).unwrap();
        assert_eq!(st.selection.ids(), vec![7]);
    }

    #[test]
    fn view_mode_command() {
        let mut st = state();
        assert_eq!(st.view_mode, crate::ViewMode::Fly);
        let mut stack = CommandStack::new(10);
        stack.execute(Box::new(SetViewModeCommand { old: crate::ViewMode::Fly, new: crate::ViewMode::Orbit }), &mut st).unwrap();
        assert_eq!(st.view_mode, crate::ViewMode::Orbit);
        stack.undo(&mut st).unwrap();
        assert_eq!(st.view_mode, crate::ViewMode::Fly);
    }

    #[test]
    fn undo_auf_leerem_stack_fehler() {
        let mut st = state();
        let mut stack = CommandStack::new(10);
        assert!(stack.undo(&mut st).is_err());
        assert!(stack.redo(&mut st).is_err());
    }

    #[test]
    fn clear_leert_historie() {
        let mut st = state();
        let mut stack = CommandStack::new(10);
        stack.execute(Box::new(SetCameraCommand { old: st.camera.clone(), new: EditorCamera::new(Vec3::X) }), &mut st).unwrap();
        stack.clear();
        assert!(stack.is_empty());
        assert!(!stack.can_undo());
    }
}
