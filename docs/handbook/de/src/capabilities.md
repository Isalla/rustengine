# Capabilities & KI

Die zweite Leitidee der Engine: **Jede Funktion, die ein Mensch im
Editor bedienen kann, ist eine benannte, per JSON-Schema beschriebene
Capability, die ein KI-Agent aufrufen kann — derselbe Codepfad,
dieselben Berechtigungen.** Die Roadmap nennt das die Regel der
doppelten Adressierbarkeit.

## Die Registry

Die `re_agent::CapabilityRegistry` hält `ToolSpec` (Name, Beschreibung,
Parameter-Schema) plus eine Runner-Closure. Eine Registry bedient drei
Abnehmer: die Editor-UI, den Agent-Loop im Prozess (`run_agent` mit
einem `AiProvider` — Ollama, llama.cpp, OpenAI-kompatibel, Anthropic)
und einen MCP-Export für externe Clients.

## Welt-Werkzeuge

`re_agent::register_world_tools` bindet eine lebende `WorldPartition`
hinter einem `Arc<Mutex<…>>` an die Registry — geteilt mit dem Editor:

| Werkzeug | Was es tut |
|---|---|
| `world.stats` | Zeit, Kamera, Zell-Budget, IO-Telemetrie, HLOD, Instanzanzahl |
| `world.cells.resident` / `world.cell.state` | was wird wo gestreamt |
| `world.hlod.list` | Fernfeld-Cluster |
| `world.instances.list` / `.spawn` / `.move` / `.remove` | Szenen-Bearbeitung |
| `world.camera.set` / `world.tick` | Standpunkt setzen, Simulation weitertreiben |

```rust
use re_agent::{register_world_tools, CapabilityRegistry, SharedWorld};

let mut reg = CapabilityRegistry::new();
register_world_tools(&mut reg, shared_world);
let stats = reg.call("world.stats", serde_json::json!({})).unwrap();
```

## Warum das zählt

Weil Bearbeiten und Abfragen durch eine Tür laufen, kann ein Agent
genau das tun, was der Editor tut — kein zweites Skript-API, das
synchron gehalten werden muss, keine „Automatisierungs-Schicht“, die
von der UI wegdiftet. Der M4-Editor verwendet dieselbe Registry für
seine Panels.

Rustdoc: [`re_agent`](../../rustdoc/re_agent/index.html).
