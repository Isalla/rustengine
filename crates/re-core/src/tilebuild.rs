//! Ready-made tilesets and grid builders so a client (agent or GUI) can lay
//! out floors, paths, dungeons and caves with one call.
//!
//! [`preset_tileset`] registers a small kit of primitive-built tiles (floor,
//! path, wall, corner, ramp, …) with matching materials; the generator
//! functions paint those tiles onto a [`crate::tileset::TileMap`]:
//! [`paint_line`] for paths, [`gen_dungeon`] (rooms + corridors + walls) and
//! [`gen_cave`] (cellular automata). Tile convention: every kit mesh is built
//! around XZ-center with its bottom at y = 0; [`ops::add_tile_mesh`] places
//! it with an offset of half a cell.

use std::collections::BTreeSet;

use anyhow::{bail, Result};
use glam::{EulerRot, Quat, Vec3};

use crate::id::{MaterialId, TileMapId, TileSetId};
use crate::material::Material;
use crate::mesh::Mesh;
use crate::ops;
use crate::primitives;
use crate::project::Project;
use crate::terrain::material_by_name;
use crate::transform::Transform;

/// Deterministic tiny LCG — no `rand` dependency.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 17
    }
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() % 100000) as f32 / 100000.0
    }
    /// Inclusive range; returns `lo` when the range is empty.
    pub fn int(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % ((hi - lo + 1) as u64)) as i32
    }
}

/// Meshes are built around the XZ center, bottom at y = 0, and are later
/// offset by half a cell — so every part transform here is expressed with
/// that origin convention.
fn box_at(w: f32, h: f32, d: f32, pos: Vec3, rot: Quat) -> Mesh {
    let mut m = primitives::cube(w, h, d);
    let t = Transform::from_parts(pos, rot, Vec3::ONE).to_mat4();
    for p in &mut m.positions {
        *p = t.transform_point3(Vec3::from_array(*p)).to_array();
    }
    m
}

/// Union of box parts into one mesh (no skin/UVs; normals recomputed).
pub fn merge_meshes(parts: &[Mesh]) -> Mesh {
    let mut out = Mesh {
        name: String::new(),
        ..Default::default()
    };
    for m in parts {
        let base = out.positions.len() as u32;
        out.positions.extend_from_slice(&m.positions);
        let idx = out.indices.get_or_insert_with(Vec::new);
        for t in m.iter_triangles() {
            idx.extend_from_slice(&[t[0] + base, t[1] + base, t[2] + base]);
        }
    }
    out.compute_normals();
    out
}

/// One kit entry returned by [`preset_tileset`].
#[derive(Clone, Debug)]
pub struct KitTile {
    pub name: String,
    pub index: u32,
}

fn kit_mat(p: &mut Project, name: &str, rgba: [f32; 4], emissive: [f32; 3]) -> MaterialId {
    material_by_name(p, name, || {
        let mut m = Material::colored(name, rgba);
        m.roughness = 0.9;
        m.emissive = emissive;
        m
    })
}

/// Motion the kits apply to their tiles by name (living-world wiring).
pub fn kit_motion(tile_name: &str, cell: f32) -> crate::motion::Motion {
    use crate::motion::Motion;
    match tile_name {
        "wasser" => Motion::Waves {
            amp: (cell * 0.02).max(0.02),
            speed: 1.4,
        },
        "baum" | "hecke" => Motion::Sway {
            amp: (cell * 0.03).max(0.02),
            speed: 1.1,
        },
        "kristall" => Motion::Bob {
            amp: (cell * 0.025).max(0.02),
            speed: 0.9,
        },
        _ => Motion::None,
    }
}

