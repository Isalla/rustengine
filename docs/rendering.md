# Rendering and effects pipeline

English | [Deutsch](rendering.de.md)

Plan of the visual engine: from the current rasterization (M1) through
modern pipelines to ray/path tracing. All under one roof: HDR pipeline
throughout, every stage swappable, the fallback chain downward always
playable — even in the browser.

## 0. Foundations carried by every stage

* **HDR pipeline:** scene values physical (nits), tonemapping (ACES,
  AgX as alternative curve) as its own post pass, exposure automatic
  with eye adaptation and manual override.
* **Temporal data mandatory from M5:** depth and motion-vector buffers
  are part of the pipeline because upscalers, GI, reflections and
  reconstruction build on them.
* **Quality-tier system:** every feature reports which tier (WebGPU /
  deck/integrated / desktop / desktop+RT) it needs; the project
  renderer assembles a pipeline profile from that.

## 1. State M1/M2 (rasterization V1) — ✅

* Forward+ with clustered lights, cascaded shadows, post chain (bloom,
  tonemap), glTF materials; offscreen and window over
  `GfxBackend`/wgpu (Vulkan/Metal/WebGPU, DirectX via wgpu).

## 2. Pipeline V2 (M5) — deferred HDR and shadows 2.0

* G-buffer deferred lighting (albedo, normal, material IDs,
  depth+motion) for many dynamic lights; forward patch for
  transparent/emissive objects.
* **G-buffer layout — decided ([ADR-0003](adr/0003-gbuffer-first-visibility-buffer.md)):**
  Box 1 ships the classic MRT G-buffer (albedo+metallic, octahedral
  normals, roughness/material, depth+motion) — the only surface source
  every tier including WebGPU can run. The lighting pass reads through
  one abstract surface-fetch interface; the **visibility buffer**
  (instance+primitive ID, materials from bindless arrays) joins later
  as a second, desktop-tier surface source alongside the GPU-driven
  path below — same lighting, ~60 MB → ~8 MB geometry traffic at 1080p
  under the strategy camera.
* **Render graph (implemented, M5):** the pass chain (cull → G-buffer →
  lighting → post → FSR1) is declared once as data in `re_render::graph`;
  `re-wgpu` compiles it per configuration and its executor follows the
  compiled order — the code no longer hardcodes the chain (the sorter
  runs light culling before geometry; both are independent).
  Transitions ship with the plan: implicit on wgpu (the backend derives
  them from attachment and bind use), explicit data for later backends.
  Texture aliasing follows with a dedicated target pool — the
  lifetimes are computed, wgpu still allocates. The shadow pass joined
  as a conditional node with Shadows 2.0 (present only when shadows
  are configured; the executor addresses passes by resolved name, not
  fixed index).
