# ADR-0003: G-buffer first, visibility buffer as second surface source

English | [Deutsch](0003-gbuffer-first-visibility-buffer.de.md)

- **Status:** Accepted
- **Date:** 2026-11-25
- **Deciders:** Waldemar Friesen (product), RE engine team
- **Relates to:** [rendering.md](../rendering.md) (§2, pipeline V2),
  [ADR-0001](0001-modular-editor-frontend.md) (tiers & presets aside —
  here: fallback doctrine), roadmap M5 (deferred HDR, GPU-driven draw path)

## Context

Deferred lighting needs per-pixel surface data (albedo, normal,
roughness, material parameters). Two proven suppliers exist:

**Classic MRT G-buffer.** The geometry pass writes several render
targets (≈ 12–16 bytes/pixel). Lighting reads them back as plain
textures. Works on every backend including WebGPU in the browser;
every channel is directly inspectable in RenderDoc. Costs: fixed
channel layout (every pixel pays for every channel), and full
bandwidth for overdrawn pixels — at 1080p roughly 30 MB written and
30 MB read per frame. Our signature view (strategy camera: units,
vegetation and props overlapping heavily) is exactly the overdraw case.

**Visibility buffer.** The geometry pass writes one packed integer
(instance + primitive ID) plus depth (≈ 8 bytes total). The lighting
pass looks up the triangle, reconstructs barycentrics and fetches
material data from bindless arrays. Costs almost nothing under
overdraw and imposes no fixed material layout. But it requires
storage-buffer geometry and texture arrays with non-uniform indexing
— available on Vulkan/DX12/Metal, **not in the WebGPU browser tier** —
and per-pixel material branching needs tile classification to stay fast.

The engine doctrine already decides the frame: every feature needs a
downward fallback chain that stays playable, and the WebGPU tier is a
first-class target (M9). Building both lighting passes separately
would double the maintenance of the most complex shader in the engine.

## Decision

**Sequence, not either/or:**

1. **M5 Box 1 (deferred HDR) ships with the classic MRT G-buffer.**
   Layout: albedo+metallic, octahedral normals (`re-render::gbuffer`),
   roughness/material parameters, depth+motion. It is the only surface
   source every tier — including WebGPU — can run.
2. **The lighting pass reads through one abstract surface-fetch
   interface** (`surface_at(pixel)`), not hardcoded G-buffer texture
   reads. In WGSL this is realized as a shader permutation: the
   lighting algorithm is shared, the surface-fetch stage is generated
   per source.
3. **The visibility buffer joins later as a second surface source for
   the desktop tier**, delivered together with the GPU-driven draw
   path (same prerequisites: bindless arrays, indirect geometry). Same
   lighting pass, different fetch stage — never a second lighting
   implementation.

## Consequences

**Positive**

- One lighting algorithm to maintain; the surface source is a variant.
- WebGPU/browser fallback is guaranteed from day one (G-buffer only).
- The bandwidth win where it matters (strategy-camera overdraw:
  ~60 MB → ~8 MB per frame at 1080p) is available later without
  rearchitecting.
- No fixed material ceiling on desktop: new material features cost
  buffer reads, not G-buffer width.

**Costs / risks (accepted)**

- The surface-fetch abstraction must be designed **now**, before the
  first lighting shader lands — retrofitting it into hardcoded texture
  reads is the expensive mistake this ADR prevents.
- Shader permutation machinery (shared lighting code, generated fetch
  stage) is real plumbing in `re-wgpu`.
- Visibility-buffer debugging (ID images, reconstruction) is harder;
  mitigated by keeping the G-buffer path alive as a reference mode.

## Phasing

- **Now (M5 Box 1):** MRT G-buffer + lighting pass behind the
  surface-fetch interface.
- **M5 GPU-driven box:** bindless arrays + indirect draw + Hi-Z
  (CPU reference: `re-render::cull`); the visibility buffer rides
  along as the desktop-tier surface source.
- **Gate:** the visibility buffer only ships when a scene renders
  identically through both sources (pixel-diff test on the same
  lighting pass).