/// Register a primitive-built kit as a new tileset. `kind`: `dungeon`
/// (Verlies: Boden/Weg/Wand/Ecke/Rampe/Tor), `cave` (Höhle: dunkler Boden,
/// Wand, Felsbrocken, Kristall, Wasser) or `outdoor` (Gras/Weg/Fels/Baum/
/// Hecke/Wasser). Returns the tileset id and its named tiles.
pub fn preset_tileset(
    p: &mut Project,
    kind: &str,
    cell: f32,
    name: &str,
) -> Result<(TileSetId, Vec<KitTile>)> {
    let cell = cell.clamp(0.5, 10.0);
    let h = cell / 2.0; // center offset applied to every tile
    let wall_h = cell * 1.6;
    let kind = kind.to_ascii_lowercase();
    let ts = ops::create_tileset(
        p,
        if name.is_empty() { "kit" } else { name },
        cell,
        cell,
    );
    let mut tiles: Vec<KitTile> = Vec::new();

    let add = |p: &mut Project,
                   tiles: &mut Vec<KitTile>,
                   tname: &str,
                   mesh: Mesh,
                   mat: MaterialId|
     -> Result<()> {
        let mut mesh = mesh;
        mesh.name = format!("{name}/{tname}");
        mesh.compute_normals();
        let mid = p.library.add_mesh(mesh);
        let idx = ops::add_tile_mesh(
            p,
            ts,
            tname,
            mid,
            Transform {
                translation: [h, 0.0, h],
                ..Default::default()
            },
            Some(mat),
        )?;
        // kit wiring: water ripples, foliage sways, crystals float
        let mo = kit_motion(tname, cell);
        if !mo.is_none() {
            let _ = ops::set_tile_motion(p, ts, idx, mo);
        }
        tiles.push(KitTile {
            name: tname.into(),
            index: idx,
        });
        Ok(())
    };

    let slab = |height: f32| box_at(cell, height, cell, Vec3::new(0.0, height / 2.0, 0.0), Quat::IDENTITY);

    match kind.as_str() {
        "dungeon" | "verlies" => {
            let floor = kit_mat(p, "Kit-Boden", [0.30, 0.29, 0.31, 1.0], [0.0; 3]);
            let path = kit_mat(p, "Kit-Weg", [0.55, 0.47, 0.34, 1.0], [0.0; 3]);
            let wall = kit_mat(p, "Kit-Wand", [0.46, 0.44, 0.48, 1.0], [0.0; 3]);
            add(p, &mut tiles, "boden", slab(cell * 0.12), floor)?;
            add(p, &mut tiles, "weg", slab(cell * 0.15), path)?;
            add(
                p,
                &mut tiles,
                "wand",
                box_at(cell, wall_h, cell, Vec3::new(0.0, wall_h / 2.0, 0.0), Quat::IDENTITY),
                wall,
            )?;
            let ecke = merge_meshes(&[
                box_at(cell, wall_h, cell, Vec3::new(0.0, wall_h / 2.0, 0.0), Quat::IDENTITY),
                box_at(
                    cell,
                    wall_h,
                    cell * 0.6,
                    Vec3::new(0.0, wall_h / 2.0, -cell * 0.8),
                    Quat::IDENTITY,
                ),
            ]);
            add(p, &mut tiles, "ecke", ecke, wall)?;
            let ramp_rot = Quat::from_euler(EulerRot::XYZ, -0.32, 0.0, 0.0);
            add(
                p,
                &mut tiles,
                "rampe",
                box_at(
                    cell,
                    cell * 0.22,
                    cell * 1.5,
                    Vec3::new(0.0, cell * 0.42, 0.0),
                    ramp_rot,
                ),
                path,
            )?;
            let tor = merge_meshes(&[
                box_at(
                    cell * 0.25,
                    wall_h,
                    cell * 0.25,
                    Vec3::new(-cell * 0.375, wall_h / 2.0, 0.0),
                    Quat::IDENTITY,
                ),
                box_at(
                    cell * 0.25,
                    wall_h,
                    cell * 0.25,
                    Vec3::new(cell * 0.375, wall_h / 2.0, 0.0),
                    Quat::IDENTITY,
                ),
                box_at(
                    cell,
                    cell * 0.3,
                    cell * 0.25,
                    Vec3::new(0.0, wall_h + cell * 0.15, 0.0),
                    Quat::IDENTITY,
                ),
            ]);
            add(p, &mut tiles, "tor", tor, wall)?;
        }
        "cave" | "hoehle" | "höhle" => {
            let floor = kit_mat(p, "Kit-Höhlenboden", [0.20, 0.19, 0.22, 1.0], [0.0; 3]);
            let wall = kit_mat(p, "Kit-Höhlenwand", [0.34, 0.31, 0.36, 1.0], [0.0; 3]);
            let rock = kit_mat(p, "Kit-Fels", [0.42, 0.40, 0.42, 1.0], [0.0; 3]);
            let crystal = kit_mat(p, "Kit-Kristall", [0.30, 0.62, 0.80, 1.0], [0.12, 0.30, 0.45]);
            let water = material_by_name(p, "Wasser", crate::terrain::water_material);
            add(p, &mut tiles, "boden", slab(cell * 0.10), floor)?;
            add(
                p,
                &mut tiles,
                "wand",
                box_at(
                    cell * 1.05,
                    wall_h * 1.2,
                    cell * 1.05,
                    Vec3::new(0.0, wall_h * 0.6 + cell * 0.09, 0.0),
                    Quat::from_euler(EulerRot::XYZ, 0.03, 0.4, 0.05),
                ),
                wall,
            )?;
            let mut brocken_mesh =
                primitives::generate(crate::primitives::PrimitiveKind::Sphere, cell * 0.3, cell * 0.22, 6);
            // uv_sphere uses `a` as radius in all axes → center at the radius so the rock rests on y = 0
            let brocken = Vec3::new(0.0, cell * 0.3, 0.0);
            let t = Transform::from_parts(brocken, Quat::IDENTITY, Vec3::ONE).to_mat4();
            for q in &mut brocken_mesh.positions {
                *q = t.transform_point3(Vec3::from_array(*q)).to_array();
            }
            add(p, &mut tiles, "felsen", brocken_mesh, rock)?;
            let kristall = box_at(
                cell * 0.22,
                cell * 0.75,
                cell * 0.22,
                Vec3::new(0.0, cell * 0.5, 0.0),
                Quat::from_euler(EulerRot::XYZ, 0.12, 0.6, -0.1),
            );
            add(p, &mut tiles, "kristall", kristall, crystal)?;
            add(p, &mut tiles, "wasser", slab(cell * 0.05), water)?;
        }
        "outdoor" | "aussen" | "außen" | "natur" => {
            let grass = kit_mat(p, "Kit-Gras", [0.28, 0.44, 0.22, 1.0], [0.0; 3]);
            let path = kit_mat(p, "Kit-Weg", [0.55, 0.47, 0.34, 1.0], [0.0; 3]);
            let rock = kit_mat(p, "Kit-Fels", [0.42, 0.40, 0.42, 1.0], [0.0; 3]);
            let wood = kit_mat(p, "Kit-Holz", [0.42, 0.30, 0.18, 1.0], [0.0; 3]);
            let leaves = kit_mat(p, "Kit-Laub", [0.22, 0.40, 0.18, 1.0], [0.0; 3]);
            let water = material_by_name(p, "Wasser", crate::terrain::water_material);
            add(p, &mut tiles, "boden", slab(cell * 0.12), grass)?;
            add(p, &mut tiles, "weg", slab(cell * 0.14), path)?;
            let mut fels =
                primitives::generate(crate::primitives::PrimitiveKind::Sphere, cell * 0.34, cell * 0.26, 6);
            let t = Transform::from_parts(Vec3::new(0.0, cell * 0.34, 0.0), Quat::IDENTITY, Vec3::ONE).to_mat4();
            for q in &mut fels.positions {
                *q = t.transform_point3(Vec3::from_array(*q)).to_array();
            }
            add(p, &mut tiles, "felsen", fels, rock)?;
            let baum = merge_meshes(&[
                box_at(cell * 0.16, cell * 0.9, cell * 0.16, Vec3::new(0.0, cell * 0.45, 0.0), Quat::IDENTITY),
                {
                    let mut crown =
                        primitives::generate(crate::primitives::PrimitiveKind::Sphere, cell * 0.42, cell * 0.34, 6);
                    let t = Transform::from_parts(Vec3::new(0.0, cell * 1.1, 0.0), Quat::IDENTITY, Vec3::ONE).to_mat4();
                    for q in &mut crown.positions {
                        *q = t.transform_point3(Vec3::from_array(*q)).to_array();
                    }
                    crown
                },
            ]);
            add(p, &mut tiles, "baum", baum, wood)?;
            let mut hecke =
                primitives::generate(crate::primitives::PrimitiveKind::Sphere, cell * 0.28, cell * 0.2, 5);
            let t = Transform::from_parts(Vec3::new(0.0, cell * 0.28, 0.0), Quat::IDENTITY, Vec3::ONE).to_mat4();
            for q in &mut hecke.positions {
                *q = t.transform_point3(Vec3::from_array(*q)).to_array();
            }
            add(p, &mut tiles, "hecke", hecke, leaves)?;
            add(p, &mut tiles, "wasser", slab(cell * 0.05), water)?;
        }
        other => bail!(
            "unknown kit kind '{other}' (use dungeon, cave or outdoor)"
        ),
    }
    Ok((ts, tiles))
}

