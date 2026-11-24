# AI directly inside the engine — concept (re-agent)

English | [Deutsch](ki-integration.de.md)

Two questions, one base pattern:

1. *Can the AI (local models or the ChatGPT/Claude API) work directly in
   the editor, without the MCP detour?*
2. *Can the things 3D-Gen-Studio does (image → animated 3D character,
   mesh editing like in Blender, movable building parts) happen in one
   program that the AI merely forwards?*

Both yes — and both are the same system: **a registry for engine
functions** that lives in-process.

## 1. One core, two access roads

```
                          ┌───────────────────────────────┐
                          │  RE editor / engine process  │
                          │                               │
  Provider trait  ◄──────►│  CapabilityRegistry           │◄────► MCP server
  (re-agent)              │  (every engine function =     │      (re-mcp,
   · Ollama               │   a named tool with           │       fallback for
   · llama.cpp            │   JSON schema)                │       Claude Desktop,
   · Strata/OpenAI-comp.  │                               │       OpenCode & co.)
   · Anthropic            │  run_agent(provider, reg…)    │
                          └───────────────────────────────┘
```

* **Direct (preferred):** editor and AI run in the same process.
  `run_agent` passes the tool list to the model, the model answers with
  tool calls, and the registry executes them **with zero transport
  cost** — direct access to scene, meshes, undo stack, filesystem.
  HTTP endpoints are then *just details* (Ollama :11434,
  llama-server :8080, OpenAI-compatible proxies such as Strata, the
  Anthropic API).
* **MCP (stays supported):** `re-mcp` exports exactly the same registry
  outward. Where no direct connection is possible or desired (another
  machine, a third-party app), the AI still drives the editor.
  No engine function is ever written twice.

Security: a `PermissionHook` decides before every writing tool whether
the AI may act (suggest / ask / automatic). API keys never live in the
project, only as a reference to an environment variable.

## 2. "3D-Gen inside the engine" — what belongs where

3D-Gen-Studio = services (GPU, models) + UI + files. RE pulls the roles
apart. Base decision (motivation and details: [`vram.md`](vram.md)):
**generation preferably runs in the same process**, because only a
single VRAM budget can be managed if editor and generator are not to
fight each other over scarce VRAM:

| Capability (3D-Gen-Studio) | RE building block | Where does it run? |
|---|---|---|
| Image → mesh (Tripo/Tencent-style generators) | `GenService` trait: in-process runner **preferred**, HTTP/cloud adapters optional | same process (weights loaded/evicted on demand) — or a second machine |
| Text → image, image editing | `GenService` (image), likewise in-process/optional service | same process |
| Retopo / UV / repair / bake | `GenService` (mesh processing) or CPU paths in `re-geom` | engine (CPU) or GPU module with budget |
| Auto-rig / rig transfer | `GenService` + `re-core::skeleton` | engine + optional service help |
| Editing a mesh **afterwards** (vertices, extrude, pivot, mirror, boolean approaches) | editor tools on `re-core::Mesh` | **engine, CPU, deterministic** |
| Movable elements (doors, cranes, portcullises) | sockets/joints: `Skeleton` instances or a `RigidJoint` component on the part | **engine** |
| Playing/retargeting animation | `re-core::animation` + trajectories | **engine** |
| Assembling everything + correcting | `CapabilityRegistry` + agent loop | **engine** |

Important: 3D-Gen-Studio's generator code is closed — RE does not copy
it but addresses it (and ComfyUI and cloud APIs) through its own
`GenService` interface. The engine stays open; the service is
swappable. Target state: open pipelines (e.g. TripoSG/Trellis) run as
in-process runners directly in the engine and report to the VRAM budget
(`vram.md`) — external services remain as adapters for second GPU
machines and cloud models.

### Register services instead of installing them

Preinstalled generation services (ComfyUI, mesh/rig tools) are **not
installed or shipped by the engine** but registered in the project as
endpoints — exactly the way 3D-Gen-Studio keeps its ComfyUI instance in
the application settings by address and port:

```json
{
  "services": {
    "comfyui":   "http://127.0.0.1:8188",
    "meshtools": "http://127.0.0.1:8200",
    "rigtools":  "http://127.0.0.1:8300"
  }
}
```

Rules:

* At project start the engine only checks reachability (e.g. ComfyUI
  `GET /system_stats`) and shows or hides generation capabilities in
  the registry — it never starts or installs foreign programs unasked.
* Anyone with their own instance (or one on a second machine) just
  enters the address; the project stays portable, the service remains
  the machine's business.
* API keys still do not live in the project but as references to
  environment variables (section 1).

### From one mesh to movable units (the tank case)

Image→mesh delivers ONE mesh (hull, turret, wheels — a single lump).
Movable parts need their own units, WITHOUT regenerating:

* `mesh.split_by_parts` — separate loose islands automatically into
  child nodes (often the case with generated assets: the turret sits
  physically detached).
* `mesh.split_region` — manually by selection (editor brush/box or an
  AI predicate: "all faces above the RING with radius r").
  UVs and texture reference stay identical — only the node structure changes.
* `rig.add_socket(pivot, axis)` — one rotation axis per part (turret-Y
  at the ring, cannon-X, wheel axes); `rig.attach(part, socket)`.
* `rig.rigid_parts` — one joint per loose part, the fast path for
  robots/vehicles.
* Skinning alternative: region → bone instead of splitting (one mesh,
  performance); mixable with sockets.

The outliner still shows the tank as ONE group; AI chain:
generate → split → sockets → anim. Mis-selections are corrected by the
human in the editor (M5) or the AI re-picks the region — it never
regenerates.

### How the AI works (target picture, M6)

"Build a windmill with rotating wings":
1. The AI creates a building mesh + wing mesh via `GenService`
2. imports them into the scene (`scene.import_glb`)
3. sets sockets: `rig.add_socket(body, "wing_axis")`, `rig.attach(part)`
4. adds an animation: `anim.add_twist("wing", axis, 90°/s)`
5. verifies by itself: `scene.stats`, `render.offscreen` → image back
   into the conversation, correction loop until it fits

Every step is a registry tool; the human sees everything live in the
editor and can intervene at any time (undo, permission hook, manually
keep modeling the mesh).

## 3. Implementation steps

* **M1 (now):** `re-agent` as scaffold — registry, provider traits,
  agent loop, `SceneDigest` (tests green). `re-wgpu` renders offscreen.
* **M4 (editor view):** HTTP transport for the providers (a small
  blocking HTTP), editor viewport, first capabilities (scene/material/
  import). The MCP adapter exports the same list.
* **M5 (Blender-lite):** mesh-editing tools (select/move/extrude/spin/
  mirror/pivot, UV basics) as capabilities — that makes "rebuild
  afterwards" AI-operable.
* **M6 (gen loop):** `GenService` adapters (ComfyUI/3D-Studio API/
  cloud), sockets/joints + auto-rig, image-back-to-the-AI correction
  loop.
