//! Eingabe-Modell von RE — unabhängig von winit/Windowing-Backends.
//!
//! Regeln aus `docs/architektur.md`: Engine-Code matcht auf [`Key`]
//! (layout-unabhängig, physische Taste) oder bekommt gedruckte Zeichen als
//! [`InputEvent::Char`]. Die Backend-Module (winit, später Web/Konsole)
//! übersetzen nur nach hierher — und [`InputState`] sammelt den Zustand,
//! damit Renderer und Gameplay nicht am Event-Stream kleben.

use std::collections::HashSet;

/// Physische Taste (QWERTZ/QUERTZ egal — Scancode-Mapping in den Backends).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    // Buchstaben
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    // Zahlen (obere Zeile)
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    // Funktionstasten
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    // Steuerung
    Escape,
    Enter,
    Space,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    // Modifikatoren
    ControlLeft,
    ControlRight,
    ShiftLeft,
    ShiftRight,
    AltLeft,
    AltRight,
    /// Windows-/Super-/Cmd-Taste.
    SuperLeft,
    SuperRight,
    // Lock-Tasten
    CapsLock,
    NumLock,
    ScrollLock,
    /// Ziffernblock
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
    NumpadEnter,
    NumpadDecimal,
    // Rest des Standard-Layouts
    Minus,
    Equal,
    LeftBracket,
    RightBracket,
    Backslash,
    IntlBackslash,
    Semicolon,
    Apostrophe,
    Comma,
    Dot,
    Slash,
    /// Grave/Accent (`) neben 1 — bei manchen Layouts IntlBackslash.
    GraveAccent,
}

/// AT-Scancode → [`Key`] (x86-Standard-Layout, wie winit/X11/Wayland liefern).
///
/// Bewusst komplett ohne `unwrap_or`: unbekannte Codes werden zu `None`,
/// damit ein neuer Backend-Code nie still eine falsche Taste erzeugt.
/// (Der Windows-only MapVirtualKey-Pfad in [`crate::desktop`] mappt separat.)
pub fn key_from_scancode(code: u32) -> Option<Key> {
    let k = match code {
        0x01 => Key::Escape,
        0x02 => Key::Digit1,
        0x03 => Key::Digit2,
        0x04 => Key::Digit3,
        0x05 => Key::Digit4,
        0x06 => Key::Digit5,
        0x07 => Key::Digit6,
        0x08 => Key::Digit7,
        0x09 => Key::Digit8,
        0x0A => Key::Digit9,
        0x0B => Key::Digit0,
        0x0C => Key::Minus,
        0x0D => Key::Equal,
        0x0E => Key::Backspace,
        0x0F => Key::Tab,
        0x10 => Key::Q,
        0x11 => Key::W,
        0x12 => Key::E,
        0x13 => Key::R,
        0x14 => Key::T,
        0x15 => Key::Y,
        0x16 => Key::U,
        0x17 => Key::I,
        0x18 => Key::O,
        0x19 => Key::P,
        0x1A => Key::LeftBracket,
        0x1B => Key::RightBracket,
        0x1C => Key::Enter,
        0x1D => Key::ControlLeft,
        0x1E => Key::A,
        0x1F => Key::S,
        0x20 => Key::D,
        0x21 => Key::F,
        0x22 => Key::G,
        0x23 => Key::H,
        0x24 => Key::J,
        0x25 => Key::K,
        0x26 => Key::L,
        0x27 => Key::Semicolon,
        0x28 => Key::Apostrophe,
        0x29 => Key::GraveAccent,
        0x2A => Key::ShiftLeft,
        0x2B => Key::Backslash,
        0x2C => Key::Z,
        0x2D => Key::X,
        0x2E => Key::C,
        0x2F => Key::V,
        0x30 => Key::B,
        0x31 => Key::N,
        0x32 => Key::M,
        0x33 => Key::Comma,
        0x34 => Key::Dot,
        0x35 => Key::Slash,
        0x36 => Key::ShiftRight,
        0x37 => Key::NumpadMultiply,
        0x38 => Key::AltLeft,
        0x39 => Key::Space,
        0x3A => Key::CapsLock,
        0x3B => Key::F1,
        0x3C => Key::F2,
        0x3D => Key::F3,
        0x3E => Key::F4,
        0x3F => Key::F5,
        0x40 => Key::F6,
        0x41 => Key::F7,
        0x42 => Key::F8,
        0x43 => Key::F9,
        0x44 => Key::F10,
        0x45 => Key::NumLock,
        0x46 => Key::ScrollLock,
        0x47 => Key::Numpad7,
        0x48 => Key::Numpad8,
        0x49 => Key::Numpad9,
        0x4A => Key::NumpadSubtract,
        0x4B => Key::Numpad4,
        0x4C => Key::Numpad5,
        0x4D => Key::Numpad6,
        0x4E => Key::NumpadAdd,
        0x4F => Key::Numpad1,
        0x50 => Key::Numpad2,
        0x51 => Key::Numpad3,
        0x52 => Key::Numpad0,
        0x53 => Key::NumpadDecimal,
        0x57 => Key::F11,
        0x58 => Key::F12,
        // Extended-Codes (AT-Set-1 E0-Präfix, von winit als Ein-Code geliefert).
        0x60 => Key::NumpadEnter, // E0 1C
        0x61 => Key::ControlRight, // E0 1D
        0x62 => Key::NumpadDivide, // E0 35
        0x64 => Key::AltRight, // E0 38
        0x7D => Key::SuperLeft, // E0 5B
        0x7E => Key::SuperRight, // E0 5F
        _ => return None,
    };
    Some(k)
}

