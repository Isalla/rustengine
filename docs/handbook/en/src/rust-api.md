# Using the Engine from Rust

The engine is a workspace of ordinary Rust crates — depend on the ones
you need, no code generation, no plugin loader:

```toml
[dependencies]
re-core = { path = "engine/crates/re-core" }
re-world = { path = "engine/crates/re-world" }
re-render = { path = "engine/crates/re-render" }
re-wgpu = { path = "engine/crates/re-wgpu" }
re-platform = { path = "engine/crates/re-platform" }
glam = "0.30"
```

Math runs on `glam` types (`Vec2`, `Vec3`, `Mat4`) across all crate
boundaries. Errors are ordinary enums: `GfxError`, `LoadError`,
`CapabilityError`, each with `thiserror` messages.

## The traits you may implement

| Trait | Crate | You implement it when… |
|---|---|---|
| `CellSource` | `re-world` | cells come from somewhere unusual (network, database, procedural) |
| `GpuUpload` | `re-world` | you feed a different GPU layer or record uploads for tests |
| `GfxBackend` | `re-render` | you attach another rendering backend |
| `App` | `re-platform` | you drive a window/event loop (game, editor, tool) |

A minimal `App`:

```rust
use re_platform::{App, AppResponse, PlatformEvent, run_app, WindowOptions};

struct MyGame;

impl App for MyGame {
    fn on_event(&mut self, ev: PlatformEvent) -> AppResponse {
        match ev {
            PlatformEvent::Tick { dt, .. } => { /* simulate, render */ }
            PlatformEvent::Closing => return AppResponse::Exit,
            _ => {}
        }
        AppResponse::Continue
    }
}

run_app(&mut MyGame, WindowOptions::default()).unwrap();
```

## Async without a runtime

The engine deliberately has no tokio. IO worker threads live inside
`AsyncSource`; the one-shot awaits (`wgpu` device setup) use a minimal
`block_on` exported by the platform layer. Game code stays synchronous
per frame — async is a storage detail, never a calling convention.

## Determinism contract

Anything in `re-world` replays identically for identical inputs: fixed
event ordering, no wall-clock reads, no hidden global state. Keep it
that way in your systems if you want replays and headless tests.

## Feature flags

- `re-wgpu` / `re-platform`: `window` (default) pulls in winit; build
  `--no-default-features` for headless CI and single-board machines.

Full signatures for every item above:
[rustdoc index](../../rustdoc/).
