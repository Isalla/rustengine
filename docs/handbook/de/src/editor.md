# Der Editor

Der Editor ist keine zweite Anwendung neben der Engine. Er ist ein
weiterer Verbraucher derselben [`CapabilityRegistry`](./capabilities.md),
die auch die KI-Agentin nutzt — die Regel der doppelten Adressierbarkeit,
konkret gemacht. Alles, was das Fenster später per Knopfdruck tut, ist
heute schon namentlich aufrufbar, und umgekehrt.

## Die Kompositionswurzel

`re_editor::EditorContext` hält alle geteilten Handles, die der Editor
braucht, und vereint sie in einer Registry:

| Handle | Enthält | Adressiert durch |
|---|---|---|
| `state` | Kamera, Selektion, Ansichtsmodus, Zähler | `editor.*` |
| `undo` | den `CommandStack<EditorState>` | `editor.undo` / `.redo` / `.history` |
| `project` | ein `ProjectDoc` (Projekt + Pfad + Dirty-Flag) | `project.*`, `import.*` |
| `world` | die ECS-`World` | `ecs.*` |
| `registry` | welche Components inspizierbar sind | `ecs.entity.*` |

```rust
use glam::Vec3;
use re_editor::EditorContext;
use serde_json::json;

let ctx = EditorContext::new(Vec3::new(0.0, 80.0, 0.0));

// Die Kamera — durch dieselbe Tür wie später das Fenster.
ctx.call("editor.camera.set", json!({ "x": 12.0, "yaw": 0.4 })).unwrap();

// Ein Projekt, per Pfad gespeichert und geladen.
ctx.call("project.new", json!({ "name": "MeineSzene" })).unwrap();
ctx.call("project.save", json!({ "path": "meine_szene.wbproj" })).unwrap();
```

## Undo/Redo als umkehrbare Systeme

Jede Editor-Aktion ist ein `Command`: `apply` führt sie aus, `undo` dreht
sie zurück. Der `CommandStack` hält einen Cursor über die Historie; eine
neue Aktion nach einem Undo verwirft den Redo-Zweig (Standard-Semantik).
Die Befehle sind über ihr Ziel generisch, derselbe Stack bedient also
Editor-Zustand, Szene oder eine Welt-Referenz.

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
stack.undo(&mut state).unwrap(); // zurück zum Ursprung
```

Das Fly-Kamera-Bewegen ist bewusst *nicht* undo-bar — Undo gilt für Szenen-
und Dokument-Änderungen, nicht dafür, wohin du deinen Blickpunkt stellst.

## Outliner & Inspector über der ECS

`re_ecs::World` speichert Components typgelöscht, daher braucht der
Inspector eine `ComponentRegistry`, die weiß, wie sie jeden anzuzeigenden
Typ serialisiert. Einmal registriert, ist der Typ über die
Capability-Schiene lesbar und schreibbar:

```rust
use re_editor::ComponentRegistry;

let mut reg = ComponentRegistry::new();
reg.register::<re_core::transform::Transform>("Transform");
```

| Werkzeug | Wirkung |
|---|---|
| `ecs.entities` | alle lebenden Entities auflisten |
| `ecs.entity.components` | welche Component-Typen eine Entity trägt |
| `ecs.entity.get` | eine Component als JSON lesen |
| `ecs.entity.set` | Component-Felder aus JSON schreiben |

## Import und Projektdateien

`re_editor::import` kapselt die Modell-Leser aus `re_io` (glTF/GLB,
OBJ mit MTL-Material — die Endung entscheidet) hinter drei
Werkzeugen: `import.preview` meldet Zähler für Meshes/Materialien/Knoten/
Animationen, ohne das offene Dokument anzufassen; `import.gltf` und
`import.obj` laden die Datei als aktuelles Projekt und markieren es
dirty. `re_editor::project` hält das Dokument (`ProjectDoc`) mit Pfad
und Dirty-Flag, damit auch Speichern/Laden als Roundtrip adressierbar ist.

## Stand

Heute liefert der Editor die Szenen-Ansicht — eine frei fliegende Kamera
in die streamende Welt, mit Headless-Smoke-Pfad für CI — und ein
vollständiges Bildschirm-Frontend: ein `default`-Preset mit Menüleiste,
Werkzeugleiste, Outliner, Inspektor und Konsole über dem Docking-Layout,
das sich an gegriffenen Splittern umskalieren und per Tab-Ziehen umlagern
lässt. Alle Panels sind datengetrieben (ADR-0001) sprechen die Engine nur
über `EditorContext::call` an, sodass die Agentin genau das kann, was die
Maus kann — bis hin zu den direkt editierbaren Werten im Inspektor:
Zahlenfeld klicken und tippen, oder am Eigenschaftsnamen scrubben (mit
Shift feiner); geschrieben wird dabei immer über `ecs.entity.set`.

Rustdoc: [`re_editor`](../../rustdoc/re_editor/index.html).
