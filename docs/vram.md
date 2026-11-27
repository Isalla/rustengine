# One process, one VRAM budget

English | [Deutsch](vram.de.md)

## Why everything runs in ONE program (the starting problem)

Typical working environment: a 3D editor + an external
image→mesh generator (ComfyUI/Tripo/3D-Gen-Studio) as separate
programs. On scarce consumer cards (12 GB) both compete for the same
VRAM — each keeps its reserves permanently occupied, the editor crashes
with "VRAM too low" even though only one of the programs is actually
computing.

RE's solution: **engine, editor, AI hookup and generation share one
process and thus a single VRAM household.** Only one program can decide
*when* which data may leave the graphics card:

* The generator (diffusion/DiT weights, 4–8 GB) is **no permanently
  resident service** but a module loaded on demand and **thrown out of
  VRAM** again after the job.
* Editor reserves (texture mip levels, backups, bake intermediate
  buffers) are **recoverable**: the RAM copy stays, the GPU copy is
  destroyed and re-uploaded when needed.
* AI inference (local: Ollama/llama.cpp or embedded runners) reports
  its needs to the same budget.

External service processes remain possible (ComfyUI on a second GPU,
cloud APIs) — but they are an option, not the base architecture: what
can run in the same process runs in the same process.

## Budget manager (design, M5/M6)

In `re-render` (backend-independent, carried out by
`re-wgpu`/Vulkan):

```
VramBudget
  ├─ total_bytes            (per device limit/query, configurable)
  ├─ reservations           (module → reserve: editor | gen | inference | headroom)
  └─ tracked resources      (every texture/buffer with: byte size, priority,
                             RAM mirror yes/no, last used)
```

Rules:

1. **Every GPU resource is registered** — allocation goes through the
   manager only (`create_texture_tracked(desc, prio, mirror)`).
   No mirror, no evacuation.
2. **Evacuation is visible, not magic:** on budget pressure the manager
   sorts by priority × age and *destroys* GPU copies (`texture = None`,
   RAM data stays). On next access the normal upload path reloads.
   Visible symptom in the editor: one frame of flicker, never a crash.
3. **Gen runs reserve up front:** `Gen.begin(bytes)` evacuates editor
   material until the reserve is reportably free; if that fails
   (because everything is high-priority) the result is an *error with
   numbers* ("need 6.2 GiB, 3.1 GiB free — what may yield?"), not an
   OOM crash. `Gen.end()` releases; editor material comes back lazily
   on next access.
4. **Priorities** (fixed, documented): the editor's running render
   resources = high; mip levels beyond display need, bake intermediate
   buffers, preview caches = medium; gen weights = high *during* a job,
   otherwise 0 (RAM/disk); inference KV cache = high while answering.
5. **Web target:** the browser manages VRAM itself; there the manager is
   only a budget counter + attribution log (no evacuation) so behavior
   does not fundamentally differ between platforms.

## Consequences for existing commitments

* `docs/ki-integration.md` §2: `GenService` stays the **interface**, but
  the recommended implementation is **in-process** (open models, e.g.
  burn/candle runners for image→mesh / image→image). HTTP/cloud
  adapters remain as `GenService` variants for second GPUs and large
  models.
* Resource creation in `re-wgpu` gets the tracking hook before large
  scenarios (map textures) are built — otherwise the manager later
  cannot evacuate what it never saw.
* The cinema editor (M7) is included: preview renderers lower their
  budget when a gen job starts (same process, same rules).
