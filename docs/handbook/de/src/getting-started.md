# Erste Schritte

## Voraussetzungen

Eine stabile Rust-Toolchain (ab 1.85) und Git. Unter Linux braucht der
Fenster-Build die üblichen X11-/Wayland-Entwicklungsbibliotheken;
Headless-Builds verlangen außer der Toolchain nichts.

## Bauen und testen

```sh
git clone https://github.com/Isalla/rustengine
cd rustengine
cargo build --workspace
cargo test --workspace
```

Auf Maschinen ohne Display (Server, CI, Raspberry Pi) lässt sich das
Backend headless bauen — das Feature `window` ist das, was winit
hereinzieht:

```sh
cargo build -p re-wgpu --no-default-features
```

## Der Streaming-Rundflug

`stream-demo` ist der End-zu-End-Beweis der Welt-Pipeline: Die Welt
wird als prozedurales Gelände in Blockdateien gebacken, durch
asynchrone Datei-IO, ein Streaming-Budget, den GPU-Handoff und das
Fernfeld-HLOD gestreamt — und eine Kamera fliegt darüber hinweg
und geht unterwegs in die Ego-Perspektive über.

```sh
cargo run -p re-wgpu --bin stream-demo -- \
    240 flight.png 960 540 ./flight_cells
```

Argumente: Frames, Ausgabe-PNG, Breite, Höhe, Zellen-Ordner. Der Lauf
printet pro Teilsystem eine Berichtszeile:

```text
Welt erzeugt: 2322 Blöcke (1156 B/Block) in ./flight_cells/ — Flugstrecke 800 m
Flug beendet: 240 Frames, geladen=458, fehlgeschlagen=0, verdrängt=104, …
HLOD: 195 Cluster registriert, 65 Übernahmen, 0 Rückgaben, …
IO-Telemetrie: 458 Lasten, 0 Fehler, Latenz Ø 16.7 ms, … Gesundheit 0.67
Ego-Handoff: Attach bei Frame 80, Blende 1.0 s, 102 Frames in Ego-Höhe …
```

Der letzte Frame landet in `flight.png` (plus ein zweites Bild mitten
aus der Blende). Auf langsamer Hardware dauert jeder Frame einen
Moment — der Software-Renderer ist ehrlich, nur geduldig.

## Der Szenen-Betrachter

`re-demo` rendert ein Worldbuilder-Projekt (`.wbproj`) offscreen zu PNG
oder öffnet es im echten Fenster mit Orbit-Kamera:

```sh
cargo run -p re-wgpu --bin re-demo -- scene.wbproj out.png
cargo run -p re-wgpu --bin re-demo -- scene.wbproj --live
```

## Nächster Schritt

Der [Rundgang durch die Architektur](./architecture.md) erklärt, was
die Berichtszeilen oben eigentlich gemessen haben.
