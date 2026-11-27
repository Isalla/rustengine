# KI direkt in der Engine — Konzept (re-agent)

[English](ki-integration.md) | Deutsch

Zwei Fragen, ein Grundmuster:

1. *Kann die KI (lokale Modelle oder ChatGPT/Claude-API) direkt im
   Editor arbeiten, ohne Umweg über MCP?*
2. *Können die Dinge, die 3D-Gen-Studio kann (Bild → animierte 3D-Figur,
   Mesh-Bearbeitung wie in Blender, bewegliche Gebäudeteile), in einem
   Programm passieren, das die KI nur noch durchschaltet?*

Beides ja — und beides ist dasselbe System: **eine Registry für
Engine-Funktionen**, die im Prozess wohnt.

## 1. Ein Kern, zwei Zufahrten

```
                         ┌───────────────────────────────┐
                         │  RE-Editor / Engine-Prozess  │
                         │                               │
  Provider-Trait ◄──────►│  CapabilityRegistry           │◄────► MCP-Server
  (re-agent)            │  (jede Engine-Funktion =      │      (re-mcp,
   · Ollama              │   ein benanntes Werkzeug mit  │       Fallback für
   · llama.cpp           │   JSON-Schema)                │       Claude Desktop,
   · Strata/OpenAI-komp. │                               │       OpenCode & Co.)
   · Anthropic           │  run_agent(provider, reg…)    │
                         └───────────────────────────────┘
```

* **Direkt (bevorzugt):** Editor und KI laufen im selben Prozess.
  `run_agent` schiebt dem Modell die Werkzeugliste, das Modell antwortet
  mit Tool-Calls, die Registry führt sie **ohne Transportkosten** aus —
  direkter Zugriff auf Szene, Meshes, Undo-Stack, Dateisystem.
  HTTP-Endpunkte sind danach *nur noch Details* (Ollama :11434,
  llama-server :8080, OpenAI-kompatible Proxies wie Strata, Anthropic API).
* **MCP (geplant):** `re-mcp` wird exakt dieselbe Registry nach außen
  exportieren. Wo keine Direktverbindung möglich oder gewünscht
  ist (andere Maschine, fremde App), steuert die KI den Editor trotzdem.
  Nichts an Engine-Funktionen wird zweimal geschrieben.

Sicherheit: `PermissionHook` entscheidet vor jedem schreibenden Werkzeug,
ob die KI darf (vorschlagen / rückfragen / automatisch). API-Keys stehen
nie im Projekt, nur als Verweis auf eine Umgebungsvariable.

## 2. "3D-Gen in der Engine" — was wohin gehört

3D-Gen-Studio = Dienste (GPU, Modelle) + Oberfläche + Dateien. RE nimmt
die Rollen auseinander. Grundentscheidung (Motivation und Details:
[`vram.de.md`](vram.de.md)): **Generierung läuft bevorzugt im selben Prozess**,
weil nur ein einziges VRAM-Budget verwaltet werden kann, wenn Editor und
Generator nicht gegeneinander um knappen VRAM kämpfen wollen:

| Fähigkeit (3D-Gen-Studio) | RE-Baustein | Wo läuft es? |
|---|---|---|
| Bild → Mesh (Tripo/Tencent-artige Generatoren) | `GenService`-Trait: in-process-Runner **bevorzugt**, HTTP-/Cloud-Adapter optional | derselbe Prozess (Gewichte bedarfsweise laden/entladen) — oder zweiter Rechner |
| Text → Bild, Bild-Bearbeitung | `GenService` (Bild), ebenso in-process/optional Dienst | derselbe Prozess |
| Retopo / UV / Repair / Bake | `GenService` (Mesh-Verarbeitung) oder CPU-Wege in `re-geom` | Engine (CPU) bzw. GPU-Modul mit Budget |
| Auto-Rig / Rig übertragen | `GenService` + `re-core::skeleton` | Engine + optionale Diensthilfe |
| Mesh **nachträglich bearbeiten** (Vertices, Extrude, Pivot, Spiegeln, Boole-Ansätze) | Editor-Werkzeuge auf `re-core::Mesh` | **Engine, CPU, deterministisch** |
| Bewegliche Elemente (Türen, Krane, Fallgitter) | Sockets/Joints: `Skeleton`-Instanzen oder `RigidJoint`-Komponente am Bauteil | **Engine** |
| Animation abspielen/retargeten | `re-core::animation` + Trajektorien | **Engine** |
| Alles zusammensetzen + korrigieren | `CapabilityRegistry` + Agent-Loop | **Engine** |

