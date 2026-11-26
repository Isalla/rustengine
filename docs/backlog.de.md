# Backlog — geparkte Ideen

Gewollte, aber nicht terminierte Ideen. Jeder Eintrag hält den Gedanken
frisch fest; die Terminierung erfolgt, wenn ein Meilenstein Platz hat.
Nichts hier ist ein Versprechen — geparkt heißt geparkt.

## B-1: Snappen, das die Form anpasst (Editor)

**Wunsch (Benutzer, 26.11.2026):** Wenn im Editor Assets aneinander
snappen, soll die Engine sie so weit verformen, dass sie tatsächlich
zusammenpassen — ohne dass eines der Assets hinterher von Hand
angepasst werden muss.

**Beispiele:**

- Ein Wandsegment dehnt/stutzt sein Ende, sodass es in jedem Winkel
  exakt an das nächste anschließt.
- Rohre, Kabel oder Schienen verbinden sich automatisch mit dem Port,
  an den sie snappen.
- Treppen treffen den Boden ohne Spalt; Terrain-Kacheln verlaufen an
  Nähten ineinander über.
- 2D-Äquivalent (unser MMO-Fall): Autotiling — ein Piece formt seine
  Ränder um, je nachdem welche Nachbarn vorhanden sind.

**Design-Zwänge (warum das zur Engine-Doktrin passt):**

- **Regel der doppelten Adressierbarkeit (M11):** Snap-und-Anpassen ist
  eine Registry-Werkzeug-Operation. Mensch und KI müssen exakt dieselbe
  Anpassung über dieselben Werkzeuge aufrufen können.
- **Zerstörungsfrei:** Die Anpassung ist ein berechneter Parameter
  (Snap-Bedingung + erzeugte Variante), niemals ein Eingriff ins
  Quell-Asset. Das Original-Mesh bleibt unversehrt, das Ergebnis ist
  reversibel.
- **Deterministisch:** Gleiche Eingaben (Asset, Nachbar, Snap-Punkt) →
  gleiches Ausgabe-Mesh — damit funktioniert es auch in prozeduralen
  Welten und in Replays.

**Bekannte Verwandte (für spätere Recherche):** 2D-Autotiling /
Wang-Kacheln, Mesh-Bridging/-Stitching, parametrische Assets
(Shape-Grammars, CityEngine-Art), UE-Snap-to-Surface plus prozedurale
Mesh-Komponenten.

**Status:** geparkt — Bau erst, wenn das Grundgerüst des Editors steht.
