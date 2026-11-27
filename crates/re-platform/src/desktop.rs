//! Desktop-Treiber: winit 0.30 Event-Loop → [`PlatformEvent`].
//!
//! Nur hinter Feature `window`. Der Treiber erzeugt das Fenster, hält es als
//! `Arc<Window>` und gibt es via [`App::attach_window`] an die App — die
//! Surface-Lebensdauer koppelt sich dadurch an [`WindowHandleSource`]
//! (wgpu sichert das Handle-Source-Feld selbst).
//!
//! Timing: `ControlFlow::Poll` + `Instant`-Differenzen (winit liefert kein
//! Frame-Budget; vsync-artige Drossel kommt später über Present-Modes).

use std::sync::Arc;
use std::time::Instant;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton as WMouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey, Key as WKey};
use winit::window::{Icon, Window, WindowId};

use crate::app::{App, AppResponse, PlatformEvent, WindowOptions};
use crate::input::{InputEvent, Key, MouseButton};

/// Fenster-Handle-Quelle als `Arc` — hält das Fenster lebendig, solange eine
/// Surface (oder sonstiger Consumer) es referenziert.
///
/// wgpu speichert das übergebene Source-Objekt in `Surface::_handle_source`,
/// damit überlebt das Fenster die Surface-Erzeugung, ohne dass unser
/// Session-State sich selbst referenzieren muss.
#[derive(Clone)]
pub struct WindowHandleSource(Arc<Window>);

impl WindowHandleSource {
    /// Titel zur Laufzeit ändern (Statusanzeigen im Demo/Editor).
    pub fn set_title(&self, title: &str) {
        self.0.set_title(title);
    }
    /// Innere Größe in physischen Pixeln.
    pub fn inner_size(&self) -> (u32, u32) {
        let s = self.0.inner_size();
        (s.width, s.height)
    }
    /// HiDPI-Faktor (1.0 = 100 %).
    pub fn scale_factor(&self) -> f64 {
        self.0.scale_factor()
    }
    /// Typgelöschtes Handle für Backend-Traits (`GfxBackend::set_window_source`):
    /// ein `Arc<re_wgpu::winit::window::Window>`, das der Empfänger
    /// zurück-downcastet — so kennt diese Schicht kein wgpu und umgekehrt.
    pub fn as_any(&self) -> Arc<dyn std::any::Any + Send + Sync> {
        Arc::clone(&self.0) as Arc<dyn std::any::Any + Send + Sync>
    }
}

impl HasWindowHandle for WindowHandleSource {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.0.window_handle()
    }
}

impl HasDisplayHandle for WindowHandleSource {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.0.display_handle()
    }
}

fn map_button(b: WMouseButton) -> MouseButton {
    match b {
        WMouseButton::Left => MouseButton::Left,
        WMouseButton::Right => MouseButton::Right,
        WMouseButton::Middle => MouseButton::Middle,
        WMouseButton::Back => MouseButton::Back,
        WMouseButton::Forward => MouseButton::Forward,
        WMouseButton::Other(n) => MouseButton::Other(n),
    }
}

