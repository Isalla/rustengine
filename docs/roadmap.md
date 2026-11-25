# Milestone roadmap

[English](roadmap.md) | [Deutsch](roadmap.de.md)

The full checklist between „clone the repo" and „ship the game", with
honest state of repair. ✅ done and test-covered · 🟡 partially built ·
⬜ not started. Background on any block:
[architecture](architecture.md) · [cinema](cinema.md) ·
[rendering](rendering.md) · [vram](vram.md) ·
[ki-integration](ki-integration.md).

## What runs today

* glTF scenes load and render offscreen (`re-demo`, `re-wgpu`)
* window + input + event loop (`re-platform`, tested headless)
* ECS with generation-checked handles (`re-ecs`)
* world partition: clipmap streaming, quality tiers T0..T4, byte
  budget, pooling — deterministically testable without a GPU
  (`re-world`, incl. async IO worker pool reading cell files)
* cells move into VRAM asynchronously and are released after a grace
  period — the `stream-demo` fly-over exercises the whole chain
  (`re-world::handoff`, `re-wgpu`)

## M0 — Repo & core data model ✅

- [x] MIT-licensed workspace, crate layout, bilingual docs
- [x] Scene graph, meshes, materials + textures (`re-core`)
- [x] Animations & skeletons, terrain & tilesets (`re-core`)
- [x] glTF 2.0 import/export incl. texture embedding (`re-io`)

## M1 — First pixels ✅

- [x] Backend-neutral render layer: `GfxBackend`, camera, billboard
      batches (`re-render`)
- [x] wgpu backend: Vulkan/Metal/WebGPU, DirectX via wgpu; offscreen
      and windowed (`re-wgpu`)
- [x] Demo binary renders a glTF scene with textures (`re-demo`)

## M2 — Platform layer ✅

- [x] Windows, input, event loop, delta time (`re-platform`/winit)
- [x] Headless mode — every layer testable without a display

## M3 — ECS & world streaming ✅

- [x] **M3a** Entity-component-system: generation handles, sparse-set
      storage, system schedule (`re-ecs`)
- [x] **M3b** Grid & clipmap: anchor-quantized active cell set, hole-free
      coverage, ≤2× overlap at band borders (`re-world::grid/clipmap`)
- [x] **M3b** Quality tiers T0..T4 with hysteresis and dwell time —
      minimap marker to cinematic mesh as one continuous function
      (`re-world::tier`)
- [x] **M3b** Streamer: byte budget, deterministic priority, eviction
      coarse-first then LRU, pooling instead of unloading
      (`re-world::stream`)
- [x] **M3c** Async IO pipeline: worker-thread pool behind `CellSource`,
      file-backed block store, cancel-safe, never blocks the caller
      (`re-world::io`)
- [x] **M3c** GPU handoff: triple-buffered `CellHandoff` with per-frame
      byte quota, retire grace, deferred release; `upload_cell`/
      `release_cell` in the backend traits, cell residency in `re-wgpu`
      — demonstrated end-to-end by `stream-demo` (fly-over of a
      formula world streamed through the real GPU pipeline)
- [x] **M3d** HLOD: baked far-field summary meshes at the clipmap rim
      (`re-world::hlod` — cluster registration, take-over zone with
      hysteresis, fade-out; demonstrated by `stream-demo`)
- [x] **M3e** First-person handoff: control transfer mid-flight without
      scene rebuild (`re-world::ego` — smoothed camera blend, input
      ownership, world streaming untouched; demonstrated by `stream-demo`)
- [x] IO telemetry: measured load latency/throughput drives tier
      hysteresis and dwell adaptively (slow disk → degrade earlier)
- [x] MCP tools: agent-facing capability bindings for streaming and
      scene queries (`re-agent::world` — world/cell/instance tools on the
      shared `WorldPartition`, same registry the editor uses)

## M4 — Editor ⬜ ← current focus

- [ ] Scene view with live camera into the streaming world
- [ ] Outliner & inspector over the ECS
- [ ] Import pipeline UI wrapping `re-io`
- [ ] Undo/redo as reversible systems
- [ ] Worldbuilder project loader

## M5 — Render pipeline V2 ⬜

- [ ] Deferred HDR pipeline
- [ ] Shadow 2.0 (cascaded + clustered)
- [ ] Upscaling adapters: FSR / XeSS / DLSS ([rendering](rendering.md))

## M6 — Dynamic GI & reflections ⬜

- [ ] Lumen-style software ray-tracing path
- [ ] Hardware RT as optional quality switch
- [ ] `re-native` backend for vendor RT stacks

## M7 — Path tracing ⬜

- [ ] Progressive path tracer
- [ ] Ray reconstruction / denoising
- [ ] Offline & bake modes

