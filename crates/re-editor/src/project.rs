//! Worldbuilder-Projekt-Loader (M4e): `.wbproj`-Dateien laden, speichern
//! und als Dokument halten. Der Zustand (Pfad, Dirty-Flag) lebt im
//! [`ProjectDoc`] — der Inhalt ist ein `re_core::Project`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use re_core::project::Project;
use re_agent::capability::{CapabilityRegistry, ToolSpec};
use serde_json::json;

/// Dokument-Wrapper um ein `Project` mit Pfad und Dirty-Flag.
#[derive(Clone)]
pub struct ProjectDoc {
    pub project: Option<Project>,
    pub path: Option<PathBuf>,
    pub dirty: bool,
}

impl ProjectDoc {
    pub fn new_empty() -> Self {
        Self { project: None, path: None, dirty: false }
    }

    pub fn new_with(project: Project) -> Self {
        Self { project: Some(project), path: None, dirty: true }
    }

    pub fn new_named(name: impl Into<String>) -> Self {
        Self { project: Some(Project::new(name)), path: None, dirty: true }
    }

    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let p = Project::load(path.as_ref())?;
        Ok(Self { project: Some(p), path: Some(path.as_ref().to_path_buf()), dirty: false })
    }

    pub fn save(&mut self, path: Option<&Path>) -> anyhow::Result<PathBuf> {
        let target = path.map(PathBuf::from).or_else(|| self.path.clone())
            .ok_or_else(|| anyhow::anyhow!("kein Speicherpfad gesetzt"))?;
        let proj = self.project.as_ref().ok_or_else(|| anyhow::anyhow!("kein Projekt geladen"))?;
        proj.save(&target)?;
        self.path = Some(target.clone());
        self.dirty = false;
        Ok(target)
    }

    pub fn close(&mut self) {
        self.project = None;
        self.path = None;
        self.dirty = false;
    }

    pub fn name(&self) -> Option<String> {
        self.project.as_ref().map(|p| p.name.clone())
    }

    pub fn is_loaded(&self) -> bool { self.project.is_some() }
    pub fn is_dirty(&self) -> bool { self.dirty }
    pub fn mark_dirty(&mut self) { self.dirty = true; }

    pub fn mesh_count(&self) -> usize {
        self.project.as_ref().map(|p| p.library.meshes.len()).unwrap_or(0)
    }

    pub fn material_count(&self) -> usize {
        self.project.as_ref().map(|p| p.library.materials.len()).unwrap_or(0)
    }

    pub fn node_count(&self) -> usize {
        self.project.as_ref().map(|p| p.scene.nodes.len()).unwrap_or(0)
    }

    pub fn fingerprint(&self) -> Option<String> {
        self.project.as_ref().map(|p| p.fingerprint())
    }
}

pub type SharedProjectDoc = Arc<Mutex<ProjectDoc>>;

fn lock(doc: &SharedProjectDoc) -> Result<MutexGuard<'_, ProjectDoc>, String> {
    doc.lock().map_err(|_| "Projekt-Sperre vergiftet".to_string())
}

