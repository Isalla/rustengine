# Render- und Effekte-Pipeline

[English](rendering.md) | Deutsch

Plan der visuellen Engine: von der aktuellen Rasterisierung (M1) über
moderne Pipelines bis Ray-/Pfad-Verfolgung. Alles unter einem Dach:
HDR-Pipeline durchgängig, jede Stufe austauschbar, Fallback-Kette nach
unten immer spielbar — auch im Browser.

## 0. Grundlagen, die alle Stufen tragen

* **HDR-Pipeline:** Szenenwerte physikalisch (Nits), Tonemapping (ACES,
  AgX als alternative Kurve) als eigener Post-Pass, Belichtung
  automatisch mit Augen-Adaption und manuellem Override.
* **Temporale Daten ab M5 verpflichtend:** Tiefen- und
  Bewegungsvektor-Puffer sind Teil der Pipeline, weil Upscaler, GI,
  Reflexionen und Reconstruction darauf aufbauen.
* **Quality-Tier-System:** jedes Feature meldet, welche Stufe
  (WebGPU / Deck/Integriert / Desktop / Desktop+RT) es braucht; der
  Projekt-Renderer setzt daraus ein Pipeline-Profil zusammen.

## 1. Stand M1/M2 (Rasterisierung V1) — ✅

* Forward+ mit Cluster-Lichtern, kaskadierte Schatten, Post-Kette
  (Bloom, Tonemap), glTF-Materialien; Offscreen und Fenster über
  `GfxBackend`/wgpu (Vulkan/Metal/WebGPU, DirectX über wgpu).

## 2. Pipeline V2 (M5) — Deferred HDR und Schatten 2.0

* G-buffer-Deferred-Lighting (albedo, normal, material-IDs,
  Depth+Motion) für viele dynamische Lichter; Forward-Patch für
  transparente/emissive Objekte.
* **G-Buffer-Layout — entschieden ([ADR-0003](adr/0003-gbuffer-first-visibility-buffer.de.md)):**
  Box 1 erscheint mit dem klassischen MRT-G-Buffer (Albedo+Metallic,
  oktaedrische Normalen, Roughness/Material, Depth+Motion) — der
  einzige Oberflächen-Lieferant, den jede Stufe inklusive WebGPU läuft.
  Der Lighting-Pass liest über eine abstrakte
  Oberflächen-Abfrage-Schnittstelle; der **Visibility Buffer**
  (Instance+Primitive-ID, Material aus Bindless-Arrays) kommt später
  als zweite Quelle für die Desktop-Stufe, zusammen mit dem
  GPU-getriebenen Pfad unten — gleiches Lighting, ~60 MB → ~8 MB
  Geometrie-Traffic bei 1080p unter der Strategie-Kamera.
* **Render-Graph (umgesetzt, M5):** die Pass-Kette (Cull → G-Buffer →
  Lighting → Post → FSR1) wird einmal als Daten in `re_render::graph`
  deklariert; `re-wgpu` kompiliert sie je Konfiguration, und der
  Executor folgt dem kompilierten Ablauf — der Code schreibt die
  Reihenfolge nicht mehr vor (der Sortierer stellt das Licht-Culling
  vor die Geometrie; beide sind unabhängig). Die Übergänge liegen dem
  Plan bei: auf wgpu implizit (das Backend leitet sie aus Attachment-
  und Bind-Nutzung ab), als Daten für spätere explizite Backends.
  Textur-Aliasing folgt mit einem eigenen Target-Pool — die
  Lebensspannen sind berechnet, wgpu allokiert noch selbst. Der
  Shadow-Pass zieht als Knoten ein, wenn Schatten 2.0 kommt.
* Schatten 2.0: kaskadiert mit stabilisierten Texeln, gescreente
  Schatten-Karten-Atlas; vorbereitend für Virtual-Shadow-Map-artige
  Feinheiten (spätere Stufe, braucht Mesh-Streaming aus M3).
* **GPU-getriebener Draw-Pfad (nach den Kern-Boxen):** bindless
  Textur-/Material-Arrays (wgpu `TEXTURE_BINDING_ARRAY` + Non-Uniform
  Indexing, wo unterstützt), Compute-Culling (Frustum +
  Hi-Z-Occlusion-Pyramide) schreibt Indirect-Draw-Argumente; die CPU
  schickt ein Multi-Draw pro Mesh-Batch. Wo die Features fehlen, gilt
  der klassische Pro-Batch-Pfad als Fallback.
