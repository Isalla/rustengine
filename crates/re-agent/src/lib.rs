//! # re-agent — KI direkt im Engine-Prozess
//!
//! Grundidee (wie `wb-mcp` es im Worldbuilder vorgemacht hat): Jede
//! Engine-Funktion, die ein Mensch im Editor bedienen kann, ist hier eine
//! **Capability** mit Name, Beschreibung und JSON-Parameter-Schema.
//!
//! Zwei Wege, diese Capabilities zu erreichen — bewusst derselbe Kern:
//!
//! 1. **Direkt (bevorzugt):** Der Editor hält eine `CapabilityRegistry`
//!    und spricht sie im Prozess über `run_agent(provider, registry, …)`
//!    an. Als Provider dienen Ollama, llama.cpp (`llama-server`), ein
//!    lokaler Strata-Proxy oder öffentliche API-Endpunkte (OpenAI-
//!    kompatibel, Anthropic) — die KI nutzt die Engine-Funktionen ohne
//!    Umweg über einen externen MCP-Client.
//! 2. **MCP (Fallback):** `re-mcp` (später) exportiert exakt dieselbe
//!    Registry über MCP/stdio, damit Claude Desktop, OpenCode & Co. den
//!    Editor von außen steuern können, wenn keine Direktverbindung möglich
//!    ist (fremder Rechner, andere App, Sandbox).
//!
//! Dieses Crate enthält M1 das Daten-Grundgerüst: Registry, Provider-
//! Traits, Kontext-Digest der Szene und den Agent-Loop. Die HTTP-Transports
//! (Ollama/OpenAI/Anthropic) kommen mit M4 an dasselbe Trait —
//! `AiProvider::complete` bleibt davon unberührt.

pub mod agent;
pub mod capability;
pub mod context;
pub mod provider;

pub use agent::{run_agent, AgentError};
pub use capability::{CapabilityError, CapabilityRegistry, ToolSpec};
pub use context::SceneDigest;
pub use provider::{AiProvider, AssistantTurn, ChatMessage, ChatRequest, ProviderConfig, ProviderKind, Role, ToolCall};
