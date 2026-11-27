# ADR-0002: Autor-Oberflächen und das KI-Assistenten-Panel

[English](0002-authoring-surfaces-and-ai-assistant.md) | Deutsch

- **Status:** Angenommen
- **Datum:** 2026-11-25
- **Entscheider:** Waldemar Friesen (Produkt), RE-Engine-Team
- **Bezug:** [ADR-0001](0001-modular-editor-frontend.de.md) (preset-gesteuertes
  Frontend), [ki-integration.de.md](../ki-integration.de.md) (Regel der
  doppelten Adressierbarkeit, `re-agent`), Roadmap M4 (Editor)

## Kontext

Viele Entwickler arbeiten weiterhin gern direkt am Code; andere verknüpfen
Verhalten lieber visuell, ohne Code zu schreiben. Der Editor muss **beides**
bedienen — und es dem Nutzer erlauben, ganz in der einen, ganz in der
anderen Oberfläche zu leben oder frei zwischen beiden zu wechseln. Welche
Oberfläche auch immer eine Person wählt: das zugrunde liegende Verhalten
muss identisch sein — eine Engine, ein Satz Systemen, ein Satz
Capabilities.

Das baut direkt auf ADR-0001 auf: Das Frontend ist datengetrieben, jedes
Control ist ein Capability-Aufruf, und Panels sind andockbare Widgets über
einem geteilten Kern. Dieses ADR entscheidet, **was die Autor-Oberflächen
sind**, **wie sie zusammenhängen** und **welche Rolle der KI-Assistent**
(`re-agent`) darin spielt.

## Entscheidung

### 1. Zwei Autor-Oberflächen, eine zugrunde liegende Komposition

- **Code-Editor** — ein vollwertiger Editor für Entwickler, die Code
  schreiben.
- **Visueller Node-Graph** — Funktionen/Systeme verknüpfen, ohne Code zu
  schreiben, im Sinne von Unreal Blueprints.

Das sind **keine zwei Systeme**, sondern **zwei Ansichten derselben
Sache**: Ein Node-Graph ist ein visueller Editor für dieselbe ECS-`Schedule`
-/Capability-Komposition, die Code textuell ausdrückt. Beide erzeugen
dasselbe Artefakt, also bleibt beides für die Agentin adressierbar (Regel
der doppelten Adressierbarkeit).

### 2. Der Code-Editor

- **Links: Projektbaum** — jeder Ordner und jede Datei im Projekt zum
  schnellen Wechseln.
- **Tabs** — geöffnete Dateien oben als Tabs; Wechseln mit einem Klick.
- **Syntax-Highlighting** — sofort, damit der Entwickler auf einen Blick
  sieht, ob der Code wohlgeformt ist oder ob etwas fehlt.
- **Diagnostik als Vorschlag, nie als Edit** — der Editor hebt Fehler und
  Warnungen hervor und darf Fixes anbieten, aber **ersetzt niemals von
  selbst etwas**. Der Entwickler entscheidet.
- Rückgrat: ein **LSP-Client** (rust-analyzer) liefert Vervollständigung,
  Diagnostik und Code-Actions; ein optionales Syntax-Highlighting
  (tree-sitter/syntect) ergänzt es.

### 3. Der visuelle Node-Graph

- Drag & Drop von Knoten, die Funktionen/Systeme/Capabilities darstellen,
  verdrahtet über Daten- und Executions-Pins (Event → Funktion → Ausgabe),
  wie Blueprints.
- Ein **Graph-Browser** (Graphs / Functions / Macros / Variables) und ein
  **Details-Panel** für den gewählten Knoten.
- Knotentypen sind selbst als Capabilities registriert, damit der Graph
  genau wie der Code von der Agentin bearbeitbar ist.

### 4. Ein Assistenten-Panel (rechts), drei Modi

Diagnostik und KI-Assistent sind **dieselbe Gattung** — Rückmeldung zum
Code — deshalb leben sie in **einem** Panel, nicht in zwei. Das Panel zeigt
einen einzigen, typ-markierten Stream:

| Modus | Richtung | Inhalt |
|---|---|---|
| **Passiv** | Engine → Entwickler | Fehler, Warnungen, Hinweise, Folge-Aktionen |
| **Live** | Engine → Entwickler, beim Tippen | vorausgesehene Fortsetzung (Ghost-Text) |
| **Dialog** | Entwickler → Engine, bei Bedarf | Chat: fragen, ohne ein neues Fenster zu öffnen |

- Jeder Eintrag ist anklickbar und springt zur betreffenden Code-Stelle.
- Das Panel und der Projektbaum sind **andock- und abschaltbar** (jederzeit
  öffnen/schließen) gemäß ADR-0001.

