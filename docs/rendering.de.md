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
* **Offene Entscheidung — G-Buffer-Layout:** klassisches MRT-G-Buffer
  vs. Visibility Buffer (der erste Pass schreibt nur Instance- +
  Primitive-ID, der Lighting-Pass rekonstruiert die Materialdaten aus
  Bindless-Arrays). Der Visibility Buffer passt zum GPU-getriebenen Pfad
  unten; entscheiden, bevor der erste GPU-Pass landet.
* **Render-Graph:** die Pass-Kette (Shadow → G-Buffer → Lighting → Post)
  wird einmal als Daten deklariert; der Graph plant die Pässe, verwaltet
  ihre Zwischen-Texturen (Zuteilung und Aliasing) und die
  Textur-Übergänge über die Backend-Traits — die Backends bleiben dumm,
  die Pipeline bleibt deklarativ.
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
* **Upscaling-Adapter (`re-upscale`)** — Trait, Backends als Module:
  * `Nis`/`Easu`-artiger räumlicher Upscaler (überall, Pflicht-Fallback)
  * **FSR 2/3** (zeitlich + Frame Generation): AMD liefert MIT-lizenzierte
    Shader; Port nach WGSL für alle Plattformen, natives Timing auf
    Desktop.
  * **XeSS**: läuft über mitgelieferten SPIR-V-Dispatcher; nur natives
    Backend, nicht WebGPU.
  * **DLSS (Super Resolution, Ray Reconstruction, Frame Generation)**:
    proprietäres NVIDIA-SDK, nur natives Backend (`re-native`),
    optionaler Feature-Build; Lizenzbedingungen des SDK bleiben beim
    Anwender.
  * Kein Upscaler ist Pflicht: natives 1:1-Rendern bleibt Modus.
* Motion-Vektor- und Tiefenformate von Anfang an kompatibel zu FSR2/3
  gehalten (Reverse-Z-Konvention dokumentiert).

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
