//! Welt-Raster: Zell-Schlüssel und die Achsen-Mathematik.
//!
//! Eine Zelle ist durch ihre Ebene und ihre Gitterkoordinaten auf dieser
//! Ebene eindeutig identifiziert: [`CellKey`]. Die Ebenen stammen aus
//! [`crate::clipmap`]; ihre Zellkanten liegen wegen `size = base · 2^ebene`
//! immer auf dem Raster der feineren Ebenen — eine Zelle ist also stets
//! eine saubere Teilmenge feinerer Zellen.

/// Identität einer Zelle: Ebene (0 = feinste) + Gitterkoordinaten darauf.
///
/// Die Koordinaten beziehen sich auf das Raster EIGENER Ebene, nicht auf
/// das Basisraster — sonst bräuchte jede Ebene eigene Bereiche.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellKey {
    /// Clipmap-Ebene, 0 ist die feinste.
    pub level: u8,
    /// Spalte im Ebenen-Raster (darf negativ sein).
    pub x: i64,
    /// Zeile im Ebenen-Raster.
    pub y: i64,
}

impl CellKey {
    pub fn new(level: u8, x: i64, y: i64) -> Self {
        Self { level, x, y }
    }
}

/// Abrunden nach minus Unendlich — `as i64` würde Richtung 0 schneiden
/// und damit Zellen um den Nullpunkt herum falsch zuordnen.
pub(crate) fn floor_div(v: f64, size: f64) -> i64 {
    (v / size).floor() as i64
}

/// Weltposition → Zell-Koordinate auf einer Ebene gegebener Zellgröße.
pub fn world_to_cell(pos: f64, size: f64) -> i64 {
    floor_div(pos, size)
}

/// Mittelpunkt einer Zelle in Welt-Einheiten.
pub fn cell_center(coord: i64, size: f64) -> f64 {
    (coord as f64 + 0.5) * size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_div_shneidet_richtung_minus_unendlich() {
        assert_eq!(floor_div(0.9, 1.0), 0);
        assert_eq!(floor_div(-0.1, 1.0), -1);
        assert_eq!(floor_div(-1.0, 1.0), -1);
        assert_eq!(floor_div(-1.1, 1.0), -2);
        assert_eq!(floor_div(2.5, 2.0), 1);
        assert_eq!(floor_div(-2.5, 2.0), -2);
    }

    #[test]
    fn zelle_und_mittelpunkt_sind_invers() {
        let size = 4.0;
        for &p in &[-7.3, -4.0, -0.5, 0.0, 0.5, 3.9, 4.0, 12.125] {
            let c = world_to_cell(p, size);
            // Der Mittelpunkt seiner Zelle liegt in der Zelle …
            assert_eq!(world_to_cell(cell_center(c, size), size), c);
            // … und die Zelle deckt p ab.
            assert!(cell_center(c, size) - size / 2.0 <= p && p < cell_center(c, size) + size / 2.0);
        }
    }

    #[test]
    fn zellenschluessel_sind_sortierbar_und_gehash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(CellKey::new(0, -1, 2));
        set.insert(CellKey::new(0, -1, 2));
        set.insert(CellKey::new(1, -1, 2));
        assert_eq!(set.len(), 2);
        assert!(CellKey::new(0, 9, 9) < CellKey::new(1, -99, -99));
    }
}
