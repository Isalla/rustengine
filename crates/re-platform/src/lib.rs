//! # re-platform — Plattform-Schicht der Rust Engine (RE)
//!
//! Meilenstein M2: Fenster, Event-Loop, Eingabe, Zeit. Ab M6 zusätzlich
//! `wasm32-unknown-unknown` (Web) und Tier-3-Targets für Retro-Konsolen.
//!
//! Bausteine: [`App`] + [`PlatformEvent`] als einheitliche Sprache,
//! [`run_app`] (Desktop mit Feature `window`) und [`run_headless`]
//! (Tests/CI/Pi). Das winit-Fenster wird durch [`WindowHandleSource`]
//! durchgereicht, damit Backends wie `re-wgpu` daraus ihre Surface bauen.
//!
//! Architektur-Regel: Dieses Crate kennt nur [`PlatformApi`]. Die
//! eigentlichen Betriebssystem-/SDK-Implementierungen (winit für
//! Desktop, `wasm-bindgen` fürs Web, Devkit-SDKs hinter NDA) sind
//! austauschbare Module — genau wie die `GfxBackend`-Module in
//! `re-render`. So bleibt der Kern veröffentlicht, während private
//! Konsolen-Module eingeklinkt werden koennen.

pub mod app;
pub mod input;

pub use app::{
    block_on, run_app, run_headless, App, AppResponse, Clock, PlatformEvent, SyntheticClock,
    TickTime, WallClock, WindowOptions,
};
pub use input::{key_from_scancode, InputEvent, InputState, Key, MouseButton};

/// Desktop-Treiber (winit) — nur mit Feature `window` im Build.
#[cfg(feature = "window")]
pub mod desktop;
#[cfg(feature = "window")]
pub use desktop::WindowHandleSource;

/// Welche Zielplattform gerade gebaut wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetPlatform {
    Linux,
    Windows,
    MacOs,
    Web,
    /// Nintendo Switch (Homebrew), PSP, GBA, NDS, … — oeffentlich machbar.
    RetroConsole,
    /// Devkit-Ziele hinter NDA (PS4/PS5, Xbox, Switch-SDK) — private Module.
    ConsoleNda,
}

impl TargetPlatform {
    /// Plattform zur Compile-Zeit bestimmen.
    pub fn current() -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            TargetPlatform::Web
        }
        #[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
        {
            TargetPlatform::Linux
        }
        #[cfg(target_os = "windows")]
        {
            TargetPlatform::Windows
        }
        #[cfg(target_os = "macos")]
        {
            TargetPlatform::MacOs
        }
    }
}

/// Fenster-/Event-/Zeit-Anschluss der Engine.
///
/// Der Engine-Kern ruft pro Frame nur diese vier Methoden — woher die
/// Daten kommen (X11/Wayland, Win32, Browser, Konsole) entscheidet das
/// aktive Modul.
pub trait PlatformApi: Send {
    /// OS-Events abholen und in den Eingabepuffer schreiben.
    fn pump_events(&mut self);

    /// Groesse des Hauptfensters in Pixeln (Physical).
    fn window_size(&self) -> (u32, u32);

    /// True, wenn der User das Fenster geschlossen/beenden gewaehlt hat.
    fn should_close(&self) -> bool;

    /// Sekundo seit Programmstart — Delta-Time-Berechnung.
    fn now_secs(&self) -> f64;

    /// Fenstertitel setzen (optional).
    fn set_title(&mut self, _title: &str) {}
}

/// Headless-Plattform: kein Fenster, endliche Frames. Laeuft auf dem
/// Pi ohne Display, in CI und fuer Server-Betrieb.
pub struct Headless {
    total_frames: u32,
    done_frames: u32,
    start: std::time::Instant,
    size: (u32, u32),
}

impl Headless {
    /// `frames` = Anzahl Frames, nach denen `should_close` true liefert.
    pub fn new(frames: u32, size: (u32, u32)) -> Self {
        Self {
            total_frames: frames,
            done_frames: 0,
            start: std::time::Instant::now(),
            size: size.max((1, 1)),
        }
    }
}

impl PlatformApi for Headless {
    fn pump_events(&mut self) {
        self.done_frames += 1;
    }
    fn window_size(&self) -> (u32, u32) {
        self.size
    }
    fn should_close(&self) -> bool {
        self.done_frames >= self.total_frames
    }
    fn now_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_schliesst_nach_frames() {
        let mut p = Headless::new(3, (640, 480));
        assert!(!p.should_close());
        for _ in 0..3 {
            p.pump_events();
        }
        assert!(p.should_close());
        assert!(p.now_secs() >= 0.0);
    }
}