* **Shadows 2.0 — cascaded (implemented, M5):** one depth layer per
  cascade in a `Depth32Float` 2D-array texture, sphere-fitted with
  texel snapping, matrices computed per frame by the CPU core
  (`re_render::shadow` — the tap-for-tap reference). The lighting pass
  reconstructs world positions from depth and samples 3x3 PCF through
  a hardware comparison sampler (`LessEqual`: reference ≤ texel —
  exactly the core's `compare_bilinear` direction; the map is stored
  in the core's row convention via a flipped light matrix). The test
  gate proves map depths against the analytic projection and every
  floor pixel against core PCF on the readback map. Screen-space
  shadow-map atlas and virtual-shadow-map-style refinements remain
  later stages (they need mesh streaming from M3); point lights stay
  unshadowed until then.
* **GPU-driven draw path (after the core boxes):** bindless
  texture/material arrays (wgpu `TEXTURE_BINDING_ARRAY` + non-uniform
  indexing where supported), compute culling (frustum + Hi-Z occlusion
  pyramid) writing indirect draw args; the CPU submits one multi-draw
  per mesh batch. Falls back to the classic per-batch path wherever the
  features are missing.
* Watchlist: meshlets / mesh shaders (Nanite-style geometry) are still
  experimental in wgpu; the VSM refinement above is the entry point once
  they mature.
* SMAA + optional TAA; everything in WGSL over wgpu. Debug path:
  RenderDoc frame capture works out of the box on the native
  Vulkan/DX12/Metal paths.
* **TAA quality doctrine (product requirement: no blur, no ghosts).**
  TAA is the pass where sloppy work is paid for twice — too much
  accumulation makes ghosts, too hard a clamp makes mush. Binding
  rules for the GPU implementation (CPU reference:
  `re-render::temporal`):
  * Cap the accumulation window (≤ 8–16 effective samples) and reset
    fully on disocclusion — an unbounded history is blur source no. 1.
  * History fetch must not blur: dithered or bicubic resampling, never
    plain bilinear (double-blur is the classic TAA mush).
  * Clamp in YCoCg with a variance box plus extension toward the
    history (Jimenez 2014) — hard RGB boxes shimmer on contrast edges.
  * Velocity must be sub-pixel accurate (object + camera motion,
    reverse Z); wrong velocity ghosts in a way no clamp can hide.
  * A sharpening pass (CAS-style) after the temporal resolve restores
    edge contrast — sharpening belongs to TAA, not to the optional
    extras.
  * Quality gates before any TAA version ships: reference scenes
    (pan, rotation, fine geometry — fence/grass/powerline) measured
    against a supersampled ground truth; no ghost pixels above a
    delta threshold, sharpness within a few percent of the ground
    truth. The concrete numbers are set by the first implementation
    and stay in this file.
* **Upscaling adapter (`re-upscale`)** — trait, backends as modules:
  * **FSR 1 (EASU + RCAS) — implemented** (M5): CPU core
    `re_render::fsr` (port of AMD `ffx_fsr1.h`, MIT) + WGSL mirror in
    the deferred chain behind the tone map. Pi-tier path: internal
    1/scale render, EASU + RCAS to display, 8-bit intermediate exactly
    like AMD's. The WGSL mirrors the CPU core tap-for-tap (pixel-diff
    gate); the quality gate requires FSR1's MAE against the native
    frame to beat bilinear.
  * **FSR 2/3** (temporal + frame generation): AMD ships MIT-licensed
    shaders; ported to WGSL for all platforms, native timing on desktop.
  * **FSR 4 / „Redstone“** (ML super resolution, ray regeneration, frame
    generation): open source (MIT) but hardware-locked to RDNA 4; native
    backend only.
  * **XeSS**: runs via the shipped SPIR-V dispatcher; native backend
    only, not WebGPU. XeSS 2's frame generation is Arc-locked.
  * **DLSS (super resolution, ray reconstruction, frame generation)**:
    proprietary NVIDIA SDK, native backend only (`re-native`), optional
    feature build; the SDK's license terms remain the user's concern.
    Current generation: DLSS 4/4.5 (RTX, multi-frame generation);
    reference integration: `dlss_wgpu` (bevyengine) shows the wgpu
    route works.
  * No vendor approval is required for any adapter — the constraints
    are license type, hardware lock-in and platform. Proprietary
    binaries are never vendored in this repo: game projects supply the
    SDK path themselves (as UE/Unity handle it).
  * No upscaler is required: native 1:1 rendering stays a mode.
* Motion-vector and depth formats kept compatible with FSR2/3 from the
  start (reverse-Z convention documented).
* **Texture compression is an offline decision (asset bake, M13):**
  GPUs never read raw RGBA — they read block-compressed formats, and
  which block family a GPU decodes in hardware is platform law:
  desktop ships **BC7/BC3** (the DXT/S3TC line), mobile and Raspberry
  Pi (V3D) ship **ETC2** in hardware — DXT is *not* a Pi feature, it is
  the desktop family — modern mobile also **ASTC** (not in v3d). One
  source asset, one baked variant per device profile; **Basis
  Universal** as the optional single-source route (one file, transcode
  to the target format at load). Compressed textures stay compressed
  in RAM/VRAM and decode free on read — on a bandwidth-starved GPU
  like the Pi's this is a frame-rate feature, not just a memory one.
  The M13 profile checker flags uncompressed or wrongly-targeted
  textures by name; the Pi-tier upscaler pairing is FSR1 (see above).

## 3. Dynamic GI and reflections (M6) — "Lumen-like"

Two paths sharing one hookup to the surface data:

* **Software ray tracing (default, runs everywhere):** mesh distance
  fields per object + a global SDF clipmap around the camera; a
  surface cache (texel binding) keeps lit surface data ready.
  Diffuse GI via raymarching against SDFs, screenspace supplement for
  near details.
