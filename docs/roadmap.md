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

## M3 — ECS & world streaming 🟡 ← current focus

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
- [ ] **M3c** GPU handoff: payload → staging buffer → VRAM, triple
      buffering so render and update never wait for upload
- [ ] **M3d** HLOD: baked far-field summary meshes at the clipmap rim
- [ ] **M3e** First-person handoff: control transfer mid-flight without
      scene rebuild
- [ ] MCP tools: agent-facing capability bindings for streaming and
      scene queries (builds on the `re-agent` registry scaffold)

## M4 — Editor ⬜

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

## Wiki — documentation in three layers ⬜

- [ ] rustdoc on GitHub Pages, rebuilt on every push
- [ ] User handbook EN/DE as mdBook
- [ ] API browser inside the editor (`cargo rustdoc --output-format
      json` feeding the same registry idea as the agent capabilities)

## Signature feature — seamless perspective switch 🟡

*The engine's reason to exist: strategy overview ↔ unit first-person
with no loading screen, and the same world underneath a shooter or an
MMO. The RTS is the first test project; shooter and MMO follow.*

- [x] Data side of the flight: tiers + streaming + pooling never
      interrupt (M3a/M3b)
- [x] Real file IO behind the same trait (M3c, in progress)
- [ ] GPU side of the flight (M3c upload, M3d HLOD)
- [ ] Camera & timeline layer ([cinema](cinema.md))
- [ ] Control handoff (M3e)
- [ ] First playable: RTS vertical slice exercising the full round trip

---

*Update discipline: every merged feature flips its checkbox in this
file and its twin [roadmap.de.md](roadmap.de.md) in the same commit.*