/// winit-`KeyCode` (portabler physischer Code) → RE-[`Key`].
///
/// Plattform-Keys ohne RE-Gegenstück (Medientasten, F13+, IME) werden zu
/// `None` — sie tauchen höchstens noch als [`InputEvent::Char`] auf.
fn map_key(code: KeyCode) -> Option<Key> {
    let k = match code {
        KeyCode::KeyA => Key::A,
        KeyCode::KeyB => Key::B,
        KeyCode::KeyC => Key::C,
        KeyCode::KeyD => Key::D,
        KeyCode::KeyE => Key::E,
        KeyCode::KeyF => Key::F,
        KeyCode::KeyG => Key::G,
        KeyCode::KeyH => Key::H,
        KeyCode::KeyI => Key::I,
        KeyCode::KeyJ => Key::J,
        KeyCode::KeyK => Key::K,
        KeyCode::KeyL => Key::L,
        KeyCode::KeyM => Key::M,
        KeyCode::KeyN => Key::N,
        KeyCode::KeyO => Key::O,
        KeyCode::KeyP => Key::P,
        KeyCode::KeyQ => Key::Q,
        KeyCode::KeyR => Key::R,
        KeyCode::KeyS => Key::S,
        KeyCode::KeyT => Key::T,
        KeyCode::KeyU => Key::U,
        KeyCode::KeyV => Key::V,
        KeyCode::KeyW => Key::W,
        KeyCode::KeyX => Key::X,
        KeyCode::KeyY => Key::Y,
        KeyCode::KeyZ => Key::Z,
        KeyCode::Digit0 => Key::Digit0,
        KeyCode::Digit1 => Key::Digit1,
        KeyCode::Digit2 => Key::Digit2,
        KeyCode::Digit3 => Key::Digit3,
        KeyCode::Digit4 => Key::Digit4,
        KeyCode::Digit5 => Key::Digit5,
        KeyCode::Digit6 => Key::Digit6,
        KeyCode::Digit7 => Key::Digit7,
        KeyCode::Digit8 => Key::Digit8,
        KeyCode::Digit9 => Key::Digit9,
        KeyCode::F1 => Key::F1,
        KeyCode::F2 => Key::F2,
        KeyCode::F3 => Key::F3,
        KeyCode::F4 => Key::F4,
        KeyCode::F5 => Key::F5,
        KeyCode::F6 => Key::F6,
        KeyCode::F7 => Key::F7,
        KeyCode::F8 => Key::F8,
        KeyCode::F9 => Key::F9,
        KeyCode::F10 => Key::F10,
        KeyCode::F11 => Key::F11,
        KeyCode::F12 => Key::F12,
        KeyCode::Escape => Key::Escape,
        KeyCode::Enter => Key::Enter,
        KeyCode::NumpadEnter => Key::NumpadEnter,
        KeyCode::Space => Key::Space,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::ControlLeft => Key::ControlLeft,
        KeyCode::ControlRight => Key::ControlRight,
        KeyCode::ShiftLeft => Key::ShiftLeft,
        KeyCode::ShiftRight => Key::ShiftRight,
        KeyCode::AltLeft => Key::AltLeft,
        KeyCode::AltRight => Key::AltRight,
        KeyCode::SuperLeft => Key::SuperLeft,
        KeyCode::SuperRight => Key::SuperRight,
        KeyCode::CapsLock => Key::CapsLock,
        KeyCode::NumLock => Key::NumLock,
        KeyCode::ScrollLock => Key::ScrollLock,
        KeyCode::Numpad0 => Key::Numpad0,
        KeyCode::Numpad1 => Key::Numpad1,
        KeyCode::Numpad2 => Key::Numpad2,
        KeyCode::Numpad3 => Key::Numpad3,
        KeyCode::Numpad4 => Key::Numpad4,
        KeyCode::Numpad5 => Key::Numpad5,
        KeyCode::Numpad6 => Key::Numpad6,
        KeyCode::Numpad7 => Key::Numpad7,
        KeyCode::Numpad8 => Key::Numpad8,
        KeyCode::Numpad9 => Key::Numpad9,
        KeyCode::NumpadAdd => Key::NumpadAdd,
        KeyCode::NumpadSubtract => Key::NumpadSubtract,
        KeyCode::NumpadMultiply => Key::NumpadMultiply,
        KeyCode::NumpadDivide => Key::NumpadDivide,
        KeyCode::NumpadDecimal => Key::NumpadDecimal,
        KeyCode::NumpadComma => Key::NumpadDecimal, // DE-Dezimaltrenner auf dem Block
        KeyCode::Minus => Key::Minus,
        KeyCode::Equal => Key::Equal,
        KeyCode::BracketLeft => Key::LeftBracket,
        KeyCode::BracketRight => Key::RightBracket,
        KeyCode::Backslash => Key::Backslash,
        KeyCode::IntlBackslash => Key::IntlBackslash,
        KeyCode::Semicolon => Key::Semicolon,
        KeyCode::Quote => Key::Apostrophe,
        KeyCode::Comma => Key::Comma,
        KeyCode::Period => Key::Dot,
        KeyCode::Slash => Key::Slash,
        KeyCode::Backquote => Key::GraveAccent,
        _ => return None,
    };
    Some(k)
}

/// winit-Session, die eine [`App`] antreibt.
struct Session<'a> {
    app: &'a mut dyn App,
    options: WindowOptions,
    window: Option<Arc<Window>>,
    scale: f64,
    start: Instant,
    last: Option<Instant>,
    exit: bool,
}

/// Programm-Icon (RGBA, bei der Kompilierung aus `media/icons/` eingebettet).
/// Nutzen X11/Wayland-Taskbar und Windows-Taskbar; die Datei-Icon-Verknuepfung
/// der EXE entsteht separat ueber `media/icons/icon.res` (siehe build.rs von
/// re-wgpu). Kein Icon statt Fehler, falls die Rohdaten je anders skaliert
/// werden — der Desktop zeigt dann sein Platzhalterbild.
fn app_icon() -> Option<Icon> {
    const RGBA: &[u8] = include_bytes!("../../../media/icons/icon-128.rgba");
    Icon::from_rgba(RGBA.to_vec(), 128, 128).ok()
}

