//! Live-Modus (Feature `window`): echtes Fenster mit Orbit-Kamera.
//!
//! Zugleich der Referenzverbrauch der M2-Plattform-Schicht: Die App kennt
//! nur [`PlatformEvent`]s, das Fenster-Handle geht via [`App::attach_window`]
//! ans Backend, und pro [`PlatformEvent::Tick`] wird die Szene neu composed
//! und auf die Surface präsentiert.

use anyhow::Result;
use glam::Vec3;
use re_core::material::Material;
use re_core::project::Project;
use re_render::{Camera, GfxBackend};
use re_platform::{
    run_app, App, AppResponse, InputEvent, Key, MouseButton, PlatformEvent, WindowHandleSource,
    WindowOptions,
};

use crate::gather;
use re_wgpu::WgpuBackend;

/// Orbit-Kamera um einen Punkt: Gier-/Nickwinkel plus Augen-Abstand.
#[derive(Debug, Clone, Copy)]
struct Orbit {
    yaw: f32,
    pitch: f32,
    dist: f32,
    center: Vec3,
}

impl Orbit {
    fn eye(&self) -> Vec3 {
        let dir = Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        );
        self.center + dir * self.dist
    }

    fn camera(&self, width: u32, height: u32) -> Camera {
        let mut cam = Camera::perspective(
            self.eye(),
            self.center,
            Vec3::Y,
            45f32.to_radians(),
            width.max(1) as f32 / height.max(1) as f32,
        );
        cam.clear_color = [0.52, 0.68, 0.85, 1.0];
        cam
    }

    /// Zentrum in Blickrichtung (waagerecht), seitlich und hoch schieben.
    fn fly(&mut self, fwd: f32, strafe: f32, up: f32) {
        let flat = Vec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos()); // Blick von oben
        let side = Vec3::new(-flat.z, 0.0, flat.x);
        self.center += flat * fwd + side * strafe + Vec3::Y * up;
    }
}

/// Demo-App: Eingaben sammeln, Kamera führen, jeden Tick rendern.
pub struct LiveApp {
    project: &'static Project,
    fallback: &'static Material,
    backend: Option<WgpuBackend>,
    window_src: Option<WindowHandleSource>,
    size: (u32, u32),
    orbit: Orbit,
    radius: f32,
    keys: Vec<Key>,
    dragging: bool,
    /// Letzte absolute Zeigerposition — `MouseMove` liefert Koordinaten,
    /// keine Deltas; die Orbit-Drehung braucht aber Differenzen.
    last_mouse: Option<(f64, f64)>,
    frames: u64,
    clock: std::time::Instant,
}

impl LiveApp {
    fn new(project: &'static Project) -> Self {
        Self {
            project,
            // Die Demo hält alles bis Prozessende — 'static erspart der App
            // selbstreferenzielle Structs (Batches borrowen das Projekt).
            fallback: Box::leak(Box::new(Material::default())),
            backend: None,
            window_src: None,
            size: (1280, 720),
            orbit: Orbit { yaw: std::f32::consts::FRAC_PI_4, pitch: 0.61, dist: 10.0, center: Vec3::ZERO },
            radius: 1.0,
            keys: Vec::new(),
            dragging: false,
            last_mouse: None,
            frames: 0,
            clock: std::time::Instant::now(),
        }
    }

    fn held(&self, k: Key) -> bool {
        self.keys.contains(&k)
    }

    fn on_input(&mut self, ev: &InputEvent) -> AppResponse {
        match ev {
            InputEvent::Key { key, pressed, repeat } => {
                if *pressed {
                    if !*repeat && *key == Key::Escape {
                        return AppResponse::Exit;
                    }
                    if !self.keys.contains(key) {
                        self.keys.push(*key);
                    }
                } else {
                    self.keys.retain(|k| k != key);
                }
            }
            InputEvent::MouseButton { button: MouseButton::Left, pressed } => {
                self.dragging = *pressed;
            }
            InputEvent::MouseMove { x, y } => {
                // Deltas aus absoluten Positionen bilden; der erste Move
                // setzt nur die Referenz (kein Sprung über das Fenster).
                let delta = self.last_mouse.map(|(px, py)| (x - px, y - py));
                self.last_mouse = Some((*x, *y));
                if self.dragging {
                    if let Some((dx, dy)) = delta {
                        self.orbit.yaw -= dx as f32 * 0.005;
                        self.orbit.pitch =
                            (self.orbit.pitch + dy as f32 * 0.005).clamp(-0.05, 1.45);
                    }
                }
            }
            InputEvent::Wheel { dy, .. } => {
                let f = (1.0 - *dy as f32 * 0.12).clamp(0.5, 2.0);
                self.orbit.dist =
                    (self.orbit.dist * f).clamp(self.radius * 0.25, self.radius * 8.0);
            }
            _ => {}
        }
        AppResponse::Continue
    }