## M8 — Media stack ⬜

- [ ] Audio: SFX event system (`re-audio`)
- [ ] VFX: GPU particles, effect graphs (`re-vfx`)
- [ ] Physics (`re-physics`)
- [ ] Animation: blends, IK, layers

## M9 — Online: the Go side ⬜

- [ ] Netcode service (Go) with server-side interest management
- [ ] Dedicated server
- [ ] Build farm & launcher tooling
- [ ] WebGPU web build of the runtime

## M10 — Consoles & shipping ⬜

- [ ] NDA SDK backends docked to the public traits (private repos)
- [ ] Certification helpers, publishing pipeline

## M11 — AI-native editor: UE5 breadth, plugin open ⬜

*The editor of this engine is built for two kinds of users from day
one: the human at the viewport and the agent at the registry. The
target is the functional breadth of the UE5 editor — landscape,
sequencer, VFX, audio, visual scripting, profiling — realized as
plugins over one capability registry ([ki-integration](ki-integration.md)),
so that every window a human can open is a tool the AI can call, and
one sentence can drive what used to take a hundred clicks.*

**Foundations**

- [ ] Plugin architecture: engine & editor features ship as plugins
      against a stable extension API (panels, importers, ECS systems,
      render passes, registry tools); dynamic loading, versioned,
      sandboxed third-party plugins opt-in (native `.so` or Wasm)
- [ ] Double addressability rule: no editor feature ships without a
      capability-registry tool; the UI is a client of the same tools
      (undo-safe, batchable by the agent)
- [ ] Scripting layer: embedded scripting (e.g. Lua or Rust hot-load)
      for game logic and editor automation, also exposed as tools

**Editor breadth (each item = UI panel + registry tools + plugin)**

- [ ] Level editor: transform gizmos, snapping, modular kit assembly
- [ ] Landscape & scatter: sculpting, paint layers, foliage/procedural
      generation (PCG-style graph, deterministic seeds)
- [ ] Material editor: node graph compiling to WGSL variants
- [ ] Sequencer: tracks for camera, animation, audio, FX, properties —
      timeline editable by hand or from a text description
- [ ] VFX editor over `re-vfx` graphs; audio editor over `re-audio`
      events; navigation & AI tools (navmesh, behavior graphs)
- [ ] Profiler & frame debugger reading the render/GPU telemetry

**AI content pipelines (providers behind traits, results baked as
normal assets — the simulation core stays deterministic)**

- [ ] Text → cutscene: one sentence becomes storyboard, shot list,
      camera keys, blocking and edits on the sequencer; reviewable,
      re-editable by hand, renderable via the path-tracing bake
- [ ] AI sound: SFX and music generation wired into the sound-event
      system (prompt → asset → placed on the timeline or in-game)
- [ ] AI voices: runtime NPC dialogue via LLM providers with budget &
      safety layer, TTS for delivery, lip-sync baked to animation
- [ ] Assistant in the editor: direct in-process agent (Ollama,
      llama.cpp, API) plus the MCP export — same registry, both doors
- [ ] Generation telemetry: cost/latency accounting per tool call so
      an agent run can be capped and audited

## Wiki — documentation in three layers 🟡

- [x] rustdoc on GitHub Pages, rebuilt on every push (`.github/workflows/docs.yml`)
- [x] User handbook EN/DE as mdBook (`docs/handbook/{en,de}`)
- [ ] API browser inside the editor (`cargo rustdoc --output-format
      json` feeding the same registry idea as the agent capabilities)

## Signature feature — seamless perspective switch 🟡

*The engine's reason to exist: strategy overview ↔ unit first-person
with no loading screen, and the same world underneath a shooter or an
MMO. The RTS is the first test project; shooter and MMO follow.*

- [x] Data side of the flight: tiers + streaming + pooling never
      interrupt (M3a/M3b)
- [x] Real file IO behind the same trait (M3c)
- [x] GPU handoff of the flight: triple buffering, cell residency,
      deferred release (`re-world::handoff`, `re-wgpu`)
- [x] Far-field HLOD at the clipmap rim (M3d)
- [x] IO telemetry & adaptive tier rules (`re-world::telemetry`)
- [x] Strategy ↔ first-person handoff mid-flight, no scene rebuild
      (`re-world::ego`)
- [x] Agent-facing world tools: streaming & scene queries on the shared
      capability registry (`re-agent::world`)
- [ ] Camera & timeline layer ([cinema](cinema.md))
- [ ] First playable: RTS vertical slice exercising the full round trip

---

*Update discipline: every merged feature flips its checkbox in this
file and its twin [roadmap.de.md](roadmap.de.md) in the same commit.*
