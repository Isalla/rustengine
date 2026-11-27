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
| `UE5_19.png` | **abgetrenntes (schwebendes) Panel**: Inhaltsbrowser aus dem Editor gerissen — eigener OS-Titelbalken mit Min/Max/Schließen, eigene Tab-Zeile, schwebt neben dem Hauptfenster über dem Desktop; darunter Output-Log ausgeklappt |
| `cmd.png` | Konsolen-Typ-Auswahl (Cmd / Python / Python (REPL)) mit Häkchen — der Konsolen-Einstieg |
| `Editordiagnose.png` | modaler Dialog als Fenster: Titelbalken mit Schließen-Knopf, Tab-Pills (Alles anzeigen / Nur Warnungen), Zahnrad-Menü, Tabelle mit Spaltenköpfen (Aktuell / Expected / Fehlschläge / Notify), Sektions-Zeilen, Checkbox-Spalte, status-gefärbte Werte; linke Icon-Schiene mit Text-Knöpfen (ausgewählt = blau) |
| `Trace-Objekt_1/2.png` | reichhaltiges Toolbar-Dropdown: Suchfeld, Kleinkapett-Sektions-Labels, Radio-Einträge (gefüllter Punkt), Checkbox-Einträge, Shortcut-Spalte, deaktivierte Einträge, **fokusiertes Textfeld (blauer Rand)**, Combo-Box, **Untermenü-Flyout** mit Hover-Hervorhebung (blau), weißer Tooltip über dem Menü |
| `Zen-Server.png` | Statusleisten-Dropdown mit Sektions-Labels + ZweitEbene-Untermenü (Icon-Einträge), Hover-Hervorhebung |
| `revisionskontrolle.png` | Statusleisten-Menü mit deaktiviertem Eintrag (Verbots-Icon) und Tooltip über dem Menü |
| `UE5_20_farbpicker.png` | **Details-Farb-Picker** geöffnet aus einer Eigenschafts-Zeile: Farbkreis, Wert- und Sättigungs-Regler, RGB-Regler, HSV-Regler, Hex-Feld, Voreinstellungs-Fächerraster, Pipetten/Invertieren-Icons, Reset-Pfeile je Kanal, OK / Abbrechen |
| `UE5_21_enumdropdown.png` | **geöffnetes Enum-Dropdown** (Viewport-Projektionsmodus): aktueller Eintrag mit Häkchen, Shortcut-Spalte, Kleinkapett-Sektions-Labels, lebende Zahlenfelder im Menü, **Untermenü-Flyouts** mit Pfeilen |
| `UE5_22_playoverlay.png` | **Play-Modus**: Viewport auf Spielansicht umgeschaltet, Play-Regler der Toolbar durch Pause/Überspringen/Stopp ersetzt, Overlay-Text im Bild („Spieler [U] wird gestartet" + Hinweiszeile), dazu Hintergrund-Vorgangs-Meldung mit Spinner („Offener Vorgang: Lichtbuild bearbeiten …") |
| `UE5_23_console_autocomplete.png` | **Konsolen-Autovervollständigungs-Liste**: 20 passende CVars über dem fokussierten Eingabefeld, je mit Kategorie-Buchstaben-Icon, gemeinsamer Präfix gleich, abweichender Schwanz hervorgehoben |

Bewiesene Interaktions-Zustände: Hover/ausgewählt (blau), deaktiviert
(grau, mit Verbots-Icon), Tooltip (weiße Box, auch über Menüs verankert;
dunkle Variante mit Icon), Untermenü-Pfeil + geöffnetes Flyout,
Separatoren mit Kleinkapett-Labels, Textkürzung mit Ellipse, dünne
Scrollbars, Tab-Schließen-Knopf, Menü-Suchfeld, Kontextmenü-Suche,
Inline-Textbearbeitung mit Auswahl, fokusiertes Textfeld (Akzent-Rand),
Radio-Einträge, Checkbox-Einträge und Checkbox-Tabellenspalten,
Combo-Box, Häkchen-Menüpunkte, farbige Log-Zeilen, Asset-Hover-Karten,
modale Message-Box, modaler Dialog mit Tabelle, Tab-Pills,
Content-Drawer-Kachel-Regal, **abgetrenntes schwebendes Fenster**,
**Farb-Picker-Dialog**, **Enum-Dropdown mit Häkchen + Shortcuts +
lebenden Feldern**, **Konsolen-Autovervollständigung**,
**Play-Mode-Overlay-Text und Hintergrund-Vorgangs-Meldung**.

## 4. Lücken — im Referenz-Satz noch fehlt

Am 2026-11-27 geschlossen (zweiter Aufnahmelauf, `.share/referenz/ue5-ui/`):
Konsolen-Autovervollständigung (`UE5_23_console_autocomplete.png`),
Details-Farb-Picker (`UE5_20_farbpicker.png`), Enum-Dropdown
(`UE5_21_enumdropdown.png`), Play-Mode-Overlay (`UE5_22_playoverlay.png`),
abgetrenntes schwebendes Panel (`UE5_19.png`).

Was bleibt — und warum es das Ende dessen ist, was ein Aufnahmelauf
holen kann:

1. **Dock-Drop-Hinweise während des Ziehens** — der abgetrennte
   Titelbalken ist bewiesen (`UE5_19`), die Drop-Ziel-Linien sind nicht
   erreichbar: der Automatisierungs-Drag ist atomar (down→move→up ohne
   den Game-Thread freizugeben), es gibt also keinen Frame zwischen
   Drücken und Loslassen, den man aufnehmen könnte.
