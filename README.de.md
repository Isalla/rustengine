# RE — Rust Engine

[English](README.md) | Deutsch

![RE — Rust Engine](media/hero.webp)

Open-Source-Spiel-Engine: Engine-Laufzeit und Editor in Rust, Dienste
und Werkzeuge (Server, Buildfarmen, Launcher) in Go. Eine Codebasis für
PC und Web, Konsolen-Backends andockbar. Rendering über wgpu aus einer
einzigen Shader-Quelle (WGSL); Engine-Funktionen sind als typisierte
Capabilities exponiert und können von Skripten, Editoren und KI-Agenten
direkt aufgerufen werden.

RE ist aus einem Worldbuilder genannten Level-Editor hervorgegangen und
hat dessen Datenmodell übernommen: Szenengraf, Meshes, Materialien mit
Texturen, Animationen/Skelette, Terrain und Tilesets, glTF-2.0-Pipeline.

**Leitfeature der Architektur:** nahtloses Wechseln zwischen
Perspektiven und Genres — die Kamera steigt ohne Ladebildschirm und
ohne Schnitt aus der Strategie-Übersicht in die Ego-Perspektive einer
Einheit und übergibt dabei die Steuerung; auch wieder zurück. Die
Engine hält Weltdaten, Streaming und Quality-Tiers während des
Wechsels durchgängig in Bewegung, nichts lädt mitten im Flug neu. Der
Runtime selbst wohnen keine Spielregeln inne — nur deshalb bedient
derselbe Stapel Strategie, Shooter und MMO. Das erste Projekt, das sie
damit prüft, ist ein Strategiespiel; Shooter und MMO sind als weitere
Testfelder geplant. Details: [docs/architecture.de.md](docs/architecture.de.md).

**Status:** frühe Entwicklung. Kern-Datenmodell, Import/Export, erstes
Backend, Fenster-Layer, ECS und der World-Streaming-Kern sind
funktionsfähig — inklusive asynchroner Datei-IO, GPU-Handoff,
Fernfeld-HLOD, IO-Telemetrie und der Steuerungs-Übergabe Strategie↔Ego
mitten im Flug, demonstriert vom `stream-demo`-Rundflug. Als nächstes
folgt der Editor (M4), Audio/Physik Meilenstein für Meilenstein
(siehe unten).

**Privatsphäre:** keine Telemetrie, kein Phone-Home, keine
Update-Checks. Engine-Binaries kontaktieren ausschließlich Endpunkte,
die ein Mensch ausdrücklich gesetzt hat; wo in diesem Repo „Telemetrie"
steht, sind lokale Leistungszähler gemeint
([ADR-0004](docs/adr/0004-no-telemetry-no-phone-home.de.md)).

## Crates

| Crate | Zweck | Status |
|---|---|---|
| `re-core` | Datenmodell: Szene, Mesh, Material, Animation, Terrain, Tilesets | fertig |
| `re-io` | glTF-2.0-Import/Export inkl. Textur-Einbettung | fertig |
| `re-ecs` | Entity-Component-System: Welt, Sparse-Set-Speicher, Schedule | fertig |
| `re-world` | World-Partition: Rasterzellen, Clipmap-Streaming, Quality-Tiers T0..T4, Streaming-Budget, HLOD, IO-Telemetrie, Ego-Übergabe | fertig |
| `re-render` | Backend-neutrale Render-Schicht (`GfxBackend`), Kamera, Billboards | in Arbeit |
| `re-wgpu` | wgpu-Backend: Vulkan/Metal/WebGPU (DirectX über wgpu), Offscreen und Fenster | in Arbeit |
| `re-platform` | Fenster, Eingabe, Event-Loop, Zeit (winit; headless für Tests) | in Arbeit |
| `re-agent` | Capability-Registry + Anbindung von KI-Providern (Ollama, llama.cpp, OpenAI-kompatibel, Anthropic) + Welt-Werkzeuge für Agenten | in Arbeit |
| `re-editor` | Editor-Kern: Szenen-Ansicht, Undo/Redo, Outliner & Inspector über der ECS, glTF-Import, Projekt-Loader — alles als Capabilities | in Arbeit |

Geplant: `re-editor` (M4, in Arbeit), natives Backend `re-native` (M6/M7 für
Raytracing und DLSS), `re-audio`/`re-vfx`/`re-physics` (M8), Go-Dienste
unter `services/` (M9).

## Schnellstart

```sh
cargo build --workspace
cargo test --workspace
```

Headless-Builds ohne Fenster-Bibliotheken:
`cargo build -p re-wgpu --no-default-features`.

## Doku

Bei jedem Push aus diesem Repo veröffentlicht (GitHub Pages):