### 5. Der Assistent ist engine-bewusst, angetrieben von `re-agent`

Der Assistent ist kein angeklebter Chatbot, sondern der **bestehende
`re-agent`** (Ollama, llama.cpp, OpenAI-kompatibel, Anthropic) plus die
**CapabilityRegistry**, sichtbar als Panel. Weil er sich den Kontext mit dem
Editor teilt (offene Datei, Selektion, Cursor) und Projekt und Registry
lesen kann, antwortet er **über dieses Projekt** und kann RE-spezifische
Folge-Aktionen vorschlagen: *„das als Capability registrieren“*, *„dieses
System zur `Schedule` hinzufügen"*, *„über `world.instances.spawn` ins
Level setzen"*. Das kann die KI eines fremden Editors nicht; unsere kann
es, weil die Vorschläge die Registry selbst sind.

- **Lokal-als-Default** (Ollama / llama.cpp), damit Code die Maschine nicht
  verlassen muss; Cloud-Provider bleiben wählbar.
- **Frei lesen, schreiben nur auf Klick:** Der Assistent darf Capabilities
  schreibgeschützt aufrufen (Datei lesen, Welt abfragen); jede Änderung
  wird nur als Vorschlag angeboten, den der Entwickler auslöst — derselbe
  Grundsatz wie bei der Diagnostik.

### 6. Quellen-Provenienz und bidirektionale Navigation

Um von einem Symptom zur Ursache zu springen, trägt **jede Capability /
jedes System / jeder Knoten eine Rückwärts-Referenz auf seinen
Definitionsort** (Datei + Zeile, oder eine Registry-ID, die auf einen
Quellort aufgelöst wird). Das muss von Anfang an eingebaut sein — es ist
schwer nachzurüsten.

Weil Code und Graph zwei Ansichten einer Komposition sind, läuft die
Verknüpfung **in beide Richtungen**:

- **Node → Code:** Fehler an einem Knoten → „zeig mir die Stelle“ → die
  Agentin löst den Quellort des Knotens auf, der Editor öffnet ihn auf
  dieser Zeile; der Entwickler korrigiert.
- **Code → Node:** Cursor auf einer System-Funktion → der zugehörige Knoten
  wird im Graph hervorgehoben.

Der Sprung ist selbst eine Capability (`editor.reveal`) — auslösbar per
Menschen-Klick **oder** durch die Agentin im Chat. Doppelte
Adressierbarkeit, ein weiteres Mal.

## Konsequenzen

**Positiv**

- Eine Engine, ein Verhalten; die Oberfläche ist Präferenz, kein Fork.
- Der Assistent ist projekt- und engine-bewusst, nicht generisch.
- Diagnostik, Vorhersage und Chat teilen sich einen Kontext und ein Panel.
- Fehler sind über Oberflächen hinweg navigierbar (Graph ↔ Code) dank
  Provenienz.

**Kosten / Risiken (bewusst angenommen)**

- **Provenienz ist harte Voraussetzung** — Capability-/System-Registrierung
  muss den Quellort ab Tag eins erfassen.
- LSP-Integration (rust-analyzer) ist echtes Plumbing; der Assistent
  braucht eine Kontext-Grenze (was geht ans Modell?) und eine
  Provider-Wahl (lokal vs. Cloud) pro Nutzer.
- Ein Node-Graph mit Executions- und Daten-Pins ist erhebliche UI-Arbeit;
  er nutzt das Docking-System aus ADR-0001 weiter, ist aber ein eigenes
  Widget.
- Datenschutz/Kosten beim Senden von Code an Modelle: abgemildert durch den
  Lokal-Default und explizite Kontext-Grenzen.

## Phasen

- **P1 — Code-Editor + LSP.** Projektbaum, Tabs, Highlighting,
  rust-analyzer-Diagnostik (Vorschläge, keine Edits). **Provenienz wird ab
  hier bei jeder Capability- und System-Registrierung erfasst.**
- **P2 — Assistenten-Panel.** Passiver Diagnostik-Stream, dann
  Live-Vorhersage, dann der `re-agent`-Dialog; `editor.reveal` für den
  Sprung zur Quelle.
- **P3 — visueller Node-Graph.** Graph-Editor + Browser + Details, gemappt
  auf die `Schedule`/Capabilities, mit bidirektionaler Navigation zum Code.

## Nicht Teil davon (zunächst)

Die Umsetzung beginnt später mit den Frontend-Phasen aus ADR-0001.
Dieses ADR legt die Form und die
übergreifenden Anforderungen fest (Provenienz, Vorschlag statt Ersetzen,
engine-bewusster Assistent); es legt keine Widget-Interna und nicht das
genaue Manifest-Format fest — das entscheiden P1–P3.