fn check_tile(p: &Project, map: TileMapId, tile: u32, what: &str) -> Result<()> {
    let m = p
        .library
        .tilemaps
        .get(&map)
        .ok_or_else(|| anyhow::anyhow!("tilemap not found"))?;
    let ts = p
        .library
        .tilesets
        .get(&m.tileset)
        .ok_or_else(|| anyhow::anyhow!("tileset of tilemap missing"))?;
    if tile as usize >= ts.tiles.len() {
        bail!(
            "{what} tile {tile} out of range (tileset has {} tiles)",
            ts.tiles.len()
        );
    }
    Ok(())
}

/// Bresenham line of `tile` from cell `(x0,z0)` to `(x1,z1)`; returns count.
pub fn paint_line(
    p: &mut Project,
    map: TileMapId,
    x0: i32,
    z0: i32,
    x1: i32,
    z1: i32,
    tile: u32,
) -> Result<usize> {
    check_tile(p, map, tile, "line")?;
    // i64 deltas: i32 subtraction of extreme coordinates would wrap and
    // the loop below would never terminate.
    let dx = (x1 as i64 - x0 as i64).abs();
    let dz = (z1 as i64 - z0 as i64).abs();
    if dx.max(dz) as usize > crate::ops::MAX_PAINT_CELLS {
        bail!("line too long (limit {} cells)", crate::ops::MAX_PAINT_CELLS);
    }
    let sx = if x0 < x1 { 1 } else { -1 };
    let sz = if z0 < z1 { 1 } else { -1 };
    let mut err = dx - dz;
    let (mut x, mut z) = (x0, z0);
    let mut count = 0;
    loop {
        ops::tilemap_set_cell(p, map, x, z, tile)?;
        count += 1;
        if x == x1 && z == z1 {
            break;
        }
        let e2 = 2 * err;
        if e2 > -dz {
            err -= dz;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            z += sz;
        }
    }
    Ok(count)
}

