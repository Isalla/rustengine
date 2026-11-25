# ADR-0003: G-Buffer zuerst, Visibility Buffer als zweite Oberflächenquelle

[English](0003-gbuffer-first-visibility-buffer.md) | Deutsch

- **Status:** Angenommen
- **Datum:** 2026-11-25
- **Entscheider:** Waldemar Friesen (Produkt), RE-Engine-Team
- **Bezug:** [rendering.de.md](../rendering.de.md) (§2, Pipeline V2),
  Fallback-Doktrin ([architecture.de.md](../architecture.de.md)),
  Roadmap M5 (Deferred HDR, GPU-getriebener Draw-Pfad)

## Kontext

Deferred-Lighting braucht Oberflächen-Daten pro Pixel (Albedo, Normale,
Roughness, Materialparameter). Dafür existieren zwei bewährte Lieferanten:

**Klassischer MRT-G-Buffer.** Der Geometrie-Pass schreibt mehrere
Render-Targets (≈ 12–16 Byte/Pixel). Das Lighting liest sie als normale
Texturen zurück. Läuft auf jedem Backend inklusive WebGPU im Browser;
jeder Kanal ist in RenderDoc direkt anschaubar. Kosten: festes
Kanal-Layout (jeder Pixel zahlt jeden Kanal) und volle Bandbreite auch
für übermalte Pixel — bei 1080p etwa 30 MB Schreib- plus 30 MB
Lese-Traffic pro Frame. Unsere Leitfeature-Ansicht (Strategie-Kamera:
Einheiten, Vegetation und Props überlappen stark) ist genau dieser
Overdraw-Fall.

**Visibility Buffer.** Der Geometrie-Pass schreibt eine einzige
ganzzahlige ID (Instance + Primitive) plus Tiefe (≈ 8 Byte gesamt). Der
Lighting-Pass schlägt das Dreieck nach, rekonstruiert Baryzentrik und
lädt Materialdaten aus Bindless-Arrays. Kostet bei Overdraw praktisch
nichts und erzwingt kein festes Material-Layout. Aber es braucht
Storage-Buffer-Geometrie und Textur-Arrays mit Non-Uniform-Indexing —
verfügbar auf Vulkan/DX12/Metal, **nicht in der WebGPU-Browser-Stufe** —
und die Material-Verzweigung pro Pixel braucht Kachel-Klassifizierung,
um schnell zu bleiben.

Die Engine-Doktrin gibt den Rahmen schon vor: jedes Feature braucht eine
abwärts spielbare Fallback-Kette, und die WebGPU-Stufe ist
erstklassiges Ziel (M9). Zwei getrennte Lighting-Pässe zu bauen würde
die Wartung des komplexesten Shaders der Engine verdoppeln.

## Entscheidung

**Reihenfolge statt Entweder-Oder:**

1. **M5 Box 1 (Deferred HDR) erscheint mit dem klassischen MRT-G-Buffer.**
   Layout: Albedo+Metallic, oktaedrische Normalen
   (`re-render::gbuffer`), Roughness/Materialparameter, Depth+Motion.
   Er ist der einzige Oberflächen-Lieferant, den jede Stufe — inklusive
   WebGPU — laufen kann.
2. **Der Lighting-Pass liest über eine abstrakte
   Oberflächen-Abfrage-Schnittstelle** (`surface_at(pixel)`), nicht über
   hartkodierte G-Buffer-Textur-Reads. In WGSL umgesetzt als
   Shader-Permutation: der Lighting-Algorithmus ist geteilt, die
   Oberflächen-Abfrage wird je Quelle erzeugt.
3. **Der Visibility Buffer kommt später als zweite Oberflächenquelle
   für die Desktop-Stufe**, zusammen mit dem GPU-getriebenen Draw-Pfad
   (dieselben Voraussetzungen: Bindless-Arrays, indirekte Geometrie).
   Derselbe Lighting-Pass, andere Abfrage-Stufe — niemals eine zweite
   Lighting-Implementierung.

## Konsequenzen

**Positiv**

- Ein Lighting-Algorithmus zur Pflege; die Oberflächenquelle ist eine
  Variante.
- WebGPU/Browser-Fallback ist ab Tag eins garantiert (nur G-Buffer).
- Der Bandbreitensieg, wo er zählt (Strategie-Kamera-Overdraw:
  ~60 MB → ~8 MB pro Frame bei 1080p), ist später ohne Umbau verfügbar.
- Kein festes Material-Limit auf Desktop: neue Material-Features
  kosten Buffer-Reads, keine G-Buffer-Breite.

**Kosten / Risiken (bewusst angenommen)**

- Die Oberflächen-Abstraktion muss **jetzt** entworfen werden, bevor der
  erste Lighting-Shader landet — sie nachträglich aus hartcodierten
  Textur-Reads herauszubrechen ist der teure Fehler, den dieses ADR
  verhindert.
- Shader-Permutations-Maschinerie (geteilter Lighting-Code, erzeugte
  Abfrage-Stufe) ist echtes Plumbing in `re-wgpu`.
- Visibility-Buffer-Debugging (ID-Bilder, Rekonstruktion) ist schwerer;
  abgemildert dadurch, dass der G-Buffer-Pfad als Referenz-Modus
  erhalten bleibt.

## Phasen

- **Jetzt (M5 Box 1):** MRT-G-Buffer + Lighting-Pass hinter der
  Oberflächen-Abfrage-Schnittstelle.
- **M5 GPU-Driven-Box:** Bindless-Arrays + Indirect Draw + Hi-Z
  (CPU-Referenz: `re-render::cull`); der Visibility Buffer reitet als
  Desktop-Oberflächenquelle mit.
- **Gate:** Der Visibility Buffer erscheint erst, wenn eine Szene über
  beide Quellen identisch rendert (Pixel-Diff-Test auf demselben
  Lighting-Pass).