- **Nutzer-Handbuch:** <https://isalla.github.io/rustengine/handbook/de/> —
  [English](https://isalla.github.io/rustengine/handbook/en/)
- **API-Referenz (Rustdoc):** <https://isalla.github.io/rustengine/rustdoc/>
- **Startseite:** <https://isalla.github.io/rustengine/>

Quellen: Handbuch-Markdown in `docs/handbook/{en,de}`, API-Doku aus den
Crate-Kommentaren selbst (`cargo doc --workspace`).

## Architektur-Prinzipien

- **Eine Shader-Quelle.** Alle Pipeline-Varianten laufen auf WGSL über
  wgpu; Linux/Vulkan ist Erstziel, DirectX und WebGPU folgen dem Backend.
- **Eine Fähigkeitsschicht.** Editor-Werkzeuge, Konsolenbefehle und
  KI-Aufrufe teilen sich dieselbe Registry mit JSON-Schemata und
  Berechtigungs-Hook: [docs/ki-integration.de.md](docs/ki-integration.de.md).
- **Ein Prozess, ein VRAM-Budget.** Inhaltsgenerierung läuft als
  Modul der Engine, nicht als zweites Programm, das um den
  Grafikspeicher konkurriert: [docs/vram.de.md](docs/vram.de.md).
- **Schnittstelle statt Gestrüpp.** Backends (Grafik, Plattform, Audio)
  implementieren öffentliche Traits; plattformspezifischer Code ist
  austauschbar angekoppelt.

## Meilensteine

Die lebende Checkliste mit jedem Unterpunkt: [docs/roadmap.de.md](docs/roadmap.de.md).
Systematische Fahndung — Bedrohungsmodell, Audit-Gänge und Fund-Log:
[docs/security.de.md](docs/security.de.md) · [docs/bugs.de.md](docs/bugs.de.md).

- **M0** — Repo, Workspace, Kern-Datenmodell und glTF-Pipeline ✅
- **M1** — `re-render` + `re-wgpu`: Szenen mit glTF-Texturen rendern (Offscreen) ✅
- **M2** — `re-platform`: Fenster, Eingabe, Delta-Time, Surface-Rendering ✅
- **M3** — ECS + Welt-Streaming ✅ (`re-ecs` + `re-world`: Raster, Clipmap, Tiers, asynchrone IO-Pipeline, GPU-Handoff, HLOD, IO-Telemetrie, Ego-Übergabe, Welt-Werkzeuge für Agenten)
- **Wiki** — Doku in drei Stufen 🟡: Rustdoc auf GitHub Pages ✅, Nutzer-Handbuch EN/DE als mdBook ✅, API-Browser im Editor (M4)
- **M4** — Editor: Szenen-View, Outliner, Inspector, Import-Pipelines, Undo
- **M5** — Render-Pipeline V2: Deferred HDR, Schatten 2.0, Upscaling-Adapter (FSR/XeSS/DLSS) ([docs/rendering.de.md](docs/rendering.de.md))
- **M6** — Dynamische GI und Reflexionen: Lumen-artiger Software-RT-Pfad, Hardware-RT optional
- **M7** — Pfad-Verfolgung und Ray-Reconstruction (progressiv, Offline-Modus, Bake-Ausgaben)
- **M8** — Audio (SFX-Events), VFX (GPU-Partikel, Effekt-Graphen), Physik, Animation
- **M9** — Go-Dienste (Netcode, dedizierter Server, Buildfarm) + WebGPU-Build
- **M10** — Konsolen-Backends (private SDK-Module), Publishing
- **M11** — KI-nativer Editor: ein Assistenten-Panel (Passiv/Live/Chat), jede Editor-Fähigkeit ein Registry-Tool für Menschen und Agenten gleichermaßen ([docs/ki-integration.de.md](docs/ki-integration.de.md), ADR-0002)
- **M12** — Prozedurale Welten: Autoren-Beschreibung zuerst, Seed daraus abgeleitet (Content-Adresse); Runtime-Chunk-Gen über denselben `CellSource`, PCG-Graphen als ECS-Schedules, Erzeugen → Nachbearbeiten
- **M13** — Geräte-Profile & Auto-Downgrade: Profile als Daten (Pi 3/4/5 → Desktop), Profil-Prüfer, rückholbare Downgrade-Werkzeugkette (Mip-Kappung, LODs, Sprite-Backen, Vorwärts-Fallback) ([docs/roadmap.de.md](docs/roadmap.de.md))
- **Leitfeature** — Strategie↔Ego: Kameraflug in die Einheit ohne Ladebildschirm; die Steuerungs-Übergabe selbst funktioniert bereits mitten im Flug (M3e), die volle Flug-Choreografie folgt mit [docs/cinema.de.md](docs/cinema.de.md)

## Konsolen

Offene Ziele (PSP, Nintendo-Switch-Homebrew u. ä.) werden vollständig in
diesem Repo geführt. Für Devkit-Ziele gilt das in der Branche übliche
Muster: Dieses Repo definiert nur die Backend-Traits; die
SDK-abhängige Implementierung bleibt im privaten Repo des jeweiligen
NDA-Inhabers und wird dagegen gebaut.

## Beitragen

Issues und Diskussionen gern vor größeren Änderungen. Commits klein
halten, `cargo test --workspace` muss grün sein.

## Lizenz

MIT — Copyright (c) 2026 Waldemar Friesen. Wer dieses Projekt oder Teile
davon weiterverbreitet (auch veränderte Fassungen) muss den Copyright-
Vermerk und den Lizenztext unverändert übernehmen; siehe [LICENSE](LICENSE).
Portierter Drittanbieter-Code und mitgelieferte Assets tragen eigene
Hinweise; siehe [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES.de.md).

Eine freundliche Bitte — völlig freiwillig und keine Bedingung: Wenn ein
Spiel oder Werkzeug mit dieser Engine entsteht, würde ich mich über einen
Credit freuen („Powered by the RE Rust Engine“ im Abspann, ein Link auf
dieses Repository oder ein Stern darauf).
