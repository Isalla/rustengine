//! Der Agent-Loop: Modell antwortet → Registry führt Werkzeuge aus →
//! Ergebnisse zurück ins Gespräch → bis nur noch Text kommt.
//!
//! Läuft direkt im Engine-Prozess (kein MCP-Transport nötig). Jeder
//! Werkzeug-Aufruf kann vorher durch die `PermissionHook` bestätigt
//! werden — so entscheidet der Mensch, ob die KI nur vorschlägt oder
//! auch schreibt.

use crate::capability::CapabilityRegistry;
use crate::provider::{AiProvider, ChatMessage, ChatRequest};
use serde_json::json;

/// Schrittlimit, damit eine kaputte KI nicht endlos Tool-Calls dreht.
pub const DEFAULT_MAX_STEPS: usize = 24;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Provider: {0}")]
    Provider(#[from] crate::provider::ProviderError),
    #[error("Werkzeug: {0}")]
    Capability(#[from] crate::capability::CapabilityError),
    #[error("Schrittlimit erreicht ({0}) — KI kommt nicht zum Abschluss")]
    StepLimit(usize),
}

/// Erlaubnis-Hook vor jedem Werkzeug-Aufruf.
/// M1: nur der Trait; der Editor liefert die UI-Implementierung (M4).
pub trait PermissionHook: Send {
    /// false = Aufruf blockieren (KI bekommt "vom Benutzer abgelehnt").
    fn approve(&mut self, tool: &str, args: &serde_json::Value) -> bool;
}

/// Alles ohne Rückfrage erlauben (Testläufe, Read-only-Sitzungen).
pub struct AllowAll;

impl PermissionHook for AllowAll {
    fn approve(&mut self, _tool: &str, _args: &serde_json::Value) -> bool {
        true
    }
}

/// Einmal nachfragen, danach nicht wieder (Editor-Standard in M4).
#[derive(Default)]
pub struct AskOnce {
    asked: bool,
}

impl AskOnce {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PermissionHook for AskOnce {
    fn approve(&mut self, _tool: &str, _args: &serde_json::Value) -> bool {
        if self.asked {
            return true;
        }
        self.asked = true;
        // Hier öffnet in M4 der Editor einen Bestätigungsdialog.
        true
    }
}

/// Ein Durchlauf. `req.messages` wächst um Assistant- und Tool-Nachrichten;
/// am Ende wird der letzte Text zurückgegeben.
pub fn run_agent(
    provider: &mut dyn AiProvider,
    registry: &CapabilityRegistry,
    req: &mut ChatRequest,
    hook: &mut dyn PermissionHook,
    max_steps: usize,
) -> Result<String, AgentError> {
    req.tools = registry.specs();
    let mut last_text;

    for _step in 0..max_steps {
        let turn = provider.complete(req)?;
        last_text = turn.text().to_string();

        let calls = turn.calls().to_vec();
        if calls.is_empty() {
            if !last_text.is_empty() {
                req.messages.push(ChatMessage::assistant(last_text.clone()));
            }
            return Ok(last_text);
        }

        // Assistant-Nachricht mit den Calls (serialisiert) voranstellen —
        // Konvention aller Tool-Call-Protokolle (OpenAI/Anthropic/Ollama).
        req.messages.push(ChatMessage {
            role: crate::provider::Role::Assistant,
            content: serde_json::to_string(&calls).unwrap_or_default(),
            tool_call_id: None,
        });

        for call in calls {
            let result = if hook.approve(&call.name, &call.args) {
                match registry.call(&call.name, call.args.clone()) {
                    Ok(v) => v,
                    Err(e) => json!({ "fehler": e.to_string() }),
                }
            } else {
                json!({ "fehler": "vom Benutzer abgelehnt" })
            };
            tracing::info!(tool = %call.name, "Agent-Werkzeug ausgeführt");
            req.messages.push(ChatMessage::tool(call.call_id, result.to_string()));
        }
    }
    Err(AgentError::StepLimit(max_steps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::ToolSpec;
    use crate::provider::{AssistantTurn, ProviderKind, ProviderError, Role, ToolCall};
    use serde_json::json;

    /// Stub: erste Runde ruft `scene.stats` auf, zweite Runde liefert Text.
    struct ScriptedProvider {
        step: usize,
    }

    impl AiProvider for ScriptedProvider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Ollama
        }
        fn complete(&mut self, req: &ChatRequest) -> Result<AssistantTurn, ProviderError> {
            self.step += 1;
            let tool_answered = req.messages.iter().any(|m| m.role == Role::Tool);
            if self.step == 1 {
                Ok(AssistantTurn::Calls(vec![ToolCall {
                    call_id: "c1".into(),
                    name: "scene.stats".into(),
                    args: json!({}),
                }]))
            } else if tool_answered {
                Ok(AssistantTurn::Text("Fertig: 1 Mesh.".into()))
            } else {
                Err(ProviderError::Protocol("Tool-Antwort fehlt".into()))
            }
        }
    }

    #[test]
    fn loop_ruft_werkzeug_und_finished() {
        let mut reg = CapabilityRegistry::new();
        reg.register(ToolSpec::simple("scene.stats", "Anzahl der Objekte"), |_| {
            Ok(json!({ "meshes": 1 }))
        });
        let mut provider = ScriptedProvider { step: 0 };
        let mut req = ChatRequest {
            messages: vec![ChatMessage::user("was ist in der szene?")],
            ..Default::default()
        };
        let out = run_agent(&mut provider, &reg, &mut req, &mut AllowAll, 4).unwrap();
        assert_eq!(out, "Fertig: 1 Mesh.");
        assert!(req.messages.iter().any(|m| m.content.contains("\"meshes\":1")));
    }

    #[test]
    fn abgelehntes_werkzeug_kommt_als_fehler_zurueck() {
        struct DenyAll;
        impl PermissionHook for DenyAll {
            fn approve(&mut self, _t: &str, _a: &serde_json::Value) -> bool {
                false
            }
        }
        let mut reg = CapabilityRegistry::new();
        reg.register(ToolSpec::simple("scene.stats", "Anzahl der Objekte"), |_| Ok(json!("ok")));
        let mut provider = ScriptedProvider { step: 0 };
        let mut req = ChatRequest {
            messages: vec![ChatMessage::user("mach was")],
            ..Default::default()
        };
        let out = run_agent(&mut provider, &reg, &mut req, &mut DenyAll, 4).unwrap();
        assert_eq!(out, "Fertig: 1 Mesh.");
        assert!(req.messages.iter().any(|m| m.content.contains("abgelehnt")));
    }

    #[test]
    fn schrittlimit_bricht_ab() {
        struct NeverDone;
        impl AiProvider for NeverDone {
            fn kind(&self) -> ProviderKind {
                ProviderKind::Ollama
            }
            fn complete(&mut self, _r: &ChatRequest) -> Result<AssistantTurn, ProviderError> {
                Ok(AssistantTurn::Calls(vec![ToolCall {
                    call_id: "x".into(),
                    name: "ping".into(),
                    args: json!({}),
                }]))
            }
        }
        let mut reg = CapabilityRegistry::new();
        reg.register(ToolSpec::simple("ping", "antwortet pong"), |_| Ok(json!("pong")));
        let mut provider = NeverDone;
        let mut req = ChatRequest::default();
        let err = run_agent(&mut provider, &reg, &mut req, &mut AllowAll, 3).unwrap_err();
        assert!(matches!(err, AgentError::StepLimit(3)));
    }
}
