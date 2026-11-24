//! Entität-Handles und Index-Allokator mit Generationen.

/// Handle in die Welt: Slot-Index + Generation.
///
/// Nach dem Freigeben steigt die Generation des Slots; ein altes Handle
/// zeigt dann ins Leere (`World::alive` und alle Zugriffe liefern `false`
/// beziehungsweise `None`), auch wenn dieselbe Index-Stelle später von
/// einer neuen Entität benutzt wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entity {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

impl Entity {
    /// Slot-Index (praktisch für Per-Index-Nebenlisten des Renderers).
    pub fn index(self) -> u32 {
        self.index
    }

    /// Stand des Generationenzählers dieses Slots (ab 1).
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// Vergibt Indizes wieder, ohne dass veraltete Handles weiterleben.
#[derive(Default)]
pub(crate) struct Alloc {
    /// Generation pro Slot; 0 = Slot wurde noch nie belegt.
    generations: Vec<u32>,
    /// Dichte Liste lebender Slots und Rückzeiger für O(1)-Entfernen.
    live: Vec<u32>,
    live_pos: Vec<u32>,
    free: Vec<u32>,
}

impl Alloc {
    pub(crate) fn spawn(&mut self) -> Entity {
        let index = match self.free.pop() {
            Some(i) => i,
            None => {
                let i = self.generations.len() as u32;
                self.generations.push(1);
                self.live_pos.push(0);
                i
            }
        };
        // Ein freigegebener Slot wurde durch despawn bereits weitergeschaltet;
        // hier gibt es keine zweite Erhöhung — das neue Handle erhält einfach
        // den aktuellen Stand (genau +1 gegenüber dem alten).
        self.live_pos[index as usize] = self.live.len() as u32;
        self.live.push(index);
        Entity {
            index,
            generation: self.generations[index as usize],
        }
    }

    pub(crate) fn alive(&self, e: Entity) -> bool {
        e.generation != 0
            && (e.index as usize) < self.generations.len()
            && self.generations[e.index as usize] == e.generation
    }

    pub(crate) fn despawn(&mut self, e: Entity) -> bool {
        if !self.alive(e) {
            return false;
        }
        self.generations[e.index as usize] += 1;
        self.free.push(e.index);
        let pos = self.live_pos[e.index as usize] as usize;
        let last = self.live.len() - 1;
        self.live.swap(pos, last);
        self.live_pos[self.live[pos] as usize] = pos as u32;
        self.live.pop();
        true
    }

    pub(crate) fn entity(&self, index: u32) -> Entity {
        Entity {
            index,
            generation: self.generations[index as usize],
        }
    }

    pub(crate) fn live_len(&self) -> usize {
        self.live.len()
    }

    pub(crate) fn iter_live(&self) -> impl Iterator<Item = Entity> + '_ {
        self.live.iter().map(|&i| self.entity(i))
    }
}
