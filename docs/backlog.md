# Backlog — parked ideas

Ideas that are wanted but not scheduled. Each entry records the thought
while it is fresh; scheduling happens when a milestone has room for it.
Nothing here is a promise — parked means parked.

## B-1: Snapping that adapts shape (editor)

**Wish (user, 26.11.2026):** when assets snap to each other in the
editor, the engine should deform them far enough that they actually fit
together — no manual adjustment of either asset afterwards.

**Examples:**

- A wall segment stretches/miters its end so it meets the next segment
  exactly, at any angle.
- Pipes, cables or rails auto-connect to the port they snap to.
- Stairs meet a floor without a gap; terrain tiles blend at seams.
- 2D-style equivalent (our MMO case): autotiling — a piece reshapes its
  edges based on which neighbours are present.

**Design constraints (why this fits the engine doctrine):**

- **Rule of double addressability (M11):** snap-and-adapt is a registry
  tool operation. Human and AI must be able to invoke the exact same
  adaptation through the same tools.
- **Non-destructive:** the adaptation is a computed parameter (snap
  constraint + generated variant), never an edit of the source asset.
  The original mesh stays intact and the result is reversible.
- **Deterministic:** same inputs (asset, neighbour, snap point) → same
  output mesh, so it works in procedural worlds and replays too.

**Known relatives (for later research):** 2D autotiling / Wang tiles,
mesh bridging/stitching, parametric assets (shape grammars, CityEngine
style), UE snap-to-surface + procedural mesh components.

**Status:** parked — build after the editor's basic framework stands.

## B-2: Blackwell tech demo (neural tier, vendor sidecar)

**Wish (user, 26.11.2026):** once the engine is done, stand up a tech
demo that addresses the RTX 5000 (Blackwell) architecture — its new
low-precision tensor formats (FP4/MX block-scaled, FP8; note: BF16 is
Ampere-era, int8 older) — to test what the engine can do at the
high end and what genuinely cool gaming features those units enable.

**Honest constraint:** wgpu exposes no tensor cores / FP4 /
cooperative-matrix extensions. These need raw Vulkan
(`VK_NV_cooperative_vector`, `VK_KHR_cooperative_matrix`) or D3D12 —
so the demo must be a **vendor sidecar** (native module or inference
runtime feeding textures/results back into the pipeline), never a
requirement. Device profiles (M13) get an optional *neural tier*;
the portable pipeline and the Pi stay untouched by it.

**Demo candidates (ranked by MMO payoff):**

1. Neural texture compression — decode via FP4 inference; several
   times more world per VRAM byte (pairs with the compression
   doctrine in rendering.md).
2. Radiance caches — bake GI by training, sample it nearly free.
3. Self-built ML upscaling / frame generation as a learning project
   around DLSS-style pipelines.
4. Local NPC inference — small models in FP4 on the consumer card.

**Status:** parked — after the engine core is feature-complete and a
Blackwell card is actually on the bench.
