# Editor-UI — Presets, Identität, Referenz-Satz

[English](editor-ui.md)

Dieses Dokument ist die Arbeits-Spec für das Editor-Frontend. Es hält
die Preset-Entscheidung ([ADR-0001](adr/0001-modular-editor-frontend.md))
fest, die Identität des eigenen Default-Looks, das Inventar des
gelieferten UE5-Referenz-Satzes und die Lücken, die er noch hat.

## 1. Zwei Gesichter, ein Codebase

Ein Frontend-Preset ist **Daten, kein Code**: ein Layout-Manifest
(welche Panels wo docken), Control→Capability-Bindings und ein Theme
(Farben, Metriken, Labels). Das Widget-Set (Viewport, Outliner,
Inspector, Konsole, Content Browser) und die Capability-Registry sind
gemeinsam über alle Presets.

Die Entscheidung (Produkt, 2026-11-26): die Engine liefert ihr
**eigenes `default`-Preset** als Gesicht, und das **`ue5`-Preset als
optionalen Look** für Umsteiger von Unreal. So bleibt die Identität
der Engine eigenständig — der UE5-Layout ist eine Variante, nicht die
Marke.

## 2. Identität des `default`-Presets

| Aspekt | `default` (eigenes Gesicht) | `ue5` (optional) |
| --- | --- | --- |
| Chrome | schlanke Kommando-Leiste + **Command-Palette** (Strg+Space) statt klassischer Menüleiste | klassische Menüleiste, neun Menüs, deutsche Labels |
| Farben | warme Dunkelgraus, **Rust-Orange** als Akzent | UE5 „Dunkel": kalte Fast-Schwarz-Graus, Blau-Akzent (exakte Werte im Theme-JSON) |
| Assistent | **Assistent-Dock als gleichwertiger Panel-Bürger** — die M11-Doktrin der doppelten Adressierbarkeit, sichtbar gemacht | Assistent vorhanden, nicht vorn |
| Panels | dieselben geteilten Widgets, eigene Anordnung aus dem Manifest | UE5-Anordnung aus den Screenshots |

Das Docking-System, Outliner, Inspector, Viewport-Host und Konsole
werden **einmal** gebaut; ein Preset ordnet und beschriftet sie nur neu.

## 3. Inventar des Referenz-Satzes (geliefert 2026-11-26)

Ort: `.share/referenz/ue5-ui/` (außerhalb des Repos — Referenz-Material,
kein ausgeliefertes Asset).

