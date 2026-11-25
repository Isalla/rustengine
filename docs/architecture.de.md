# Architektur: Sprachen, Module, Weltstreaming

[English](architecture.md) | Deutsch

Dieses Papier hält die Grundentscheidungen fest, die aus dem Leitfeature
folgen (unten). Es ersetzt keine Details: Rendering-Pipeline und
Licht-Qualitätsstufen sind in [rendering.de.md](rendering.de.md) eigens
beschrieben. Einzelne Grundentscheidungen sind als
[Architecture Decision Records](adr/README.de.md) festgehalten.

## 1. Leitfeature: Strategie ↔ Ego ohne Ladebildschirm

Ziel ist ein Strategiespiel, das nahtlos aus der Übersichtsperspektive in
die Ego-Perspektive einer Einheit wechselt:

* Die Kamera **fliegt auf die Einheit zu**, das Bild fährt in sie hinein
  und übernimmt dabei die Ego-Steuerung — ohne Schnitt, ohne Blende, ohne
  Ladebildschirm.
* Dieselbe Welt, dieselben Objekte: Was aus der Distanz ein
  Billboard-Trupp ist, ist aus der Nähe eine detaillierte Figur in
  begehbarem Gelände.
* Der Rückwechsel funktioniert genauso (Einheit „verlassen", Kamera
  fliegt hinaus und wird wieder zur RTS-Kamera).

Was das technisch von der Engine verlangt:

1. **Streaming statt Level-Laden:** Alle Maßstäbe (Karte, Gebäude,
   Einheit, Innenraum) liegen in einem Koordinatensystem und werden nach
   Bedarf herangeschafft und freigegeben (Abschnitt 5).
2. **Qualitätsstufen pro Objekt:** Eine Einheit existiert gleichzeitig
   als Billboard-Batch, als Spiel-Mesh und als Detail-Mesh; die Stufen
   übergeben sich während des Fluges weich (Abschnitt 5.4).
3. **Kein Frame-Knoten:** Übergänge laufen über Pooling und
   Detail-Fade, nicht über Szenenwechsel; die ECS-Welt ist zu jedem
   Zeitpunkt konsistent.
4. **Kamera als erstklassiges Objekt:** Spline-Flug, der Gelände
   ausweicht, Übergabe der Eingabe — dieselbe Mechanik wie die
   Kamera-Timeline in [cinema.de.md](cinema.de.md).

## 2. Sprachaufteilung: Rust und Go

| Schicht | Sprache | Beispiele |
|---|---|---|
| Engine-Laufzeit (Renderer, ECS, Physik, Audio, Streaming) | Rust | `re-*`-Crates |
| Editor (UI, Szenen-View, Import-Pipelines) | Rust | `re-editor` |
| Module und Import-Werkzeuge | Rust oder jede Sprache | `sc1-studio` (Sprite-Import eines klassischen Strategiespiels) |
| **Dienste und Werkzeuge** | **Go** | dedizierter Game-Server, Matchmaking/Lobby, Asset-Buildfarm, Hotloader/Launcher, Telemetrie-Receiver |

Grundsätze:

* **Go läuft nie im Frame-Loop.** Rendering und Simulation verlangen
  deterministische Latenzen; ein Garbage-Collector hat im Bildpfad nichts
  verloren.
* **Die Grenze ist der Prozess.** Go-Dienste sprechen über gRPC/protobuf
  (Server) oder CLI + JSON (Werkzeuge) mit der Engine. Ein abstürzender
  Build-Worker wirft nicht das laufende Spiel heraus.
* **Spiel-Logik der Serverseite ist Go oder Rust, beides vorgesehen:**
  Zustands-Simulation (Lobby, Account, Asset-Katalog) gern in Go; wo
  Spielregeln 1:1 mit der Engine geteilt werden müssen, läuft der
  dedizierte Server als reiner Rust-Prozess mit Headless-Backend.
* Die Capability-Registry (siehe
  [ki-integration.de.md](ki-integration.de.md)) wird Dienste-Kommandos ebenfalls
  als JSON beschreiben; ein Go-Werkzeug kann sie damit wie ein KI-Agent
  bedienen.