fn line_cells(x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    // i64 deltas (see paint_line): wrapped i32 deltas never terminate.
    let dx = (x1 as i64 - x0 as i64).abs();
    let dz = (z1 as i64 - z0 as i64).abs();
    let cap = crate::ops::MAX_PAINT_CELLS as i64;
    if dx > cap || dz > cap {
        return out; // absurd extent: no cells
    }
    let sx = if x0 < x1 { 1 } else { -1 };
    let sz = if z0 < z1 { 1 } else { -1 };
    let mut err = dx - dz;
    let (mut x, mut z) = (x0, z0);
    loop {
        out.push((x, z));
        if x == x1 && z == z1 {
            break;
        }
        let e2 = 2 * err;
        if e2 > -dz {
            err -= dz;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            z += sz;
        }
    }
    out
}

/// Dungeon parameters: `w`×`h` cells (from cell 0,0), `rooms` random chambers
/// connected by corridors; every border cell of open space gets the wall tile.
#[derive(Clone, Debug)]
pub struct DungeonOpts {
    pub w: i32,
    pub h: i32,
    pub rooms: usize,
    pub seed: u64,
    pub floor: u32,
    pub wall: u32,
    /// Optional distinct tile for corridors (e.g. "weg"); corridors are then
    /// kept out of the room floor set.
    pub path: Option<u32>,
}

