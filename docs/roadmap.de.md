# Meilenstein-Roadmap

[English](roadmap.md) | [Deutsch](roadmap.de.md)

Die komplette Checkliste zwischen „Repo klonen" und „Spiel veröffentlichen" —
mit ehrlichem Haken-Stand. ✅ fertig und testabgedeckt · 🟡 teilweise gebaut ·
⬜ noch nicht begonnen. Hintergrund zu jedem Block:
[architecture.de](architecture.de.md) · [cinema.de](cinema.de.md) ·
[rendering.de](rendering.de.md) · [vram.de](vram.de.md) ·
[ki-integration.de](ki-integration.de.md).

## Was heute schon läuft

* glTF-Szenen laden und offscreen rendern (`re-demo`, `re-wgpu`)
* Fenster + Eingabe + Event-Loop (`re-platform`, headless getestet)
* ECS mit generationsgeprüften Handles (`re-ecs`)
* World-Partition: Clipmap-Streaming, Quality-Tiers T0..T4, Byte-Budget,
  Pooling — ohne GPU deterministisch testbar (`re-world`, inkl.
  asynchronem IO-Thread-Pool, der Zelldateien von der Platte lädt)

## M0 — Repo & Kern-Datenmodell ✅

- [x] MIT-lizenziertes Workspace, Crate-Aufbau, zweisprachige Doku
- [x] Szenengraf, Meshes, Materialien + Texturen (`re-core`)
- [x] Animationen & Skelette, Terrain & Tilesets (`re-core`)
- [x] glTF-2.0-Import/Export inkl. Textur-Einbettung (`re-io`)

## M1 — Erste Pixel ✅

- [x] Backend-neutrale Render-Schicht: `GfxBackend`, Kamera, Billboard-
      Batches (`re-render`)
- [x] wgpu-Backend: Vulkan/Metal/WebGPU, DirectX über wgpu; Offscreen
      und Fenster (`re-wgpu`)
- [x] Demo-Binary rendert glTF-Szene mit Texturen (`re-demo`)

## M2 — Plattform-Layer ✅

- [x] Fenster, Eingabe, Event-Loop, Delta-Zeit (`re-platform`/winit)
- [x] Headless-Modus — jede Schicht ohne Display testbar

## M3 — ECS & Welt-Streaming 🟡 ← aktueller Fokus

- [x] **M3a** Entity-Component-System: Generations-Handles, Sparse-Set-
      Speicher, System-Schedule (`re-ecs`)
- [x] **M3b** Raster & Clipmap: anker-quantisierter aktiver Zellbestand,
      lochfreie Abdeckung, ≤2-fache Überlappung an Bandgrenzen
      (`re-world::grid/clipmap`)
- [x] **M3b** Quality-Tiers T0..T4 mit Hysterese und Verweilzeit — vom
      Minimap-Punkt zur Kino-Mesh als durchgehende Funktion
      (`re-world::tier`)
- [x] **M3b** Streamer: Byte-Budget, deterministische Priorität,
      Verdrängung grob-vor-fein dann LRU, Pooling statt Entladen
      (`re-world::stream`)
- [x] **M3c** Asynchrone IO-Pipeline: Worker-Thread-Pool hinter
      `CellSource`, Datei-Blockstore, abbruchfest, blockiert den
      Aufrufer nie (`re-world::io`)
- [ ] **M3c** GPU-Handoff: Payload → Staging-Buffer → VRAM,
      Triple-Buffering, damit Render- und Update-Thread nie auf einen
      Upload warten
- [ ] **M3d** HLOD: vorgebackene Fernfeld-Meshes am Clipmap-Rand
- [ ] **M3e** Ego-Übergabe: Steuerungsübergabe mitten im Flug ohne
      Szenen-Neuaufbau
- [ ] MCP-Werkzeuge: Agenten-Zugriff auf Streaming und Szenen-Abfragen
      (auf Basis der Capability-Registry aus `re-agent`)

## M4 — Editor ⬜

- [ ] Szenen-Ansicht mit live Kamera in die streamende Welt
- [ ] Outliner & Inspector über der ECS
- [ ] Import-Pipeline-UI um `re-io`
- [ ] Undo/Redo als umkehrbare Systeme
- [ ] Worldbuilder-Projekt-Loader

## M5 — Render-Pipeline V2 ⬜

- [ ] Deferred-HDR-Pipeline
- [ ] Shadow 2.0 (kaskadiert + geclustert)
- [ ] Upscaling-Adapter: FSR / XeSS / DLSS ([rendering.de](rendering.de.md))

## M6 — Dynamische GI & Reflexionen ⬜

- [ ] Lumen-artiger Software-Raytracing-Pfad
- [ ] Hardware-RT als optionaler Qualitätsschalter
- [ ] `re-native`-Backend für Hersteller-RT-Stacks

## M7 — Pfad-Tracing ⬜

- [ ] Progressiver Pfad-Tracer
- [ ] Ray-Reconstruction / Entrauschen
- [ ] Offline- und Bake-Modi

## M8 — Medien-Stack ⬜

- [ ] Audio: SFX-Event-System (`re-audio`)
- [ ] VFX: GPU-Partikel, Effektgraphen (`re-vfx`)
- [ ] Physik (`re-physics`)
- [ ] Animation: Blends, IK, Ebenen

## M9 — Online: die Go-Seite ⬜

- [ ] Netcode-Dienst (Go) mit serverseitigem Interest-Management
- [ ] Dedicated Server
- [ ] Buildfarm & Launcher-Werkzeuge
- [ ] WebGPU-Web-Build der Runtime

## M10 — Konsolen & Release ⬜

- [ ] NDA-SDK-Backends an die offenen Traits andocken (private Repos)
- [ ] Zertifizierungs-Helfer, Veröffentlichungs-Pipeline

## Wiki — Doku in drei Stufen ⬜

- [ ] Rustdoc auf GitHub Pages, automatisch bei jedem Push
- [ ] Nutzer-Handbuch EN/DE als mdBook
- [ ] API-Browser im Editor (`cargo rustdoc --output-format json`,
      geteiltes Registry-Konzept mit den Agenten-Capabilities)

## Leitfeature — nahtloser Perspektivwechsel 🟡

*Der Sinn der Engine: Strategie-Übersicht ↔ Ego-Sicht einer Einheit ohne
Ladebildschirm — und dieselbe Welt unter einem Shooter oder MMO. Das RTS
ist das erste Testprojekt; Shooter und MMO folgen.*

- [x] Datenseite des Flugs: Tiers + Streaming + Pooling unterbrechen
      nie (M3a/M3b)
- [x] Echtes Datei-IO hinter demselben Trait (M3c, läuft gerade)
- [ ] GPU-Seite des Flugs (M3c-Upload, M3d-HLOD)
- [ ] Kamera- & Timeline-Layer ([cinema.de](cinema.de.md))
- [ ] Steuerungs-Übergabe (M3e)
- [ ] Erster spielbarer Rundflug: RTS-Vertikalschnitt durch alles

---

*Update-Disziplin: Jede gemergte Funktion setzt in diesem File und
seinem Zwilling [roadmap.md](roadmap.md) im selben Commit ihren Haken.*