2. **Wert-Drag in den Details** (eine Zahl scrubben) — dieselbe
   atomare Eingabeursache; der Drag selbst ist der aufzunehmende Zustand.
3. **Fortschritts-Dialog** — kein Auslöser ohne Nebenwirkung. Ein
   Live-Coding-Compile meldet `CompileNotStarted` (in diesem Projekt
   deaktiviert), eine CVar-Änderung kompiliert aus dem Cache ohne
   sichtbaren Balken, und die verbleibenden Auslöser (alle speichern,
   Lichtbuild, Plugin-Installation) verändern das Projekt. Die
   Hintergrund-Vorgangs-Meldung mit Spinner in
   `UE5_22_playoverlay.png` ist der nächstliegende ehrliche Ersatz.

### 4.1 Was die UE5.8-MCP-Oberfläche nicht erreicht (Aufnahme-Doktrin)

Notiert, damit ein künftiger Lauf das nicht neu herausfindet:

- **Widget-Refs sind session-lokal.** Jede MCP-Session nummeriert sie
  neu, also muss Auflösen → Klicken → Aufnehmen in einer Session
  passieren.
- **Das Details-Panel steckt nur dann im Barrierefreiheitsbaum, wenn
  sein Tab aktiv ist und etwas ausgewählt ist.** Ein inaktiver Tab trägt
  nichts bei — was als „Panel ist nicht zugänglich" gelesen wird, wenn
  es das nicht ist.
- **Kleine Enums rendern als Segment-Buttons, nicht als Dropdown**
  (Actor-Mobilität: Statisch/Stationär/Beweglich inline). Ein Dropdown
  braucht ein Enum mit genug Werten — das Projektionsmodus-Feld zeigt
  die echte Dropdown-Form.
- **Eigenschafts-Zeilen sind einzelne flache Knoten** ohne Kind-Widgets;
  ein Klick in die Mitte kann das Label treffen und einen Tooltip
  auslösen statt des Editier-Felds. Wert-Zellen brauchen einen
  Koordinaten-Klick.
- **Modale Dialoge ignorieren synthetische Klicks** (Abbrechen im
  Farb-Picker feuerte nie). Es sind echte OS-Fenster: mit `WM_CLOSE`
  schließen.
- **Flüchtige Popups sterben beim Aufnehmen.** Das Bild-Tool nimmt Fokus
  und schließt Menüs; stattdessen den Bildschirmbereich per GDI greifen.
- **Die Menüleisten-Befehlssuche steckt nicht im Barrierefreiheitsbaum**,
  und ihr Ergebnis-Popup überlebt automatisierte Eingaben nicht.
  Strg+Leertaste ist in diesem Build unbelegt. Die
  Konsolen-Autovervollständigung (`UE5_23_console_autocomplete.png`) ist
  der erreichbare gleichwertige Beleg.
- **Kein Klick auf absolute Koordinaten und kein Kontextmenü-Taste** im
  Inspector-Werkzeugsatz; alles, was das braucht, ist entworfen
  unerreichbar.

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

## 6. Raster-Konventionen (der CPU-Kern ist die Wahrheit, die GPU spiegelt)

Das UI zeichnet durch genau eine Draw-List (`re_render::ui::DrawList`);
der CPU-Referenz-Rasterizer (`rasterize`) und der GPU-Pass
(`re_wgpu::ui::UiGpu`, Shader `ui.wgsl`) müssen dieselben Pixel
erzeugen — dieselbe Doctrine wie beim Schatten-Kern. `tests/ui_gpu.rs`
ist das Tor: eine repräsentative Szene (Flächen, gesnapste Linien,
verschachtelte Clips bis an den Rand, überlappendes Alpha-Blending,
Glyphen-Text zweier Fonts mit Umlauten und Geviertstrich) difft bei
**max 1/255 je Kanal, kein Pixel darüber** (das 1 LSB ist die
RGBA16F-Zwischenspeicherung; genau das erlaubt das Tor).

Die geteilten Konventionen, die den Spiegel exakt machen:

- **Pixelgesnapste Geometrie**: Glyphen-Quads rasten beim Layouten auf
  das Ganzzahl-Pixelraster ein (der Pen bleibt fraktional, die
  Quad-Kanten nicht). Alle Kanten sind ganzzahlig, Pixelzentren (x.5)
  treffen nie eine Kante, und keine Subpixel-Quantisierung eines
  Rasterizers kann eine Abdeckungs-Spalte kippen. Die Glyphen-
  Abtastung wird 1:1 — jedes Ziel-Pixel trifft genau ein Atlas-Texel,
  das Bilinear degeneriert zum Texelwert.
- **Integer-Scissor**: `scissor_rect()` klemmt den Float-Clip auf ein
  Pixel-Box (floor/ceil); CPU-Rasterizer und `set_scissor_rect`
  verbrauchen dieselbe Box.
- **Manuelles Bilinear auf der GPU**: der Shader filtert den Atlas per
  `textureLoad` in exakt der Operationsreihenfolge des CPU-Kerns —
  Hardware-Filtergewichte sind festkomma und würden um LSBs divergieren.
- **Lineare Float-Akkumulation**: die GPU rendert in RGBA16F
  (premultipliziertes `ONE, ONE_MINUS_SRC_ALPHA` == die CPU-Blend-
  Formel) und quantisiert erst nach dem Readback auf 8 Bit — wie die
  CPU-Referenz.
- Textfarben sind lineares RGBA; der Fenster-Pfad kompositt das UI nach
  dem Tone-Mapping auf die Szene (der UI-Pass selbst kodiert nie sRGB).
