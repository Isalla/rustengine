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

## B-2: Blackwell-Techdemo (Neural-Tier, Vendor-Sidecar)

**Wunsch (Benutzer, 26.11.2026):** Wenn die Engine fertig ist, eine
Techdemo aufsetzen, die die RTX-5000-Architektur (Blackwell) anspricht
— deren neue Niedrigpräzisions-Tensor-Formate (FP4/MX mit
Block-Skalierung, FP8; zur Einordnung: BF16 ist Ampere-Alter, int8
älter) — um zu testen, was die Engine im High-End kann und welche
wirklich coolen Gaming-Features diese Einheiten ermoeglichen.

**Ehrliche Grenze:** wgpu exponiert keine Tensor-Cores / FP4 /
Cooperative-Matrix-Extensionen. Die gibt es nur ueber rohes Vulkan
(`VK_NV_cooperative_vector`, `VK_KHR_cooperative_matrix`) oder D3D12 —
die Demo muss also ein **Vendor-Sidecar** sein (natives Modul oder
Inference-Runtime, das Texturen/Ergebnisse zurueck in die Pipeline
einspeist), niemals eine Voraussetzung. Die Geraete-Profile (M13)
bekommen ein optionales *Neural-Tier*; die portable Pipeline und der
Pi bleiben davon unberuehrt.

**Demo-Kandidaten (nach MMO-Nutzen sortiert):**

1. Neuronale Texturkompression — Dekodierung per FP4-Inferenz;
   mehrfach mehr Welt pro VRAM-Byte (passt zur
   Kompressions-Doktrin in rendering.md).
2. Radiance Caches — GI per Training backen, nahezu gratis abtasten.
3. Selbstgebauteres ML-Upscaling / Frame-Generation als
   Lernprojekt rund um DLSS-artige Pipelines.
4. Lokale NPC-Inferenz — kleine Modelle in FP4 direkt auf der Karte.

**Status:** geparkt — wenn der Engine-Kern funktionsvollstaendig ist
und eine Blackwell-Karte real auf dem Pruefstand liegt.
