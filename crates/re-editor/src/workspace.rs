//! Workspace-Sandbox (Sicherheitsreview M4): Capability-Pfade sind
//! nicht vertrauenswürdig — UI, Agentin und spätere MCP-Clients speisen
//! sich aus dieselbe Registry, und ein Modell kann beliebige Argumente
//! erzeugen. [`Workspace`] begrenzt alle dateibasierten Capabilities
//! (project.load/save, import.*) auf einen Wurzelordner.
//!
//! Die Kommandozeile des Betreibers ist davon ausgenommen: Sie ist
//! vertrauenswürdige Eingabe und geht direkt an `ProjectDoc`/`re-io`.
//! Die Wurzel ist die aktuelle Arbeitsdirectory, überschreibbar über
//! `RE_WORKSPACE_ROOT` (Tests, portable Setups).

use std::path::{Component, Path, PathBuf};

/// Vertrauenswürdiger Wurzelordner für alle Capability-Pfade.
#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// Wurzel ist bereits kanonisiert aufgelöst; schlägt das fehl
    /// (Ordner weg), bleibt der Pfad unvalidiert — `resolve` prüft dann
    /// gegen dieselbe rohe Wurzel und scheitert konservativ.
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        Self { root }
    }

    /// `RE_WORKSPACE_ROOT`, sonst der aktuelle Arbeitsordner des
    /// Prozesses.
    pub fn from_env_or_cwd() -> Self {
        match std::env::var_os("RE_WORKSPACE_ROOT") {
            Some(r) if !r.is_empty() => Self::new(PathBuf::from(r)),
            _ => Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Wandelt ein Capability-Argument in einen absoluten Pfad innerhalb
    /// der Workspace um. Abgelehnt werden: leere Pfade, `..`-Komponenten
    /// (auch innerhalb gültiger Präfixe), Ausflüge über Symlinks und
    /// absolute Pfade außerhalb der Wurzel. Der Rückgabepfad ist
    /// kanonisiert, soweit der Ordner existiert — die Datei selbst darf
    /// fehlen (Sparziel).
    pub fn resolve(&self, raw: &str) -> Result<PathBuf, String> {
        if raw.trim().is_empty() {
            return Err("Parameter 'path' ist leer".to_string());
        }
        let p = Path::new(raw);
        if p.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(format!("Pfade mit '..' sind nicht erlaubt: {raw}"));
        }
        let joined = if p.is_absolute() { p.to_path_buf() } else { self.root.join(p) };
        let canonical = canonical_existing_prefix(&joined)?;
        if !canonical.starts_with(&self.root) {
            return Err(format!("Pfad liegt außerhalb des Workspace: {raw}"));
        }
        Ok(canonical)
    }
}

/// Kanonisiert den tiefsten existierenden Vorfahren und hängt die
/// noch fehlenden Komponenten an (Ziel einer Speicherung existiert oft
/// noch nicht). Die angehängten Teile sind reine Namen ohne `..`,
/// weil das der Aufrufer bereits verworfen hat.
fn canonical_existing_prefix(p: &Path) -> Result<PathBuf, String> {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = p;
    loop {
        match cur.canonicalize() {
            Ok(mut c) => {
                for name in tail.iter().rev() {
                    c.push(name);
                }
                return Ok(c);
            }
            Err(_) => match (cur.file_name(), cur.parent()) {
                (Some(name), Some(parent)) if !parent.as_os_str().is_empty() => {
                    tail.push(name.to_os_string());
                    cur = parent;
                }
                _ => return Err(format!("Pfad nicht auflösbar: {}", p.display())),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn workspace() -> (PathBuf, Workspace) {
        let dir = std::env::temp_dir().join(format!("re_ws_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("da.wbproj"), "{}").unwrap();
        (dir.clone(), Workspace::new(&dir))
    }

    #[test]
    fn relative_und_absolute_pfade_innerhalb() {
        let (dir, ws) = workspace();
        assert_eq!(ws.resolve("da.wbproj").unwrap(), dir.join("da.wbproj").canonicalize().unwrap());
        let abs = dir.join("sub/neu.wbproj");
        assert_eq!(ws.resolve(abs.to_str().unwrap()).unwrap(), abs);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ausbruch_wird_abgelehnt() {
        let (dir, ws) = workspace();
        assert!(ws.resolve("../andere.wbproj").is_err());
        assert!(ws.resolve("sub/../../andere.wbproj").is_err());
        assert!(ws.resolve("/etc/passwd").is_err());
        assert!(ws.resolve("").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aus_der_workspace_wird_abgelehnt() {
        let (dir, ws) = workspace();
        let aussen = std::env::temp_dir().join(format!("re_ws_out_{}", std::process::id()));
        fs::create_dir_all(&aussen).unwrap();
        fs::write(aussen.join("geheim.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&aussen, dir.join("link")).unwrap();
        assert!(ws.resolve("link/geheim.txt").is_err());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&aussen);
    }
}
