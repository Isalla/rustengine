# Die Engine aus Rust ansprechen

Die Engine ist ein Workspace gewöhnlicher Rust-Crates — man hängt die
an, die man braucht, ohne Codegenerierung, ohne Plugin-Loader:

```toml
[dependencies]
re-core = { path = "engine/crates/re-core" }
re-world = { path = "engine/crates/re-world" }
re-render = { path = "engine/crates/re-render" }
re-wgpu = { path = "engine/crates/re-wgpu" }
re-platform = { path = "engine/crates/re-platform" }
glam = "0.30"
```

Rechnung läuft über `glam`-Typen (`Vec2`, `Vec3`, `Mat4`) — an allen
Crate-Grenzen. Fehler sind gewöhnliche Enums: `GfxError`, `LoadError`,
`CapabilityError`, jeweils mit `thiserror`-Meldungen.

## Traits, die man implementieren kann

| Trait | Crate | Man implementiert ihn, wenn… |
|---|---|---|
| `CellSource` | `re-world` | Zellen von irgendwo Ungewöhnlichem kommen (Netzwerk, Datenbank, prozedural) |
| `GpuUpload` | `re-world` | eine andere GPU-Schicht gespeist oder Uploads für Tests aufgezeichnet werden |
| `GfxBackend` | `re-render` | ein weiteres Rendering-Backend angebunden wird |
| `App` | `re-platform` | ein Fenster-/Event-Loop getrieben wird (Spiel, Editor, Werkzeug) |

Eine minimale `App`:

```rust
use re_platform::{App, AppResponse, PlatformEvent, run_app, WindowOptions};

struct MeinSpiel;

impl App for MeinSpiel {
    fn on_event(&mut self, ev: PlatformEvent) -> AppResponse {
        match ev {
            PlatformEvent::Tick { dt, .. } => { /* simulieren, rendern */ }
            PlatformEvent::Closing => return AppResponse::Exit,
            _ => {}
        }
        AppResponse::Continue
    }
}

run_app(&mut MeinSpiel, WindowOptions::default()).unwrap();
```

## Async ohne Runtime

Die Engine hat bewusst kein tokio. IO-Worker-Threads leben in
`AsyncSource`; die einmaligen Awaits (`wgpu`-Device-Setup) nutzen einen
minimalen `block_on`, den die Plattform-Schicht exportiert. Spielcode
bleibt pro Frame synchron — async ist ein Speicherdetail, nie eine
Aufrufkonvention.

## Determinismus-Zusage

Alles in `re-world` spielt bei gleichen Eingaben identisch: feste
Ereignisreihenfolge, keine Wanduhr-Zugriffe, kein versteckter
globaler Zustand. Wer Replays und Headless-Tests will, hält eigene
Systeme ebenfalls so.

## Feature-Flags

- `re-wgpu` / `re-platform`: `window` (Default) zieht winit; für
  Headless-CI und Einplatinenrechner `--no-default-features` bauen.

Vollständige Signaturen zu jedem Punkt:
[Rustdoc-Index](../../rustdoc/).
