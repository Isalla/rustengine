# Architecture: languages, modules, world streaming

English | [Deutsch](architecture.de.md)

This paper records the base decisions that follow from the signature
feature (below). It does not replace details: the rendering pipeline and
light quality tiers are described separately in
[rendering.md](rendering.md). Individual base decisions are recorded as
[Architecture Decision Records](adr/README.md).

## 1. Signature feature: strategy ↔ first-person without a loading screen

The goal is a strategy game that switches seamlessly from the overview
perspective into a unit's first-person view:

* The camera **flies onto the unit**, the view moves into it and hands
  over first-person control along the way — no cut, no fade, no loading
  screen.
* The same world, the same objects: what is a billboard squad from a
  distance is a detailed character in walkable terrain up close.
* Switching back works the same way (leave the unit, the camera flies
  out and becomes the RTS camera again).

What this technically demands from the engine:

1. **Streaming instead of level loading:** all scales (map, building,
   unit, interior) live in one coordinate system and are brought in and
   released on demand (section 5).
2. **Quality tiers per object:** a unit exists simultaneously as a
   billboard batch, as a game mesh and as a detail mesh; the tiers hand
   over smoothly during the flight (section 5.4).
3. **No frame hitches:** transitions run through pooling and detail
   fades, not scene switches; the ECS world is consistent at every point
   in time.
4. **Camera as a first-class object:** spline flight that yields to
   terrain, hand-over of input — the same mechanics as the camera
   timeline in [cinema.md](cinema.md).

## 2. Language split: Rust and Go

| Layer | Language | Examples |
|---|---|---|
| Engine runtime (renderer, ECS, physics, audio, streaming) | Rust | `re-*` crates |
| Editor (UI, scene view, import pipelines) | Rust | `re-editor` |
| Modules and import tools | Rust or any language | `sc1-studio` (sprite import for a classic strategy game) |
| **Services and tools** | **Go** | dedicated game server, matchmaking/lobby, asset build farm, hot loader/launcher, telemetry receiver |

Principles:

* **Go never runs in the frame loop.** Rendering and simulation demand
  deterministic latencies; a garbage collector has no place in the frame
  path.
* **The boundary is the process.** Go services talk to the engine over
  gRPC/protobuf (servers) or CLI + JSON (tools). A crashing build worker
  does not kick out the running game.
* **Server-side game logic in Go or Rust, both intended:** state
  simulation (lobby, account, asset catalog) happily in Go; where game
  rules must be shared 1:1 with the engine, the dedicated server runs as
  a pure Rust process with a headless backend.
* The capability registry (see
  [ki-integration.md](ki-integration.md)) will also describe service
  commands as JSON; a Go tool can operate it just like an AI agent.

## 3. Module architecture: three coupling levels

Extensibility is the core promise — new formats, new imports, new
generators should not have to move into the engine kernel:

1. **In-process module (Rust crate):** registers capabilities and asset
   importers in the registry, uses `re-core` types. Maximum performance,
   maximum integration (example: a future AI content module
   image→mesh).
2. **Out-of-process tool (any language):** a CLI program with a
   documented JSON/file contract (input: folder or manifest, output:
   engine asset). The engine launches it, reads the result and versions
   it as an import-pipeline stage. This is how `sc1-studio` docks in
   today: GRP sprite sequences → RGBA sheet + row plan → engine billboard.
3. **Service (Go, long-lived):** own process with a network interface —
   game server, build farm, translation/LLM gateway. Engine and editor
   are clients. Existing instances (including third-party ones, e.g. a
   preinstalled ComfyUI) are merely registered in the project as an
   endpoint — the engine itself installs and launches nothing.

All three levels register in the same registry (levels 2/3 via an
adapter capability), which is why the editor UI, keyboard commands and
AI agents see extensions identically.

## 4. Data storage and scene format

* Scenes remain the JSON-based `.wbproj` format with content addressing
  for binary data (meshes, textures).
* **Asset DB as an intermediate layer:** import writes normalized assets
  into a project cache (hash → file); the scene references hashes only.
  That makes streaming, re-import and HLOD builds idempotent.
