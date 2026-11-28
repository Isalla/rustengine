# Milestone roadmap

[English](roadmap.md) | [Deutsch](roadmap.de.md)

The full checklist between „clone the repo“ and „ship the game“, with
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

- [x] Scene view with live camera into the streaming world
      (`re-editor` — free-fly camera, streaming world follows the
      viewpoint; windowed + headless smoke path)
- [x] Outliner & inspector over the ECS (`re-editor::outliner` — `ComponentRegistry` with type-erased JSON access, `Outliner`, `ecs.entities/entity.components/entity.get/entity.set` capabilities; `re-ecs` `World` made `Send`)
- [x] Import pipeline wrapping `re-io` (`re-editor::import` — `import_gltf`, `import.preview`/`import.gltf` capabilities, Roundtrip-Test)
- [x] Undo/redo as reversible systems (`re-editor::undo` — `Command<T>` trait, `CommandStack` with cursor, built-in commands for camera/selection/view-mode, `editor.undo/redo/history` capabilities)
- [x] Worldbuilder project loader (`re-editor::project` — `ProjectDoc` mit Pfad/Dirty, `project.new/load/save/close/info` capabilities, Roundtrip-Test)
- [x] Editor frontend P1: widget toolkit (draw list, text atlas, input
      routing, docking manifest, theme) + the `default` preset with the
      engine's own identity (`re-render::ui` widgets, `re-editor::layout`
      manifest + `presets/default` data, `re-editor::panels` over
      `EditorContext::call`; windowed overlay via `render_with_ui` +
      `--ui-png` CPU proof; data-driven per ADR-0001; the `ue5`
      preset (P2) follows later
- [x] Docking interaction on the default layout (grab splitters to
      resize panels, grab tabs to redock with edge/center drop zones —
      `dock_splitters`/`dock_set_ratio`/`dock_take`/`dock_put` in
      `re-render::ui`, `drop_zones` + drag ghost in `re-editor::panels`;
      OS-level floating windows deferred)
- [x] Direct value editing in the inspector (click a number field to
      type, drag the property name to scrub — Shift for fine steps;
      `Ui::drag`/`Ui::scrub` in `re-render::ui`, editable number leaves
      in `re-editor::panels`; every write stays on the
      `ecs.entity.set` capability path)
- [ ] More file formats (breadth over single-tool migration):
      registry-driven importers/exporters — FBX (meshes & animation),
      OBJ legacy and USD (scene interchange) on top of the existing
      glTF path. Open standards only; content from other engines
      arrives through their official exporters (UE/Unity export
      FBX/USD/glTF themselves). Proprietary scene formats
      (`.uasset`/`.umap`, `.unity`) stay unsupported — parked as an
      idea until the vendors publish open formats; game code never
      migrates anyway

## M5 — Render pipeline V2 ⬜

- [x] Deferred pipeline: MRT G-buffer (`deferred_v1`) + lighting pass over
      the abstract `surface_at` interface — GPU output pixel-diff verified
      against the CPU reference (ADR-0003, `re-wgpu/tests/deferred.rs`)
- [x] HDR float targets + tone mapping: the lighting pass writes an
      RGBA32F target (values > 1.0 survive), a tone-mapping pass mirrors
      the `re-render::hdr` curves in WGSL (ACES/Reinhard; AgX pending
      its mirror) — pixel-diff verified against `HdrPipeline::process()`
- [x] Clustered point lights in the deferred lighting pass: the shader
      finds each pixel's cluster (tile x log depth slice, depth
      reconstructed from the depth buffer) and sums its lights —
      pixel-diff verified against the CPU reference through the HDR
      target and the tone-mapped output
- [x] GPU compute light culling: a compute pass mirrors
      `cluster_view_aabb` + `sphere_intersects_aabb` and writes the
      per-cluster light lists — verified index-for-index against
      `cull_lights()` on the CPU across every cluster (the CPU core
      remains the test reference)
- [x] Shadow 2.0 cascaded: the GPU depth pass mirrors
      `re-render::shadow` tap-for-tap — one depth layer per cascade in
      a 2D-array map, sphere fit with texel snapping (computed by the
      CPU core per frame), 3x3 PCF through a hardware comparison
      sampler; map probes and every floor pixel verified against the
      CPU reference on the readback map
- [ ] Shadow 2.0 clustered: shadow-casting point lights via the
      cluster lists ([rendering](rendering.md))
- [x] Upscaling adapter FSR1 (EASU + RCAS) behind the deferred chain's
      tone map — the Pi-tier path (M13): render internally at 1/scale,
      upscale to display. The WGSL mirrors `re_render::fsr` (port of
      AMD `ffx_fsr1.h`, MIT) tap-for-tap, pixel-diff verified, and the
      quality gate proves FSR1 beats bilinear against the native frame
