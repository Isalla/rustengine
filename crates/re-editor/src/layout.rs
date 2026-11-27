//! Layout-Manifest und Presets (ADR-0001, M4 P1).
//!
//! Ein Preset ist genau drei Schichten: **Manifest** (welche Panels wo
//! docken, welche Controls welche Capability binden), **Theme** (Farben,
//! Metriken, Labels) und das geteilte Widget-Set in `re-render`. Dieses
//! Modul definiert die Datenformate und lädt das eingebaute `default`-
//! Preset der Engine; Rust-Quellcode enthält weder Labels noch Layout.

use anyhow::{bail, Context, Result};
use re_render::ui::{DockNode, Theme};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Ein Control-Eintrag (Menüpunkt, Toolbar-Knopf, Shortcut-Ziel):
/// bindet einen Capability-Namen plus Argumente — das Widget ruft
/// niemals Engine-Interna direkt, sondern `EditorContext::call`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Command {
    /// Label-Schlüssel im Theme (Fehlstand zeigt den rohen Namen).
    #[serde(default)]
    pub id: String,
    /// Capability-Name; leer bei Separatoren.
    #[serde(default)]
    pub capability: String,
    /// Argumente für den Call (JSON, Standard `{}`).
    #[serde(default)]
    pub args: Value,
    /// Reiner Trenner im Menü.
    #[serde(default)]
    pub separator: bool,
}

/// Aufklapp-Menü der Menüleiste.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Menu {
    pub id: String,
    #[serde(default)]
    pub items: Vec<Command>,
}

/// Tastatur-Kürzel → Capability. `key` ist der Name von
/// `re_platform::Key` (z. B. `"S"`, `"Escape"`), plus Modifikatoren.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shortcut {
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    pub capability: String,
    #[serde(default)]
    pub args: Value,
}

/// Das Layout-Manifest: Dock-Baum + Chrome (Menüleiste, Toolbar,
/// Shortcuts). Reine Daten — `dock_layout()` rechnet den Baum in
/// Panel-Platzierungen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutManifest {
    pub name: String,
    pub dock: DockNode,
    #[serde(default)]
    pub menu_bar: Vec<Menu>,
    #[serde(default)]
    pub toolbar: Vec<Command>,
    #[serde(default)]
    pub shortcuts: Vec<Shortcut>,
}

/// Preset = Manifest + Theme (ADR-0001).
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub manifest: LayoutManifest,
    pub theme: Theme,
}

impl LayoutManifest {
    pub fn from_json(text: &str) -> Result<Self> {
        serde_json::from_str(text).context("layout manifest")
    }

    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        Self::from_json(&text).with_context(|| format!("parse {}", path.display()))
    }

    /// Das eingebaute `default`-Manifest der Engine (M4 P1). Ein
    /// Parse-Fehler hier ist ein Build-Fehler, kein Nutzereingang.
    pub fn embedded() -> Self {
        Self::from_json(include_str!("../presets/default/layout.json"))
            .expect("eingebautes layout-Manifest")
    }

    /// Alle Capability-Namen, die das Manifest bindet (Validierung,
    /// Tests, später: Kommando-Palette).
    pub fn bound_capabilities(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self
            .menu_bar
            .iter()
            .flat_map(|m| m.items.iter())
            .chain(self.toolbar.iter())
            .map(|c| c.capability.as_str())
            .chain(self.shortcuts.iter().map(|s| s.capability.as_str()))
            .filter(|c| !c.is_empty())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Prüft, dass jede gebundene Capability in der Registry existiert
    /// (`has` kommt vom `EditorContext`). Ein Preset, das ins Leere
    /// bindet, ist ein Datenfehler — er soll früh sichtbar werden.
    pub fn validate_against(&self, has: impl Fn(&str) -> bool) -> Result<()> {
        for cap in self.bound_capabilities() {
            if !has(cap) {
                bail!("manifest binds unknown capability '{cap}'");
            }
        }
        Ok(())
    }
}

impl Preset {
    /// Das eingebaute `default`-Preset: warmes Dunkelgrau, Rust-Orange,
    /// Inter — die eigene Identität der Engine (ADR-0001 P1).
    pub fn embedded_default() -> Self {
        let theme: Theme = serde_json::from_str(include_str!("../presets/default/theme.json"))
            .expect("eingebautes default-Theme");
        Self { manifest: LayoutManifest::embedded(), theme }
    }