* Go build-farm tools consume the same cache as a plain directory — no
  proprietary database between processes.

## 5. World streaming without loading screens

Modeled on world-partition/HLOD concepts of current engines (reference:
"Building Open World Landscapes with Unreal Engine 5"), built for a Rust
renderer:

1. **Grid cells:** the world is divided into equally sized cell blocks
   (configured per project, typically 64–256 m). Each block contains
   transform lists, collision and sound regions — small enough for
   streaming, large enough to avoid culling overhead.
2. **Clipmap levels:** several levels around the camera with growing
   cell size and falling detail density; far-away terrain renders as an
   HLOD mesh.
3. **Async load pipeline:** an IO thread pool loads block files, a
   worker turns them into GPU resources; hand-over goes through triple
   buffering with resource handles so render and update threads never
   block. Visibility changes only shift priorities, never the scene.
4. **Quality tiers (LOD system "T0..T4")** per instance:
   * `T0` — cinematic/near detail mesh (AI- or artist-made),
   * `T1` — game mesh with animation,
   * `T2` — visual hull/simplified mesh (reconstructed from sprite views),
   * `T3` — billboard batch (32 directions, one draw call per squad),
   * `T4` — icon/marker on the minimap.
   The camera flight (section 1) thus becomes a pure tier-transition
   task: `T3 → T2 → T1 → T0` while flying in.
5. **GPU-driven culling:** instance and triangle culling run on the GPU
   (indirect draw), occlusion via Hi-Z; the CPU only submits batch
   parameters.
6. **Pooling instead of unloading:** unit instances and effect pools are
   reused; resources stay reserved for a few seconds after leaving a
   zone (eviction delay) — the flight back costs no IO.

**Storage requirements.** Every cell is one small file
(`l{level}_x{x}_y{y}.bin`), so streaming reads are small and random —
what matters is IOPS and latency, not sequential throughput. Guidance
for a full clipmap loadout (hundreds of resident cells, fast flights
trigger hundreds of small reads per minute):

| Medium | Verdict |
| --- | --- |
| NVMe SSD | target — full-speed flight keeps every tier warm |
| SATA SSD | minimum — sustained streaming works, fine tiers may lag briefly under maximal demand |
| spinning disk | runs degraded — budget and tier hysteresis hold coarse detail longer; the render loop still never blocks |

The engine never requires a specific disk: the IO pipeline is fully
async (`re-world::io`) and everything slow first becomes
less detail, never a stutter or a loading screen. Planned on top of
that: IO telemetry — measured load latency and throughput drive tier
hysteresis and dwell adaptively, so a slow disk degrades early instead
of dropping cells late (see [roadmap](roadmap.md), M3).

## 6. ECS

* Archetype-based ECS (`re-ecs`, M3): components as plain SoA storage,
  systems as jobs; the renderer reads world data through a view
  interface without intermediate copies.
* Scene-graph nodes (the editing state in `re-core`) are the source of
  ECS instances at startup; editor changes write back.
* Transform propagation runs as a parallelized system pass (references:
  "Data-Oriented Programming", "C++ Concurrency in Action" — patterns
  transferable to Rust).

## 7. Editor

* Immediate-mode UI on the engine's own renderer (one window, one VRAM
  budget; no second GUI toolkit in the composited frame).
* Panels: scene view with gizmos, outliner, inspector (capabilities as
  forms), asset browser with import stages, console.
* **Every UI action is a capability with an undo entry** — the same
  protocol serves mouse, scripts and AI agents; that is the Unreal-editor
  expectation: everything usable, everything automatable.

## 8. Decisions (short ADRs)

* **WGSL stays the only shader source.** For RT and DLSS paths an
  optional native backend (`re-native`, Vulkan) is introduced that can
  additionally compile HLSL/SPIR-V kernels; the default path is
  unaffected.
* **wgpu first, native extensions where needed.** Ray tracing and
  DLSS/FSR3 frame generation need driver access that wgpu cannot
  guarantee today; they run behind feature flags with a screen-space
  fallback.
* **Go stays outside the frame path** (section 2); no FFI in simulation
  or render loops.
* **License MIT**, copyright held by the author; modules inherit this
  model.
