# Capabilities & AI

The engine's second signature idea: **every function a human can operate
in the editor is a named, JSON-schema'd capability that an AI agent can
call — the same code path, the same permissions.** The roadmap calls
this the rule of double addressability.

## The registry

`re_agent::CapabilityRegistry` holds `ToolSpec` (name, description,
parameter schema) plus a runner closure. One registry serves three
consumers: the editor UI, the in-process agent loop (`run_agent` with
an `AiProvider` — Ollama, llama.cpp, OpenAI-compatible, Anthropic), and
an MCP export for external clients.

## World tools

`re_agent::register_world_tools` binds a live `WorldPartition` into the
registry behind an `Arc<Mutex<…>>` shared with the editor:

| Tool | What it does |
|---|---|
| `world.stats` | time, camera, cell budget, IO telemetry, HLOD, instance count |
| `world.cells.resident` / `world.cell.state` | what is streamed where |
| `world.hlod.list` | far-field clusters |
| `world.instances.list` / `.spawn` / `.move` / `.remove` | scene editing |
| `world.camera.set` / `world.tick` | move the viewpoint, advance the simulation |

```rust
use re_agent::{register_world_tools, CapabilityRegistry, SharedWorld};

let mut reg = CapabilityRegistry::new();
register_world_tools(&mut reg, shared_world);
let stats = reg.call("world.stats", serde_json::json!({})).unwrap();
```

## Why it matters

Because editing and querying run through one door, an agent can do
exactly what the editor does — no second scripting API to keep in sync,
no "automation layer" that drifts from the UI. The M4 editor consumes
the same registry for its own panels.

Rustdoc: [`re_agent`](../../rustdoc/re_agent/index.html).
