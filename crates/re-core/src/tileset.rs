//! 3D tile support: a [`TileSet`] is a reusable modular kit (floor, wall,
//! corner, ramp …); a [`TileMap`] paints cells of one tileset onto an XZ grid.
//!
//! Convention: cell `(x, z)` covers the square from `(x*cell_w, 0, z*cell_d)`
//! to `((x+1)*cell_w, 0, (z+1)*cell_d)`. Tiles are authored however you like;
//! `Tile::offset` is applied on top of the cell transform, so a mesh that was
//! modelled around its own centre aligns by offsetting it half a cell.

use crate::id::{MaterialId, MeshId, TileSetId};
use crate::transform::Transform;
use serde::{Deserialize, Serialize};

/// One variant inside a tileset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    pub name: String,
    pub mesh: MeshId,
    /// Extra transform applied after the cell placement (alignment, rotation …).
    #[serde(default)]
    pub offset: Transform,
    /// Material used by every cell of this tile (falls back to the mesh's own
    /// material handling when `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<MaterialId>,
    /// Procedural motion for every cell of this tile (viewer draw time).
    #[serde(default, skip_serializing_if = "crate::motion::Motion::is_none")]
    pub motion: crate::motion::Motion,
}

/// Reusable collection of tiles sharing a grid metric.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TileSet {
    pub name: String,
    /// Cell size: `[width in X, depth in Z]`, meters.
    pub cell: [f32; 2],
    pub tiles: Vec<Tile>,
}

impl TileSet {
    pub fn new(name: impl Into<String>, cell_w: f32, cell_d: f32) -> Self {
        Self {
            name: name.into(),
            cell: [cell_w, cell_d],
            tiles: Vec::new(),
        }
    }

    /// Transform that moves the origin to the corner of cell `(x, z)`.
    pub fn cell_transform(&self, x: i32, z: i32) -> Transform {
        Transform {
            translation: [
                x as f32 * self.cell[0],
                0.0,
                z as f32 * self.cell[1],
            ],
            ..Transform::default()
        }
    }
}

/// A single painted cell. `tile` indexes [`TileSet::tiles`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileCell {
    pub x: i32,
    pub z: i32,
    pub tile: u32,
}

/// Painted grid referencing a tileset. Later entries win; painting an
/// existing cell overwrites it in place.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TileMap {
    pub name: String,
    pub tileset: TileSetId,
    pub cells: Vec<TileCell>,
}

impl TileMap {
    pub fn new(name: impl Into<String>, tileset: TileSetId) -> Self {
        Self {
            name: name.into(),
            tileset,
            cells: Vec::new(),
        }
    }

    pub fn get(&self, x: i32, z: i32) -> Option<u32> {
        self.cells
            .iter()
            .rev()
            .find(|c| c.x == x && c.z == z)
            .map(|c| c.tile)
    }

    /// Paint (or overwrite) one cell.
    pub fn set(&mut self, x: i32, z: i32, tile: u32) {
        if let Some(c) = self.cells.iter_mut().find(|c| c.x == x && c.z == z) {
            c.tile = tile;
        } else {
            self.cells.push(TileCell { x, z, tile });
        }
    }

    /// Remove one cell; returns whether anything was there.
    pub fn erase(&mut self, x: i32, z: i32) -> bool {
        let n = self.cells.len();
        self.cells.retain(|c| !(c.x == x && c.z == z));
        self.cells.len() != n
    }

    pub fn clear(&mut self) -> usize {
        let n = self.cells.len();
        self.cells.clear();
        n
    }

    /// Inclusive bounds `(min_x, min_z, max_x, max_z)` of painted cells.
    pub fn bounds(&self) -> Option<(i32, i32, i32, i32)> {
        let mut it = self.cells.iter();
        let first = it.next()?;
        let (mut x0, mut z0, mut x1, mut z1) = (first.x, first.z, first.x, first.z);
        for c in it {
            x0 = x0.min(c.x);
            z0 = z0.min(c.z);
            x1 = x1.max(c.x);
            z1 = z1.max(c.z);
        }
        Some((x0, z0, x1, z1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_overwrite_erase() {
        let mut m = TileMap::new("map", TileSetId(0));
        m.set(0, 0, 0);
        m.set(2, 3, 1);
        assert_eq!(m.get(2, 3), Some(1));
        m.set(2, 3, 0); // overwrite in place
        assert_eq!(m.cells.len(), 2);
        assert_eq!(m.get(2, 3), Some(0));
        assert!(m.erase(0, 0));
        assert!(!m.erase(0, 0));
        assert_eq!(m.bounds(), Some((2, 3, 2, 3)));
    }

    #[test]
    fn cell_transform_steps_by_cell_size() {
        let ts = TileSet::new("kit", 2.0, 0.5);
        let t = ts.cell_transform(3, -2);
        assert_eq!(t.translation, [6.0, 0.0, -1.0]);
    }
}
