//! Die Welt: Entitäten, ihre Komponenten und typisierte Zugriffe.

use std::any::TypeId;
use std::collections::HashMap;

use crate::entity::{Alloc, Entity};
use crate::storage::{pair_iter, SparseSet, Storage};

/// Markierung für alle speicherbaren Komponenten.
///
/// Jede `'static`-Type mit `Send + Sync` ist automatisch Komponente —
/// eine zusätzliche Registrierung gibt es nicht.
pub trait Component: std::any::Any + Send + Sync + 'static {}
impl<T: std::any::Any + Send + Sync> Component for T {}

/// Sammlung aller Entitäten und ihrer Komponenten.
#[derive(Default)]
pub struct World {
    alloc: Alloc,
    storages: HashMap<TypeId, Box<dyn Storage>>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    /// Neue Entität ohne Komponenten.
    pub fn spawn(&mut self) -> Entity {
        self.alloc.spawn()
    }

    /// Entfernt die Entität samt allen Komponenten; `false` für
    /// bereits tote oder unbekannte Handles.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.alloc.alive(entity) {
            return false;
        }
        for storage in self.storages.values_mut() {
            storage.remove_entity(entity);
        }
        self.alloc.despawn(entity);
        true
    }

    /// Ist das Handle noch aktuell (von keiner neueren Entität abgelöst)?
    pub fn alive(&self, entity: Entity) -> bool {
        self.alloc.alive(entity)
    }

    /// Alle lebenden Entitäten in Speicherfolge.
    pub fn entities(&self) -> Vec<Entity> {
        self.alloc.iter_live().collect()
    }

    pub fn len(&self) -> usize {
        self.alloc.live_len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn storage_of<T: Component>(&self) -> Option<&SparseSet<T>> {
        self.storages
            .get(&TypeId::of::<T>())
            .and_then(|s| s.as_any().downcast_ref::<SparseSet<T>>())
    }

    fn entry<T: Component>(&mut self) -> &mut SparseSet<T> {
        self.storages
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(SparseSet::<T>::new()))
            .as_any_mut()
            .downcast_mut::<SparseSet<T>>()
            .expect("Speicher-Tabelle passt zum TypeId")
    }

    /// Setzt die Komponente und gibt die vorherige zurück; `None` bei
    /// toter Entität.
    pub fn insert<T: Component>(&mut self, entity: Entity, component: T) -> Option<T> {
        if !self.alloc.alive(entity) {
            return None;
        }
        self.entry::<T>().insert(entity, component)
    }

    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        self.storage_of::<T>()?.get(entity)
    }

    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        if !self.alloc.alive(entity) {
            return None;
        }
        self.entry::<T>().get_mut(entity)
    }

    /// Entfernt die Komponente und gibt ihren Wert zurück.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        self.storages
            .get_mut(&TypeId::of::<T>())?
            .as_any_mut()
            .downcast_mut::<SparseSet<T>>()?
            .remove(entity)
    }

    pub fn has<T: Component>(&self, entity: Entity) -> bool {
        self.storage_of::<T>()
            .is_some_and(|s| s.contains(entity))
    }

    /// Wie viele Entitäten tragen diese Komponente?
    pub fn count<T: Component>(&self) -> usize {
        self.storage_of::<T>()
            .map(|s| s.len())
            .unwrap_or(0)
    }

    /// Iteriert alle Entitäten mit `T` (unveränderlich).
    pub fn for_each<T: Component>(&self, f: impl FnMut(Entity, &T)) {
        if let Some(set) = self.storage_of::<T>() {
            set.for_each(f);
        }
    }

    /// Iteriert alle Entitäten mit `T` (ändernd).
    pub fn for_each_mut<T: Component>(&mut self, f: impl FnMut(Entity, &mut T)) {
        // Tabelle kurzzeitig aus der Registry nehmen: so bekommt der
        // Closure exklusiven Zugriff, ohne die Welt doppelt zu borgen.
        if let Some(boxed) = self.storages.remove(&TypeId::of::<T>()) {
            let mut boxed = boxed;
            if let Some(set) = boxed.as_any_mut().downcast_mut::<SparseSet<T>>() {
                set.for_each_mut(f);
            }
            self.storages.insert(TypeId::of::<T>(), boxed);
        }
    }

    /// Join über zwei Komponenten; die kleinere Tabelle treibt.
    ///
    /// Die beiden Typen müssen verschieden sein (`debug_assert`).
    pub fn for_each_pair<A: Component, B: Component>(
        &mut self,
        f: impl FnMut(Entity, &mut A, &mut B),
    ) {
        debug_assert_ne!(
            TypeId::of::<A>(),
            TypeId::of::<B>(),
            "for_each_pair braucht zwei verschiedene Komponenten-Typen"
        );
        let mut a = self.storages.remove(&TypeId::of::<A>());
        let mut b = self.storages.remove(&TypeId::of::<B>());
        if let (Some(a), Some(b)) = (&mut a, &mut b) {
            if let (Some(a), Some(b)) = (
                a.as_any_mut().downcast_mut::<SparseSet<A>>(),
                b.as_any_mut().downcast_mut::<SparseSet<B>>(),
            ) {
                pair_iter(a, b, f);
            }
        }
        if let Some(x) = a {
            self.storages.insert(TypeId::of::<A>(), x);
        }
        if let Some(x) = b {
            self.storages.insert(TypeId::of::<B>(), x);
        }
    }
}