* Watchlist: Meshlets / Mesh-Shader (Nanite-artige Geometrie) sind in
  wgpu noch experimentell; die VSM-Feinheiten oben sind der
  Einstiegspunkt, sobald sie reif sind.
* SMAA + optionales TAA; alles in WGSL über wgpu. Debug-Pfad:
  RenderDoc-Frame-Capture funktioniert auf den nativen
  Vulkan/DX12/Metal-Pfaden ohne Zusatzarbeit.
* **TAA-Qualitäts-Doktrin (Produkt-Anforderung: kein Brei, keine
  Geister).** TAA ist der Pass, den schlampige Umsetzung doppelt
  bezahlt: zu viel Akkumulation macht Geister, zu hartes Klemmen macht
  Brei. Bindende Regeln für die GPU-Implementierung (CPU-Referenz:
  `re-render::temporal`):
  * Akkumulationsfenster begrenzen (≤ 8–16 wirksame Samples) und bei
    Disokklusion vollständig zurücksetzen — eine unendliche History
    ist Blur-Quelle Nr. 1.
  * History-Zugriff darf nicht verwischen: gedithelter oder
    bikubischer Sample, nie pauschal bilinear (Doppel-Verwischung ist
    der klassische TAA-Brei).
  * Klemmen in YCoCg mit Variance-Box plus Dehnung zur History
    (Jimenez 2014) — harte RGB-Boxen shimmern an Kontrastkanten.
  * Velocity muss subpixelgenau sein (Objekt- plus Kamerabewegung,
    Reverse Z); falsche Velocity geistert auf eine Art, die kein
    Clamp verstecken kann.
  * Ein Schärfen-Pass (CAS-Art) nach dem temporalen Resolve stellt
    den Kantenkontrast wieder her — Schärfen gehört zur TAA, nicht zu
    den optionalen Extras.
  * Qualitäts-Gates, bevor eine TAA-Version erscheint: auf
    Referenzszenen (Schwenk, Rotation, feine Geometrie — Zaun/Gras/
    Stromleitung) gegen supersampled Ground Truth gemessen; keine
    Ghost-Pixel über Schwellwert, Schärfe innerhalb weniger Prozent
    der Ground Truth. Die konkreten Zahlen setzt die erste
    Implementierung, sie bleiben in dieser Datei stehen.
* **Upscaling-Adapter (`re-upscale`)** — Trait, Backends als Module:
  * **FSR 1 (EASU + RCAS) — umgesetzt** (M5): CPU-Kern `re_render::fsr`
    (Port von AMD `ffx_fsr1.h`, MIT) + WGSL-Spiegelung in der
    Deferred-Kette hinter dem Tone-Mapping. Pi-Pfad: intern mit
    1/Skala rendern, EASU + RCAS aufs Display, 8-Bit-Zwischenpuffer
    wie bei AMD. Die WGSL spiegelt den CPU-Kern Zug-fuer-Zug
    (Pixel-Diff-Tor); das Qualitaetstor verlangt, dass FSR1 im MAE
    gegen den nativen Frame Bilinear schlaegt.
  * **FSR 2/3** (zeitlich + Frame Generation): AMD liefert MIT-lizenzierte
    Shader; Port nach WGSL für alle Plattformen, natives Timing auf
    Desktop.
  * **FSR 4 / „Redstone"** (ML-Super-Resolution, Ray Regeneration, Frame
    Generation): quelloffen (MIT), aber hardware-seitig an RDNA 4
    gebunden; nur natives Backend.
  * **XeSS**: läuft über mitgelieferten SPIR-V-Dispatcher; nur natives
    Backend, nicht WebGPU. XeSS 2 (Frame Generation) ist an Arc gebunden.
  * **DLSS (Super Resolution, Ray Reconstruction, Frame Generation)**:
    proprietäres NVIDIA-SDK, nur natives Backend (`re-native`),
    optionaler Feature-Build; Lizenzbedingungen des SDK bleiben beim
    Anwender. Aktuelle Generation: DLSS 4/4.5 (RTX, Multi-Frame
    Generation); Referenz-Integration: `dlss_wgpu` (bevyengine) zeigt,
    dass der wgpu-Weg funktioniert.
  * Keine der Adapter-Firmen verlangt eine Freigabe pro Titel — die
    Grenzen sind Lizenztyp, Hardware-Bindung und Plattform. Proprietäre
    Binaries werden nie in diesem Repo gebündelt: das Spiel-Projekt
    stellt den SDK-Pfad selbst bereit (wie UE/Unity es handhaben).
  * Kein Upscaler ist Pflicht: natives 1:1-Rendern bleibt Modus.