## 3. Modul-Architektur: drei Koppelungsstufen

Erweiterbarkeit ist das Kernversprechen — neue Formate, neue Importe,
neue Generatoren sollen nicht in den Engine-Kernel müssen:

1. **In-Process-Modul (Rust-Crate):** registriert Capabilities und
   Asset-Importer in der Registry, nutzt `re-core`-Typen. Maximale
   Leistung, maximale Integration (Beispiel: künftiges
   KI-Generierungsmodul Bild→Mesh).
2. **Out-of-Process-Werkzeug (jede Sprache):** ein CLI-Programm mit
   dokumentiertem JSON-/Datei-Vertrag (Eingabe: Ordner oder Manifest,
   Ausgabe: Engine-Asset). Die Engine startet es, liest das Ergebnis und
   versioniert es als Import-Pipeline-Stufe. So dockt heute `sc1-studio`
   an: GRP-Sprite-Folgen → RGBA-Sheet + Zeilenplan → Engine-Billboard.
3. **Dienst (Go, langlebig):** eigener Prozess mit Netzwerk-Interface —
   Game-Server, Buildfarm, Übersetzer-/LLM-Gateway. Engine und Editor
   sind Clients. Vorhandene Instanzen (auch fremde, z. B. eine
   vorinstallierte ComfyUI) werden lediglich im Projekt als Endpunkt
   eingetragen — die Engine installiert und startet selbst nichts.

Alle drei Stufen melden sich in derselben Registry an (Stufe 2/3 über
eine Adapter-Capability), wodurch Editor-UI, Tastaturbefehle und
KI-Agenten die Erweiterungen identisch sehen.

## 4. Datenhaltung und Szenenformat

* Szenen bleiben das JSON-basierte `.wbproj`-Format mit
  Content-Addressing für Binärdaten (Meshes, Texturen).
* **Asset-DB als Zwischenschicht:** Der Import schreibt normalisierte
  Assets in einen Projekt-Cache (Hash → Datei), die Szene referenziert
  nur Hashes. Das macht Streaming, Re-Import und HLOD-Builds
  idempotent.
* Go-Buildfarm-Werkzeuge konsumieren denselben Cache als Verzeichnis —
  keine proprietäre Datenbank zwischen den Prozessen.

## 5. Welt-Streaming ohne Ladebildschirm

