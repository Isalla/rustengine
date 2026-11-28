# Meilenstein-Roadmap

[English](roadmap.md) | [Deutsch](roadmap.de.md)

Die komplette Checkliste zwischen „Repo klonen“ und „Spiel veröffentlichen“ —
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
* Zellen wandern asynchron in den VRAM und werden verzögert wieder
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

- [x] Szenen-Ansicht mit live Kamera in die streamende Welt
      (`re-editor` — frei fliegende Kamera, streamende Welt folgt dem
      Blickpunkt; Fenster- + Headless-Smoke-Pfad)
- [x] Outliner & Inspector über der ECS (`re-editor::outliner` — `ComponentRegistry` mit typgelöschtem JSON-Zugriff, `Outliner`, `ecs.entities/entity.components/entity.get/entity.set`-Capabilities; `re-ecs`-`World` auf `Send` gestellt)
- [x] Import-Pipeline um `re-io` (`re-editor::import` — `import_gltf`, `import.preview`/`import.gltf`-Capabilities, Roundtrip-Test)
- [x] Undo/Redo als umkehrbare Systeme (`re-editor::undo` — `Command<T>`-Trait, `CommandStack` mit Cursor, Built-in-Commands für Kamera/Selektion/Ansichtsmodus, `editor.undo/redo/history`-Capabilities)
- [x] Worldbuilder-Projekt-Loader (`re-editor::project` — `ProjectDoc` mit Pfad/Dirty, `project.new/load/save/close/info`-Capabilities, Roundtrip-Test)
- [x] Editor-Frontend P1: Widget-Toolkit (Draw-List, Text-Atlas,
      Input-Routing, Docking-Manifest, Theme) + das `default`-Preset mit
      eigener Identität der Engine (`re-render::ui`-Widgets,
      `re-editor::layout`-Manifest + `presets/default`-Daten,
      `re-editor::panels` über `EditorContext::call`; Fenster-Overlay via
      `render_with_ui` + `--ui-png`-CPU-Beweis; datengetrieben nach
      ADR-0001; das `ue5`-Preset (P2) folgt später
- [x] Docking-Interaktion im default-Layout (Splitter greifen und
      Panels umskalieren, Tabs greifen und mit Rand-/Mittelzonen
      umlagern — `dock_splitters`/`dock_set_ratio`/`dock_take`/
      `dock_put` in `re-render::ui`, `drop_zones` + Drag-Geist in
      `re-editor::panels`; OS-eigene Schwebefenster zurückgestellt)
- [x] Direkt editierbare Werte im Inspektor (Zahlenfeld klicken und
      tippen, am Eigenschaftsnamen scrubben — Shift für feine Schritte;
      `Ui::drag`/`Ui::scrub` in `re-render::ui`, editierbare
      Zahlenblätter in `re-editor::panels`; jeder Schreibzug bleibt auf
      dem `ecs.entity.set`-Capability-Weg)
- [x] Mehr Dateiformate (Breite statt Einzel-Tool-Migration):
      OBJ-Leser/-Schreiber (+MTL) zusätzlich zum bestehenden
      glTF-Pfad, Endungs-Dispatch in `re_io::import/export`;
      zentraler Textur-Lader (`re_io::tex`) für PNG/JPEG/TGA/BMP/QOI
      plus natives DDS — unkomprimiert nach Bitmasken, BC1–BC5
      (DXT1/3/5/ATI1/ATI2) und BC7 (eigener Decoder, bitgenau nach
      Spezifikation), inklusive DX10-Kopf. KTX2-Container liest es
      ebenfalls nativ (unkomprimiert und BC-Payloads, bitgleich zu
      DDS). Mip-Ketten kommen mit der Datei (DDS/KTX2) oder rechnet
      `build_mip_chain`; `re-wgpu` lädt die volle Kette und sampelt
      trilinear. OpenEXR liest das reine Rust-Crate `exr` (linearer
      Fließkomma wird geklemmt und sRGB-kodiert als RGBA8). ETC2/EAC —
      das Pi-Zielformat — decodiert ebenfalls nativ (RGB8, RGB8A1,
      RGBA8, EAC R11/RG11 über eigenen CPU-Decoder); signierte
      EAC-Formate und ASTC melden klar zurück.
      Binär-FBX ab 7.4 importiert Meshes, Materialien, Skins und
      Animations-Takes über das reine Rust-Crate `fbxcel` (nur Import;
      ASCII, vor-7.4 und voll komprimiertes FBX melden sauber zurück).
      Nur offene Standards;
      Inhalte aus anderen Engines kommen über deren offizielle Exporter
      (UE/Unity exportieren selbst FBX/USD/glTF).
- [ ] Format-Nachzug: USD (ASCII-Teilmenge) als weiterer Importierer;
      proprietäre Szenenformate
      (`.uasset`/`.umap`, `.unity`) bleiben unsupported — als Idee
      geparkt, bis die Hersteller offene Formate veröffentlichen;
      Spiel-Code wandert ohnehin nie mit
- [ ] Textur-Nachzug: Basis-Transcoding in KTX2 (ETC1S, UASTC) und
      ASTC-Decodierung — Breite auf
      der Decoder-Seite für den Streaming-Pfad

## M5 — Render-Pipeline V2 ⬜

- [x] Deferred-Pipeline: MRT-G-Buffer (`deferred_v1`) + Lighting-Pass über
      die abstrakte `surface_at`-Schnittstelle — GPU-Ausgabe per Pixel-Diff
      gegen die CPU-Referenz verifiziert (ADR-0003, `re-wgpu/tests/deferred.rs`)
- [x] HDR-Float-Ziele + Tone Mapping: der Lighting-Pass schreibt ein
      RGBA32F-Ziel (Werte > 1,0 ueberleben), ein Tone-Mapping-Pass
      spiegelt die Kurven aus `re-render::hdr` in WGSL (ACES/Reinhard;
      AgX noch offen) — Pixel-Diff-verifiziert gegen `HdrPipeline::process()`
- [x] Geclusterte Punktlichter im Deferred-Lighting-Pass: der Shader
      findet den Cluster jedes Pixels (Kachel x logarithmische
      Tiefenscheibe, Tiefe aus dem Depth-Buffer rekonstruiert) und
      summiert dessen Lichter — Pixel-Diff gegen die CPU-Referenz durch
      das HDR-Ziel und das tone-gemappte Ergebnis verifiziert
- [x] GPU-Compute-Licht-Culling: ein Compute-Pass spiegelt
      `cluster_view_aabb` + `sphere_intersects_aabb` und schreibt die
      Licht-Listen je Cluster — Index-fuer-Index gegen `cull_lights()`
      auf der CPU verifiziert, ueber alle Cluster (der CPU-Kern bleibt
      die Referenz im Test)
- [x] Shadow 2.0 kaskadiert: der GPU-Tiefen-Pass spiegelt
      `re-render::shadow` Zug-fuer-Zug — eine Depth-Schicht je Kaskade
      in einer 2D-Array-Map, Kugel-Fit mit Texel-Snapping (rechnet der
      CPU-Kern pro Frame), 3x3-PCF durch Hardware-Compare-Sampler;
      Map-Sonden und jeder Bodenpixel gegen die CPU-Referenz auf der
      Readback-Map verifiziert
- [ ] Shadow 2.0 geclustert: schattenwerfende Punktlichter über die
      Cluster-Listen ([rendering.de](rendering.de.md))
- [x] Upscaling-Adapter FSR1 (EASU + RCAS) hinter dem Tone-Mapping der
      Deferred-Kette — der Pi-Pfad (M13): intern mit 1/Skala rendern,
      aufs Display hochskalieren. Die WGSL spiegelt `re_render::fsr`
      (Port von AMD `ffx_fsr1.h`, MIT) Zug-fuer-Zug, Pixel-Diff-
      verifiziert, und das Qualitaetstor beweist: FSR1 schlaegt Bilinear
      gegen den nativen Frame
- [ ] Desktop-Tier-Adapter: FSR2/3 (zeitlich), XeSS, DLSS
      ([rendering.de](rendering.de.md))
- [x] Render-Graph: die Deferred-Kette (Cull → G-Buffer → Lighting →
      Post → FSR1) ist einmal als Daten deklariert (`re_render::graph`);
      der Executor in `re-wgpu` folgt dem kompilierten Plan — die
      Ausführungs-Spur beweist die Reihenfolge, die Pixel bleiben
      dieselben (alle Pixel-Diff-Tore weiter grün). Die Übergänge
      liegen dem Plan bei (auf wgpu implizit, als Daten für spätere
      explizite Backends); Textur-Aliasing folgt mit einem Target-Pool —
      die Lebensspannen sind schon berechnet
- [ ] GPU-getriebener Draw-Pfad (nach den Kern-Boxen): bindless
      Textur-/Material-Arrays, Compute-Culling (Frustum + Hi-Z-Occlusion)
      schreibt Indirect-Draw-Argumente, ein Multi-Draw pro Mesh-Batch

## M6 — Dynamische GI & Reflexionen ⬜

- [ ] Lumen-artiger Software-Raytracing-Pfad
- [ ] Hardware-RT als optionaler Qualitätsschalter
- [ ] `re-native`-Backend für Hersteller-RT-Stacks
- [ ] Volumetrischer Nebel & Lichtschächte (Foxel-Raster, pro Cluster per
      Compute injiziert; Niedrigauflösungs-Stufe läuft auf WebGPU)
- [ ] Radiance Cascades als günstige GI-Stufe (Compute-only, weiche
      GI/Schatten ohne Hardware-RT, browserfähig)

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
      (GPU-Zeit je Pass über wgpu-Timestamp-Queries)

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

## M12 — Prozedurale Welten (Runtime + Editor) ⬜

*Rising World und Minecraft haben gezeigt: Spieler akzeptieren eine
generierte Welt als das Spiel selbst. Der deterministische Headless-
Kern (`re-world`) ist genau dafür gebaut — mit einer Designregel aus
dem Produkt: **die Autoren-Beschreibung ist der Input, der Seed wird
aus ihr abgeleitet** (Hash des Parameter-Dokuments), nie umgekehrt.
Kein Seed-Glücksspiel.*

- [ ] Beschreibung zuerst: die Autorin definiert Absicht — Biome-
      Karten, Feature-Regeln („Fluss vom Gebirge zum südlichen Meer“),
      Dichte-Graphen, Control-Bitmaps. Der Seed wird *abgeleitet*:
      deterministischer Hash des kanonischen Parameter-Dokuments, die
      Welt-ID ist eine Content-Adresse. Varianten = gezählter
      Hash-Suffix („zehn ähnliche“). Hash-Namensräume je Layer: wer
      eine Sektion ändert, würfelt nur deren Zufallsstrom neu — der
      Rest der Welt bleibt identisch. Gezielte Änderungen wirken wie
      Inpainting in der Bildgenerierung: die Control-Bitmaps sind die
      Maske — eine gemalte Änderung würfelt nur die Zellen neu, die
      sie lesen (plus Blend-Rand), nie die ganze Welt
- [ ] Deterministischer Generator-Kern: Noise-Felder und
      Regelauswertung auf den abgeleiteten Strömen —
      plattformübergreifend reproduzierbar, im Multiplayer wandert die
      Beschreibung (oder ihr Hash) statt Geometrie
- [ ] Runtime-Chunk-Generierung: der Streamer behandelt generierte
      Zellen wie Datei-Zellen (derselbe `CellSource`-Trait) —
      unendliche Terrain unter den bestehenden Budget-, Tier- und
      HLOD-Regeln
- [ ] PCG-Graphen als ECS-Schedules: Spawner, Biome-Regeln,
      WFC-artige Tile-Assemblierung — einmal authored, läuft im Spiel
      und im Editor (doppelte Adressierbarkeit: jeder Generator ist
      ein Registry-Tool)
- [ ] Erzeugen → Nachbearbeiten: Hand-Edits bleiben als
      Delta-/Override-Regionen über der generierten Basis liegen; ein
      Bake-Schritt exportiert jede Region zu statischen Zellen

## M13 — Geräte-Profile & Auto-Downgrade ⬜

*Produkt-Testfall des Eigentümers: ein MMO-Client, der auch auf
Raspberry-Pi-Hardware laufen muss. Die Engine-Regel kehrt die
übliche um: das Spiel wird nicht klein für das schwächste Gerät
gebaut, sondern richtig — und die Engine skaliert es herunter.
Ehrliche Grenze: kein vollautomatisches „läuft überall“ wird
versprochen; geliefert wird erkennen + vorschlagen + ein Klick
ausführen, denn jeder Schritt ist ein Registry-Tool (doppelte
Adressierbarkeit, M11), das Mensch und Agent gleich aufrufen.*

- [ ] Geräte-Profile als Daten: Fähigkeits-Manifeste (RAM-Kappe,
      GPU-Tier, Ziel-fps, Feature-Flags) — Pi 3/4/5 (V3D), WebGPU,
      Mobile, Desktop. Dieselben Profile treiben das CI-Render-Harness,
      damit „Pi-Tier“ eine messbare Aussage ist, kein Gefühl.
      Render-Felder pro Profil: interne Render-Skala + Upscaler-Wahl
      (Pi-Tier = FSR1 räumlich, Desktop = FSR2/3/DLSS) +
      Textur-Zielformat (Pi = ETC2, Desktop = BC7, Web =
      Basis-Transcode) — siehe
      [rendering.de.md](rendering.de.md) „Texturkompression ist eine
      Offline-Entscheidung"
- [ ] Profil-Prüfer: ein Gang über das Projekt, der *meldet*, was das
      Ziel bricht — Texturen über der RAM-Kappe, Pflicht-Deferred,
      Partikel-Budgets, Entity-Dichte, Draw-Call-Zahl — mit Zahlen und
      dem jeweils genannten Fix-Werkzeug je Befund
- [ ] Downgrade-Werkzeugkette (Registry-Tools, jeder Schritt rückholbar
      und protokolliert): Mip-Kappung bei Texturen, neu geretargete
      Asset-Varianten, Textur-Back ins Block-Format des Profils (ETC2
      für Pi/Mobile, BC7 für Desktop, Basis als Einziquellen-Weg),
      High-Poly → LOD/Optimieren, Echtzeitlicht → backen, 3D-Requisite
      → Sprite-Backen über den Iso-Baker (`re-core::iso`),
      Vorwärts-Fallback statt Deferred (ADR-0003-Tiering)
- [ ] Beweis-Schleife: das Spiel gegen ein Profil im Headless-Harness
      laufen lassen — Frame-Zeit, Speicher, Bandbreite pro Tier
      gemessen; der Roadmap-Kasten erst sichert sich, wenn ein echtes
      Projekt auf echter Pi-Hardware besteht

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
