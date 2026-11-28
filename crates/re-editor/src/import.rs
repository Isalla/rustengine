//! Import-Pipeline (M4): Modell-Dateien in ein Projekt laden — dünn
//! über `re_io::import` (glTF/glB/OBJ nach Endung), aber mit Zählern
//! und als Capability adressierbar, damit Editor und Agent denselben
//! Einstieg nutzen.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use re_core::project::Project;
use re_io;
use re_agent::capability::{CapabilityRegistry, ToolSpec};
use serde_json::json;

use crate::project::{ProjectDoc, SharedProjectDoc};
use crate::workspace::Workspace;

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

/// Lädt eine Modell-Datei; `re_io::import` entscheidet nach der
/// Endung (glTF, glB, OBJ). Der Name bleibt aus Gründen der
/// Rückwärtskompatibilität — er ist seit je der Import-Einstieg.
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

/// Modell-Datei in das Editor-Projekt laden — Einstieg für alle
/// Import-Capabilities (Format ergibt sich aus der Endung).
fn import_into(d: &SharedProjectDoc, w: &Workspace, path: &str) -> Result<serde_json::Value, String> {
    let path = w.resolve(path)?;
    let r = import_gltf(&path).map_err(|e| e.to_string())?;
    let summary = import_summary(&r);
    let mut doc = d.lock().map_err(|_| "Projekt-Sperre vergiftet".to_string())?;
    *doc = ProjectDoc { project: Some(r.project), path: Some(r.path.clone()), dirty: true };
    Ok(json!({ "ok": true, "import": summary }))
}

pub fn register_import_tools(reg: &mut CapabilityRegistry, doc: SharedProjectDoc, ws: Arc<Workspace>) {
    {
        let d = doc.clone();
        let w = ws.clone();
        reg.register(
            ToolSpec::new("import.gltf", "glTF/glB importieren und als aktuelles Projekt laden (Workspace-intern)", json!({ "typ": "object", "felder": { "path": { "typ": "string" } } })),
            move |args| {
                let path = args.get("path").and_then(|v| v.as_str()).ok_or("Parameter 'path' fehlt")?;
                import_into(&d, &w, path)
            },
        );
    }
    {
        let d = doc.clone();
        let w = ws.clone();
        reg.register(
            ToolSpec::new("import.obj", "Wavefront OBJ mit MTL-Material importieren und als aktuelles Projekt laden (Workspace-intern)", json!({ "typ": "object", "felder": { "path": { "typ": "string" } } })),
            move |args| {
                let path = args.get("path").and_then(|v| v.as_str()).ok_or("Parameter 'path' fehlt")?;
                import_into(&d, &w, path)
            },
        );
    }
    {
        let w = ws;
        reg.register(
            ToolSpec::new("import.preview", "Modell-Datei einlesen und Zähler liefern, ohne das Projekt zu ersetzen (Workspace-intern)", json!({ "typ": "object", "felder": { "path": { "typ": "string" } } })),
            move |args| {
                let path = args.get("path").and_then(|v| v.as_str()).ok_or("Parameter 'path' fehlt")?;
                let path = w.resolve(path)?;
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
        let ws = Arc::new(Workspace::new(&dir));
        let doc = SharedProjectDoc::new(Mutex::new(ProjectDoc::new_empty()));
        let mut reg = CapabilityRegistry::new();
        register_import_tools(&mut reg, doc.clone(), ws.clone());
        register_project_tools(&mut reg, doc.clone(), ws);
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
    fn capability_importiert_obj_mit_mtl() {
        let dir = std::env::temp_dir().join(format!("re_imp4_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("m.mtl"), "newmtl Blau\nKd 0 0 1\nNs 50\n").unwrap();
        fs::write(
            dir.join("s.obj"),
            "mtllib m.mtl\nusemtl Blau\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvn 0 0 1\nf 1//1 2//1 3//1\n",
        )
        .unwrap();
        let ws = Arc::new(Workspace::new(&dir));
        let doc = SharedProjectDoc::new(Mutex::new(ProjectDoc::new_empty()));
        let mut reg = CapabilityRegistry::new();
        register_import_tools(&mut reg, doc.clone(), ws.clone());
        register_project_tools(&mut reg, doc.clone(), ws);
        let res = reg.call("import.obj", json!({ "path": "s.obj" })).unwrap();
        assert!(res["ok"].as_bool().unwrap());
        assert_eq!(res["import"]["meshes"], json!(1));
        assert_eq!(res["import"]["materialien"], json!(1), "MTL hängt am Mesh");
        let info = reg.call("project.info", json!({})).unwrap();
        assert!(info["geladen"].as_bool().unwrap());
        // Die Capability ist ein Label — das Format entscheidet die
        // Endung, also läuft derselbe Import auch über import.gltf.
        assert!(reg.call("import.gltf", json!({ "path": "s.obj" })).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fehlende_datei_fehler() {
        let dir = std::env::temp_dir().join(format!("re_imp3_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let doc = SharedProjectDoc::new(Mutex::new(ProjectDoc::new_empty()));
        let mut reg = CapabilityRegistry::new();
        register_import_tools(&mut reg, doc.clone(), Arc::new(Workspace::new(&dir)));
        assert!(reg.call("import.gltf", json!({ "path": "nope.gltf" })).is_err());
        assert!(reg.call("import.preview", json!({ "path": "nope.gltf" })).is_err());
        // Ausbruch aus dem Workspace, bevor überhaupt gelesen wird.
        assert!(reg.call("import.preview", json!({ "path": "/etc/passwd" })).is_err());
        assert!(reg.call("import.gltf", json!({ "path": "../../geheim.gltf" })).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
