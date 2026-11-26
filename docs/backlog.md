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
