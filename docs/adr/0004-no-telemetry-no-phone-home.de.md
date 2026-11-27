# ADR-0004: Keine Telemetrie, kein Phone-Home

[English](0004-no-telemetry-no-phone-home.md) | Deutsch

- **Status:** Angenommen
- **Datum:** 2026-11-27
- **Entscheider:** Waldemar Friesen (Produkt), RE-Engine-Team
- **Bezug:** [architecture.de.md](../architecture.de.md),
  [security.de.md](../security.de.md), [ki-integration.de.md](../ki-integration.de.md),
  [ADR-0002](0002-authoring-surfaces-and-ai-assistant.de.md) (Assistenten-Panel)

## Kontext

Kommerzielle Engines und Editoren telefonieren nach Hause:
Nutzungsdaten, Abbruchberichte und Session-Beacons (Session-IDs,
App-IDs, Versionen) gehen ungefragt an Händler-Endpunkte — im
Unreal-Editor-Log stehen bis heute `datarouter`-POSTs. Diese Engine läuft
als selbst gehosteter MMO-Client auf eigener Hardware (bis hinunter zum
Raspberry Pi daheim). Vertrauen ist ein eigenes Feature. Die Produkt-
regel ist schlicht: **fragen statt sammeln.** Die Software erfährt nie
von sich aus etwas über jemanden; wenn der Betreiber etwas über eine
Person wissen will, fragt er die Person, und die Person entscheidet, ob
sie antwortet.

## Entscheidung

1. **Kein Phone-Home.** Engine-Binaries (Editor, Client, Werkzeuge)
   bauen niemals von sich aus Netzwerkverbindungen auf. Abgehende
   Verbindungen gehen ausschließlich zu Endpunkten, die ein Mensch in
   diesem Lauf ausdrücklich gesetzt hat (Serveradresse, Projekt-
   einstellung, CLI-Argument). Nie erlaubt: Telemetrie/Analytics,
   Abbruchberichte, Lizenz-/Aktivierungs-Pings, Update-Checks,
   Präsenz-Beacons, install-eindeutige Kennungen/Fingerabdrücke.
2. **Offline ist ein unterstützter Zustand.** Editor und Werkzeuge
   müssen ohne Netzwerk voll nutzbar sein; ein fehlendes Netz ist nie
   ein Fehlerzustand (headless und `--ui-png` beweisen das für das
   Rendern bereits).
3. **Externe Werkzeuge sind deren Sache.** MCP-Adapters, KI-Provider
   und Build-Dienste sind eigene Prozesse, die der Nutzer startet und
   konfiguriert. Was die über das Netz tun, deren Ding — in der
   Integrationsdoku offengelegt, für die Engine unsichtbar, solange sie
   nicht laufen. Verbindet das Assistenten-Panel (ADR-0002) einen
   Provider, setzt den Endpunkt und die Zugangsdaten der Mensch, und
   jeder abgehende Aufruf steht sichtbar in der Konsole.
4. **„Telemetrie" heißt in diesem Repo lokale Messwerte.** Wo immer das
   Wort steht (z. B. IO-Telemetrie in `re-world`), meint es lokale
   Leistungszähler, die Streaming-/Qualitätsentscheidungen speisen und
   in Logs landen — nichts verlässt die Maschine. Neue Zähler folgen
   derselben Regel.
5. **Abhängigkeits-Riegel.** Keine HTTP-/TLS-/Websocket-/Analytics-
   Client-Crates im Abhängigkeitsgraphen; `scripts/check-phone-home.sh`
   lässt den Build scheitern, falls einer auftaucht. Ein künftiges
   Transport-Crate für echtes Spiel-Netzwerk (M6+) verlangt eine
   ausdrückliche Ergänzung dieses ADR und bleibt an Regel 1 gebunden
   (nur menschengesetzte Endpunkte).
6. **Abstürze schreiben lokale Dateien.** Abbruch-Logs und Minidumps
   landen im Projektarbeitsverzeichnis, wo der Mensch sie liest und
   entscheidet — Teilen ist Sache der Person, nie automatisch.

## Konsequenzen

- Die Engine läuft am Pi ohne Netzkabel, und die CI hält den
  Abhängigkeitsgraphen ehrlich.
- Die Audit-Checkliste bekommt pro Meilenstein einen Phone-Home-Scan.
- Player-facing-Dienste (M6+) müssen Wissen als ausdrückliche,
  postenweise Zustimmung bauen (Formular, sichtbares Feld, Opt-in) —
  nie als Hintergrund-Sammlung.
