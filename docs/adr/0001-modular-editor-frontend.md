# ADR-0001: Modular, preset-driven editor frontend

English | [Deutsch](0001-modular-editor-frontend.de.md)

- **Status:** Accepted
- **Date:** 2026-11-25
- **Deciders:** Waldemar Friesen (product), RE engine team
- **Relates to:** [architecture.md](../architecture.md),
  [ki-integration.md](../ki-integration.md) (rule of double addressability),
  [ADR-0002](0002-authoring-surfaces-and-ai-assistant.md) (authoring
  surfaces over this frontend), [editor-ui.md](../editor-ui.md) (preset
  identity, reference set, open gaps), roadmap M4 (editor)

## Context

The editor must feel immediately familiar to users arriving from different
content tools — Unreal Engine 5, Unity, and others. Each of those tools
arranges the *same* underlying operations (move the camera, select an
entity, edit a component, save the project, import a mesh) under different
menu names, panel positions, icons and keyboard shortcuts.

The engine already separates **function** from **presentation**: every
editor operation is a named capability in the
[`CapabilityRegistry`](../../crates/re-agent/src/capability.rs), and
[`EditorContext`](../../crates/re-editor/src/context.rs) is the single
composition root that owns the shared state and exposes those capabilities.
This is the rule of double addressability (M11): the human UI, the in-proc
agent and MCP all call the *same* door.

The question this ADR answers: **how should the editor UI be built so that
"UE5 layout" and "Unity layout" are variants of one codebase rather than
two forks** — without hardcoding a single clone and retrofitting the
second later.

## Decision

Build the editor frontend **data-driven and preset-based**. A *frontend*
(also called a *preset* or *layout*) is **not** code that reimplements
features; it is data that arranges shared widgets over shared capabilities.
A preset is exactly three layers:

1. **Layout manifest** — which panels exist, where they dock, their sizes
   and titles. Declarative data (e.g. RON/TOML), not Rust control flow.
2. **Control → capability binding** — every menu item, toolbar button and
   shortcut names a capability plus its arguments (e.g. `project.save`,
   `ecs.entity.set`). The widget calls `EditorContext::call(name, args)`;
   it never calls engine internals directly.
3. **Theme** — colors, fonts, spacing, metrics, icons **and user-facing
   labels** (the label set is the i18n layer, so a preset can ship DE/EN
   names for the same capability).

A concrete preset = **manifest + theme** over a **shared widget set**
(viewport, outliner, inspector, console, content browser) and the **shared
capability registry**. `ue5` and `unity` are two such presets, not two
programs. The user picks a preset at startup; custom presets are just more
data.

### What this forbids

- Hardcoding one engine's layout into the panel code.
- Letting a widget call engine internals instead of a capability.
- Putting labels, icons or colors in Rust source instead of the theme.

### What stays shared (built once, reused by every preset)

- The **docking/panel system** (dock, tab, float, resize) — the single
  largest piece of editor-UI work, and the reason presets are cheap: it is
  written once and every preset reuses it.
- The **widget set** (outliner tree, inspector, viewport host, console).
- The **capability registry** and `EditorContext` (already exist).

## Consequences

**Positive**

- Adding a second layout (Unity) is mostly **data**, not code: reorder the
  same panels, rename menus, swap the theme.
- Reference screenshots (UE5, Unity, …) are the **spec for a preset**, not
  input to the engine core — clean separation, and the core never changes
  when a layout does.
- Every preset is automatically scriptable by the agent, because its
  controls are capability calls.
- i18n and theming come for free: they are part of the preset, not
  scattered through code.

**Costs / risks (accepted, honestly)**

- The docking system is real effort — but paid **once**, shared by all
  presets.
- Visual fidelity per engine (icons, exact look) is **art** effort, not
  code; the theme format must carry those resources.
- Some tools differ in **interaction flow**, not just arrangement (e.g.
  Unity's "add component" vs UE5's details panel). These need small
  per-preset logic, but that logic stays capability-driven (a flow is an
  ordered sequence of `ecs.*`/`project.*` calls), so it does not fork the
  core.

## Phasing

- **P1 — toolkit + `default` preset, built data-driven from day one.**
  Implement the viewport + outliner + inspector + console, but bind
  every control to a capability name, read the layout from a manifest,
  and read look/labels from a theme. The first preset is the engine's
  **own identity** (warm dark grays, rust-orange accent, command
  palette, assistant dock) — not a clone of any host tool. Costs
  almost nothing extra over hardcoding because the capabilities
  already exist.
- **P2 — extract the `ue5` preset** as the optional look for users
  arriving from Unreal. The UE5 reference screenshots are its spec
  (menu order, panel placement, German labels).
- **P3 — add the `unity` preset** as a second manifest + theme; add preset
  selection at startup; later allow user-authored manifests.

## Out of scope (for now)

The UE5 reference screenshots were delivered on 2026-11-26 — the
frontend build is unblocked and starts with P1. The inventory of the
set and the gaps still open in it live in
[editor-ui.md](../editor-ui.md). This ADR fixes the *shape* the
implementation must fit into; the manifest file format is decided in
P1.