* **Hardware ray tracing (optional, desktop+RT):** ray queries /
  raytracing pipeline over the native backend (Vulkan); once wgpu
  extensions are production-ready the driver moves there — the
  algorithms stay identical.
* Reflections: chain of screenspace → SDF/RT rays → light-probe share;
  every hit lands in the surface cache so GI and reflection share the
  same energy.
* Volumetric fog & light shafts: a froxel grid (3D texture) injected per
  cluster by compute — atmosphere on every tier, low-resolution on
  WebGPU.
* Radiance Cascades: compute-only soft GI and shadows as a cheap tier
  beside the SDF path — no ray tracing, browser-capable.
* WebGPU tier: probes + SSAO/SSR, no rays; identical materials, lower
  effect resolution.
* References: "Unreal Engine 5 Best Practices" and the "UE5
  Performance Optimization Cookbook" for operating parameters; paper
  knowledge (The Order: Real-time Global Illumination) enters as
  technique.

## 4. Path tracing and reconstruction (M7)

* **Progressive path tracer** as its own renderer mode (PC/console
  backends): the same scene, BVH from the streaming system, many
  samples across frames; the goal is "offline quality" in the editor
  for bakes and stills — in the game the M6 path keeps running.
* **ReSTIR-like importance reuse** and temporal accumulation against
  noise; then **ray reconstruction** as a denoiser stage:
  * NVIDIA SDK (DLSS-SR/RR) as optional native backend,
  * open fallback with a self-built spatio-temporal reconstructor.
* Bake outputs (lightmaps, SDFs, HLODs) are produced by a Go build
  farm (section 2 in [architecture.md](architecture.md)) so the engine
  UI stays responsive while computing.

## 5. VFX: particles and effects (M8)

* GPU particles (compute): emission, forces, collision against SDFs and
  heightfield, ribbon and mesh renderers; effects as an editable node
  graph in the editor (Niagara-like, data-driven).
* Screen-space effects library: distortion, heat shimmer, explosions,
  hit flashes — a recipe collection from the "UE5 VFX Cookbook"
  translated into the engine's own renderer terms.
* Budgets per effect class (count, VRAM, fill) run through the
  capability registry so AI and tools respect them.

## 6. Audio and SFX (M8)

* Rust runtime mixer: buses, sidechaining, reverb zones, 3D
  spatialization; sample playback with streaming from the asset DB.
* **SFX as events, not files:** events (tree/graph) trigger layers,
  random variants, pitch-volume spread — editable like particle
  graphs, fired by animation events and gameplay signals.
* Console/headless tests: a silent backend that only validates graphs.

## 7. Animation and character (M8)

* Skeletal animation (accelerated, GPU skinning), blend trees and state
  machines as editable graphs; "UE5 Character Creation, Animation and
  Cinematics" as workflow reference.
* For the signature feature: the same unit must exist as squad
  billboard, game mesh and first-person camera carrier — the tiers
  share transform and state machine.

## 8. Web path (M9, continuously testable)

* wgpu→WebGPU: pipeline V2 with SMAA/TAA and spatial upscale, GI probes
  instead of rays; the target is a playable strategy-game prototype
  including the camera flight (quality tiers work there too).
* Go services stay server-side; the browser is a client like any other.

## 9. Milestones of this pipeline

| Stage | Content | Gate |
|---|---|---|
| M5 | deferred HDR, shadows 2.0, `re-upscale` (NIS + FSR2/3 port), motion/depth, render graph, GPU-driven draw path | identical scene playable on deck and desktop |
| M6 | GI/reflections (SDF + surface cache, HW-RT option), volumetric fog, light editor | interior scene lit correctly without bakes |
| M7 | path tracer + reconstruction, bake outputs | still image in offline quality from the game scene |
| M8 | GPU particles + effect graphs, audio mixer + SFX events | test scene: gunfire with light, smoke, sound |
| M9 | WebGPU build of the signature feature | strategy→first-person camera flight in the browser |

## 10. Realistic limits

* The DLSS/XeSS SDKs are hardware- and license-dependent — they are
  adapters, never a precondition; the engine works fully without them.
* Hardware RT under wgpu is a moving target; the SDF path is
  deliberately not a stopgap but the portable base solution.
* Large scenes cost memory: the VRAM-budget rules from
  [vram.md](vram.md) also apply to GI intermediate buffers.