* Motion-Vektor- und Tiefenformate von Anfang an kompatibel zu FSR2/3
  gehalten (Reverse-Z-Konvention dokumentiert).
* **Texturkompression ist eine Offline-Entscheidung (Asset-Back,
  M13):** GPUs lesen nie rohes RGBA — sie lesen blockkomprimierte
  Formate, und welche Block-Familie eine GPU in Hardware dekodiert, ist
  Plattformgesetz: Desktop liefert **BC7/BC3** (die DXT/S3TC-Linie),
  Mobile und Raspberry Pi (V3D) liefern **ETC2** in Hardware — DXT ist
  *kein* Pi-Merkmal, es ist die Desktop-Familie — modernes Mobile
  zusätzlich **ASTC** (in v3d nicht). Ein Quell-Asset, eine
  zurückgebackene Variante pro Geräte-Profil; **Basis Universal** als
  optionaler Einziquellen-Weg (eine Datei, beim Laden ins Zielformat
  transcodiert). Komprimierte Texturen bleiben in RAM/VRAM komprimiert
  und dekodieren beim Auslesen kostenlos — auf einer bandbreiten-hungrigen
  GPU wie der Pi-GPU ist das ein Bildraten-Merkmal, nicht nur ein
  Speicher-Merkmal. Der M13-Profil-Prüfer meldet unkomprimierte oder
  falsch geretargete Texturen namentlich; die Upscaler-Parung des
  Pi-Tiers ist FSR1 (siehe oben).

## 3. Dynamische GI und Reflexionen (M6) — „Lumen-artig"

Zwei Pfade mit gemeinsamer Anbindung an die Oberflächendaten:

* **Software-Ray-Tracing (Standard, läuft überall):** Mesh-Distance-
  Felder pro Objekt + globales SDF-Clipmap um die Kamera;
  Surface-Cache (Texel-Bindung) hält beleuchtete Oberflächendaten vor.
  Diffuse GI über Raymarch gegen SDFs, Screenspace-Ergänzung für
  nahe Details.
* **Hardware-Ray-Tracing (optional, Desktop+RT):** Ray Queries/
  Raytracing-Pipeline über natives Backend (Vulkan), sobald wgpu-
  Extensions produktionsreif sind, wandert der Treiber dorthin; die
  Algorithmen bleiben identisch.
* Reflexionen: Kette aus Screenspace → SDF/RT-Strahlen →
  Light-Probe-Anteil; jeder Treffer endet im Surface-Cache, damit GI
  und Reflexion dieselbe Energie teilen.
* Volumetrischer Nebel & Lichtschächte: ein Foxel-Raster (3D-Textur),
  pro Cluster per Compute injiziert — Atmosphäre auf jeder Stufe, auf
  WebGPU in niedriger Auflösung.
* Radiance Cascades: weiche GI und Schatten rein per Compute als günstige
  Stufe neben dem SDF-Pfad — ohne Ray-Tracing, browserfähig.
* WebGPU-Stufe: Probes + SSAO/SSR, keine Strahlen; identische
  Materialien, weniger Auflösung der Effekte.
* Referenzen: „Unreal Engine 5 Best Practices" und
  „UE5 Performance Optimization Cookbook" für Betriebsparameter;
  Paper-Wissen (The Order: Real-time Global Illumination) fließt als
  Verfahren ein.

## 4. Pfad-Verfolgung und Rekonstruktion (M7)

* **Progressiver Pfad-Tracer** als eigener Renderer-Modus (PC/
  Konsolen-Backends): dieselbe Szene, BVH aus dem Streaming-System,
  viele Samples über Frames; Ziel ist „Offline-Qualität" im Editor für
  Bakes und Standbilder — im Spiel läuft weiter der M6-Pfad.