    /// Backend + Surface aus dem Fenster-Handle.
    fn init_backend(&mut self, width: u32, height: u32) -> Result<()> {
        let mut backend = WgpuBackend::new();
        if let Some(src) = &self.window_src {
            backend.set_window_source(src.as_any())?;
        }
        backend.init()?;
        backend.resize(width, height);
        eprintln!(
            "live: {} Fenster {}x{} — Ziehen=drehen, Rad=Zoom, WASD/QE=fliegen, Shift=4×, Esc=Ende",
            backend.name(),
            width,
            height
        );
        self.backend = Some(backend);
        Ok(())
    }

    /// Ein Frame: Szenen-Batches composed → render (presentt selbst).
    fn draw(&mut self) -> Result<()> {
        let Some(backend) = self.backend.as_mut() else { return Ok(()) };
        let camera = self.orbit.camera(self.size.0, self.size.1);
        let (batches, _bounds) = gather(self.project, self.fallback);
        backend.render(&camera, &batches)?;
        Ok(())
    }
}

impl App for LiveApp {
    fn attach_window(&mut self, window: WindowHandleSource) {
        self.window_src = Some(window);
    }

    fn on_event(&mut self, event: PlatformEvent) -> AppResponse {
        match event {
            PlatformEvent::Created { width, height, .. } => {
                self.size = (width.max(1), height.max(1));
                // Kamera-Preset auf die Szenenmitte (wie im Headless-Standbild).
                let (_probe, (mn, mx)) = gather(self.project, self.fallback);
                self.orbit.center = (mn + mx) * 0.5;
                self.radius = (mx - mn).length().max(1.0) * 0.5;
                self.orbit.dist = self.radius * 2.3;
                match self.init_backend(self.size.0, self.size.1) {
                    Ok(()) => AppResponse::Continue,
                    Err(e) => {
                        eprintln!("Backend-Fehler: {e:#}");
                        AppResponse::Exit
                    }
                }
            }
            PlatformEvent::Resized { width, height, .. } => {
                self.size = (width.max(1), height.max(1));
                if let Some(b) = self.backend.as_mut() {
                    let (w, h) = self.size;
                    b.resize(w, h);
                }
                AppResponse::Continue
            }
            PlatformEvent::Input(ev) => self.on_input(&ev),
            PlatformEvent::Tick { dt, .. } => {
                let speed = self.radius * 0.7 * dt;
                let boost =
                    if self.held(Key::ShiftLeft) || self.held(Key::ShiftRight) { 4.0 } else { 1.0 };
                let mut move_by = Vec3::ZERO;
                if self.held(Key::W) {
                    move_by.x += speed * boost;
                }
                if self.held(Key::S) {
                    move_by.x -= speed * boost;
                }
                if self.held(Key::D) {
                    move_by.y += speed * boost;
                }
                if self.held(Key::A) {
                    move_by.y -= speed * boost;
                }
                if self.held(Key::E) {
                    move_by.z += speed * boost;
                }
                if self.held(Key::Q) {
                    move_by.z -= speed * boost;
                }
                if move_by != Vec3::ZERO {
                    self.orbit.fly(move_by.x, move_by.y, move_by.z);
                }
                if let Err(e) = self.draw() {
                    eprintln!("Render-Fehler: {e:#}");
                    return AppResponse::Exit;
                }
                self.frames += 1;
                if self.frames % 120 == 0 {
                    let secs = self.clock.elapsed().as_secs_f64().max(0.001);
                    eprintln!(
                        "live: {} frames ({:.1} fps)",
                        self.frames,
                        self.frames as f64 / secs
                    );
                }
                AppResponse::Continue
            }
            PlatformEvent::Closing => AppResponse::Exit,
        }
    }
}

/// Fenster-Treiber starten; blockiert, bis die App endet.
pub fn run(project: &'static Project) -> Result<()> {
    let mut app = LiveApp::new(project);
    let options = WindowOptions {
        title: "RE — Fenster-Modus (wgpu)".into(),
        width: 1280,
        height: 720,
    };
    run_app(&mut app, options).map_err(anyhow::Error::msg)
}
