# Getting Started

## Prerequisites

A stable Rust toolchain (1.85 or newer) and Git. On Linux the windowed
build needs the usual X11/Wayland development libraries; headless builds
need nothing beyond the toolchain.

## Build and test

```sh
git clone https://github.com/Isalla/rustengine
cd rustengine
cargo build --workspace
cargo test --workspace
```

On machines without a display (servers, CI, a Raspberry Pi) build the
backend headless — the `window` feature is what pulls in winit:

```sh
cargo build -p re-wgpu --no-default-features
```

## The streaming fly-over

`stream-demo` is the end-to-end proof of the world pipeline: it bakes a
procedural terrain world into block files, streams it through async
file IO, a streaming budget, the GPU handoff and far-field HLOD — and
flies a camera over it, switching to first-person on the way.

```sh
cargo run -p re-wgpu --bin stream-demo -- \
    240 flight.png 960 540 ./flight_cells
```

Arguments: frames, output PNG, width, height, cells directory. The run
prints one summary line per subsystem:

```text
Welt erzeugt: 2322 Blöcke (1156 B/Block) in ./flight_cells/ — Flugstrecke 800 m
Flug beendet: 240 Frames, geladen=458, fehlgeschlagen=0, verdrängt=104, …
HLOD: 195 Cluster registriert, 65 Übernahmen, 0 Rückgaben, …
IO-Telemetrie: 458 Lasten, 0 Fehler, Latenz Ø 16.7 ms, … Gesundheit 0.67
Ego-Handoff: Attach bei Frame 80, Blende 1.0 s, 102 Frames in Ego-Höhe …
```

The last frame is written to `flight.png` (plus a second image taken
mid-blend). On slow hardware each frame takes a moment — the software
renderer is honest, just patient.

## The scene viewer

`re-demo` renders a Worldbuilder project (`.wbproj`) offscreen to PNG
or opens it in a real window with an orbit camera:

```sh
cargo run -p re-wgpu --bin re-demo -- scene.wbproj out.png
cargo run -p re-wgpu --bin re-demo -- scene.wbproj --live
```

## What to read next

[Architecture Tour](./architecture.md) explains what each summary line
above actually measured.