/// Maustasten — Namen wie im Backend, `Other` fängt Extras ab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// Eingabe-Ereignis (backend-übersetzt, positionen in Fenster-Pixeln).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    /// Physische Taste gedrückt/losgelassen; `repeat` = Auto-Wiederholung.
    Key { key: Key, pressed: bool, repeat: bool },
    /// Gedrucktes Zeichen (Layout/Shift/UTF-8 bereits angewandt).
    Char(char),
    /// Zeiger bewegt, Koordinaten relativ zur Fenster-Innenkante.
    MouseMove { x: f64, y: f64 },
    /// Maustaste gedrückt/losgelassen.
    MouseButton { button: MouseButton, pressed: bool },
    /// Scroll-Rad; Zeilen-Deltas sind auf ±1 pro Raste genormt.
    Wheel { dx: f64, dy: f64 },
}

/// Akkumulierter Eingabezustand pro Frame — die bequeme Front für Spielschleifen.
///
/// [`Self::apply`] frisst [`InputEvent`]s, [`Self::frame_start`] nullt die
/// Frame-bezogenen Differenzen (`mouse_delta`, `wheel`).
#[derive(Debug, Default, Clone)]
pub struct InputState {
    pub keys: HashSet<Key>,
    pub buttons: HashSet<MouseButton>,
    /// Letzte Zeigerposition im Fenster (None, bevor etwas bewegt wurde).
    pub mouse: Option<(f64, f64)>,
    /// Zeigerbewegung seit [`Self::frame_start`] (kein Sprung beim ersten Update).
    pub mouse_delta: (f64, f64),
    /// Aufsummierte Rad-Deltas seit [`Self::frame_start`].
    pub wheel: f32,
}

impl InputState {
    /// Ereignis in den Zustand einarbeiten.
    pub fn apply(&mut self, ev: &InputEvent) {
        match *ev {
            InputEvent::Key { key, pressed, .. } => {
                if pressed {
                    self.keys.insert(key);
                } else {
                    self.keys.remove(&key);
                }
            }
            InputEvent::Char(_) => {}
            InputEvent::MouseMove { x, y } => {
                if let Some((px, py)) = self.mouse {
                    self.mouse_delta.0 += x - px;
                    self.mouse_delta.1 += y - py;
                }
                self.mouse = Some((x, y));
            }
            InputEvent::MouseButton { button, pressed } => {
                if pressed {
                    self.buttons.insert(button);
                } else {
                    self.buttons.remove(&button);
                }
            }
            InputEvent::Wheel { dx, dy } => {
                self.wheel += (dy + dx) as f32;
            }
        }
    }

    /// Taste gerade gedrückt?
    pub fn pressed(&self, key: Key) -> bool {
        self.keys.contains(&key)
    }

    /// Maustaste gerade gedrückt?
    pub fn button(&self, b: MouseButton) -> bool {
        self.buttons.contains(&b)
    }

    /// Frame-Differenzen nullen (vor der Eingabe-Auswertung eines Frames).
    pub fn frame_start(&mut self) {
        self.mouse_delta = (0.0, 0.0);
        self.wheel = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scancode_mapping() {
        assert_eq!(key_from_scancode(0x11), Some(Key::W));
        assert_eq!(key_from_scancode(0x1E), Some(Key::A));
        assert_eq!(key_from_scancode(0x39), Some(Key::Space));
        assert_eq!(key_from_scancode(0x01), Some(Key::Escape));
        assert_eq!(key_from_scancode(0xFFFF), None);
    }

    #[test]
    fn zustand_haelt_und_differenzen() {
        let mut s = InputState::default();
        s.apply(&InputEvent::Key { key: Key::W, pressed: true, repeat: false });
        assert!(s.pressed(Key::W));
        s.apply(&InputEvent::Key { key: Key::W, pressed: false, repeat: false });
        assert!(!s.pressed(Key::W));

        // Erstes Update setzt nur die Position (kein falscher Delta-Sprung).
        s.apply(&InputEvent::MouseMove { x: 100.0, y: 50.0 });
        assert_eq!(s.mouse_delta, (0.0, 0.0));
        s.apply(&InputEvent::MouseMove { x: 110.0, y: 45.0 });
        assert_eq!(s.mouse_delta, (10.0, -5.0));
        s.apply(&InputEvent::Wheel { dx: 0.0, dy: 1.0 });
        s.apply(&InputEvent::Wheel { dx: 0.0, dy: 1.0 });
        assert_eq!(s.wheel, 2.0);
        s.frame_start();
        assert_eq!(s.mouse_delta, (0.0, 0.0));
        assert_eq!(s.wheel, 0.0);
        assert_eq!(s.mouse, Some((110.0, 45.0)));
    }
}
