# ADR-0001: Modulares, preset-gesteuertes Editor-Frontend

[English](0001-modular-editor-frontend.md) | Deutsch

- **Status:** Angenommen
- **Datum:** 2026-11-25
- **Entscheider:** Waldemar Friesen (Produkt), RE-Engine-Team
- **Bezug:** [architecture.de.md](../architecture.de.md),
  [ki-integration.de.md](../ki-integration.de.md) (Regel der doppelten
  Adressierbarkeit), Roadmap M4 (Editor)

## Kontext

Der Editor muss Nutzern, die von anderen Content-Werkzeugen kommen —
Unreal Engine 5, Unity und andere — sofort vertraut wirken. Jedes dieser
Werkzeuge ordnet die *gleichen* Grundoperationen (Kamera bewegen, Entity
auswählen, Component bearbeiten, Projekt speichern, Mesh importieren) unter
anderen Menünamen, Panel-Positionen, Icons und Tastenkürzeln an.

Die Engine trennt **Funktion** und **Darstellung** bereits: Jede
Editor-Operation ist eine benannte Capability in der
[`CapabilityRegistry`](../../crates/re-agent/src/capability.rs), und
[`EditorContext`](../../crates/re-editor/src/context.rs) ist die eine
Kompositionswurzel, die den geteilten Zustand hält und diese Capabilities
exponiert. Das ist die Regel der doppelten Adressierbarkeit (M11):
Menschen-Oberfläche, In-Prozess-Agentin und MCP rufen durch *dieselbe* Tür.

Dieses ADR beantwortet die Frage: **Wie bauen wir die Editor-Oberfläche,
dass „UE5-Layout" und „Unity-Layout" Varianten EINER Codebasis sind und
nicht zwei Forks** — ohne erst einen Klon hartzucodieren und den zweiten
später dran zu bauen?

## Entscheidung

Das Editor-Frontend wird **datengetrieben und preset-basiert** gebaut. Ein
*Frontend* (auch *Preset* oder *Layout* genannt) ist **kein** Code, der
Funktionen neu implementiert; es ist Daten, die geteilte Widgets über
geteilte Capabilities anordnen. Ein Preset ist genau drei Schichten:

1. **Layout-Manifest** — welche Panels es gibt, wo sie andocken, ihre
   Größe und Titel. Deklarative Daten (z. B. RON/TOML), kein
   Rust-Kontrollfluss.
2. **Control → Capability-Bindung** — jeder Menüpunkt, Toolbar-Button und
   jedes Kürzel nennt eine Capability samt Argumenten (z. B.
   `project.save`, `ecs.entity.set`). Das Widget ruft
   `EditorContext::call(name, args)`; es ruft niemals direkt
   Engine-Interna.
3. **Theme** — Farben, Fonts, Abstände, Metriken, Icons **und
   Nutzer-Labels** (die Label-Menge ist die i18n-Schicht, ein Preset kann
   also DE/EN-Namen für dieselbe Capability tragen).

Ein konkretes Preset = **Manifest + Theme** über einem **geteilten
Widget-Set** (Viewport, Outliner, Inspector, Konsole, Content-Browser) und
der **geteilten Capability-Registry**. `ue5` und `unity` sind zwei solche
Presets, nicht zwei Programme. Der Nutzer wählt beim Start ein Preset;
eigene Presets sind einfach weitere Daten.

### Was das verbietet

- Das Layout eines einen Editors in den Panel-Code hartcodieren.
- Ein Widget ruft Engine-Interna statt einer Capability.
- Labels, Icons oder Farben in Rust-Quelltext statt im Theme ablegen.

### Was geteilt bleibt (einmal gebaut, von jedem Preset genutzt)

- Das **Docking-/Panel-System** (andocken, tabben, floaten, resize) — der
  mit Abstand größte Brocken Editor-UI-Arbeit, und genau der Grund, warum
  Presets billig sind: Es wird einmal geschrieben und jedes Preset nutzt es
  weiter.
- Das **Widget-Set** (Outliner-Baum, Inspector, Viewport-Host, Konsole).
- Die **Capability-Registry** und `EditorContext` (existieren bereits).

## Konsequenzen

**Positiv**

- Ein zweites Layout (Unity) ist überwiegend **Daten**, kein Code: dieselben
  Panels umsortieren, Menüs umbenennen, Theme tauschen.
- Referenz-Screenshots (UE5, Unity, …) sind die **Spezifikation eines
  Presets**, nicht Input in den Engine-Kern — saubere Trennung, und der
  Kern ändert sich nie, wenn sich ein Layout ändert.
- Jedes Preset ist automatisch von der Agentin steuerbar, weil seine
  Controls Capability-Aufrufe sind.
- i18n und Theming gibt es geschenkt: Sie gehören zum Preset, nicht
  verteilt über den Code.

**Kosten / Risiken (bewusst angenommen, ehrlich)**

- Das Docking-System ist echter Aufwand — aber **einmal** bezahlt, von
  allen Presets geteilt.
- Visuelle Treue pro Engine (Icons, exakter Look) ist **Art**-Aufwand,
  kein Code; das Theme-Format muss diese Ressourcen tragen.
- Manche Werkzeuge unterscheiden sich im **Interaktionsablauf**, nicht nur
  in der Anordnung (z. B. Unitys „Komponente hinzufügen" vs. UE5s
  Details-Panel). Die brauchen kleine Pro-Preset-Logik, aber diese Logik
  bleibt capability-getrieben (ein Ablauf ist eine geordnete Folge von
  `ecs.*`/`project.*`-Aufrufen) und forkt daher den Kern nicht.

## Phasen

- **P1 — ein Layout, von Anfang an datengetrieben gebaut.** Viewport +
  Outliner + Inspector + Konsole implementieren, aber jedes Control an
  einen Capability-Namen binden, das Layout aus einem Manifest lesen und
  Look/Labels aus einem Theme. Kostet gegenüber Hartcodieren kaum Mehr-
  arbeit, weil die Capabilities schon existieren.
- **P2 — das `ue5`-Preset extrahieren.** Die UE5-Referenz-Screenshots
  werden seine Spezifikation (Menüreihenfolge, Panel-Position, deutsche
  Labels).
- **P3 — das `unity`-Preset ergänzen** als zweites Manifest + Theme;
  Preset-Auswahl beim Start; später nutzer-eigene Manifeste.

## Nicht Teil davon (zunächst)

Das Frontend selbst ist **noch nicht** gebaut. Die Umsetzung startet,
sobald die UE5-Referenz-Screenshots vollständig sind, damit die erste
Panel-Arbeit ein exaktes Ziel hat. Dieses ADR legt die *Form* fest, in die
die Umsetzung passen muss; es legt das Manifest-Dateiformat noch nicht
final fest — das entscheidet P1.