| Element | Specifiziert |
| --- | --- |
| `UE5_1.png` | komplettes Editor-Layout + Datei-Menü: Menüstruktur, Shortcut-Spalte, deaktivierte Einträge, Tooltip-Stil, Content-Drawer-Leiste |
| `UE5_2.png` | Bearbeiten-Menü (Verlauf, Zwischenablage, Konfiguration) |
| `UE5_3_*.png` | Fenster-Menü + Untermenüs — die Menge der öffenbaren Panels (Details 1–4, Outliner, Inhaltsbrowser, Output-Log, …), Layout laden/speichern |
| `UE5_4_*.png` | Werkzeuge-Menü + Untermenüs (inkl. Suche-im-Untermenü-Muster) |
| `UE5_5_*.png` | Erstellen-Menü — Radio- und Häkchen-Einträge in Menüs |
| `UE5_6_*.png` | Plattformen-Menü — zweistufige Untermenüs, deaktivierte Untermenü-Info |
| `UE5_7_*.png` | Auswahl-Menü + Untermenüs |
| `UE5_8_*.png` | Actor-Menü + Untermenüs (lange Labels, Kürzung) |
| `UE5_9.png` | Hilfe-Menü |
| `UE5_10.png` | voller Editor gefüllt: Toolbar (Play-Regler, Dropdowns, Transform-Werkzeuge, Einstellungen), Outliner mit Suche/Filter/Mehrfachauswahl, Details-Panel (Tabs, Sektionen, XYZ-Zeilen, Key/Value-Info, Checkboxen), Klassenwähler-Dialog mit offenem Dropdown, untere Tab-Leiste, Statusleiste, Viewport-Gizmo |
| `editor-einstellungen.png` | Einstellungs-Dialog: Sidebar-Baum, Suche, Filter-Chips, Sektionen, Checkboxen, Buttons, Spinners, Schloss-Icons |
| `projekteinstellungen.png` | Einstellungs-Dialog-Variante: Kategorie-Baum, Dropdown-Felder, Array-Element-Zeilen |
| `plugins.png` | Plugin-Browser: Sidebar mit Zählern, Zeilen mit Checkbox/Icon/Badge/Version |
| `…_Aussehen_Dunkel.json` | **exakte UE5-Theme-Farben** (`EStyleColor::*`, lineares RGBA) — das `ue5`-Theme startet mit diesen Werten |
| `EditorPerProjectUserSettings.ini`, `Editor Settings Backup….ini`, `Project Settings Backup….ini` | echte Editor-Metriken, Shortcut-Belegungen, Dock-Layout — Quelle für Abstände/Größen |
| `UE5_11.png` | Inhaltsbrowser geöffnet (Raster-Ansicht): Sidebar-Baum (Favoriten/Alle/Sammlungen), Kollektions-Leiste, Brotkrumen, Asset-Werkzeugleiste, Ansichtsumschalter, Auswahle-Statusleiste; Outliner mit echtem Baum (Sichtbarkeits-Augen, Namenssuche, Filter-Knopf, Spaltenkopf, untere Level-Tabs) |
| `UE5_12_outputlog.png` | Output-Log ausgeklappt: farbige Zeilen (Warnung gelb, Fehler rot, Info weiß), Kategorie-Filter-Dropdown, „Enthaltungen anzeigen"-Checkbox, Suchfeld, Kopieren/Leeren-Icons |
| `UE5_13.png` | Inhaltsbrowser Listen-Ansicht-Umschalter + Asset-Hover-Tooltip (Vorschau + Metadaten-Karte) |
| `UE5_14.png` | Inhaltsbrowser-Dateiliste mit **Inline-Umbenennung** (Textfeld in Bearbeitung, Auswahlhervorhebung) und status-gefärbten Zeilen |
| `UE5_15.png` | Rechtsklick-**Kontext-Menü** im Viewport: Suchfeld, Kleinkapett-Sektions-Labels, Shortcut-Spalte, Häkchen-Einträge, deaktivierte Einträge, Untermenü-Pfeile |
| `UE5_16.png` | **Akteure platzieren**-Panel: Klassen-Suche, Kategorie-Liste, link Icon-Schiene mit Flyout, Panel-Schließen-Knopf |
| `UE5_17.png` | geöffnetes Toolbar-Dropdown (Anzeigemodus) + weißer Hover-Tooltip |
| `UE5_18.png` | Kontext-Menü des Viewport-Tabs (Häkchen, deaktivierte Einträge, Sektions-Labels) |
| `Bewegungseditor*.png` | Sequencer-Referenz für M11: Spur-Ansicht (Lineal, Playhead, Key-Rauten, Kamera-Spur), Icon-Schiene mit Icon+Text-Flyout-Rastern (ausgewählt = blau), modale Message-Box („NumLock aktiviert") |

Bewiesene Interaktions-Zustände: Hover/ausgewählt (blau), deaktiviert
(grau), Tooltip (weiße Box), Untermenü-Pfeil, Separatoren mit
Kleinkapett-Labels, Textkürzung mit Ellipse, dünne Scrollbars,
Tab-Schließen-Knopf, Menü-Suchfeld, Kontextmenü-Suche, Inline-
Textbearbeitung mit Auswahl, Häkchen-Menüpunkte, farbige Log-Zeilen,
Asset-Hover-Karten, modale Message-Box.

## 4. Lücken — im Referenz-Satz noch fehlt

Der gelieferte Satz deckt jetzt alle Kern-Widgets. Was bleibt, ist
Feinarbeit; Toolkit und `ue5`-Preset können beide weitermachen:

1. **Konsole / Command-Palette** mit Autovervollständigungs-Liste.
2. **Schwebendes Panel + Dock-Drop-Hinweise** — abgetrennter
   Titelbalken, Tab-Zieh-Ghost, Drop-Ziel-Linien (UE5_16 zeigt ein
   gedocktes Tab-Panel, keinen Schweb-Zieh-Zustand).
3. **Content-Drawer ausgeklappt** (geöffnetes unteres Regal).
4. **Details-Interaktions-Zustände** — offenes Enum-Dropdown,
   Farb-Picker, Wert-Drag, Reset-Icon-Hover (Inline-Umbenennung ist
   jetzt durch `UE5_14` abgedeckt).
5. **Fortschritts-Dialog** (die modale Message-Box deckt das
   Bewegungseditor-Bild ab).
6. Play-Mode-Overlay im Viewport (optional).

Icons: die Screenshots zeigen Menü-Icons zu klein für Vektor-Extraktion.
Nach ADR-0001 ist Icon-Treue Art-Arbeit im Theme — das Toolkit liefert
einfach gezeichnete Glyphen; ein passendes Icon-Set kann sie später ohne
Code-Änderung ersetzen.

## 5. P1-Umfang (der Build, der jetzt startet)

- **Widget-Toolkit auf `re-render`**: Draw-List (Rechtecke, gerundete
  Rechtecke, Linien, Scissor), Text per Font-Atlas (`ab_glyph`, Inter —
  SIL OFL, gebündelt unter `assets/fonts/`), Input-Routing über
  `re-platform`-Ereignisse, Immediate-Mode-Widget-Schicht.
- **Theme-Datei**: Farben/Metriken/Labels als Daten; das
  `default`-Theme (warme Graus + Rust-Orange) kommt zuerst, das
  `ue5`-Theme startet mit den Werten aus dem JSON oben.
- **Layout-Manifest**: Panels, Dock-Seiten, Größen, Titel als Daten;
  das Docking (Split/Tab/Float/Resize) ist das gemeinsame System.
- **Default-Layout**: Kommando-Leiste, Viewport Mitte, Outliner links,
  Inspector rechts, Konsole unten, Assistent-Dock.
- Jedes Control bindet einen Capability-Namen (`EditorContext::call`) —
  niemals Engine-Interna (doppelte Adressierbarkeit).
