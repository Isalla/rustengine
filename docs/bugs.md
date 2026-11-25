# Bug hunt — checklist, conventions, findings log

English | [Deutsch](bugs.de.md)

Purpose: hunt bugs in the engine **on purpose** — systematic passes
over the known fragile corners — and keep one curated list of what was
found. GitHub Issues is the intake channel; this file is the engine's
truth: every confirmed bug gets an ID here, and no fix ships without a
regression test in the same commit.

## Conventions

- **ID:** `BUG-###`, assigned when confirmed, never reused.
- **Severity:** `crash` · `wrong-result` (silent — the dangerous kind)
  · `perf` · `cosmetic`.
- **Status:** `open` → `fixing` → `fixed(<commit>)` · `wontfix(reason)`.
- Every row needs a minimal reproduction — a failing test counts and is
  preferred.

## Hunt checklist (systematic passes)

Run per milestone, or when a subsystem gets touched. Each line is a
question to ask of the code, not a vibe.

**Determinism (`re-world`, `re-render` math)**
- [ ] Same input twice → byte-identical output? (run any demo twice,
      diff the artifacts)
- [ ] Any hidden iteration order over `HashMap`/set that leaks into
      output or draw order?

**Streaming & IO**
- [ ] Zero-size inputs: 0×0 render target, empty cell file, truncated
      file mid-table
- [ ] Budget exhaustion: does eviction make progress or thrash?
- [ ] Slow-disk simulation (telemetry spike): degrade, never stall the
      render loop

**Renderer**
- [ ] NaN/Inf pixels under extremes: exposure 0 and 1e9, degenerate
      triangles (zero area), normals at the octahedral poles
- [ ] Resize storms: window to 1×1 and back mid-frame, no panic, no
      stale history (TAA must reset, see rendering.md doctrine)
- [ ] Motion vectors on first frame (no previous view-proj)

**Editor & undo**
- [ ] Undo across boundaries: undo after import, after project switch,
      after crash-recovery load — stack must stay consistent
- [ ] Redo truncation on new command after partial undo

**Scale & limits**
- [ ] 10k entities, 1k lights, 64 clusters/slice: no O(n²) surprises in
      per-frame paths
- [ ] Large world coordinates (1e6 m): float precision — is anything
      subtracting big positions without anchoring?

**API misuse**
- [ ] Caller errors are `Result`, never panics in library code;
      panics reserved for invariant violations with a message

## Findings log

| ID | Date | Area | Severity | Symptom | Root cause | Status |
|----|------|------|----------|---------|------------|--------|
| — | — | — | — | no findings recorded yet | — | — |

*The log stays honest: `fixed` rows keep their root cause — the next
milestone's hunt pass reads them as candidate patterns.*
