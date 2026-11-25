//! Import-Pipeline (M4): glTF/glB in ein Projekt laden — dünn über
//! `re_io::import`, aber mit Zählern und als Capability adressierbar,
//! damit Editor und Agent denselben Einstieg nutzen.

use std::path::{Path, PathBuf};
use re_core::project::Project;
use re_io;
use re_agent::capability::{CapabilityRegistry, ToolSpec};
use serde_json::json;

use crate::project::{ProjectDoc, SharedProjectDoc};

/// Ergebnis eines Imports: das geladene Projekt plus eine kurze
/// Statistik für UI/Agent.
#[derive(Debug)]
pub struct ImportResult {
    pub project: Project,
    pub path: PathBuf,
    pub meshes: usize,
    pub materials: usize,
    pub nodes: usize,
    pub animations: usize,
}

pub fn import_gltf(path: impl AsRef<Path>) -> anyhow::Result<ImportResult> {
    let path = path.as_ref().to_path_buf();
    let project = re_io::import(&path).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(ImportResult {
        meshes: project.library.meshes.len(),
        materials: project.library.materials.len(),
        nodes: project.scene.nodes.len(),
        animations: project.library.animations.len(),
        project,
        path,
    })
}

pub fn import_summary(r: &ImportResult) -> serde_json::Value {
    json!({
        "pfad": r.path,
        "name": r.project.name,
        "meshes": r.meshes,
        "materialien": r.materials,
        "knoten": r.nodes,
        "animationen": r.animations,
    })
}

pub fn register_import_tools(reg: &mut CapabilityRegistry, doc: SharedProjectDoc) {
    {
        let d = doc.clone();
        reg.register(
            ToolSpec::new("import.gltf", "glTF/glB importieren und als aktuelles Projekt laden", json!({ "typ": "object", "felder": { "path": { "typ": "string" } } })),
            move |args| {
                let path = args.get("path").and_then(|v| v.as_str()).ok_or("Parameter 'path' fehlt")?;
                let r = import_gltf(path).map_err(|e| e.to_string())?;
                let summary = import_summary(&r);
                let mut doc = d.lock().map_err(|_| "Projekt-Sperre vergiftet".to_string())?;
                *doc = ProjectDoc { project: Some(r.project), path: Some(r.path.clone()), dirty: true };
                Ok(json!({ "ok": true, "import": summary }))
            },
        );
    }
    {
        reg.register(
            ToolSpec::new("import.preview", "glTF/glB einlesen und Zähler liefern, ohne das Projekt zu ersetzen", json!({ "typ": "object", "felder": { "path": { "typ": "string" } } })),
            move |args| {
                let path = args.get("path").and_then(|v| v.as_str()).ok_or("Parameter 'path' fehlt")?;
                let r = import_gltf(path).map_err(|e| e.to_string())?;
                Ok(import_summary(&r))
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::register_project_tools;
    use re_agent::CapabilityRegistry;
    use std::sync::Mutex;
    use re_core::primitives::{self, PrimitiveKind};
    use re_core::project::Project;
    use re_core::scene::NodeKind;
    use re_core::transform::Transform;
    use serde_json::json;
    use std::fs;

    fn make_and_export_gltf(dir: &std::path::Path) -> std::path::PathBuf {
        let mut p = Project::new("imp");
        let mesh = primitives::generate(PrimitiveKind::Box, 1.0, 1.0, 1);
        let mid = p.library.add_mesh(mesh);
        p.scene.insert(None, re_core::scene::Node {
            name: "Cube".into(),
            transform: Transform::default(),
            kind: NodeKind::MeshInstance { mesh: mid, material: None, skeleton_node: None },
            ..Default::default()
        });
        let path = dir.join("demo.gltf");
        re_io::export(&p, &path).expect("export");
        path
    }

    #[test]
    fn import_liefert_projekt_mit_zaehlern() {
        let dir = std::env::temp_dir().join(format!("re_imp_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = make_and_export_gltf(&dir);
        let r = import_gltf(&path).unwrap();
        assert!(r.meshes >= 1);
        assert!(r.nodes >= 1);
        assert_eq!(r.project.name, "demo");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn capabilities_importieren_in_projekt() {
        let dir = std::env::temp_dir().join(format!("re_imp2_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = make_and_export_gltf(&dir);
        let doc = SharedProjectDoc::new(Mutex::new(ProjectDoc::new_empty()));
        let mut reg = CapabilityRegistry::new();
        register_import_tools(&mut reg, doc.clone());
        register_project_tools(&mut reg, doc.clone());
        let res = reg.call("import.gltf", json!({ "path": path })).unwrap();
        assert!(res["ok"].as_bool().unwrap());
        assert!(res["import"]["meshes"].as_u64().unwrap() >= 1);
        let info = reg.call("project.info", json!({})).unwrap();
        assert!(info["geladen"].as_bool().unwrap());
        assert!(info["dirty"].as_bool().unwrap());
        // preview ändert nichts
        let before = reg.call("project.info", json!({})).unwrap()["knoten"].clone();
        reg.call("import.preview", json!({ "path": path })).unwrap();
        let after = reg.call("project.info", json!({})).unwrap()["knoten"].clone();
        assert_eq!(before, after);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fehlende_datei_fehler() {
        let doc = SharedProjectDoc::new(Mutex::new(ProjectDoc::new_empty()));
        let mut reg = CapabilityRegistry::new();
        register_import_tools(&mut reg, doc.clone());
        assert!(reg.call("import.gltf", json!({ "path": "/nope/nope.gltf" })).is_err());
        assert!(reg.call("import.preview", json!({ "path": "/nope/nope.gltf" })).is_err());
    }
}