    /// Preset aus einem Ordner laden (`layout.json` + `theme.json`).
    pub fn load(dir: impl AsRef<std::path::Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let manifest = LayoutManifest::load(dir.join("layout.json"))?;
        let theme_path = dir.join("theme.json");
        let theme = if theme_path.exists() {
            let text = std::fs::read_to_string(&theme_path)
                .with_context(|| format!("read {}", theme_path.display()))?;
            serde_json::from_str(&text).with_context(|| format!("parse {}", theme_path.display()))?
        } else {
            Theme::default()
        };
        Ok(Self { manifest, theme })
    }
}

/// Setzt `Tabs.active` im Dock-Baum auf das Panel `id`; gibt zurück,
/// ob der Baum das Panel enthält (Tab-Klick-Auswertung).
pub fn activate_tab(node: &mut DockNode, id: &str) -> bool {
    match node {
        DockNode::Leaf { .. } => false,
        DockNode::Tabs { active, panels } => match panels.iter().position(|p| p == id) {
            Some(i) => {
                *active = i;
                true
            }
            None => false,
        },
        DockNode::Split { first, second, .. } => {
            activate_tab(first, id) || activate_tab(second, id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeddedes_manifest_und_theme_passen_zusammen() {
        let p = Preset::embedded_default();
        assert_eq!(p.manifest.name, "default");
        assert_eq!(p.theme.name, "default");
        // Dock-Baum referenziert genau die Panels des Widget-Sets.
        let mut panels = Vec::new();
        collect_panels(&p.manifest.dock, &mut panels);
        for want in ["outliner", "inspector", "viewport", "console", "assistant"] {
            assert!(panels.contains(&want.to_string()), "Panel {want} fehlt im Dock-Baum");
        }
        // Theme-Labels decken alle Chrome-IDs ein (Menüs, Panels, Commands).
        for m in &p.manifest.menu_bar {
            assert!(p.theme.labels.contains_key(&m.id), "Label {} fehlt", m.id);
            for it in &m.items {
                if !it.separator {
                    assert!(p.theme.labels.contains_key(&it.id), "Label {} fehlt", it.id);
                }
            }
        }
    }

    fn collect_panels(node: &DockNode, out: &mut Vec<String>) {
        match node {
            DockNode::Leaf { panel } => out.push(panel.clone()),
            DockNode::Tabs { panels, .. } => out.extend(panels.iter().cloned()),
            DockNode::Split { first, second, .. } => {
                collect_panels(first, out);
                collect_panels(second, out);
            }
        }
    }

    #[test]
    fn activate_tab_findet_tab_gruppen() {
        let mut m = LayoutManifest::embedded();
        assert!(activate_tab(&mut m.dock, "assistant"));
        let mut panels = Vec::new();
        collect_panels(&m.dock, &mut panels);
        // Nach Aktivierung muss das Assistant-Panel den Inhalt bekommen.
        let pl = re_render::ui::dock_layout(
            &m.dock,
            re_render::ui::Rect::new(0.0, 0.0, 1280.0, 720.0),
            26.0,
            4.0,
        );
        assert!(pl.iter().any(|p| p.panel == "assistant"), "assistant nicht aktiv");
        assert!(!activate_tab(&mut m.dock, "unbekannt"));
    }

    #[test]
    fn manifest_bindet_nur_existierende_capabilities() {
        use crate::context::EditorContext;
        use glam::Vec3;
        let ctx = EditorContext::new(Vec3::ZERO);
        let m = LayoutManifest::embedded();
        m.validate_against(|c| ctx.has(c))
            .expect("default-Manifest muss sich gegen die Registry binden");
        // Ein erdachter Name muss ablehnen.
        let bad = LayoutManifest {
            name: "bad".into(),
            dock: m.dock,
            menu_bar: vec![Menu {
                id: "menu.x".into(),
                items: vec![Command {
                    id: "x".into(),
                    capability: "gibt.es.nicht".into(),
                    args: Value::Null,
                    separator: false,
                }],
            }],
            toolbar: Vec::new(),
            shortcuts: Vec::new(),
        };
        assert!(bad.validate_against(|c| ctx.has(c)).is_err());
    }
}
