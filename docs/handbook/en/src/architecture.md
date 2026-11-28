# Architecture Tour

## Crates and their jobs

| Crate | Job |
|---|---|
| `re-core` | Data model: scene graph, meshes, materials, animations, terrain, tilesets |
| `re-io` | Scene file I/O: glTF 2.0/GLB, OBJ+MTL; texture loading incl. native DDS (BC1–BC5) |
| `re-ecs` | Entity-component-system: world, sparse-set storage, schedule |
| `re-world` | World partition: grid, clipmap, tiers, streaming, HLOD, telemetry, ego handoff |
| `re-render` | Backend-neutral render layer: `GfxBackend` trait, camera, draw batches |
| `re-wgpu` | The wgpu backend (Vulkan/Metal/WebGPU), offscreen and windowed |
| `re-platform` | Windows, input, event loop (`App` trait), time |
| `re-agent` | Capability registry, AI providers, agent-facing world tools |
| `re-editor` | Editor frontend: document state, undo stack, outliner, project/import capabilities |

Dependencies point one way: `re-world` knows nothing about rendering,
`re-render` knows nothing about windows, and `re-core` knows nothing
about any of it. The binaries in `re-wgpu` (`re-demo`, `stream-demo`)
are where the layers meet.

## The data flow of one frame

```text
re-platform (events, dt)
   └─ re-world   WorldPartition::update(camera, dt)
        ├─ clipmap follows the camera → cell demand
        ├─ streamer loads/evicts within budget (async IO threads)
        ├─ tiers re-grade instances (T0..T4)
        └─ HLOD takes over at the clipmap rim
   └─ re-world   CellHandoff: triple-buffered upload, byte quota/frame
        └─ re-render  DrawBatch list → GfxBackend::render
             └─ re-wgpu  WGSL pipelines, present
```

## Three doctrines

**Headless, deterministic core.** `re-world` contains no threads except
the swappable IO source, and no GPU types at all. Given the same `dt`
sequence and the same camera path, `WorldPartition::update` replays
identically — which is why the streaming core is testable on a
Raspberry Pi before any graphics exist.

**Interfaces, not spaghetti.** Every seam is a trait: `CellSource`
(where cells come from), `GpuUpload` (how they reach the GPU),
`GfxBackend` (what renders), `App` (who drives the loop). Backends and
console SDKs attach at these traits; nothing forks internally.

**One capability layer.** Editor tools and AI tools read the same
`CapabilityRegistry` — see [Capabilities & AI](./capabilities.md).

## Where to read the code

Start at `crates/re-world/src/partition.rs` — `WorldPartition` composes
every streaming subsystem and its module docs state the ordering
guarantees. Then follow `stream.rs` (budget) and `handoff.rs` (GPU
pipeline). The rustdoc for all of this is
[here](../../rustdoc/re_world/index.html).
