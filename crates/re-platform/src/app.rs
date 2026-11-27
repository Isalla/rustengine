//! App-Modell der Plattform-Schicht: Event-Typen, Traits, Treiber.
//!
//! Der Spiel-/Editor-Code implementiert nur [`App`] und kennt weder winit
//! noch Konsolen-SDKs. Jeder Backend-Treiber (headless, `desktop` mit winit,
//! später Web und NDA-Konsolen) übersetzt seine Native-Events nach
//! [`PlatformEvent`] — diese Übersetzung ist ohne echtes Fenster testbar.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use crate::input::InputEvent;

/// desired Fenster beim Start (Live-Modus); headless nur durchgereicht.
#[derive(Debug, Clone)]
pub struct WindowOptions {
    pub title: String,
    pub width: u32,
    pub height: u32,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self { title: "RE".into(), width: 1280, height: 720 }
    }
}

/// Plattform-Ereignis an die App — die einzige Event-Sprache der Engine.
#[derive(Debug, Clone)]
pub enum PlatformEvent {
    /// Fenster/bzw. Rendering-Kontext steht: hier Backend + Pipelines anlegen.
    /// Die Maße entsprechen [`WindowOptions`], `scale_factor` kommt vom Fenster.
    Created { width: u32, height: u32, scale_factor: f64 },
    /// Innenfläche geändert (Physikalische Pixel). Surface rekonfigurieren!
    Resized { width: u32, height: u32, scale_factor: f64 },
    /// Tastatur/Maus (siehe [`crate::input`]).
    Input(InputEvent),
    /// Ein Frame: dt Sekunden seit dem letzten Tick, elapsed seit Start.
    Tick { dt: f32, elapsed: f32 },
    /// Fenster soll geschlossen werden (CloseRequested oder Exit-Anfrage).
    Closing,
}

/// Antwort der App auf ein Ereignis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppResponse {
    Continue,
    /// Schleife sauber beenden (Closing wird nicht mehr gesendet).
    Exit,
}

/// Eine laufende Anwendung (Editor, Demo, Spiel-Instance).
///
/// `Send` ist Absicht: Treiber dürfen die App in einem Worker-Loop halten;
/// Konsolen-Module mit nicht-Send-SDK-Codes kapseln das hinter ihrem Adapter.
pub trait App: Send {
    /// Reihenfolge pro Frame: `Created` → `Resized` (falls abweichend) →
    /// Eingaben → `Tick` (rendern!) → … → `Closing` einmal am Ende
    /// (entfällt, wenn die App vorher `Exit` liefert).
    fn on_event(&mut self, event: PlatformEvent) -> AppResponse;

    /// Der Desktop-Treiber reicht das Fenster-Handle durch, bevor `Created`
    /// kommt: daraus erzeugt die App ihr Surface (z. B.
    /// `WgpuBackend::new_for_surface`). Headless-Treiber rufen das nie auf.
    #[cfg(feature = "window")]
    fn attach_window(&mut self, _window: crate::WindowHandleSource) {}
}

/// Minimaler block_on (Identisches Muster wie re-sc/re-wgpu — kein tokio).
///
/// Für die asynchrone Surface-Init im Treiber; der eigentliche Event-Loop
/// bleibt synchron.
pub fn block_on<F: Future>(future: F) -> F::Output {
    struct ThreadWaker(std::thread::Thread);
    impl Wake for ThreadWaker {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }

    let mut future = pin!(future);
    let waker = Waker::from(std::sync::Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park_timeout(Duration::from_millis(2));
    }
}

/// Headless-Treiber: synthetic 60-Hz-Ticks, für Tests/CI/Pi ohne Display.
///
/// Eine App, die [`AppResponse::Exit`] liefert, bricht ab; sonst läuft die
/// angeforderte Tick-Zahl. `Closing` kommt genau einmal, wenn nicht Exit kam.
pub fn run_headless(app: &mut dyn App, options: WindowOptions, ticks: u64) -> u64 {
    const DT: f32 = 1.0 / 60.0;
    app.on_event(PlatformEvent::Created {
        width: options.width,
        height: options.height,
        scale_factor: 1.0,
    });
    let mut ran = 0u64;
    for i in 0..ticks {
        ran += 1;
        if app.on_event(PlatformEvent::Tick { dt: DT, elapsed: i as f32 * DT }) == AppResponse::Exit
        {
            return ran;
        }
    }
    app.on_event(PlatformEvent::Closing);
    ran
}

/// Standard-Einstieg: Desktop-Treiber mit echtem Fenster.
///
/// Ohne Feature `window` (Headless-Builds) gibt es einen Fehler statt einer
/// stillen Alternative — der Aufrufer soll bewusst `run_headless` wählen.
#[allow(unused_variables)]
pub fn run_app(app: &mut dyn App, options: WindowOptions) -> Result<(), String> {
    #[cfg(feature = "window")]
    {
        crate::desktop::run(app, options)
    }
    #[cfg(not(feature = "window"))]
    {
        Err("Dieser Build hat kein Fenster-Backend (Feature `window` aus). \
             Für Tests/CI: run_headless verwenden."
            .to_string())
    }
}

