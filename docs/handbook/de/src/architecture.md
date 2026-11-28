# Rundgang durch die Architektur

## Crates und ihre Aufgaben

| Crate | Aufgabe |
|---|---|
| `re-core` | Datenmodell: Szenengraph, Meshes, Materialien, Animationen, Terrain, Tilesets |
| `re-io` | Szenen-Datei-I/O: glTF 2.0/GLB, OBJ+MTL, binäres FBX (ab 7.4) und die USD-ASCII-Teilmenge; Textur-Laden mit Mip-Ketten, inkl. natives DDS (BC1–BC5, BC7), KTX2-Container (unkomprimiert + BC, ETC2/EAC, ASTC, Basis-UASTC) und OpenEXR |
| `re-ecs` | Entity-Component-System: Welt, Sparse-Set-Speicher, Schedule |
| `re-world` | World-Partition: Raster, Clipmap, Tiers, Streaming, HLOD, Telemetrie, Ego-Übergabe |
| `re-render` | Backend-neutrale Render-Schicht: `GfxBackend`-Trait, Kamera, Draw-Batches |
| `re-wgpu` | Das wgpu-Backend (Vulkan/Metal/WebGPU), offscreen und im Fenster |
| `re-platform` | Fenster, Eingabe, Event-Loop (`App`-Trait), Zeit |
| `re-agent` | Capability-Registry, KI-Provider, Welt-Werkzeuge für Agenten |
| `re-editor` | Editor-Frontend: Dokument-Zustand, Undo-Stack, Outliner, Projekt-/Import-Capabilities |

Die Abhängigkeiten zeigen in eine Richtung: `re-world` kennt kein
Rendering, `re-render` keine Fenster, und `re-core` kennt von alledem
nichts. Die Binaries in `re-wgpu` (`re-demo`, `stream-demo`) sind der
Ort, an dem die Schichten zusammentreffen.

## Der Datenfluss eines Frames

```text
re-platform (Ereignisse, dt)
   └─ re-world   WorldPartition::update(camera, dt)
        ├─ Clipmap folgt der Kamera → Zell-Bedarf
        ├─ Streamer lädt/verdrängt im Budget (asynchrone IO-Threads)
        ├─ Tiers stufen Instanzen neu ein (T0..T4)
        └─ HLOD übernimmt am Clipmap-Rand
   └─ re-world   CellHandoff: Triple-Buffer-Upload, Byte-Kontingent/Frame
        └─ re-render  DrawBatch-Liste → GfxBackend::render
             └─ re-wgpu  WGSL-Pipelines, Present
```

## Drei Doktrinen

**Kopfloser, deterministischer Kern.** `re-world` enthält keine Threads
außer der austauschbaren IO-Quelle und gar keine GPU-Typen. Bei
gleicher `dt`-Folge und gleicher Kamerabahn spielt
`WorldPartition::update` identisch — deshalb ist der Streaming-Kern auf
dem Raspberry Pi testbar, lange bevor Grafik existiert.

**Schnittstellen statt Spaghetti.** Jede Naht ist ein Trait:
`CellSource` (woher Zellen kommen), `GpuUpload` (wie sie die GPU
erreichen), `GfxBackend` (was rendert), `App` (wer die Schleife treibt).
Backends und Konsolen-SDKs klinken an diesen Traits an; intern wird
nichts gegabelt.

**Eine Capability-Ebene.** Editor-Werkzeuge und KI-Werkzeuge lesen
dieselbe `CapabilityRegistry` — siehe [Capabilities &
KI](./capabilities.md).

## Wo der Code anfängt

Am besten in `crates/re-world/src/partition.rs` — `WorldPartition`
setzt jedes Streaming-Teilsystem zusammen, und die Modul-Doku nennt die
Reihenfolge-Zusagen. Dann `stream.rs` (Budget) und `handoff.rs`
(GPU-Pipeline) folgen. Das Rustdoc dazu liegt
[hier](../../rustdoc/re_world/index.html).