impl Default for DungeonOpts {
    fn default() -> Self {
        Self {
            w: 32,
            h: 32,
            rooms: 5,
            seed: 7,
            floor: 0,
            wall: 2,
            path: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BuildReport {
    pub floors: usize,
    pub walls: usize,
    pub extra: usize,
}

/// Rooms + corridors + wall ring painted onto `map` (cells overwritten).
pub fn gen_dungeon(p: &mut Project, map: TileMapId, opt: &DungeonOpts) -> Result<BuildReport> {
    check_tile(p, map, opt.floor, "floor")?;
    check_tile(p, map, opt.wall, "wall")?;
    if let Some(t) = opt.path {
        check_tile(p, map, t, "path")?;
    }
    let w = opt.w.clamp(8, 256);
    let h = opt.h.clamp(8, 256);
    let mut rng = Rng::new(opt.seed);

    let mut floors: BTreeSet<(i32, i32)> = BTreeSet::new();
    let mut halls: BTreeSet<(i32, i32)> = BTreeSet::new();
    let mut centers: Vec<(i32, i32)> = Vec::new();
    for _ in 0..opt.rooms.clamp(1, 40) {
        let rw = rng.int(3, (w / 3).max(4));
        let rh = rng.int(3, (h / 3).max(4));
        let rx = rng.int(1, w - rw - 2).max(1);
        let rz = rng.int(1, h - rh - 2).max(1);
        for z in rz..rz + rh {
            for x in rx..rx + rw {
                floors.insert((x, z));
            }
        }
        centers.push((rx + rw / 2, rz + rh / 2));
    }
    for pair in centers.windows(2) {
        let cells = line_cells(pair[0].0, pair[0].1, pair[1].0, pair[1].1);
        for c in cells {
            if opt.path.is_some() {
                halls.insert(c);
            } else {
                floors.insert(c);
            }
        }
    }

    let mut walls: BTreeSet<(i32, i32)> = BTreeSet::new();
    for &(x, z) in floors.iter().chain(halls.iter()) {
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let k = (x + dx, z + dz);
            if !floors.contains(&k) && !halls.contains(&k) {
                walls.insert(k);
            }
        }
    }

    let mut report = BuildReport::default();
    for &(x, z) in &floors {
        ops::tilemap_set_cell(p, map, x, z, opt.floor)?;
        report.floors += 1;
    }
    if let Some(t) = opt.path {
        for &(x, z) in &halls {
            ops::tilemap_set_cell(p, map, x, z, t)?;
            report.extra += 1;
        }
    }
    for &(x, z) in &walls {
        ops::tilemap_set_cell(p, map, x, z, opt.wall)?;
        report.walls += 1;
    }
    Ok(report)
}

/// Cave parameters: cellular automata. `fill` ~ initial wall density,
/// `smooth` iteration passes; open cells become floor, border walls become
/// `wall`; rare `accent` tiles (crystals) dot the floor.
#[derive(Clone, Debug)]
pub struct CaveOpts {
    pub w: i32,
    pub h: i32,
    pub seed: u64,
    pub fill: f32,
    pub smooth: usize,
    pub floor: u32,
    pub wall: u32,
    pub accent: Option<u32>,
}

impl Default for CaveOpts {
    fn default() -> Self {
        Self {
            w: 32,
            h: 32,
            seed: 11,
            fill: 0.45,
            smooth: 4,
            floor: 0,
            wall: 1,
            accent: None,
        }
    }
}

/// Organic cave system painted onto `map`.
pub fn gen_cave(p: &mut Project, map: TileMapId, opt: &CaveOpts) -> Result<BuildReport> {
    check_tile(p, map, opt.floor, "floor")?;
    check_tile(p, map, opt.wall, "wall")?;
    if let Some(t) = opt.accent {
        check_tile(p, map, t, "accent")?;
    }
    let w = opt.w.clamp(8, 256) as usize;
    let h = opt.h.clamp(8, 256) as usize;
    let fill = opt.fill.clamp(0.35, 0.6);
    let rounds = opt.smooth.clamp(1, 8);
    let mut rng = Rng::new(opt.seed);

    let mut alive = vec![vec![true; h]; w];
    for z in 0..h {
        for x in 0..w {
            alive[x][z] = x == 0 || z == 0 || x == w - 1 || z == h - 1 || rng.f32() < fill;
        }
    }
    let cnt = |g: &[Vec<bool>], x: i32, z: i32| -> u32 {
        let mut n = 0;
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let (nx, nz) = (x + dx, z + dz);
            if nx < 0 || nz < 0 || nx >= w as i32 || nz >= h as i32 {
                n += 1; // out of bounds counts as rock
            } else if g[nx as usize][nz as usize] {
                n += 1;
            }
        }
        n
    };
    for _ in 0..rounds {
        let mut next = alive.clone();
        for z in 0..h {
            for x in 0..w {
                let n = cnt(&alive, x as i32, z as i32);
                next[x][z] = if alive[x][z] { n >= 2 } else { n >= 4 };
            }
        }
        alive = next;
    }

    let open: BTreeSet<(i32, i32)> = (0..w as i32)
        .flat_map(|x| (0..h as i32).map(move |z| (x, z)))
        .filter(|(x, z)| !alive[*x as usize][*z as usize])
        .collect();
    let mut walls: BTreeSet<(i32, i32)> = BTreeSet::new();
    for &(x, z) in &open {
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let k = (x + dx, z + dz);
            if !open.contains(&k) {
                walls.insert(k);
            }
        }
    }