Angelehnt an World-Partition/HLOD-Konzepte aktueller Engines
(Referenz: „Building Open World Landscapes with Unreal Engine 5"), für
einen Rust-Renderer gebaut:

1. **Grid-Zellen:** Die Welt ist in gleich große Cell-Blöcke zerlegt
   (Konfiguration pro Projekt, typisch 64–256 m). Jeder Block enthält
   Transform-Listen, Kollision und Sound-Regionen — klein genug für
   Streaming, groß genug, um Culling-Aufwand zu sparen.
2. **Clipmap-Ebenen:** Um die Kamera liegen mehrere Ebenen mit
   wachsender Zellgröße und sinkender Detaildichte; weit entfernte
   Topografie rendert als HLOD-Mesh.
3. **Async-Load-Pipeline:** IO-Thread-Pool lädt Block-Dateien, ein
   Worker zerlegt sie in GPU-Resources; Übergabe erfolgt über
   Triple-Buffering mit Resource-Handles, damit Render- und Update-Thread
   nie blockieren. Sichtwechsel ändern nur Prioritäten, nie die Szene.
4. **Qualitätsstufen (LOD-System „T0..T4")** pro Instanz:
   * `T0` — Cinematic-/Nah-Detail-Mesh (KI- oder Künstler-Mesh),
   * `T1` — Spiel-Mesh mit Animation,
   * `T2` — Visual-Hull/vereinfachtes Mesh (aus Sprite-Ansichten
     rekonstruiert),
   * `T3` — Billboard-Batch (32 Richtungen, ein Draw-Call pro Trupp),
   * `T4` — Icon/Marker auf der Minimap.
   Der Kameraflug (Abschnitt 1) ist damit ein reiner
   Stufen-Übergangs-Task: `T3 → T2 → T1 → T0` beim Hineinfliegen.
5. **GPU-getriebenes Culling:** Instanz- und Dreiecks-Culling läuft auf
   der GPU (Indirect Draw), Okklusion über Hi-Z; die CPU schickt nur
   Batch-Parameter.
6. **Pooling statt Entladen:** Einheiten-Instanzen und Effekt-Pools
   werden wiederverwendet, Resourcen bleiben nach Verlassen einer Zone
   noch einige Sekunden reserviert (Eviction-Verzögerung) — der
   Rückflug kostet keinen IO.

**Speicher-Anforderungen.** Jede Zelle ist eine kleine Datei
(`l{level}_x{x}_y{y}.bin`), das Streaming liest also viele kleine,
spitze Zugriffe — entscheidend sind IOPS und Latenz, nicht
sequenzieller Durchsatz. Richtwerte für eine volle Clipmap-Bestückung
(hunderte residente Zellen, schnelle Flüge erzeugen hunderte
Kleinzugriffe pro Minute):

| Medium | Urteil |
| --- | --- |
| NVMe-SSD | Ziel — Vollspeed-Flug hält jede Stufe warm |
| SATA-SSD | Minimum — Streaming läuft dauerhaft, feine Stufen haken kurz bei Maximalforderung |
| Drehplatte | läuft mit Abstrichen — Budget und Tier-Hysterese halten das Grobe länger; der Render-Loop blockiert trotzdem nie |

Die Engine verlangt kein bestimmtes Laufwerk: Die IO-Pipeline ist voll
asynchron (`re-world::io`), und alles Langsame wird zuerst
Detail-Armut, nie ein Ruckler oder ein Ladebildschirm. Vorgesehen sind
darauf IO-Telemetrie — gemessene Lade-Latenz und Durchsatz treiben
Tier-Hysterese und Verweilzeit adaptiv, damit eine langsame Platte
früh abwertet, statt spät Zellen zu verlieren (siehe
[roadmap](roadmap.de.md), M3).

## 6. ECS

* Archetyp-basierte ECS (`re-ecs`, M3): Komponenten als einfache
  SoA-Speicher, Systeme als Jobs; der Renderer liest die Weltdaten über
  eine View-Schnittstelle ohne Zwischenkopien.
* Szenengraf-Knoten (der Editier-Zustand aus `re-core`) sind beim
  Start Quelle der ECS-Instanzen; Editor-Änderungen schreiben zurück.
* Transform-Propagation läuft als parallelisierter System-Pass
  (Referenzen: „Data-Oriented Programming", „C++ Concurrency in Action"
  übertragbar auf Rust-Muster).

## 7. Editor

* Immediate-Mode-UI auf dem eigenen Renderer (ein Fenster, ein VRAM-
  Budget; kein zweites GUI-Toolkit im Kompositions-Bild).
* Paneele: Szenen-View mit Gizmos, Outliner, Inspector (Capabilities
  als Formulare), Asset-Browser mit Import-Stufen, Konsole.
* **Jede UI-Aktion ist eine Capability mit Undo-Eintrag** — dasselbe
  Protokoll bedient Maus, Skripte und KI-Agenten; das ist die
  Unreal-Editor-Erwartung: alles benutzbar, alles automatisierbar.

## 8. Entscheidungen (Kurz-ADRs)

* **WGSL bleibt die einzige Shader-Quelle.** Für RT- und DLSS-Pfade wird
  ein optionales natives Backend (`re-native`, Vulkan) eingeführt, das
  zusätzlich HLSL/SPIR-V-Kernel kompilieren kann; der Standardpfad bleibt
  davon unberührt.
* **wgpu zuerst, native Extensions wo nötig.** Raytracing und DLSS/FSR3
  Frame-Generation brauchen Treiber-Zugriff, den wgpu heute nicht
  garantiert; sie laufen hinter Feature-Flags mit Screen-Space-Fallback.
* **Go außerhalb des Bildpfads** (Abschnitt 2), keine FFI in
  Simulations- oder Render-Schleifen.
* **Lizenz MIT**, Copyright trägt der Urheber; Module erben dieses Modell.