/// Eigenes Clock-Trait: Treiber erzeugen Zeit, Tests diktieren sie.
pub trait Clock: Send {
    /// Sekunden seit Prozessstart (monoton).
    fn now_secs(&mut self) -> f64;
}

/// Echtzeit-Uhr für Live-Treiber.
#[derive(Debug, Default)]
pub struct WallClock {
    start: Option<Instant>,
}

impl Clock for WallClock {
    fn now_secs(&mut self) -> f64 {
        let now = Instant::now();
        let start = *self.start.get_or_insert(now);
        now.duration_since(start).as_secs_f64()
    }
}

/// Synthetic-Uhr für Tests: springt pro Aufruf um eine feste Schrittweite.
#[derive(Debug, Clone, Copy)]
pub struct SyntheticClock {
    step_secs: f64,
    now: f64,
}

impl SyntheticClock {
    pub fn new(step_secs: f64) -> Self {
        Self { step_secs, now: 0.0 }
    }
}

impl Clock for SyntheticClock {
    fn now_secs(&mut self) -> f64 {
        let v = self.now;
        self.now += self.step_secs;
        v
    }
}

/// dt/elapsed aus aufeinanderfolgenden Clock-Werten berechnen (Treiber-Kern).
/// Die Uhr wird pro Aufruf durchgereicht — Tests diktieren so die Zeit.
#[derive(Debug, Default)]
pub struct TickTime {
    last: Option<f64>,
}

impl TickTime {
    pub fn new() -> Self {
        Self::default()
    }
    /// Nächster Frame: (dt, elapsed), beide Sekunden, dt >= 0.
    pub fn tick(&mut self, clock: &mut dyn Clock) -> (f32, f32) {
        let now = clock.now_secs();
        let elapsed = now.max(0.0) as f32;
        let dt = self.last.map(|l| (now - l).max(0.0) as f32).unwrap_or(0.0);
        self.last = Some(now);
        (dt, elapsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Key;

    /// Zählt Event-Arten und beachtet ein Exit-Kommando per Taste.
    #[derive(Default)]
    struct Recorder {
        created: u32,
        ticks: u32,
        closing: u32,
        keys: u32,
        first_size: (u32, u32),
        exit_on_third_tick: bool,
    }

    impl App for Recorder {
        fn on_event(&mut self, event: PlatformEvent) -> AppResponse {
            match event {
                PlatformEvent::Created { width, height, .. } => {
                    self.created += 1;
                    self.first_size = (width, height);
                }
                PlatformEvent::Resized { .. } => {}
                PlatformEvent::Input(_) => self.keys += 1,
                PlatformEvent::Tick { .. } => {
                    self.ticks += 1;
                    if self.exit_on_third_tick && self.ticks >= 3 {
                        return AppResponse::Exit;
                    }
                }
                PlatformEvent::Closing => self.closing += 1,
            }
            AppResponse::Continue
        }
    }

    #[test]
    fn headless_reihenfolge_und_masse() {
        let mut rec = Recorder::default();
        let n = run_headless(&mut rec, WindowOptions::default(), 5);
        assert_eq!(n, 5);
        assert_eq!(rec.created, 1);
        assert_eq!(rec.ticks, 5);
        assert_eq!(rec.closing, 1);
        assert_eq!(rec.first_size, (1280, 720));
    }

    #[test]
    fn headless_exit_ohne_closing() {
        let mut rec = Recorder { exit_on_third_tick: true, ..Default::default() };
        let n = run_headless(&mut rec, WindowOptions::default(), 10);
        assert_eq!(n, 3); // Exit beim 3. Tick beendet vor Closing
        assert_eq!(rec.closing, 0);
    }

    #[test]
    fn synthetic_clock_liefert_dt() {
        let mut clock = SyntheticClock::new(1.0 / 60.0);
        let mut t = TickTime::new();
        let (dt0, el0) = t.tick(&mut clock);
        let (dt1, el1) = t.tick(&mut clock);
        assert_eq!((dt0, el0), (0.0, 0.0));
        assert!((dt1 - 1.0 / 60.0).abs() < 1e-6);
        assert!((el1 - 1.0 / 60.0).abs() < 1e-6);
    }

    #[test]
    fn run_app_ohne_window_feature_fehlt_sauber() {
        // Mit Feature `window` würde hier ein echtes Fenster aufgehen — der
        // Test ruft ihn nur headless (cfg-inkonstant) und prüft die Botschaft.
        #[cfg(not(feature = "window"))]
        {
            let mut rec = Recorder::default();
            let err = run_app(&mut rec, WindowOptions::default()).unwrap_err();
            assert!(err.contains("run_headless"));
        }
    }

    #[test]
    fn exit_response_ist_kopierbar() {
        let a = AppResponse::Exit;
        assert_eq!(a, AppResponse::Exit);
        let _ = Key::Escape; // Import-Verankerung
    }
}