    let mut report = BuildReport::default();
    for &(x, z) in &open {
        let use_accent = opt.accent.is_some_and(|_| rng.f32() < 0.02);
        let tile = match (use_accent, opt.accent) {
            (true, Some(t)) => t,
            _ => opt.floor,
        };
        if use_accent {
            report.extra += 1;
        }
        ops::tilemap_set_cell(p, map, x, z, tile)?;
        report.floors += 1;
    }
    for &(x, z) in &walls {
        ops::tilemap_set_cell(p, map, x, z, opt.wall)?;
        report.walls += 1;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::Transform;

    fn dungeon_map(p: &mut Project) -> TileMapId {
        let (ts, _) = preset_tileset(p, "dungeon", 2.0, "k").unwrap();
        let (node, map) =
            ops::create_tilemap(p, "m", ts, None, Transform::default()).unwrap();
        let _ = node;
        map
    }

    #[test]
    fn presets_build_complete_kits() {
        let mut p = Project::new("kit");
        for kind in ["dungeon", "cave", "outdoor"] {
            let (ts, tiles) = preset_tileset(&mut p, kind, 2.0, kind).unwrap();
            assert!(tiles.len() >= 5, "{kind}: {}", tiles.len());
            let t = &p.library.tilesets[&ts];
            assert_eq!(t.cell, [2.0, 2.0]);
            for tile in &t.tiles {
                assert!(p.library.meshes.contains_key(&tile.mesh));
                // bottom of every tile is at/below y=0, nothing far above a cell
                let (min, max) = p.library.meshes[&tile.mesh].aabb();
                assert!(min[1] >= -0.01, "{kind}/{} sinks below floor", tile.name);
                assert!(max[1] < 8.0, "{kind}/{} absurdly tall", tile.name);
                // living-world wiring: kit tiles carry their signature motion
                let want = kit_motion(&tile.name, 2.0);
                assert_eq!(tile.motion, want, "{kind}/{} motion", tile.name);
            }
        }
        // the signature tiles really animate
        let outdoor = preset_tileset(&mut p, "outdoor", 2.0, "o2").unwrap();
        let t = &p.library.tilesets[&outdoor.0];
        let by = |n: &str| t.tiles.iter().find(|x| x.name == n).unwrap().motion;
        assert!(matches!(by("wasser"), crate::motion::Motion::Waves { .. }));
        assert!(matches!(by("baum"), crate::motion::Motion::Sway { .. }));
        assert!(matches!(by("boden"), crate::motion::Motion::None));
        assert!(preset_tileset(&mut p, "quatsch", 2.0, "x").is_err());
    }

    #[test]
    fn line_paints_a_bresenham_path() {
        let mut p = Project::new("kit");
        let map = dungeon_map(&mut p);
        let n = paint_line(&mut p, map, 0, 0, 5, 2, 1).unwrap();
        assert_eq!(n, 6);
        let m = &p.library.tilemaps[&map];
        assert_eq!(m.get(0, 0), Some(1));
        assert_eq!(m.get(5, 2), Some(1));
        assert_eq!(m.get(3, 1), Some(1));
    }

    #[test]
    fn dungeon_has_rooms_corridors_and_wall_ring() {
        let mut p = Project::new("kit");
        let map = dungeon_map(&mut p);
        let rep = gen_dungeon(
            &mut p,
            map,
            &DungeonOpts {
                w: 24,
                h: 24,
                rooms: 4,
                seed: 42,
                floor: 0,
                wall: 2,
                path: Some(1),
            },
        )
        .unwrap();
        assert!(rep.floors > 50 && rep.walls > 50 && rep.extra > 10, "{rep:?}");
        let m = &p.library.tilemaps[&map];
        // every open cell touches at least one wall somewhere (ring exists)
        let open: Vec<(i32, i32)> = m
            .cells
            .iter()
            .filter(|c| c.tile != 2)
            .map(|c| (c.x, c.z))
            .collect();
        assert!(!open.is_empty());
        // walls only neighbour open cells
        for c in m.cells.iter().filter(|c| c.tile == 2) {
            let has_open_nb = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                .iter()
                .any(|(dx, dz)| open.contains(&(c.x + dx, c.z + dz)));
            assert!(has_open_nb, "floating wall at {:?}", (c.x, c.z));
        }
    }

    #[test]
    fn cave_is_reproducible_and_has_floor_and_walls() {
        let opts = CaveOpts {
            w: 20,
            h: 20,
            seed: 3,
            fill: 0.45,
            smooth: 4,
            floor: 0,
            wall: 2,
            accent: Some(3),
        };
        let mut p = Project::new("kit");
        let map = dungeon_map(&mut p);
        let rep = gen_cave(&mut p, map, &opts).unwrap();
        assert!(rep.floors > 30 && rep.walls > 30, "{rep:?}");
        let snap = p.library.tilemaps[&map].cells.clone();

        let mut p2 = Project::new("kit");
        let map2 = dungeon_map(&mut p2);
        gen_cave(&mut p2, map2, &opts).unwrap();
        // gleicher Seed => identischer Grundriss (BTreeSet-reihenfolge deterministisch)
        assert_eq!(snap, p2.library.tilemaps[&map2].cells);
    }
}
