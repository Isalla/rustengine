//! Capability-Registry: die eine Wahrheit darüber, was die Engine kann.
//!
//! Editor-UI, Direkt-KI und MCP-Server speisen sich alle aus derselben
//! Liste — neue Engine-Funktionen sind damit automatisch auch KI-Werkzeuge.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// JSON-Wert (Parameter/Rückgaben der Werkzeuge).
pub type Json = Value;

/// Selbstbeschreibung eines Werkzeugs — kompatibel zu OpenAI tool-schema
/// und MCP `tools/list`, daher hier generisch gehalten.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON-Schema-Fragment: `{"typ": "object", "felder": {...}}` o. ä.
    /// M1 nutzt ein vereinfachtes Schema; M4 normalisiert es vollständig.
    pub params: Json,
}

impl ToolSpec {
    pub fn new(name: impl Into<String>, description: impl Into<String>, params: Json) -> Self {
        Self { name: name.into(), description: description.into(), params }
    }

    /// Parameterloses Werkzeug.
    pub fn simple(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(name, description, json!({ "typ": "object", "felder": {} }))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    #[error("unbekanntes Werkzeug: {0}")]
    Unknown(String),
    #[error("Werkzeug '{name}' fehlgeschlagen: {reason}")]
    Run { name: String, reason: String },
}

type Runner = Box<dyn Fn(Json) -> Result<Json, String> + Send>;

pub struct Capability {
    pub spec: ToolSpec,
    runner: Runner,
}

/// Registry aller Engine-Funktionen. `call` ist `&self`, damit der
/// Agent-Loop parallel zum Editor lesen kann; schreibende Capabilities
/// kapseln ihre Innere Mutabilität selbst (Editor-Channel, Mutex, …).
#[derive(Default)]
pub struct CapabilityRegistry {
    tools: BTreeMap<String, Capability>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<F>(&mut self, spec: ToolSpec, f: F)
    where
        F: Fn(Json) -> Result<Json, String> + Send + 'static,
    {
        let name = spec.name.clone();
        tracing::debug!(tool = %name, "Capability registriert");
        self.tools.insert(name, Capability { spec, runner: Box::new(f) });
    }

    pub fn remove(&mut self, name: &str) -> Option<ToolSpec> {
        self.tools.remove(name).map(|c| c.spec)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|c| c.spec.clone()).collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Ein Werkzeug ausführen (Direktweg des Agent-Loops).
    pub fn call(&self, name: &str, args: Json) -> Result<Json, CapabilityError> {
        let cap = self
            .tools
            .get(name)
            .ok_or_else(|| CapabilityError::Unknown(name.to_string()))?;
        (cap.runner)(args).map_err(|reason| CapabilityError::Run {
            name: name.to_string(),
            reason,
        })
    }

    /// Kompakte Werkzeugliste für den System-Prompt (Token-sparend).
    pub fn prompt_block(&self) -> String {
        let mut out = String::from("Verfügbare Werkzeuge:\n");
        for c in self.tools.values() {
            out.push_str(&format!("- {}: {}\n", c.spec.name, c.spec.description));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_registriert_und_ruft_au() {
        let mut reg = CapabilityRegistry::new();
        reg.register(ToolSpec::new("add", "zwei Zahlen addieren", json!({})), |args| {
            let a = args.get("a").and_then(|v| v.as_i64()).ok_or("a fehlt")?;
            let b = args.get("b").and_then(|v| v.as_i64()).ok_or("b fehlt")?;
            Ok(json!(a + b))
        });
        assert_eq!(reg.call("add", json!({"a": 2, "b": 40})).unwrap(), json!(42));
        assert!(matches!(reg.call("fehlt", json!({})), Err(CapabilityError::Unknown(_))));
        assert!(reg.call("add", json!({})).is_err());
        assert!(reg.prompt_block().contains("add"));
    }
}
