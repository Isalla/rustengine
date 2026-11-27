//! stream-demo — die Welt kommt während der Fahrt ins Bild (M3c).
//!
//! Kein `.wbproj`, keine Szene: Die Welt entsteht aus einer Formel, wird
//! als Zell-Blöcke auf die Platte geschrieben und fliegt dann durch die
//! komplette Streaming-Kette — genau so, wie sie ein echtes Projekt
//! benutzen würde:
//!
//! ```text
//! FileStore ──► AsyncSource ──► Streamer ──► CellHandoff ──► WgpuBackend
//!   (Blöcke)     (Worker)      (Budget)     (Triple-Buffer)  (Zell-Residenz)
//! ```
//!
//! Die Kamera fliegt über das Gelände; Zellen laden im Hintergrund,
//! wandern batchweise per Handoff in die GPU-Residenz und werden
//! zeichenbar, sobald sie retiret sind. Verlassene Zellen fallen aus dem
//! Budget und werden nach der Schonfrist auf der GPU freigegeben.
//!
//! Aufruf: `stream-demo [frames] [out.png] [breite] [hoehe] [zellen-dir]`
//!
//! Ohne GPU-Adapter (Pi ohne Display-Treiber) endet die Demo sauber mit
//! Hinweis statt Fehler — die Pipeline selbst ist in `re-world`
//! headless getestet.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use glam::{Mat4, Quat, Vec2, Vec3};
use image::RgbaImage;
use re_core::material::Material;
use re_core::mesh::Mesh;
use re_render::{Camera, DrawBatch, GfxBackend, GfxError};
use re_wgpu::WgpuBackend;
use re_world::{
    AsyncSource, CellCodec, CellHandoff, CellKey, Clipmap, ClipmapConfig, EgoConfig, EgoHandoff,
    FileStore, GpuUpload, HandoffConfig, HlodCell, HlodConfig, HlodSet, Priority, StreamConfig,
    Streamer,
};

/// Kantenlänge der feinsten Zelle, in Metern.
const BASE_CELL: f64 = 64.0;
/// Höhenraster pro Zellkante (17 Stützstellen = 16 Intervalle).
const SIDE: usize = 17;
/// Bytes eines Höhenfeld-Blocks: SIDE² × f32 little-endian.
const BLOCK_BYTES: usize = SIDE * SIDE * 4;

/// Flugprofil: 200 m/s Richtung +X, sanfte Wellenlinie in Z — schnell
/// genug, dass die Streaming-Kette sichtbar arbeitet.
const SPEED: f64 = 200.0;
/// Seed-Korridor hinter der Kamera (Weltmeter, negative X-Seite).
const SEED_BACK: f64 = 500.0;
/// Seed-Korridor jenseits des Flugziels — deckt die Sichtweite der
/// groben Ebenen ab, damit keine Nachfrage ins Leere läuft.
const SEED_AHEAD: f64 = 3_600.0;
/// Halbe Breite des Seed-Korridors in Z.
const SEED_HALF_Z: f64 = 700.0;

/// Geländehöhe an einer Weltposition — bewusst glatte Wellen, damit
/// Flugpfad und Horizonte auf dem PNG etwas hergeben.
fn height_at(x: f64, z: f64) -> f32 {
    let swells = (x * 0.0031).sin() * (z * 0.0019).cos() * 13.0;
    let dunes = (x * 0.006 + z * 0.0045).sin() * 7.0;
    let ripples = (x * 0.03).cos() * (z * 0.028).sin() * 3.2;
    (swells + dunes + ripples + 12.0) as f32
}

/// Farbband einer Zelle nach mittlerer Höhe (Wasser … Schnee).
fn band_of(avg_height: f32) -> usize {
    match avg_height {
        h if h < 4.0 => 0,  // Wasser
        h if h < 6.5 => 1,  // Ufer
        h if h < 21.0 => 2, // Wiese
        h if h < 27.0 => 3, // Fels
        _ => 4,             // Schnee
    }
}

