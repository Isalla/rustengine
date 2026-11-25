# Bug-Jagd — Checkliste, Konventionen, Fund-Log

[English](bugs.md) | Deutsch

Zweck: Bugs in der Engine **absichtlich** finden — systematische Gänge
über die bekannten fragilen Ecken — und eine kuratierte Liste des
Gefundenen führen. GitHub Issues ist der Eingangskanal; diese Datei ist
die Wahrheit der Engine: Jeder bestätigte Bug bekommt hier eine ID, und
kein Fix erscheint ohne Regressionstest im selben Commit.

## Konventionen

- **ID:** `BUG-###`, vergeben bei Bestätigung, nie wiederverwendet.
- **Schwere:** `crash` · `wrong-result` (still — die gefährliche Art) ·
  `perf` · `cosmetic`.
- **Status:** `open` → `fixing` → `fixed(<commit>)` · `wontfix(Grund)`.
- Jede Zeile braucht eine minimale Reproduktion — ein fehlgeschlagener
  Test zählt und ist bevorzugt.

## Jagd-Checkliste (systematische Gänge)

Pro Meilenstein laufen lassen, oder wenn ein Subsystem angefasst wird.
Jede Zeile ist eine Frage an den Code, kein Bauchgefühl.

**Determinismus (`re-world`, `re-render`-Mathe)**
- [ ] Gleiches Input zweimal → byte-identisches Output? (Demo zweimal
      laufen lassen, Artefakte vergleichen)
- [ ] Versteckte Iterationsreihenfolge über `HashMap`/Mengen, die in
      Output oder Draw-Order sickert?

**Streaming & IO**
- [ ] Null-größige Inputs: Render-Target 0×0, leere Zelldatei,
      abgeschnittene Datei mitten in der Tabelle
- [ ] Budget-Erschöpfung: Macht die Verdrängung Fortschritt oder
      thrasht sie?
- [ ] Langsame-Platte-Simulation (Telemetrie-Spike): verschlechtern,
      nie den Render-Loop blockieren

**Renderer**
- [ ] NaN/Inf-Pixel unter Extremwerten: Belichtung 0 und 1e9,
      entartete Dreiecke (Fläche 0), Normalen an den Oktaeder-Polen
- [ ] Resize-Stürme: Fenster mitten im Frame auf 1×1 und zurück — kein
      Panic, keine uralte History (TAA muss zurücksetzen, siehe
      rendering.md-Doktrin)
- [ ] Motion-Vektoren im ersten Frame (keine vorherige View-Proj)

**Editor & Undo**
- [ ] Undo über Grenzen hinweg: Undo nach Import, nach Projektwechsel,
      nach Crash-Recovery-Laden — der Stack muss konsistent bleiben
- [ ] Redo-Abschneiden bei neuer Command nach teilweisem Undo

**Skala & Grenzen**
- [ ] 10k Entitäten, 1k Lichter, 64 Cluster/Scheibe: keine O(n²)-
      Überraschungen in Pro-Frame-Pfaden
- [ ] Große Weltkoordinaten (1e6 m): Float-Präzision — wird irgendwo
      große Position subtrahiert ohne Verankerung?

**API-Missbrauch**
- [ ] Aufrufernfehler sind `Result`, nie Panics im Bibliothekscode;
      Panics nur für Invarianten-Verletzungen mit Meldung

## Fund-Log

| ID | Datum | Bereich | Schwere | Symptom | Ursache | Status |
|----|-------|---------|---------|---------|---------|--------|
| — | — | — | — | noch keine Befunde erfasst | — | — |

*Das Log bleibt ehrlich: `fixed`-Zeilen behalten ihre Ursache — der
Jagd-Gang des nächsten Meilensteins liest sie als Muster-Kandidaten.*
