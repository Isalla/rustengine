//! Provider-Anbindung: lokale Modelle (Ollama, llama.cpp, Strata-Proxy)
//! und öffentliche APIs (OpenAI-kompatibel, Anthropic) — ein Trait.
//!
//! M1 liefert die Typen und Konfigurationen. Der eigentliche HTTP-Transport
//! (M4) hängt nur noch an `AiProvider::complete`; Agent-Loop, Registry und
//! Editor-Integration bleiben unverändert.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capability::ToolSpec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    /// Ollama-Server (http://localhost:11434) — lokale Modelle per Pull.
    Ollama,
    /// llama.cpp `llama-server` (Standard :8080) — OpenAI-kompatibler Endpunkt.
    LlamaCpp,
    /// Jeder OpenAI-kompatible Endpunkt: Strata-Proxy, OpenAI, DeepSeek, …
    OpenAiCompatible,
    /// Anthropic Messages API.
    Anthropic,
    /// Gegenstück in einem anderen Prozess (MCP/Socket) — nur als
    /// Notfall-Fallback, damit auch fremde Clients den Editor steuern können.
    McpBridge,
}

/// Wo ist der Endpoint, welches Modell, woher der Schlüssel?
/// API-Keys liegen NIE im Projekt — nur der Name der Umgebungsvariable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

impl ProviderConfig {
    pub fn ollama(model: impl Into<String>) -> Self {
        Self { kind: ProviderKind::Ollama, base_url: "http://localhost:11434".into(), model: model.into(), api_key_env: None }
    }
    pub fn llama_cpp(model: impl Into<String>, port: u16) -> Self {
        Self {
            kind: ProviderKind::LlamaCpp,
            base_url: format!("http://localhost:{port}"),
            model: model.into(),
            api_key_env: None,
        }
    }
    /// Auch für lokale Proxy-Endpunkte (z. B. Strata) — alles OpenAI-Kompatibilität.
    pub fn openai_compatible(base_url: impl Into<String>, model: impl Into<String>, key_env: impl Into<String>) -> Self {
        Self { kind: ProviderKind::OpenAiCompatible, base_url: base_url.into(), model: model.into(), api_key_env: Some(key_env.into()) }
    }
    pub fn anthropic(model: impl Into<String>) -> Self {
        Self {
            kind: ProviderKind::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            model: model.into(),
            api_key_env: Some("ANTHROPIC_API_KEY".into()),
        }
    }

    /// Schlüssel aus der konfigurierten Umgebung Variable (M4 nutzt ihn der Transport).
    pub fn api_key(&self) -> Option<String> {
        self.api_key_env.as_ref().and_then(|var| std::env::var(var).ok())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Bei Role::Tool: auf welchen ToolCall antwortet das?
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(s: impl Into<String>) -> Self { Self { role: Role::System, content: s.into(), tool_call_id: None } }
    pub fn user(s: impl Into<String>) -> Self { Self { role: Role::User, content: s.into(), tool_call_id: None } }
    pub fn assistant(s: impl Into<String>) -> Self { Self { role: Role::Assistant, content: s.into(), tool_call_id: None } }
    pub fn tool(id: impl Into<String>, result: impl Into<String>) -> Self {
        Self { role: Role::Tool, content: result.into(), tool_call_id: Some(id.into()) }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    /// Werkzeuge, die das Modell in dieser Runde anrufen darf.
    pub tools: Vec<ToolSpec>,
    pub temperature: Option<f32>,
}

/// Vom Modell angeforderter Werkzeug-Aufruf.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub args: Value,
}

/// Was das Modell als Antwort geliefert hat.
#[derive(Clone, Debug, PartialEq)]
pub enum AssistantTurn {
    Text(String),
    /// Modell will Werkzeuge ausführen — der Loop ruft sie auf und antwortet mit Role::Tool.
    Calls(Vec<ToolCall>),
    /// Manche Modelle mischen beides (Kommentar + Calls).
    Both { text: String, calls: Vec<ToolCall> },
}

impl AssistantTurn {
    pub fn calls(&self) -> &[ToolCall] {
        match self {
            AssistantTurn::Text(_) => &[],
            AssistantTurn::Calls(c) | AssistantTurn::Both { calls: c, .. } => c,
        }
    }
    pub fn text(&self) -> &str {
        match self {
            AssistantTurn::Text(t) | AssistantTurn::Both { text: t, .. } => t,
            AssistantTurn::Calls(_) => "",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("Transport-Fehler: {0}")]
    Transport(String),
    #[error("Protokoll-Fehler: Antwort nicht interpretierbar: {0}")]
    Protocol(String),
    #[error("Auth fehlgeschlagen (Schlüssel gesetzt?): {0}")]
    Auth(String),
    #[error("Provider kann das nicht: {0}")]
    Unsupported(String),
}

/// Ein KI-Backend. Implementierungen: HTTP gegen Ollama/llama.cpp/OpenAI/
/// Anthropic (M4), Test-Stub im Agent-Loop-Test, später ggf. lokale
/// llama.cpp-Library via FFI.
pub trait AiProvider: Send {
    fn kind(&self) -> ProviderKind;
    fn complete(&mut self, req: &ChatRequest) -> Result<AssistantTurn, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_helpers_aggregieren() {
        let t = AssistantTurn::Both {
            text: "schaue nach".into(),
            calls: vec![ToolCall { call_id: "1".into(), name: "scene.stats".into(), args: Value::Null }],
        };
        assert_eq!(t.calls().len(), 1);
        assert_eq!(t.text(), "schaue nach");
    }

    #[test]
    fn konfigurationen_zeigen_nur_env_namen() {
        let c = ProviderConfig::anthropic("claude-x");
        assert_eq!(c.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
        // Kein Schlüsselwort im Debug-Output (nur Env-Name, nie der Wert).
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("sk-"));
    }
}
