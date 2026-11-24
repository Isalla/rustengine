# Cinema-Editor — Zwischensequenzen im Spiel (Concept, M7)

[English](cinema.md) | Deutsch

Ein Editor-Modus, in dem Spieler:innen und die KI **cutscenes bauen**:
Kamera fliegt über die Karte, Einheiten spielen eine Szene, eine Tür
schließt sich, Musik setzt ein — alles innerhalb des Spiels/Editors,
ohne externes Videoschnittprogramm.

## Baustein: `Sequence` als Engine-Asset

Die vorhandene Animations-Schicht (`re-core::animation` mit
`Channel`, `KeyValue`, `Interpolation`, `Target::Node/Joint`) ist
bereits die halbe Timeline. Ein Cinema-Asset ist eine **Sammlung von
Tracks**, die auf dieselbe Scene-Mathematik schreiben:

| Track-Typ | Ziel | Beispiel |
|---|---|---|
| Transform | Node | Marine läuft zur Luke (Position+Rotation kurvig) |
| Camera | eigener Pfad | Spline mit Zielpunkt + FOV-Animation (Dolly/Crash-Zoom) |
| Property | Node/Material | Luke `open: 0→1` (nutzt Sockets aus dem Panzer-Fall!) |
| Visibility | Node | Einblendung von Trümmern, Fade-Out von UI |
| Audio | Mix | Sprachzeile + Musik-Swell zu Zeit X |
| FX | Emitter | Staubwolke beim Einschlag, Zeit-Signal an Gameplay |
| Dialogue | Overlay | Text/Spraakone mit Lippen-Trigger (später) |

Wichtig: **Auswertung ist eine reine Funktion** `eval(sequence, t) →
Scene-Zustand`. Dadurch ist alles scrubbar, rückspulbar und
deterministisch — Vorbedingung für Editor-Timeline UND für Offline-
Render der KI.

## Editor (M7)

Timeline-Panel unter dem 3D-Viewport (Viewport zeigt Live-Scrub):
Tracks als Liste, Keyframes als Dopesheet/Kurven, Camera-Track mit
Spline-Griffen — dieselben Auswahl-Werkzeuge wie im Mesh-Editor
(Panzer-Fall): was man anklicken kann, kann man keyen.
Playhead, Loop-Bereiche, Blend-Zonen zwischen Sequenzen.

## Im Spiel

`re-cinema`-Player: Spiel pausiert Logik, Player übergibt Kontrolle,
Sequence läuft, Signals an Gameplay (Dialog-Optionen, Spawn-Events).
Cutscene-Rig (Briefing-Kamera) und In-Engine-Cutscene sind derselbe
Code — kein „gebackenes Video" nötig.

## KI-Bedienung (Capabilities, ab M6)

* `cinema.create(name)` / `cinema.add_camera(keys)` /
  `cinema.key(node, prop, t, value)`
* `cinema.from_script("Der Marine erreicht die Luke und schließt sie")`
  — die KI nutzt Szenenwissen (`SceneDigest`) + Sockets + Animationen
  und baut die Tracks selbst; Korrektur von Hand im Editor jederzeit.
* `cinema.preview(seq, t)` → Offscreen-Frames (re-wgpu kann das
  schon) zurück ins KI-Gespräch: die KI **sieht** ihre Cutscene.
