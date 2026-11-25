# Introduction

RE is an open-source game engine: the runtime and editor are written in
Rust, services and tooling (servers, build farms, launchers) in Go. One
codebase targets PC and web, with console backends that can be attached
privately. Rendering runs on [wgpu](https://wgpu.rs) from a single WGSL
shader source.

## The signature feature

Everything in this engine is built around one demo: **the camera drops
from the top-down strategy overview into a unit's first-person view —
mid-flight, with no loading screen and no cut — and hands over control
on the way.** The same stack then serves a shooter or an MMO, because
the runtime carries no game rules.

That goal shapes the design: world data streams continuously (no scene
rebuilds), quality tiers adapt per instance, and the control handoff is
a numeric blend, not a level transition.

## Three layers of documentation

| Layer | Audience | Where |
|---|---|---|
| **This handbook** | learning the engine end to end | here |
| **rustdoc** | exact API of every crate | [`/rustdoc/`](../../rustdoc/) on this site |
| **API browser in the editor** | browsing while working | ships with M4 |

The source of truth for both generated layers is the crate
documentation itself — every public item carries rustdoc, and the
handbook chapters link into it.

## Where to go next

- New to the project? [Getting Started](./getting-started.md) builds and
  runs the streaming fly-over demo in a few minutes.
- Evaluating the design? [Architecture Tour](./architecture.md).
- Writing game or tool code? [Using the Engine from
  Rust](./rust-api.md).

The living plan with every checklist item is
[docs/roadmap.md](https://github.com/Isalla/rustengine/blob/main/docs/roadmap.md).
