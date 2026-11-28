//! Kompositionswurzel des Editors (M4).
//!
//! [`EditorContext`] hält alle geteilten Handles — Editor-Zustand,
//! Undo-Stack, Projekt-Dokument, ECS-Welt und Component-Registry — und
//! vereint sie in *einer* [`CapabilityRegistry`]. GUI, Konsole und
//! Agentin nutzen exakt denselben Einstieg (doppelte Adressierbarkeit,
//! M11): jede Fähigkeit, die das Fenster später per Knopf auslöst, ist
//! bereits über die Registry adressierbar und umgekehrt.

use std::sync::{Arc, Mutex};

use glam::Vec3;
use re_agent::capability::CapabilityRegistry;
use re_core::transform::Transform;
use re_ecs::World;
use serde_json::Value;

use crate::outliner::SharedWorld;
use crate::project::ProjectDoc;
use crate::workspace::Workspace;
use crate::{
    register_editor_tools, register_import_tools, register_outliner_tools, register_project_tools,
    register_undo_tools, CommandStack, ComponentRegistry, EditorState, Name, SharedCommandStack,
    SharedEditorState, SharedProjectDoc, SharedRegistry, Visible,
};

/// Historien-Tiefe des Standard-Undo-Stacks.
const UNDO_LIMIT: usize = 128;

/// Alle Editor-Handles plus die eine Registry, die sie adressiert.
pub struct EditorContext {
    pub state: SharedEditorState,
    pub undo: SharedCommandStack<EditorState>,
    pub project: SharedProjectDoc,
    pub world: SharedWorld,
    pub registry: SharedRegistry,
    pub workspace: Arc<Workspace>,
    pub capabilities: CapabilityRegistry,
}

impl EditorContext {
    /// Kontext mit leerer Welt, Startkamera und den eingebauten
    /// inspizierbaren Components (`Transform`, `Name`, `Visible`).
    pub fn new(start: Vec3) -> Self {
        let state: SharedEditorState = Arc::new(Mutex::new(EditorState::new(start)));
        let undo: SharedCommandStack<EditorState> =
            Arc::new(Mutex::new(CommandStack::new(UNDO_LIMIT)));
        let project: SharedProjectDoc = Arc::new(Mutex::new(ProjectDoc::new_empty()));
        let world: SharedWorld = Arc::new(Mutex::new(World::new()));

        let mut reg = ComponentRegistry::new();
        reg.register::<Transform>("Transform");
        reg.register::<Name>("Name");
        reg.register::<Visible>("Visible");
        let registry: SharedRegistry = Arc::new(reg);

        let mut capabilities = CapabilityRegistry::new();
        let workspace = Arc::new(Workspace::from_env_or_cwd());
        register_editor_tools(&mut capabilities, state.clone());
        register_undo_tools(&mut capabilities, state.clone(), undo.clone());
        register_project_tools(&mut capabilities, project.clone(), workspace.clone());
        register_import_tools(&mut capabilities, project.clone(), workspace.clone());
        register_outliner_tools(&mut capabilities, world.clone(), registry.clone());

        Self { state, undo, project, world, registry, workspace, capabilities }
    }

    /// Ist eine Fähigkeit registriert?
    pub fn has(&self, name: &str) -> bool {
        self.capabilities.has(name)
    }

    /// Eine Fähigkeit ausführen — derselbe Weg wie im Fenster und für die
    /// Agentin.
    pub fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        self.capabilities.call(name, args).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kontext_adressiert_alle_werkzeugfamilien() {
        let ctx = EditorContext::new(Vec3::ZERO);
        for name in [
            "editor.camera.get",
            "editor.camera.set",
            "editor.selection.set",
            "editor.undo",
            "editor.redo",
            "editor.history",
            "project.new",
            "project.load",
            "project.save",
            "project.info",
            "import.gltf",
            "import.obj",
            "import.preview",
            "ecs.entities",
            "ecs.entity.components",
            "ecs.entity.get",
            "ecs.entity.set",
        ] {
            assert!(ctx.has(name), "fehlt: {name}");
        }
    }

    #[test]
    fn kamera_ueber_registry() {
        let ctx = EditorContext::new(Vec3::ZERO);
        ctx.call("editor.camera.set", json!({ "x": 3.0, "yaw": 0.4 }))
            .unwrap();
        let got = ctx.call("editor.camera.get", json!({})).unwrap();
        assert_eq!(got["position"]["x"], json!(3.0));
        // Die Fly-Kamera ist per Design nicht undo-bar — der Historien-
        // Stack bleibt leer, bis Szenen-Aktionen ihn füllen.
        let hist = ctx.call("editor.history", json!({})).unwrap();
        assert_eq!(hist["anzahl"], json!(0));
    }

    #[test]
    fn projekt_und_ecs_teilen_den_kontext() {
        let ctx = EditorContext::new(Vec3::ZERO);
        ctx.call("project.new", json!({ "name": "Kontext" })).unwrap();
        let info = ctx.call("project.info", json!({})).unwrap();
        assert_eq!(info["name"], json!("Kontext"));

        // Entity direkt in die geteilte Welt setzen und über die Registry
        // zurücklesen — dasselbe Objekt, das das Fenster später zeigt.
        {
            let mut w = ctx.world.lock().unwrap();
            let e = w.spawn();
            w.insert(e, Transform::default());
            w.insert(e, Name::new("Einheit"));
        }
        let ents = ctx.call("ecs.entities", json!({})).unwrap();
        assert_eq!(ents["anzahl"], json!(1));
        let comps = ctx
            .call("ecs.entity.components", json!({ "entity": 0 }))
            .unwrap();
        assert!(comps["components"]
            .as_array()
            .unwrap()
            .contains(&json!("Name")));
    }
}