- [ ] Desktop-tier adapters: FSR2/3 (temporal), XeSS, DLSS
      ([rendering](rendering.md))
- [x] Render graph: the deferred chain (cull → G-buffer → lighting →
      post → FSR1) is declared once as data (`re_render::graph`); the
      `re-wgpu` executor follows the compiled plan — the execution
      trace proves the order, the pixels stayed identical (every
      pixel-diff gate still green). Transitions ship with the plan
      (implicit on wgpu, explicit data for later backends); texture
      aliasing follows with a target pool — the lifetimes are already
      computed
- [ ] GPU-driven draw path (after the core boxes): bindless texture/
      material arrays, compute culling (frustum + Hi-Z occlusion) writing
      indirect draw args, one multi-draw per mesh batch

## M6 — Dynamic GI & reflections ⬜

- [ ] Lumen-style software ray-tracing path
- [ ] Hardware RT as optional quality switch
- [ ] `re-native` backend for vendor RT stacks
- [ ] Volumetric fog & light shafts (froxel grid, compute-injected per
      cluster; low-res tier runs on WebGPU)
- [ ] Radiance Cascades as a cheap GI tier (compute-only soft GI/shadows,
      no hardware RT, browser-capable)

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
      (per-pass GPU timing via wgpu timestamp queries)

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

## M12 — Procedural worlds (runtime + editor) ⬜

*Rising World and Minecraft proved that players accept a generated
world as the game itself. The engine's deterministic headless core
(`re-world`) is built for exactly this — with one design rule from the
product owner: **the authored description is the input, the seed is
derived from it** (a hash of the parameter document), never the other
way around. No seed lottery.*

- [ ] Description-first generation: the author defines intent — biome
      maps, feature rules („river from the mountains to the southern
      sea"), density graphs, control bitmaps. The seed is *derived*:
      deterministic hash of the canonical parameter document, so the
      world ID is a content address. Variations = hashed counter
      suffix („ten more like this“). Per-layer hash namespaces keep
      tweaking stable: editing one section re-rolls only that layer's
      random stream, the rest of the world stays identical. Targeted
      edits work like inpainting in image generation: the control
      bitmaps are the mask — a painted change re-rolls only the chunks
      that read it (plus a blend margin), never the whole world
- [ ] Deterministic generator core: noise fields and rule evaluation
      on the derived streams — reproducible across platforms, so
      multiplayer syncs the description (or its hash) instead of
      geometry
- [ ] Runtime chunk generation: the streamer treats generated cells
      like file-backed ones (same `CellSource` trait) — infinite
      terrain under the existing budget, tier and HLOD rules
- [ ] PCG graphs as ECS schedules: spawners, biome rules, WFC-style
      tile assembly — authored once, runs in-game and in the editor
      (double addressability: every generator is a registry tool)
- [ ] Generate → author: hand edits persist as deltas/override regions
      over the generated base; a bake step exports any region to
      static cells for shipping

## M13 — Device profiles & auto-downgrade ⬜

*The product owner's test case: an MMO client that must also run on
Raspberry Pi hardware. The engine rule is the inverse of the usual
one: do not build the game small for the weakest device — build it
right and let the engine scale it down. Honest boundary: a fully
automatic "runs anywhere" is not promised; what ships is detect +
propose + one-click apply, because every step is a registry tool
(double addressability, M11) that human and agent invoke alike.*

- [ ] Device profiles as data: capability manifests (RAM cap, GPU tier,
      target fps, feature flags) — Pi 3/4/5 (V3D), WebGPU, mobile,
      desktop. The same profiles drive the CI render harness, so
      "Pi-tier" is a measurable claim, not a feeling. Render fields per
      profile: internal render scale + upscaler choice (Pi tier = FSR1
      spatial, desktop = FSR2/3/DLSS) + texture target format (Pi =
      ETC2, desktop = BC7, Web = Basis transcode) — see
      [rendering.md](rendering.md) "Texture compression is an offline
      decision"
- [ ] Profile checker: a pass over the project that *reports* what
      breaks the target — textures over the RAM cap, mandatory deferred
      features, particle budgets, entity density, draw-call counts —
      with numbers and the named fix tool per finding
- [ ] Downgrade tool chain (registry tools, each reversible and
      logged): texture mip capping, retargeted asset variants, texture
      bake to the profile's block format (ETC2 for Pi/mobile, BC7 for
      desktop, Basis as single-source route), high-poly → LOD/optimize,
      real-time light → baked, 3D prop → sprite-bake via the iso baker
      (`re-core::iso`), forward fallback path instead of deferred
      (ADR-0003 tiering)
- [ ] Proof loop: run the game against a profile in the headless
      harness — frame time, memory, bandwidth measured per tier; the
      roadmap box flips only when a real project passes on real Pi
      hardware

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
