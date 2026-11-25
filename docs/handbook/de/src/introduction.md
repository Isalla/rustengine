# Einleitung

RE ist eine Open-Source-Spiel-Engine: Runtime und Editor sind in Rust
geschrieben, Dienste und Werkzeuge (Server, Buildfarmen, Launcher) in
Go. Eine Codebasis zielt auf PC und Web, Konsolen-Backends lassen sich
privat anbinden. Das Rendering läuft auf [wgpu](https://wgpu.rs) aus
einer einzigen WGSL-Shader-Quelle.

## Das Leitfeature

Alles an dieser Engine baut auf einem Ziel: **Die Kamera fällt aus der
Strategie-Draufsicht in die Ego-Perspektive einer Einheit — mitten im
Flug, ohne Ladebildschirm und ohne Schnitt — und übergabe dabei die
Steuerung.** Dieselbe Laufzeit bedient danach einen Shooter oder ein
MMO, weil der Runtime selbst keine Spielregeln kennt.

Dieses Ziel formt das Design: Weltdaten streamen kontinuierlich (keine
Szenen-Neuaufbauten), Qualitätsstufen passen sich pro Instanz an, und
die Steuerungs-Übergabe ist eine numerische Blende, kein Levelwechsel.

## Doku in drei Ebenen

| Ebene | Für wen | Wo |
|---|---|---|
| **Dieses Handbuch** | die Engine am Stück lernen | hier |
| **Rustdoc** | exakte API jedes Crates | [`/rustdoc/`](../../rustdoc/) auf dieser Seite |
| **API-Browser im Editor** | stöbern während der Arbeit | kommt mit M4 |

Quelle beider generierter Ebenen ist die Crate-Dokumentation selbst —
jedes öffentliche Element trägt Rustdoc, und die Handbuch-Kapitel
verlinken darin.

## Wie es weitergeht

- Neu im Projekt? [Erste Schritte](./getting-started.md) bauen und
  zeigen den Streaming-Rundflug in wenigen Minuten.
- Design prüfen? [Rundgang durch die Architektur](./architecture.md).
- Spiel- oder Werkzeugcode schreiben? [Die Engine aus Rust
  ansprechen](./rust-api.md).

Der lebende Plan mit jeder Checklisten-Box ist
[docs/roadmap.md](https://github.com/Isalla/rustengine/blob/main/docs/roadmap.de.md).
