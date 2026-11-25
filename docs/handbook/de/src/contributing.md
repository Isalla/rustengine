# Mitwirken

## Grundregeln

- `cargo test --workspace` bleibt grün; jede Funktion kommt mit Tests,
  und `cargo doc` bleibt warnungsfrei (der CI prüft beides).
- Kleine Commits. Ein Feature-Commit hakt die zugehörige Box in der
  Roadmap ab — in **beiden** `docs/roadmap.md` und
  `docs/roadmap.de.md`.
- Nutzer-Doku ist zweisprachig: Englisch trägt den normalen Namen,
  Deutsch bekommt den `.de.md`-Zwilling. Code-Kommentare dürfen
  deutsch sein.
- `re-world` bleibt kopflos und deterministisch; Threads gehören nur in
  `io.rs`.

## Doku-Stil

Öffentliche Einträge tragen Rustdoc mit einzeiliger Zusammenfassung
und `# Panics`-/Safety-Hinweis, wo es zählt — die Rustdoc-Ebene der
Webseite und der künftige API-Browser im Editor werden genau aus
diesen Kommentaren gebaut; ein fehlender Doc-Kommentar ist eine
fehlende Funktion.

## Änderungen vorschlagen

Vor größeren Änderungen ein Issue öffnen. Die Meilensteine der Roadmap
markieren, was geplant ist; alles andere braucht vorher ein Gespräch.

## Lizenz

MIT — Copyright (c) 2026 Waldemar Friesen. Beiträge kommen unter
denselben Bedingungen; bei Weitergabe sind Copyright- und
Lizenztext unverändert mitzuführen.