Wichtig: Der Generator-Code von 3D-Gen-Studio ist geschlossen — RE
kopiert ihn nicht, sondern spricht ihn (und ComfyUI und Cloud-APIs) über
eigenes `GenService`-Interface an. Die Engine bleibt offen; der Dienst
ist austauschbar. Zielzustand: offene Pipelines (z. B. TripoSG/Trellis)
laufen als In-Process-Runner direkt in der Engine und melden sich beim
VRAM-Budget (`vram.de.md`) — externe Dienste bleiben als Adapter für zweite
GPU-Rechner und Cloud-Modelle erhalten.

### Dienste eintragen statt installieren

Vorinstallierte Generationdienste (ComfyUI, Mesh-/Rig-Werkzeuge) werden
**nicht von der Engine installiert oder mitgeliefert**, sondern im
Projekt als Endpunkte eingetragen — genau so, wie 3D-Gen-Studio seine
ComfyUI-Instanz über Adresse und Port in den Anwendungseinstellungen
führt:

```json
{
  "services": {
    "comfyui":   "http://127.0.0.1:8188",
    "meshtools": "http://127.0.0.1:8200",
    "rigtools":  "http://127.0.0.1:8300"
  }
}
```

Regeln:

* Die Engine prüft beim Projektstart nur die Erreichbarkeit (z. B.
  ComfyUI `GET /system_stats`) und blendet Generierungs-Capabilities in
  der Registry ein oder aus — niemals startet oder installiert sie
  unbefragt fremde Programme.
* Wer eine eigene Instanz hat (oder eine auf einem zweiten Rechner),
  trägt nur die Adresse ein; das Projekt ist transportabel, der Dienst
  bleibt Sache der Maschine.
* API-Schlüssel stehen weiterhin nicht im Projekt, sondern als Verweis
  auf Umgebungsvariablen (Abschnitt 1).

### Von einem Mesh zu beweglichen Einheiten (Panzer-Fall)

Bild→Mesh liefert EIN Mesh (Wanne, Turret, Räder — ein Klumpen).
Bewegliche Teile brauchen eigene Einheiten, OHNE Neugenerierung:

* `mesh.split_by_parts` — lose Inseln automatisch in Kind-Knoten trennen
  (bei generierten Assets oft der Fall: Turret liegt physisch getrennt auf).
* `mesh.split_region` — manuell per Auswahl (Editor-Brush/Box oder
  KI-Prädikat: "alle Faces oberhalb des RINGS mit Radius r").
  UVs und Textur-Referenz bleiben identisch — nur die Knotenstruktur ändert sich.
* `rig.add_socket(pivot, achse)` — Drehachse je Teil (Turret-Y am Ring,
  Kanonen-X, Rad-Achsen); `rig.attach(part, socket)`.
* `rig.rigid_parts` — ein Gelenk pro loses Teil, der Schnellpfad für
  Roboter/Fahrzeuge.
* Alternative Skinning: Region → Bone statt Trennen (ein Mesh, Performance);
  mischbar mit Sockets.

Der Outliner zeigt den Panzer weiterhin als EINE Gruppe; KI-Kette:
generate → split → sockets → anim. Fehlselektionen korrigiert der Mensch
im Editor (M5) oder die KI wählt die Region neu — nie wird neu generiert.

### Arbeitsweise der KI (Zielbild, M6)

"Baue eine Windmühle mit drehbaren Flügeln":
1. KI erzeugt via `GenService` Gebäude-Mesh + Flügel-Mesh
2. importiert sie in die Szene (`scene.import_glb`)
3. setzt Sockets: `rig.add_socket(body, "fluegel_achse")`, `rig.attach(part)`
4. legt Animation an: `anim.add_twist("fluegel", achse, 90°/s)`
5. prüft selbst nach: `scene.stats`, `render.offscreen` → Bild zurück ins
   Gespräch, Korrekturschleife solange, bis es passt

Jeder Schritt ist ein Registry-Werkzeug; der Mensch sieht alles im
Editor live und kann jederzeit eingreifen (Undo, Permission-Hook, manuell
weiterschrauben am Mesh).

## 3. Umsetzungsschritte

* **M1 (jetzt):** `re-agent` als Gerüst — Registry, Provider-Traits,
  Agent-Loop, `SceneDigest` (Tests grün). `re-wgpu` rendert offscreen.
* **M4 (Editor-Sicht):** HTTP-Transport für die Provider (ein kleines
  Blocking-HTTP), Editor-Viewport, erste Capabilities (Szene/Material/
  Import). MCP-Adapter exportiert dieselbe Liste.
* **M5 (Blender-lite):** Mesh-Bearbeitungswerkzeuge (Select/Move/
  Extrude/Spin/Mirror/Pivot, UV-Grundlagen) als Capabilities — damit ist
  "nachträglich umbauen" KI-bedienbar.
* **M6 (Gen-Loop):** `GenService`-Adapter (ComfyUI/3D-Studio-API/Cloud),
  Sockets/Joints + Auto-Rig, Bild-zurück-an-die-KI Korrekturschleife.