* **ReSTIR-ähnliche Wichtigkeitsnachverwendung** und zeitliche
  Accumulation gegen Rauschen; danach **Ray Reconstruction** als
  Denoiser-Stufe:
  * NVIDIA-SDK (DLSS-SR/RR) als optionales natives Backend,
  * offener Fallback mit eigenem spatio-temporalen Rekonstruktor.
* Bake-Ausgaben (Lightmaps, SDFs, HLODs) erzeugt eine Go-Buildfarm
  (Abschnitt 2 in [architecture.de.md](architecture.de.md)), damit die
  Engine-UI beim Berechnen antwortbar bleibt.

## 5. VFX: Partikel und Effekte (M8)

* GPU-Partikel (Compute): Emission, Kräfte, Kollision gegen SDFs und
  Heightfield, Ribbon- und Mesh-Renderer; Effekte als editierbarer
  Node-Graph im Editor (Niagara-artig, datengetrieben).
* Screen-Space-Effekte-Bibliothek: Distortion, Hitzeflimmern,
  Explosionen, Treffer-Blitze — Rezept-Sammlung aus dem
  „UE5 VFX Cookbook" auf eigene Renderer-Begriffe übersetzt.
* Budgets pro Effekt-Klasse (Anzahl, VRAM, Fills) laufen über die
  Capability-Registry, damit KI und Tools sie einhalten.

## 6. Audio und SFX (M8)

* Rust-Runtime-Mixer: Busse, Sidechaining, Reverb-Zonen, 3D-
  Ortsbestimmung; Sample-Wiedergabe mit Streaming aus der Asset-DB.
* **SFX als Events, nicht als Dateien:** Events (Tree/Graph) triggern
  Layer, Zufallsvarianten, Pitch-Volumen-Streuung — editierbar wie
  Partikel-Graphen, auslösbar durch Animation-Events und Gameplay-
  Signale.
* Konsolen/Headless-Tests: Stumm-Backend, das nur Graphen validiert.

## 7. Animation und Figur (M8)

* Skelett-Animation (beschleunigt, GPU-Skinning), Blend-Trees und
  State-Machines als Editier-Graphen; „UE5 Character Creation,
  Animation and Cinematics" als Referenz für Workflows.
* Für das Leitfeature: dieselbe Einheit muss als Truppen-Billboard,
  Spiel-Mesh und Ego-Kamera-Träger existieren — die Stufen teilen sich
  Transform und Zustandsmaschine.

## 8. Web-Pfad (M9, fortlaufend testbar)

* wgpu→WebGPU: Pipeline V2 mit SMAA/TAA und räumlichem Upscale,
  GI-Probes statt Strahlen; Target ist ein spielbarer Strategiespiel-
  Prototyp inkl. Kameraflug (Qualitätsstufen laufen auch dort).
* Go-Dienste bleiben Server-seitig; der Browser ist Client wie jeder
  andere.

## 9. Meilensteine dieser Pipeline

| Stufe | Inhalt | Gate |
|---|---|---|
| M5 | Deferred HDR, Schatten 2.0, `re-upscale` (NIS + FSR2/3-Port), Motion/Depth, Render-Graph, GPU-getriebener Draw-Pfad | identische Szene spieltbar auf Deck und Desktop |
| M6 | GI/Reflexionen (SDF+Surface-Cache, HW-RT-Option), volumetrischer Nebel, Licht-Editor | innere Szene ohne Bakes korrekt ausgeleuchtet |
| M7 | Pfad-Tracer + Rekonstruktion, Bake-Ausgaben | Standbild in Offline-Qualität aus Spiel-Szene |
| M8 | GPU-Partikel + Effekt-Graphen, Audio-Mixer + SFX-Events | Testszene: Beschuss mit Licht, Rauch, Sound |
| M9 | WebGPU-Build des Leitfeatures | Kameraflug Strategie→Ego im Browser |

## 10. Realistische Grenzen

* DLSS/XeSS-SDKs sind Hardware- und Lizenz-abhängig — sie sind Adapter,
  nie Grundvoraussetzung; die Engine funktioniert ohne sie vollständig.
* Hardware-RT unter wgpu ist Bewegtes Ziel; der SDF-Pfad ist bewusst
  kein Provisorium, sondern die tragbare Grundlösung.
* Große Szenen kosten Speicher: Die VRAM-Budget-Regeln aus
  [vram.de.md](vram.de.md) gelten auch für GI-Zwischenpuffer.
