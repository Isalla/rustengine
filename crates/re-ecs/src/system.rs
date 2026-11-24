//! Systeme und Schedule: die Loop ohne Fenster- oder Zeit-Bezug.

use crate::world::World;

/// Ein Durchgang über die Welt (Bewegung, KI, Streaming …).
pub trait System {
    /// Name für Konsole, Profiling und Fehlermeldungen.
    fn name(&self) -> &str {
        "system"
    }

    /// Ein Frame-Schritt; `dt` ist die Zeit seit dem letzten Lauf.
    fn update(&mut self, world: &mut World, dt: f32);
}

/// Closures sind sofort Systeme — für Zwischenschritte und Tests.
impl<F> System for F
where
    F: FnMut(&mut World, f32),
{
    fn update(&mut self, world: &mut World, dt: f32) {
        self(world, dt);
    }
}

/// Fährt Systeme in fester Reihenfolge — deterministisch und damit
/// nachvollziehbar (Grundlage für Replay und Netzwerk-Vergleiche).
#[derive(Default)]
pub struct Schedule {
    systems: Vec<Box<dyn System>>,
    tick: u64,
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, system: impl System + 'static) {
        self.systems.push(Box::new(system));
    }

    /// Ein Frame: alle Systeme in Einfügereihenfolge.
    pub fn run(&mut self, world: &mut World, dt: f32) {
        self.tick += 1;
        for system in &mut self.systems {
            system.update(world, dt);
        }
    }

    /// Wie oft wurde `run` aufgerufen?
    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn len(&self) -> usize {
        self.systems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }
}
