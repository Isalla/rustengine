# ADR-0002: Authoring surfaces and the AI assistant panel

English | [Deutsch](0002-authoring-surfaces-and-ai-assistant.de.md)

- **Status:** Accepted
- **Date:** 2026-11-25
- **Deciders:** Waldemar Friesen (product), RE engine team
- **Relates to:** [ADR-0001](0001-modular-editor-frontend.md) (preset-driven
  frontend), [ki-integration.md](../ki-integration.md) (rule of double
  addressability, `re-agent`), roadmap M4 (editor)

## Context

Many developers still like to work on code directly; others prefer wiring
behaviour visually without writing code. The editor must serve **both**,
and let a user live entirely in one, the other, or move between them freely.
Whichever surface a person chooses, the underlying behaviour must be the
same — there is one engine, one set of systems, one set of capabilities.

This builds directly on ADR-0001: the frontend is data-driven, every control
is a capability call, and panels are dockable widgets over a shared core.
This ADR decides **what the authoring surfaces are**, **how they relate**,
and **the role of the AI assistant** (`re-agent`) inside them.

## Decision

### 1. Two authoring surfaces, one underlying composition

- **Code editor** — a full editor for developers who write code.
- **Visual node graph** — connect functions/systems without writing code,
  in the spirit of Unreal Blueprints.

They are **not two systems**. They are **two views of the same thing**: a
node graph is a visual editor for the same ECS `Schedule` / capability
composition that code expresses textually. Both produce the same artifact,
so both remain addressable by the agent (rule of double addressability).

### 2. The code editor

- **Left: project tree** — every folder and file in the project, for quick
  switching.
- **Tabs** — open files tabbed across the top; switching is one click.
- **Syntax highlighting** — immediate, so the developer sees at a glance
  whether the code is well-formed or something is missing.
- **Diagnostics as suggestions, never as edits** — the editor highlights
  errors/warnings and may offer fixes, but **replaces nothing on its own**.
  The developer decides.
- Backbone: an **LSP client** (rust-analyzer) supplies completion,
  diagnostics and code actions; an optional syntax highlighter
  (tree-sitter/syntect) augments it.

### 3. The visual node graph

- Drag-and-drop nodes that represent functions/systems/capabilities, wired
  by data and execution pins (event → function → output), like Blueprints.
- A **graph browser** (Graphs / Functions / Macros / Variables) and a
  **Details** panel for the selected node.
- Node types are themselves registered as capabilities, so the graph is
  editable by the agent exactly as the code is.

### 4. One assistant panel (right), three modes

Diagnostics and the AI assistant are the **same kind of thing** — feedback
about the code — so they live in **one** panel, not two. The panel shows a
single, type-tagged stream:

| Mode | Direction | Content |
|---|---|---|
| **Passive** | engine → developer | errors, warnings, hints, follow-up actions |
| **Live** | engine → developer, while typing | predicted continuation (ghost text) |
| **Dialog** | developer → engine, on demand | chat: ask a question without opening another window |

- Each entry is clickable and jumps to the relevant code location.
- The panel and the project tree are both **dockable and toggleable**
  (open/close any time), per ADR-0001.

### 5. The assistant is engine-aware, powered by `re-agent`

The assistant is not a bolted-on chatbot. It is the **existing `re-agent`**
(Ollama, llama.cpp, OpenAI-compatible, Anthropic) plus the
**CapabilityRegistry**, surfaced as a panel. Because it shares the editor's
context (open file, selection, cursor) and can read the project and the
registry, it answers **about this project**, and can suggest RE-specific
next actions: *"register this as a capability"*, *"add this system to the
`Schedule`"*, *"spawn it via `world.instances.spawn`"*. A generic editor's
AI cannot do this; ours can, because the suggestions are the registry.

- **Local-first default** (Ollama / llama.cpp) so code need not leave the
  machine; cloud providers remain selectable.
- **Read freely, write only on click**: the assistant may call capabilities
  read-only (read a file, query the world); any mutation is offered as a
  suggestion the developer triggers — the same rule as diagnostics.

### 6. Source provenance and bidirectional navigation

To jump from a symptom to its cause, **every capability / system / node
carries a back-reference to where it is defined** (file + line, or a
registry id resolved to a source location). This must be built in from the
start — it is hard to retrofit.

Because code and graph are two views of one composition, the link runs
**both ways**:

- **Node → code:** an error on a node → "show me the place" → the agent
  resolves the node's source and the editor opens it at that line; the
  developer fixes it.
- **Code → node:** cursor on a system function → the matching node is
  highlighted in the graph.

The jump is itself a capability (`editor.reveal`), so it can be triggered
by a human click **or** by the agent in the chat — double addressability
again.

## Consequences

**Positive**

- One engine, one behaviour; the surface is a preference, not a fork.
- The assistant is project-aware and engine-aware, not generic.
- Diagnostics, prediction and chat share one context and one panel.
- Errors are navigable across surfaces (graph ↔ code) via provenance.

**Costs / risks (accepted)**

- **Provenance is a hard prerequisite** — capability/system registration
  must record source location from day one.
- LSP integration (rust-analyzer) is real plumbing; the assistant needs
  context-scoping (what is sent to the model) and provider choice
  (local vs cloud) decided per user.
- A node graph with execution + data pins is substantial UI work; it reuses
  the ADR-0001 docking system but is its own widget.
- Privacy/cost of sending code to models: mitigated by the local-first
  default and explicit context boundaries.

## Phasing

- **P1 — code editor + LSP.** Project tree, tabs, highlighting, rust-analyzer
  diagnostics (suggestions, not edits). **Provenance recorded at capability
  and system registration from here on.**
- **P2 — assistant panel.** Passive diagnostics stream, then live prediction,
  then the `re-agent` dialog; `editor.reveal` for jump-to-source.
- **P3 — visual node graph.** Graph editor + browser + details, mapped onto
  the `Schedule`/capabilities, with bidirectional navigation to code.

## Out of scope (for now)

Implementation begins later, alongside the frontend phases of ADR-0001.
This ADR fixes the shape and the cross-cutting
requirements (provenance, suggest-not-apply, engine-aware assistant); it
does not fix widget internals or the exact manifest format, which P1–P3
will finalize.