/// Die HLOD-Zusammenfassung: eine ebene Einheitsebene (1 × 1 m), die pro
/// Cluster auf Clusterkante skaliert wird. Ein echter Backer lieferte
/// hier ein vereinfachtes Terrain-Mesh — für die Demo genügt die Silhouette.
fn hlod_quad() -> Mesh {
    let mut mesh = Mesh::default();
    mesh.name = "hlod_cluster".into();
    mesh.positions = vec![[-0.5, 0.0, -0.5], [0.5, 0.0, -0.5], [0.5, 0.0, 0.5], [-0.5, 0.0, 0.5]];
    mesh.normals = Some(vec![[0.0, 1.0, 0.0]; 4]);
    mesh.indices = Some(vec![0, 1, 2, 0, 2, 3]);
    mesh
}

/// Avatar-Marker für den Ego-Handoff: zwei gekreuzte, hochkante Quadrate —
// von jeder Flugrichtung als heller Strich am Boden sichtbar.
fn avatar_cross() -> Mesh {
    let mut mesh = Mesh::default();
    mesh.name = "avatar".into();
    mesh.positions = vec![
        [-1.2, 0.0, 0.0], [1.2, 0.0, 0.0], [1.2, 3.4, 0.0], [-1.2, 3.4, 0.0],
        [0.0, 0.0, -1.2], [0.0, 0.0, 1.2], [0.0, 3.4, 1.2], [0.0, 3.4, -1.2],
    ];
    mesh.normals = Some(vec![[0.0, 0.0, 1.0]; 8]);
    mesh.indices = Some(vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    mesh
}

const MATERIALS: [Material; 5] = [
    Material {
        name: String::new(),
        base_color: [0.12, 0.30, 0.55, 1.0],
        metallic: 0.0,
        roughness: 0.35,
        emissive: [0.0; 3],
        double_sided: false,
        albedo_texture_id: None,
        normal_texture_id: None,
    },
    Material {
        name: String::new(),
        base_color: [0.76, 0.68, 0.50, 1.0],
        metallic: 0.0,
        roughness: 0.9,
        emissive: [0.0; 3],
        double_sided: false,
        albedo_texture_id: None,
        normal_texture_id: None,
    },
    Material {
        name: String::new(),
        base_color: [0.28, 0.48, 0.22, 1.0],
        metallic: 0.0,
        roughness: 0.95,
        emissive: [0.0; 3],
        double_sided: false,
        albedo_texture_id: None,
        normal_texture_id: None,
    },
    Material {
        name: String::new(),
        base_color: [0.48, 0.44, 0.42, 1.0],
        metallic: 0.0,
        roughness: 0.8,
        emissive: [0.0; 3],
        double_sided: false,
        albedo_texture_id: None,
        normal_texture_id: None,
    },
    Material {
        name: String::new(),
        base_color: [0.92, 0.94, 0.97, 1.0],
        metallic: 0.0,
        roughness: 0.4,
        emissive: [0.0; 3],
        double_sided: false,
        albedo_texture_id: None,
        normal_texture_id: None,
    },
];

/// Eine fertige Zelle: Mesh + Farbband + GPU-Kosten-Schätzung.
struct Chunk {
    mesh: Mesh,
    band: usize,
    bytes: u64,
}

/// Block-Inhalt → Mesh. Der Codec läuft auf den IO-Workern, also ohne
/// jeden GPU-Bezug: aus 17×17 Höhen werden Positionen, Normalen und
/// Indizes (Zell-lokale Koordinaten; die Weltposition kommt per Batch-
/// Transform).
fn decode_chunk(key: CellKey, bytes: &[u8]) -> Result<Chunk, String> {
    if bytes.len() != BLOCK_BYTES {
        return Err(format!("Block hat {} Byte, erwartet {}", bytes.len(), BLOCK_BYTES));
    }
    let size = (BASE_CELL * (1u64 << key.level) as f64) as f32;
    let step = size / (SIDE - 1) as f32;
    let h = |ix: usize, iz: usize| -> f32 {
        let i = iz * SIDE + ix;
        f32::from_le_bytes([bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2], bytes[i * 4 + 3]])
    };

    let mut mesh = Mesh::default();
    mesh.name = format!("l{}_x{}_y{}", key.level, key.x, key.y);
    mesh.positions = Vec::with_capacity(SIDE * SIDE);
    let mut sum = 0.0f32;
    for iz in 0..SIDE {
        for ix in 0..SIDE {
            let y = h(ix, iz);
            sum += y;
            mesh.positions.push([ix as f32 * step, y, iz as f32 * step]);
        }
    }
    // Normalen aus zentralen Differenzen (Rand: einseitige Differenz).
    let mut normals = Vec::with_capacity(SIDE * SIDE);
    for iz in 0..SIDE {
        for ix in 0..SIDE {
            let xl = ix.saturating_sub(1);
            let xr = (ix + 1).min(SIDE - 1);
            let zd = iz.saturating_sub(1);
            let zu = (iz + 1).min(SIDE - 1);
            let dx = (h(xr, iz) - h(xl, iz)) / ((xr - xl) as f32 * step);
            let dz = (h(ix, zu) - h(ix, zd)) / ((zu - zd) as f32 * step);
            let n = glam::Vec3::new(-dx, 1.0, -dz).normalize_or_zero();
            normals.push(n.to_array());
        }
    }
    mesh.normals = Some(normals);

    let mut idx = Vec::with_capacity((SIDE - 1) * (SIDE - 1) * 6);
    for iz in 0..SIDE - 1 {
        for ix in 0..SIDE - 1 {
            let a = (iz * SIDE + ix) as u32;
            let b = a + 1;
            let c = a + SIDE as u32;
            let d = c + 1;
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let bytes_cost =
        (mesh.positions.len() * 12 + mesh.normals.as_ref().map_or(0, |n| n.len() * 12)) as u64
            + (idx.len() * 4) as u64;
    mesh.indices = Some(idx);
    Ok(Chunk { mesh, band: band_of(sum / (SIDE * SIDE) as f32), bytes: bytes_cost })
}

/// Höhe-Blöcke für einen Korridor um den Flugpfad erzeugen — einmalig,
/// synchron, bevor das Streaming startet. Danach liest die Pipeline nur
/// noch von der Platte.
fn seed_blocks(store: &FileStore, clip: &Clipmap, x0: f64, x1: f64, z0: f64, z1: f64) -> usize {
    let mut written = 0usize;
    for level in 0..clip.config().levels {
        let size = clip.config().cell_size(level);
        let gx0 = (x0 / size).floor() as i64;
        let gx1 = (x1 / size).floor() as i64;
        let gz0 = (z0 / size).floor() as i64;
        let gz1 = (z1 / size).floor() as i64;
        for z in gz0..=gz1 {
            for x in gx0..=gx1 {
                let key = CellKey::new(level, x, z);
                let ox = x as f64 * size;
                let oz = z as f64 * size;
                let mut block = Vec::with_capacity(BLOCK_BYTES);
                for iz in 0..SIDE {
                    for ix in 0..SIDE {
                        let h = height_at(ox + ix as f64 * size / (SIDE - 1) as f64, oz + iz as f64 * size / (SIDE - 1) as f64);
                        block.extend_from_slice(&h.to_le_bytes());
                    }
                }
                if store.write_block(key, &block).is_ok() {
                    written += 1;
                }
            }
        }
    }
    written
}

/// Ausführende Seite des Handoffs: legt GPU-Buffer über
/// `upload_cell` an und gibt sie über `release_cell` wieder frei.
/// Pro Zelle kann mehr als eine Übergabe laufen (Neueinlieferung nach
/// Tombstone), darum die Id-Liste.
struct Exec {
    backend: WgpuBackend,
    next_id: u64,
    ids: HashMap<CellKey, Vec<u64>>,
    chunks: HashMap<CellKey, Chunk>,
    /// Wie oft der Handoff eine GPU-Ressource freigegeben hat.
    releases: usize,
}

impl GpuUpload<Chunk> for Exec {
    fn submit(&mut self, _frame: u64, jobs: Vec<(CellKey, Chunk, u64)>) {
        for (cell, chunk, _bytes) in jobs {
            let id = self.next_id;
            self.next_id += 1;
            if self.backend.upload_cell(id, &chunk.mesh).is_err() {
                continue; // Backend ohne Init — Demo-Ende kommt gleich selbst
            }
            self.ids.entry(cell).or_default().push(id);
            self.chunks.insert(cell, chunk);
        }
    }

    fn release(&mut self, cell: CellKey) {
        if let Some(ids) = self.ids.remove(&cell) {
            for id in ids {
                self.backend.release_cell(id);
                self.releases += 1;
            }
        }
        self.chunks.remove(&cell);
    }
}

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let frames: u32 = argv.first().and_then(|s| s.parse().ok()).unwrap_or(240);
    let out_path = PathBuf::from(argv.get(1).cloned().unwrap_or_else(|| "stream_render.png".into()));
    let width: u32 = argv.get(2).and_then(|s| s.parse().ok()).unwrap_or(960);
    let height: u32 = argv.get(3).and_then(|s| s.parse().ok()).unwrap_or(540);
    let cells_dir = PathBuf::from(argv.get(4).cloned().unwrap_or_else(|| "stream_cells".into()));

    // Frische Zellen-Ablage: die Demo ist ein Durchlauf, kein Cache.
    std::fs::remove_dir_all(&cells_dir).ok();
    std::fs::create_dir_all(&cells_dir)
        .with_context(|| format!("Zellen-Ordner: {}", cells_dir.display()))?;

    let clip = Clipmap::new(ClipmapConfig { base_cell: BASE_CELL, radius: 6, levels: 4 });
    let store = FileStore::new(&cells_dir);
    // Der Korridor umschließt die ganze Flugstrecke plus Sichtweite der
    // groben Ebenen — was hier fehlt, wäre eine berechtigte Lücke, aber
    // für die Demo will man failed=0 sehen.
    let fly_end = frames as f64 / 60.0 * SPEED;
    let (seed_x0, seed_x1) = (-SEED_BACK, fly_end + SEED_AHEAD);
    let blocks = seed_blocks(&store, &clip, seed_x0, seed_x1, -SEED_HALF_Z, SEED_HALF_Z);
    println!(
        "Welt erzeugt: {} Blöcke ({} B/Block) in {}/ — Flugstrecke {:.0} m",
        blocks,
        BLOCK_BYTES,
        cells_dir.display(),
        fly_end
    );

    let codec: CellCodec<Chunk> = Arc::new(|key, bytes| decode_chunk(key, bytes));
    let source = AsyncSource::new(FileStore::new(&cells_dir), codec, 4);
    // Budget knapp unter der vollen Clipmap-Last (Rohblöcke à 1156 B):
    // so zeigt der Flug die Verdrängung mitsamt GPU-Freigabe nach
    // Schonfrist, nicht nur den Happy Path.
    let mut streamer = Streamer::new(
        source,
        StreamConfig { budget_bytes: 400 * 1024, max_inflight: 16 },
    );
    let mut handoff = CellHandoff::new(HandoffConfig {
        slots: 3,
        upload_limit_bytes: 4 * 1024 * 1024,
        retire_frames: 2,
    });

    let mut backend = WgpuBackend::new();
    if let Err(e) = backend.init() {
        if matches!(e, GfxError::NoAdapter) {
            println!("Kein GPU-Adapter verfügbar — Demo ohne Fenster übersprungen (Pipeline ist headless getestet).");
            return Ok(());
        }
        return Err(e).context("Backend-init");
    }
    backend.resize(width, height);
    let mut exec = Exec {
        backend,
        next_id: 1,
        ids: HashMap::new(),
        chunks: HashMap::new(),
        releases: 0,
    };
    let mut clip = clip; // set_center braucht &mut

    let dt = 1.0 / 60.0;
    let mut camera = Camera::perspective(
        Vec3::ZERO,
        Vec3::X,
        Vec3::Y,
        60f32.to_radians(),
        width as f32 / height as f32,
    );
    camera.clear_color = [0.52, 0.68, 0.85, 1.0];

    let mut now = 0.0f64;
    let mut loaded_total = 0usize;
    let mut failed_total = 0usize;
    let mut evict_total = 0usize;
    let mut peak_gpu = 0usize;
    // const-Zugriff erzeugt sonst pro Indexing ein Temporäres.
    let materials = &MATERIALS;

    // Nachfrage nur innerhalb des gesäten Korridors — derselbe
    // Flächenindex, den seed_blocks geschrieben hat. Die Config als Kopie,
    // damit die Closure nicht auf dem Clipmap-Kredit hängt.
    let clip_cfg = *clip.config();
    let seeded = move |key: CellKey| -> bool {
        let s = clip_cfg.cell_size(key.level);
        let gx0 = (seed_x0 / s).floor() as i64;
        let gx1 = (seed_x1 / s).floor() as i64;
        let gz0 = (-SEED_HALF_Z / s).floor() as i64;
        let gz1 = (SEED_HALF_Z / s).floor() as i64;
        key.x >= gx0 && key.x <= gx1 && key.y >= gz0 && key.y <= gz1
    };

    // HLOD (M3d): jenseits der Clipmap-Reichweite füllen grobe Cluster
    // die Ferne. Der „Bake“ ist hier ein Maschenrechteck über dem
    // Korridor; übernommen wird, was näher als fade_out am Rand liegt.
    let outer = clip_cfg.cell_size(clip_cfg.levels - 1);
    let reach = clip_cfg.outer_radius();
    let mut hlod = HlodSet::new(HlodConfig {
        cell_size: outer,
        take_over: reach,
        fade_out: reach * 8.0,
        hysteresis: 0.05,
    });
    let base_cells = (outer / clip_cfg.base_cell) as u32;
    let base_cells = base_cells * base_cells * 3; // drei Ebenen grob zusammengefasst
    let gx0 = (seed_x0 / outer).floor() as i64 - 2;
    let gx1 = (seed_x1 / outer).ceil() as i64 + 2;
    let gz0 = (-SEED_HALF_Z * 4.0 / outer).floor() as i64;
    let gz1 = (SEED_HALF_Z * 4.0 / outer).ceil() as i64;
    for gz in gz0..=gz1 {
        for gx in gx0..=gx1 {
            hlod.register(HlodCell { x: gx, y: gz }, base_cells);
        }
    }
    let hlod_quads = hlod_quad();
    let hlod_material = Material {
        name: String::new(),
        base_color: [0.62, 0.66, 0.58, 1.0],
        metallic: 0.0,
        roughness: 0.9,
        emissive: [0.0; 3],
        double_sided: true,
        albedo_texture_id: None,
        normal_texture_id: None,
    };
    let mut hlod_shown_total = 0usize;
    let mut hlod_hidden_total = 0usize;

    // Ego-Übergabe (M3e): Ein Avatar wandert den Korridor entlang; ab
    // ATTACH_FRAME gleitet die Kamera von der Strategieflugbahn an ihn
    // heran und folgt ihm in Augenhöhe. Die Welt (Clipmap, Streamer,
    // HLOD) läuft dabei unverändert weiter — kein Szenen-Neuaufbau.
    const AVATAR_SPEED: f64 = 6.0; // m/s Schritttempo
    let attach_frame = (frames / 3).max(60);
    let mut rig = EgoHandoff::new(EgoConfig { duration: 1.0 }, Vec2::ZERO);
    let mut avatar = Vec2::new(
        (attach_frame as f64 * dt * SPEED - 30.0) as f32,
        (attach_frame as f64 * dt * 0.13).sin() as f32 * 160.0,
    );
    let mut prev_strategy = Vec2::ZERO;
    let mut ego_frames = 0usize;
    let avatar_mesh = avatar_cross();
    let avatar_material = Material {
        name: String::new(),
        base_color: [1.0, 0.62, 0.12, 1.0],
        metallic: 0.0,
        roughness: 0.7,
        emissive: [0.85, 0.42, 0.04],
        double_sided: true,
        albedo_texture_id: None,
        normal_texture_id: None,
    };

    for frame in 0..frames {
        now += dt;
        // Strategie-Flugbahn (Prozedur): SPEED in X, sanfte Welle in Z.
        // Der Rig führt sie als Anker weiter, damit eine spätere Rückgabe
        // den Blick exakt dort fortsetzt, wo die Kamera herkam.
        let sx = now * SPEED;
        let sz = (now * 0.13).sin() * 160.0;
        let strategy = Vec2::new(sx as f32, sz as f32);
        rig.move_anchor(strategy - prev_strategy);
        prev_strategy = strategy;
        // Avatar-Schritt (Spiellogik) und Übergabe zur Hälfte des Flugs.
        avatar += Vec2::new((AVATAR_SPEED * dt) as f32, 0.0);
        rig.set_unit_position(avatar);
        if frame == attach_frame {
            rig.attach();
        }
        rig.update(dt);
        let b = rig.ego_blend();
        if b > 0.999 {
            ego_frames += 1;
        }
        // Kamerapunkt vom Rig (stetige Blende), Höhe weich überblendet:
        // 45 m Übersicht → 1,7 m Augenhöhe.
        let cam2 = rig.camera();
        let s_ground = height_at(sx, sz) + 45.0;
        let e_ground = height_at(cam2.x as f64, cam2.y as f64) + 1.7;
        let eye = Vec3::new(
            cam2.x,
            s_ground as f32 + (e_ground as f32 - s_ground as f32) * b,
            cam2.y,
        );
        let ahead = now + 1.2;
        let ax = ahead * SPEED;
        let az = (ahead * 0.13).sin() * 160.0;
        let s_look = Vec3::new(ax as f32, height_at(ax, az) + 4.0, az as f32);
        let e_look = Vec3::new(
            cam2.x + 30.0,
            height_at((cam2.x + 30.0) as f64, cam2.y as f64) + 1.5,
            cam2.y,
        );
        let look = s_look.lerp(e_look, b);
        camera.view = Mat4::look_at_rh(eye, look, Vec3::Y);

        clip.set_center(cam2);
        if let Some(hd) = hlod.update(cam2) {
            hlod_shown_total += hd.shown.len();
            hlod_hidden_total += hd.hidden.len();
        }
        streamer.set_now(now);
        streamer.sync_demand(
            clip.cells()
                .iter()
                .filter(|a| seeded(a.key))
                .map(|a| (a.key, Priority(a.key.level, a.dist_base))),
        );
        let delta = streamer.drive();
        loaded_total += delta.loaded.len();
        failed_total += delta.failed.len();
        evict_total += delta.evicted.len();
        for (cell, chunk) in delta.loaded {
            let bytes = chunk.bytes;
            handoff.enqueue(cell, chunk, bytes);
        }
        for cell in delta.evicted {
            handoff.evict(cell);
        }

        handoff.tick(&mut exec);

        // Zeichenliste: genau die Zellen, deren Upload retiret ist.
        let mut batches: Vec<DrawBatch> = Vec::new();
        for cell in handoff.resident_cells() {
            let (Some(ids), Some(chunk)) = (exec.ids.get(&cell), exec.chunks.get(&cell)) else {
                continue;
            };
            let Some(&id) = ids.last() else { continue };
            let size = clip.config().cell_size(cell.level) as f32;
            let world = glam::Mat4::from_translation(Vec3::new(cell.x as f32 * size, 0.0, cell.y as f32 * size));
            batches.push(DrawBatch {
                mesh: &chunk.mesh,
                material: &materials[chunk.band],
                world,
                sprite: None,
                cell: Some(id),
            });
        }
        peak_gpu = peak_gpu.max(batches.len());

        // HLOD-Ebene: die sichtbaren Fern-Cluster als flache Zusammenfassung.
        for c in hlod.shown() {
            let ctr = hlod.config().cluster_center(c);
            let h = height_at(ctr.x as f64, ctr.y as f64);
            let world = Mat4::from_scale_rotation_translation(
                Vec3::new(outer as f32, 1.0, outer as f32),
                Quat::IDENTITY,
                Vec3::new(ctr.x, h + 0.6, ctr.y),
            );
            batches.push(DrawBatch {
                mesh: &hlod_quads,
                material: &hlod_material,
                world,
                sprite: None,
                cell: None,
            });
        }
        // Avatar-Marker: heller Kreuzstab, damit die Ego-Kamera im Bild
        // ihre eigene Einheit sieht.
        {
            let a_ground = height_at(avatar.x as f64, avatar.y as f64);
            batches.push(DrawBatch {
                mesh: &avatar_mesh,
                material: &avatar_material,
                world: Mat4::from_translation(Vec3::new(avatar.x, a_ground, avatar.y)),
                sprite: None,
                cell: None,
            });
        }
        exec.backend.render(&camera, &batches)?;
        // Beweisbild mitten in der Blende (75 % Ego): Übergang sichtbar.
        if frame == attach_frame + 45 {
            let (mw, mh, mrgba) = exec.backend.readback_rgba()?;
            let mid = out_path.with_file_name(format!(
                "{}-ego-mitte.png",
                out_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
            ));
            if let Some(parent) = mid.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            if let Some(img) = RgbaImage::from_raw(mw, mh, mrgba) {
                img.save(&mid).ok();
            }
        }
    }

    let stats = handoff.stats();
    println!(
        "Flug beendet: {} Frames, geladen={}, fehlgeschlagen={}, verdrängt={}, \
         GPU-Freigaben={}, GPU-Spitze={} Zellen, zuletzt resident={} ({} KiB VRAM-Gebinde)",
        frames,
        loaded_total,
        failed_total,
        evict_total,
        exec.releases,
        peak_gpu,
        stats.resident,
        stats.resident_bytes / 1024
    );
    println!(
        "HLOD: {} Cluster registriert, {} Übernahmen, {} Rückgaben, {} sichtbar am Ende",
        hlod.len(),
        hlod_shown_total,
        hlod_hidden_total,
        hlod.shown().len()
    );
    let telem = streamer.telemetry();
    println!(
        "IO-Telemetrie: {} Lasten, {} Fehler, Latenz Ø {:.1} ms, \
         Durchsatz Ø {:.1} MiB/s, Gesundheit {:.2}",
        telem.samples(),
        telem.failures(),
        telem.latency_ewma() * 1000.0,
        telem.throughput_ewma() / (1024.0 * 1024.0),
        telem.health(),
    );
    println!(
        "Ego-Handoff: Attach bei Frame {}, Blende 1.0 s, {} Frames in Ego-Höhe \
         (Modus {:?}) — Welt ohne Neuaufbau weitergestreamt",
        attach_frame,
        ego_frames,
        rig.mode(),
    );
    println!(
        "Pipeline am Ende: pending={} filling={} inflight={} — Upload-Kontingent 4 MiB/Frame",
        stats.pending, stats.filling, stats.inflight
    );

    let (w, h, rgba) = exec.backend.readback_rgba()?;
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let img = RgbaImage::from_raw(w, h, rgba).context("Readback-Puffer unplausibel")?;
    img.save(&out_path)
        .with_context(|| format!("PNG schreiben: {}", out_path.display()))?;
    println!("letzter Frame -> {}", out_path.display());
    Ok(())
}
