# The Editor

The editor is not a second application bolted beside the engine. It is one
more consumer of the same [`CapabilityRegistry`](./capabilities.md) the AI
agent uses — the rule of double addressability made concrete. Everything
the window will do with a button click is already callable by name today,
and vice versa.

## The composition root

`re_editor::EditorContext` owns every shared handle the editor needs and
wires them into a single registry:

| Handle | Holds | Addressed by |
|---|---|---|
| `state` | camera, selection, view mode, frame count | `editor.*` |
| `undo` | the `CommandStack<EditorState>` | `editor.undo` / `.redo` / `.history` |
| `project` | a `ProjectDoc` (project + path + dirty flag) | `project.*`, `import.*` |
| `world` | the ECS `World` | `ecs.*` |
| `registry` | which components are inspectable | `ecs.entity.*` |

```rust
use glam::Vec3;
use re_editor::EditorContext;
use serde_json::json;

let ctx = EditorContext::new(Vec3::new(0.0, 80.0, 0.0));

// The camera, through the same door the GUI will use.
ctx.call("editor.camera.set", json!({ "x": 12.0, "yaw": 0.4 })).unwrap();

// A project, saved and reloaded by path.
ctx.call("project.new", json!({ "name": "MyScene" })).unwrap();
ctx.call("project.save", json!({ "path": "myscene.wbproj" })).unwrap();
```

## Undo / redo as reversible systems

Every editor action is a `Command`: `apply` runs it, `undo` reverses it.
The `CommandStack` keeps a cursor over the history; a new action after an
undo drops the redo branch (standard semantics). Commands are generic over
their target, so the same stack drives the editor state, a scene, or a
world handle.

```rust
use re_editor::{CommandStack, EditorState, SetCameraCommand};
use glam::Vec3;

let mut state = EditorState::new(Vec3::ZERO);
let mut stack = CommandStack::new(128);
let old = state.camera.clone();
stack
    .execute(
        Box::new(SetCameraCommand { old, new: re_editor::EditorCamera::new(Vec3::X) }),
        &mut state,
    )
    .unwrap();
stack.undo(&mut state).unwrap(); // back to the origin
```

The camera fly-through itself is deliberately *not* undoable — undo is for
scene and document edits, not for moving your own viewpoint.

## Outliner & inspector over the ECS

`re_ecs::World` stores components type-erased, so the inspector needs a
`ComponentRegistry` that knows how to serialize each type it should show.
Register a type once and it becomes readable and writable through the
capability channel:

```rust
use re_editor::ComponentRegistry;

let mut reg = ComponentRegistry::new();
reg.register::<re_core::transform::Transform>("Transform");
```

| Tool | What it does |
|---|---|
| `ecs.entities` | list every live entity |
| `ecs.entity.components` | which component types an entity carries |
| `ecs.entity.get` | read one component as JSON |
| `ecs.entity.set` | write component fields from JSON |

## Import and project files

`re_editor::import` wraps `re_io`'s glTF/GLB reader behind two tools:
`import.preview` reports mesh/material/node/animation counts without
touching the open document; `import.gltf` loads the file as the current
project and marks it dirty. `re_editor::project` keeps the document
(`ProjectDoc`) with its path and dirty flag, so save/load round-trips are
addressable too.

## Where it stands

Today the editor ships the scene view — a free-fly camera into the
streaming world, with a headless smoke path for CI — and a full
on-screen frontend: a `default` preset with menu bar, toolbar, outliner,
inspector and console over the docking layout, resizable by grabbing
splitters and redockable by dragging tabs. Every panel is data-driven
(ADR-0001) and talks to the engine only through `EditorContext::call`,
so an agent can do exactly what the mouse can — including the
inspector's direct value editing: click a number field to type, or drag
the property name to scrub a value live (hold Shift for fine steps;
every write still goes through `ecs.entity.set`).

Rustdoc: [`re_editor`](../../rustdoc/re_editor/index.html).