impl Session<'_> {
    /// Event an die App; `Exit` beendet die Schleife (docs: kein Closing mehr).
    fn dispatch(&mut self, el: &ActiveEventLoop, ev: PlatformEvent) -> AppResponse {
        let r = self.app.on_event(ev);
        if r == AppResponse::Exit {
            self.exit = true;
            el.exit();
        }
        r
    }
}

impl ApplicationHandler for Session<'_> {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        // Nach suspend/resume (mobil/Displaywechsel) nicht zweites Fenster.
        if self.window.is_some() || self.exit {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(&self.options.title)
            .with_window_icon(app_icon())
            .with_inner_size(PhysicalSize::new(
                self.options.width.max(1),
                self.options.height.max(1),
            ));
        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("Fenster konnte nicht erzeugt werden: {e}");
                self.exit = true;
                el.exit();
                return;
            }
        };
        self.scale = window.scale_factor();
        let size = window.inner_size();
        el.set_control_flow(ControlFlow::Poll);
        let src = WindowHandleSource(Arc::clone(&window));
        self.app.attach_window(src);
        self.window = Some(window);
        self.last = Some(Instant::now());
        self.dispatch(
            el,
            PlatformEvent::Created {
                width: size.width.max(1),
                height: size.height.max(1),
                scale_factor: self.scale,
            },
        );
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if self.exit {
            return;
        }
        match event {
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    let sf = self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(self.scale);
                    self.dispatch(
                        el,
                        PlatformEvent::Resized { width: size.width, height: size.height, scale_factor: sf },
                    );
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor;
                if let Some(w) = &self.window {
                    let s = w.inner_size();
                    self.dispatch(
                        el,
                        PlatformEvent::Resized {
                            width: s.width,
                            height: s.height,
                            scale_factor,
                        },
                    );
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                // Physische Taste (Gaming-Pfad) …
                if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some(key) = map_key(code) {
                        self.dispatch(
                            el,
                            PlatformEvent::Input(InputEvent::Key {
                                key,
                                pressed,
                                repeat: event.repeat,
                            }),
                        );
                    }
                }
                // … und gedrucktes Zeichen (Texteingabe-Pfad) separat.
                if pressed {
                    if let WKey::Character(s) = &event.logical_key {
                        if let Some(c) = s.chars().next() {
                            self.dispatch(el, PlatformEvent::Input(InputEvent::Char(c)));
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.dispatch(
                    el,
                    PlatformEvent::Input(InputEvent::MouseButton {
                        button: map_button(button),
                        pressed: state == ElementState::Pressed,
                    }),
                );
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.dispatch(
                    el,
                    PlatformEvent::Input(InputEvent::MouseMove {
                        x: position.x,
                        y: position.y,
                    }),
                );
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x as f64, y as f64),
                    MouseScrollDelta::PixelDelta(p) => (p.x / 100.0, p.y / 100.0),
                };
                self.dispatch(el, PlatformEvent::Input(InputEvent::Wheel { dx, dy }));
            }
            WindowEvent::CloseRequested => {
                // Schließen kann die App nicht mehr verhindern; Closing dient
                // als letzter Aufräum-Haken.
                self.exit = true;
                self.app.on_event(PlatformEvent::Closing);
                el.exit();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.exit || self.window.is_none() {
            return;
        }
        let now = Instant::now();
        let last = *self.last.get_or_insert(now);
        let dt = now.duration_since(last).as_secs_f32().max(0.0);
        self.last = Some(now);
        let elapsed = now.duration_since(self.start).as_secs_f32();
        if self.dispatch(el, PlatformEvent::Tick { dt, elapsed }) == AppResponse::Exit {
            return; // dispatch hat el.exit() schon gerufen
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        self.exit = true;
    }
}

/// Fenster-Event-Loop ausführen; Fehler als String (App-Code ist sync).
pub fn run(app: &mut dyn App, options: WindowOptions) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("EventLoop: {e}"))?;
    let mut session = Session {
        app,
        options,
        window: None,
        scale: 1.0,
        start: Instant::now(),
        last: None,
        exit: false,
    };
    event_loop.run_app(&mut session).map_err(|e| format!("Event-Loop: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappt_mausbutton_und_scrolledeltas() {
        assert_eq!(map_button(WMouseButton::Left), MouseButton::Left);
        assert_eq!(map_button(WMouseButton::Other(9)), MouseButton::Other(9));
    }

    #[test]
    fn mappt_tastencodes() {
        assert_eq!(map_key(KeyCode::KeyW), Some(Key::W));
        assert_eq!(map_key(KeyCode::Escape), Some(Key::Escape));
        assert_eq!(map_key(KeyCode::NumpadEnter), Some(Key::NumpadEnter));
        assert_eq!(map_key(KeyCode::Quote), Some(Key::Apostrophe));
        assert_eq!(map_key(KeyCode::MediaPlayPause), None);
    }
}
