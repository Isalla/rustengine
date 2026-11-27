//! Komponenten-Speicher: eine Sparse-Tabelle pro Komponenten-Typ.

use std::any::Any;

use crate::entity::Entity;

/// typgelöschte Ansicht auf einen Sparse-Set (für die Welt-Registry).
///
/// `Send` als Supertrait, damit `World` (und damit `Mutex<World>`) über
/// Threads geteilt werden kann — Editor und Agent adressieren dieselbe
/// Welt. Alle Komponenten sind `Send + Sync` (siehe `Component`).
pub(crate) trait Storage: Any + Send {
    fn len(&self) -> usize;
    fn contains(&self, entity: Entity) -> bool;
    fn remove_entity(&mut self, entity: Entity);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

const EMPTY: u32 = u32::MAX;

/// Sparse-Set: `sparse[index]` zeigt in die dichte Spalte; dort liegen
/// `entities` (Handle) und `data` (die Komponentenwerte) Paar für Paar.
pub(crate) struct SparseSet<T> {
    sparse: Vec<u32>,
    entities: Vec<Entity>,
    data: Vec<T>,
}

impl<T: 'static> SparseSet<T> {
    pub(crate) fn new() -> Self {
        Self {
            sparse: Vec::new(),
            entities: Vec::new(),
            data: Vec::new(),
        }
    }

    /// Setzt `value`; liefert die vorherige Komponente derselben Entität.
    pub(crate) fn insert(&mut self, entity: Entity, value: T) -> Option<T> {
        let i = entity.index as usize;
        if self.sparse.len() <= i {
            self.sparse.resize(i + 1, EMPTY);
        }
        let pos = self.sparse[i];
        if pos != EMPTY {
            debug_assert_eq!(self.entities[pos as usize], entity);
            return Some(std::mem::replace(&mut self.data[pos as usize], value));
        }
        self.sparse[i] = self.data.len() as u32;
        self.entities.push(entity);
        self.data.push(value);
        None
    }

    pub(crate) fn pos(&self, entity: Entity) -> Option<usize> {
        let p = *self.sparse.get(entity.index as usize)?;
        if p != EMPTY && self.entities[p as usize] == entity {
            Some(p as usize)
        } else {
            None
        }
    }

    pub(crate) fn get(&self, entity: Entity) -> Option<&T> {
        self.pos(entity).map(|p| &self.data[p])
    }

    pub(crate) fn get_mut(&mut self, entity: Entity) -> Option<&mut T> {
        match self.pos(entity) {
            Some(p) => Some(&mut self.data[p]),
            None => None,
        }
    }

    pub(crate) fn remove(&mut self, entity: Entity) -> Option<T> {
        let p = self.pos(entity)?;
        let last = self.data.len() - 1;
        self.entities.swap(p, last);
        self.data.swap(p, last);
        self.sparse[self.entities[p].index as usize] = p as u32;
        self.sparse[entity.index as usize] = EMPTY;
        self.entities.pop();
        self.data.pop()
    }

    pub(crate) fn for_each(&self, mut f: impl FnMut(Entity, &T)) {
        for i in 0..self.data.len() {
            f(self.entities[i], &self.data[i]);
        }
    }

    pub(crate) fn for_each_mut(&mut self, mut f: impl FnMut(Entity, &mut T)) {
        for i in 0..self.data.len() {
            let entity = self.entities[i];
            f(entity, &mut self.data[i]);
        }
    }
}

impl<T: Send + Sync + 'static> Storage for SparseSet<T> {
    fn len(&self) -> usize {
        self.data.len()
    }

    fn contains(&self, entity: Entity) -> bool {
        self.pos(entity).is_some()
    }

    fn remove_entity(&mut self, entity: Entity) {
        let _ = self.remove(entity);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Gemeinsame Iteration über zwei Tabellen (verschiedene Typen): die
/// kleinere Spalte treibt, `f` erhält beide Werte einer Entität mutabel.
pub(crate) fn pair_iter<A: 'static, B: 'static>(
    a: &mut SparseSet<A>,
    b: &mut SparseSet<B>,
    mut f: impl FnMut(Entity, &mut A, &mut B),
) {
    if a.data.len() <= b.data.len() {
        for i in 0..a.data.len() {
            let entity = a.entities[i];
            if let Some(j) = b.pos(entity) {
                f(entity, &mut a.data[i], &mut b.data[j]);
            }
        }
    } else {
        for j in 0..b.data.len() {
            let entity = b.entities[j];
            if let Some(i) = a.pos(entity) {
                f(entity, &mut a.data[i], &mut b.data[j]);
            }
        }
    }
}
