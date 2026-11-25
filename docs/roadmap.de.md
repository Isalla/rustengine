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
* Zellen wandern asynchron in die VRAM und werden verzögert wieder
  freigegeben — Überflug-Demo `stream-demo` zeigt die ganze Kette
  (`re-world::handoff`, `re-wgpu`)

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

## M3 — ECS & Welt-Streaming ✅

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
- [x] **M3c** GPU-Handoff: Triple-gepufferter `CellHandoff` mit
      Byte-Kontingent pro Frame, Schonfrist beim Retire, verzögerter
      Freigabe; `upload_cell`/`release_cell` in den Backend-Traits,
      Zell-Residenz in `re-wgpu` — demonstriert von `stream-demo`
      (Überflug einer Formel-Welt durch die echte GPU-Pipeline)
- [x] **M3d** HLOD: vorgebackene Fernfeld-Meshes am Clipmap-Rand
      (`re-world::hlod` — Cluster-Registrierung, Übernahmezone mit
      Hysterese, Fade-out; demonstriert von `stream-demo`)
- [x] **M3e** Ego-Übergabe: Steuerungsübergabe mitten im Flug ohne
      Szenen-Neuaufbau (`re-world::ego` — geglättete Kamera-Überblendung,
      Eingabe-Besitz, Welt-Streaming unberührt; demonstriert von
      `stream-demo`)
- [x] IO-Telemetrie: gemessene Lade-Latenz und Durchsatz treiben
      Tier-Hysterese und Verweilzeit adaptiv (langsame Platte →
      frühere Abwertung)
- [x] MCP-Werkzeuge: Agenten-Zugriff auf Streaming und Szenen-Abfragen
      (`re-agent::world` — Welt-/Zell-/Instanz-Werkzeuge auf der geteilten
      `WorldPartition`, dieselbe Registry wie der Editor)

## M4 — Editor ⬜ ← aktueller Fokus

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

## M11 — KI-nativer Editor: UE5-Breite, Plugin-Offenheit ⬜

*Der Editor dieser Engine ist von Tag eins für zwei Arten von Nutzern
gebaut: den Menschen am Viewport und die Agentin am Registry. Ziel ist
die funktionale Breite des UE5-Editors — Landscape, Sequencer, VFX,
Audio, visuelle Skripte, Profiling — realisiert als Plugins auf einer
einzigen Capability-Registry ([ki-integration](ki-integration.de.md)):
Jedes Fenster, das ein Mensch öffnen kann, ist ein Werkzeug, das die KI
aufrufen kann — und ein Satz ersetzt hundert Klicks.*

**Fundament**

- [ ] Plugin-Architektur: Engine- und Editor-Funktionen erscheinen als
      Plugins auf einer stabilen Erweiterungs-API (Panels, Importer,
      ECS-Systeme, Render-Pässe, Registry-Werkzeuge); dynamisches Laden,
      versioniert, fremde Plugins sandboxed (natives `.so` oder Wasm)
- [ ] Regel der doppelten Adressierbarkeit: keine Editor-Funktion ohne
      Registry-Werkzeug; die UI ist Kunde derselben Werkzeuge
      (undo-sicher, von der Agentin stapelbar)
- [ ] Skripting-Ebene: eingebettete Skriptsprache (z. B. Lua oder
      Rust-Hot-Load) für Spiellogik und Editor-Automatisierung, ebenso
      als Werkzeuge exponiert

**Editor-Breite (je Punkt: UI-Panel + Registry-Werkzeuge + Plugin)**

- [ ] Level-Editor: Transform-Gizmos, Snapping, modulare Baukasten-
      montage
- [ ] Landscape & Scatter: Sculpting, Farblayer, Vegetation/PCG-artige
      Graphen mit deterministischen Seeds
- [ ] Material-Editor: Node-Graph, kompiliert zu WGSL-Varianten
- [ ] Sequencer: Spuren für Kamera, Animation, Audio, FX, Eigenschaften —
      Timeline von Hand oder aus einer Textbeschreibung editierbar
- [ ] VFX-Editor auf `re-vfx`-Graphen; Audio-Editor auf `re-audio`-
      Events; Navigations- & KI-Werkzeuge (Navmesh, Verhaltensgraphen)
- [ ] Profiler & Frame-Debugger auf der Render-/GPU-Telemetrie

**KI-Inhaltspipelines (Provider hinter Traits, Ergebnisse werden
normale Assets — der Simulationskern bleibt deterministisch)**

- [ ] Text → Cutscene: aus einem Satz werden Storyboard, Shot-Liste,
      Kamera-Keys, Blocking und Schnitte im Sequencer; nachlesbar, von
      Hand nacheditierbar, über den Pfadtracer-Bake renderbar
- [ ] KI-Sound: Generierung von Effekten und Musik, an das Schall-
      ereignissystem angeschlossen (Prompt → Asset → Timeline oder
      Spiel)
- [ ] KI-Stimmen: NPC-Dialog zur Laufzeit über LLM-Provider mit
      Budget- und Sicherheitsebene, TTS für die Stimme, Lip-Sync als
      Animation gebacken
- [ ] Assistent im Editor: direkter In-Process-Agent (Ollama,
      llama.cpp, API) plus MCP-Export — dieselbe Registry, beide Türen
- [ ] Generierungs-Telemetrie: Kosten-/Latenz-Buchführung pro
      Werkzeugaufruf, damit ein Agenten-Lauf gedeckelt und prüfbar ist

## Wiki — Doku in drei Stufen 🟡

- [x] Rustdoc auf GitHub Pages, automatisch bei jedem Push (`.github/workflows/docs.yml`)
- [x] Nutzer-Handbuch EN/DE als mdBook (`docs/handbook/{en,de}`)
- [ ] API-Browser im Editor (`cargo rustdoc --output-format json`,
      geteiltes Registry-Konzept mit den Agenten-Capabilities)

## Leitfeature — nahtloser Perspektivwechsel 🟡

*Der Sinn der Engine: Strategie-Übersicht ↔ Ego-Sicht einer Einheit ohne
Ladebildschirm — und dieselbe Welt unter einem Shooter oder MMO. Das RTS
ist das erste Testprojekt; Shooter und MMO folgen.*

- [x] Datenseite des Flugs: Tiers + Streaming + Pooling unterbrechen
      nie (M3a/M3b)
- [x] Echtes Datei-IO hinter demselben Trait (M3c)
- [x] GPU-Handoff des Flugs: Triple-Buffering, Zell-Residenz, verzögerte
      Freigabe (`re-world::handoff`, `re-wgpu`)
- [x] Fernfeld-HLOD am Clipmap-Rand (M3d)
- [x] IO-Telemetrie & adaptive Stufenregel (`re-world::telemetry`)
- [x] Strategie-↔Ego-Übergabe mitten im Flug ohne Szenen-Neuaufbau
      (`re-world::ego`)
- [x] Welt-Werkzeuge für Agenten: Streaming- und Szenen-Abfragen auf der
      geteilten Capability-Registry (`re-agent::world`)
- [ ] Kamera- & Timeline-Layer ([cinema.de](cinema.de.md))
- [ ] Erster spielbarer Rundflug: RTS-Vertikalschnitt durch alles

---

*Update-Disziplin: Jede gemergte Funktion setzt in diesem File und
seinem Zwilling [roadmap.md](roadmap.md) im selben Commit ihren Haken.*