pub fn register_project_tools(reg: &mut CapabilityRegistry, doc: SharedProjectDoc) {
    {
        let d = doc.clone();
        reg.register(
            ToolSpec::new("project.new", "Neues leeres Projekt anlegen", json!({ "typ": "object", "felder": { "name": { "typ": "string" } } })),
            move |args| {
                let mut doc = lock(&d)?;
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("untitled");
                doc.project = Some(Project::new(name));
                doc.path = None;
                doc.dirty = true;
                Ok(json!({ "ok": true, "name": name }))
            },
        );
    }
    {
        let d = doc.clone();
        reg.register(
            ToolSpec::new("project.load", "Projekt aus Datei laden", json!({ "typ": "object", "felder": { "path": { "typ": "string" } } })),
            move |args| {
                let path = args.get("path").and_then(|v| v.as_str()).ok_or("Parameter 'path' fehlt")?;
                let loaded = ProjectDoc::load(path).map_err(|e| e.to_string())?;
                let mut doc = lock(&d)?;
                *doc = loaded;
                Ok(json!({ "ok": true, "name": doc.name(), "pfad": doc.path }))
            },
        );
    }
    {
        let d = doc.clone();
        reg.register(
            ToolSpec::new("project.save", "Projekt speichern (Pfad optional, sonst zuletzt geladener)", json!({ "typ": "object", "felder": { "path": { "typ": "string" } } })),
            move |args| {
                let p = args.get("path").and_then(|v| v.as_str()).map(PathBuf::from);
                let mut doc = lock(&d)?;
                let target = doc.save(p.as_deref()).map_err(|e| e.to_string())?;
                Ok(json!({ "ok": true, "pfad": target }))
            },
        );
    }
    {
        let d = doc.clone();
        reg.register(ToolSpec::simple("project.close", "Aktuelles Projekt schließen"), move |_| {
            let mut doc = lock(&d)?;
            doc.close();
            Ok(json!({ "ok": true }))
        });
    }
    {
        let d = doc.clone();
        reg.register(ToolSpec::simple("project.info", "Projekt-Überblick: Name, Pfad, Dirty, Zähler"), move |_| {
            let doc = lock(&d)?;
            Ok(json!({
                "geladen": doc.is_loaded(),
                "name": doc.name(),
                "pfad": doc.path,
                "dirty": doc.is_dirty(),
                "meshes": doc.mesh_count(),
                "materialien": doc.material_count(),
                "knoten": doc.node_count(),
            }))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use re_agent::CapabilityRegistry;
    use serde_json::json;
    use std::fs;

    #[test]
    fn neu_und_info() {
        let doc = SharedProjectDoc::new(Mutex::new(ProjectDoc::new_empty()));
        let mut reg = CapabilityRegistry::new();
        register_project_tools(&mut reg, doc.clone());
        assert!(!reg.call("project.info", json!({})).unwrap()["geladen"].as_bool().unwrap());
        reg.call("project.new", json!({ "name": "Demo" })).unwrap();
        let info = reg.call("project.info", json!({})).unwrap();
        assert!(info["geladen"].as_bool().unwrap());
        assert_eq!(info["name"], json!("Demo"));
        assert!(info["dirty"].as_bool().unwrap());
    }

    #[test]
    fn save_und_load_roundtrip() {
        let dir = std::env::temp_dir().join("re_editor_proj_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("demo.wbproj");

        let doc = SharedProjectDoc::new(Mutex::new(ProjectDoc::new_empty()));
        let mut reg = CapabilityRegistry::new();
        register_project_tools(&mut reg, doc.clone());
        reg.call("project.new", json!({ "name": "Roundtrip" })).unwrap();
        reg.call("project.save", json!({ "path": path })).unwrap();

        // Neues Dokument, dann laden.
        let doc2 = SharedProjectDoc::new(Mutex::new(ProjectDoc::new_empty()));
        let mut reg2 = CapabilityRegistry::new();
        register_project_tools(&mut reg2, doc2.clone());
        reg2.call("project.load", json!({ "path": path })).unwrap();
        let info = reg2.call("project.info", json!({})).unwrap();
        assert_eq!(info["name"], json!("Roundtrip"));
        assert!(!info["dirty"].as_bool().unwrap());
        assert!(info["pfad"].as_str().unwrap().ends_with("demo.wbproj"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn close_leert_das_dokument() {
        let doc = SharedProjectDoc::new(Mutex::new(ProjectDoc::new_named("X")));
        let mut reg = CapabilityRegistry::new();
        register_project_tools(&mut reg, doc.clone());
        assert!(reg.call("project.info", json!({})).unwrap()["geladen"].as_bool().unwrap());
        reg.call("project.close", json!({})).unwrap();
        assert!(!reg.call("project.info", json!({})).unwrap()["geladen"].as_bool().unwrap());
    }

    #[test]
    fn load_fehlender_pfad_fehler() {
        let doc = SharedProjectDoc::new(Mutex::new(ProjectDoc::new_empty()));
        let mut reg = CapabilityRegistry::new();
        register_project_tools(&mut reg, doc.clone());
        assert!(reg.call("project.load", json!({ "path": "/does/not/exist.wbproj" })).is_err());
        assert!(reg.call("project.save", json!({})).is_err()); // kein Pfad
    }
}
