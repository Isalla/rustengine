# Sicherheit — Bedrohungsmodell, Audit-Checkliste, Fund-Log

[English](security.md) | Deutsch

Zweck: Sicherheitsprobleme in der Engine **absichtlich** finden, nach
Plan, statt zu warten, bis es jemand extern tut. Wie überall hier: Der
Befund wird vor dem Fix aufgeschrieben, der Fix verweist auf die
Fund-ID.

## Bedrohungsmodell (enginespezifische Angriffsflächen)

Nach realer Exposition sortiert, nicht nach Theorie:

1. **Das Parsen fremder Assets — Fläche Nr. 1.** Jetzt glTF; später
   FBX/OBJ/USD, Bilder und Zelldateien (Roadmap M4). Eine fehlerhafte
   oder bösartige Datei darf nie paniken, unbegrenzt allozieren oder
   endlos laufen: Größenkappen vor der Allokation, `Result` statt
   Panic bei Aufrufernfehlern, jeder Parser bekommt ein Fuzz-Korpus.
2. **Netzwerk (M9).** Netcode-Dienst, Interest-Management,
   Launcher-Downloads: jedes Paket validieren (Länge, Enum-Bereiche,
   Reihenfolge), Rate-Limit je Verbindung, die Leitung behandeln wie
   die Asset-Parser — feindlicher Input.
3. **KI-/Agenten-Integration (M11,
   [ki-integration](ki-integration.de.md)).** Engine-Inhalte sind
   *Daten*, nie Anweisungen an den Agenten: Prompt-Injection über
   Asset-Namen/Beschreibungen darf nicht als Tool-Aufruf in der
   Registry landen. ADR-0002 gibt den Rahmen vor: frei lesen, schreiben
   nur auf Klick. Budgetkappen und Audit-Trail je Agenten-Lauf.
4. **Scripting & Plugins (M11).** Wasm-Plugins per Default sandboxed;
   native Plugins sind Trusted-Code und müssen das in ihrem Manifest
   sagen. Die Capability-Registry ist die Grenze: Ein Plugin bekommt
   Werkzeuge per Erlass, nicht per Geburt.
5. **Supply-Chain.** `Cargo.lock` eincommiten; neue Abhängigkeiten sind
   ein Review-Ereignis (warum dieses Crate, was zieht es nach);
   `cargo audit` / `cargo deny` in der CI, sobald die Werkzeuge für
   diese Maschine freigegeben sind.
6. **Speichersicherheit.** Sicheres Rust in `re-core`/`re-world`/
   `re-render` überall; `unsafe` nur an Backend-FFI-Grenzen, jeder
   Block mit `// SAFETY:`-Kommentar samt Invariante.
7. **Secrets & NDA.** Konsolen-SDKs leben in privaten Repos (M10);
   keine Tokens, keine Pfade privaten Materials, keine Hersteller-
   Assets in diesem Repo — die Publikations-Regel ist auch eine
   Sicherheitsregel.

## Audit-Checkliste (pro Quartal oder pro Meilenstein)

- [ ] Parser fuzzen: Korpus aus abgeschnittenen, überdimensionierten,
      spec-brechenden glTF-Dateien; kein Panic, kein OOM, kein Timeout
      (braucht cargo-fuzz — vor der Installation fragen)
- [ ] `cargo audit` + `cargo deny` über dem Lockfile
- [ ] `unsafe`-Audit: jeder Block mit SAFETY-Kommentar; die Zahl darf
      zwischen Meilensteinen nicht still wachsen
- [ ] Capability-Registry: jedes Tool auflisten, Input-Validierung und
      Undo-Sicherheit je Tool prüfen (doppelte Adressierbarkeit gilt
      beidseitig — ein Agent steuert genauso schnell in den
      Fehlerzustand wie ein Mensch)
- [ ] Repo-Hygiene-Scan: keine absoluten Maschinenpfade, keine privaten
      SDK-Namen, keine Hersteller-Assets (Publikations-Regel)
- [ ] Netzwerk (sobald M9 existiert): Missgebildete-Paket-Tests je
      Nachrichtentyp

## Fund-Log

| ID | Datum | Bereich | Schwere | Kurzbeschreibung | Status | Fix |
|----|-------|---------|---------|------------------|--------|-----|
| — | — | — | — | noch keine Befunde erfasst | — | — |

Schwere-Skala: `critical` (Remote-Code-Execution / Datenverlust) ·
`high` (Crash aus fremdem Input, Permission-Escape) · `medium`
(begrenzter DoS, Info-Leak) · `low` (Hygiene).

## Melden

Ungefixte Schwachstellen bekommen **keine** öffentlichen Issues — dafür
GitHub Security Advisories (privat). Fixed Befunde bekommen eine Zeile
im Log oben im selben Commit wie den Fix, mit Regressionstest.

*Quellenmaterial liegt in der privaten Referenzbibliothek (LLM-Security
/ OWASP-LLM, Defensive-Security-Ops) — außerhalb dieses Repos, siehe
Bibliotheks-Index.*
