//! # re-ecs — Entity-Component-System der RE-Engine
//!
//! Die Laufzeit-Welt für Spiele: ein [`World`] hält Entitäten
//! (32-Bit-Index + Generation), Komponenten liegen pro Typ in einer
//! eigenen Sparse-Tabelle (dichte, cache-freundliche Spalten), und ein
//! [`Schedule`] läuft [`Systeme`](System) in fester Reihenfolge mit
//! Delta-Zeit.
//!
//! Der Szenengraf aus `re-core` ist der Editier-Zustand; beim Start
//! werden daraus ECS-Instanzen, die der Renderer ohne Zwischenkopien
//! mitliest (Entwurfsgrundlagen: `docs/architecture.md`).
//!
//! Bewusste Entscheidungen (M3a):
//!
//! * Sparse-Set-Speicher: Komponenten-Zugriff ist O(1), Anlegen und
//!   Entfernen ebenfalls; Iteration läuft immer über die dichte Spalte.
//! * Die archetypbasierte Speicherform (bessere Sortierung für sehr
//!   große Welten) kann später hinter derselben API einziehen, ohne
//!   Systeme oder Szenen anzufassen.
//! * Kein Fenster-, Zeit- oder Render-Bezug: `Schedule::run` bekommt
//!   `dt` von außen — damit ist alles headless testbar.

mod entity;
mod storage;
mod system;
mod world;

pub use entity::Entity;
pub use system::{Schedule, System};
pub use world::{Component, World};
