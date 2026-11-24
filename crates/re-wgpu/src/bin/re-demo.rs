//! re-demo — rendert eine `.wbproj` offscreen zu PNG oder zeigt sie im
//! echten Fenster (`--live`, M2 mit Feature `window`).
//!
//! Headless ist der komplette M1-Pfad: Projekt laden (re-core) → Szene
//! traversieren → `DrawBatch`-Liste → `WgpuBackend` → Readback → PNG.
//!
//! `--live` öffnet ein Fenster (Referenz-Anwendung der M2-Plattform):
//! Maus ziehen = Orbit, Rad = Zoom, WASD/QE = Fliegen, Esc = beenden.
//!
//! Aufruf: `re-demo <szene.wbproj> [out.png] [breite] [hoehe] [--live]`

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use glam::{Mat4, Vec3};
use image::RgbaImage;
use re_core::material::Material;
use re_core::project::Project;
use re_core::scene::NodeKind;
use re_render::{Camera, DrawBatch, GfxBackend};
use re_wgpu::WgpuBackend;

#[cfg(feature = "window")]
mod live;

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let live = argv.iter().any(|a| a == "--live");
    let positional: Vec<String> = argv
        .iter()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect();

    let Some(scene) = positional.first() else {
        bail!(
            "Keine Szene angegeben. Aufruf: re-demo <szene.wbproj> [out.png] \
             [breite] [hoehe] [--live]"
        );
    };
    let scene_path = PathBuf::from(scene);
    let out_path = PathBuf::from(
        positional
            .get(1)
            .cloned()
            .unwrap_or_else(|| "re_render.png".to_string()),
    );
    let width: u32 = positional.get(2).and_then(|s| s.parse().ok()).unwrap_or(1280);
    let height: u32 = positional.get(3).and_then(|s| s.parse().ok()).unwrap_or(720);

    let project = Project::load(&scene_path)
        .with_context(|| format!("Szene laden: {}", scene_path.display()))?;
    // Demo-Prozess: Projekt und Fallback-Material bleiben bis zum Ende
    // bestehen — als 'static abgelegt ersparen sie der Live-App
    // selbstreferenzielle Strukturen (siehe gather()).
    let project: &'static Project = Box::leak(Box::new(project));
    let fallback: &'static Material = Box::leak(Box::new(Material::default()));
    let (scene_batches, (mn, mx)) = gather(project, fallback);
    if scene_batches.is_empty() {
        bail!("Szene enthält keine renderbaren Instanzen: {}", scene_path.display());
    }
    let center = (mn + mx) * 0.5;
    let radius = (mx - mn).length().max(1.0) * 0.5;

    if live {
        #[cfg(feature = "window")]
        return live::run(project).context("Live-Fenster");
        #[cfg(not(feature = "window"))]
        {
            let _ = (project, width, height, out_path, center, radius);
            bail!("--live braucht Feature `window` (Standard); dieser Build wurde ohne gebaut");
        }
    }

    // Headless: Kamera wie im M1 (45°-Diagonale), ein Frame, Readback, PNG.
    let eye = center + Vec3::new(0.8, 0.85, 0.8).normalize() * radius * 2.3;
    let mut camera = Camera::perspective(
        eye,
        center,
        Vec3::Y,
        45f32.to_radians(),
        width as f32 / height as f32,
    );
    camera.clear_color = [0.52, 0.68, 0.85, 1.0];

    let mut backend = WgpuBackend::new();
    backend.init()?;
    backend.resize(width, height);
    backend.render(&camera, &scene_batches)?;
    let (w, h, rgba) = backend.readback_rgba()?;

    let img = RgbaImage::from_raw(w, h, rgba).context("Readback-Puffer unplausibel")?;
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    img.save(&out_path)
        .with_context(|| format!("PNG schreiben: {}", out_path.display()))?;

    println!(
        "renderer={} frames={} batches={} textures={} {}x{} -> {}",
        backend.name(),
        backend.rendered_frames(),
        scene_batches.len(),
        backend.textures_loaded(),
        w,
        h,
        out_path.display()
    );
    Ok(())
}

/// Szene → Batches; zusätzlich Welt-AABB für die Kameraeinstellung.
///
/// `project` muss 'static sein: die Demo leakt das Projekt (`Box::leak`) —
/// der Prozess endet ohnehin, und so vermeidet die Live-App
/// selbstreferenzielle Strukturen, weil ihre Batches das Projekt borrowen.
pub fn gather(
    project: &'static Project,
    fallback: &'static Material,
) -> (Vec<DrawBatch<'static>>, (Vec3, Vec3)) {
    let lib = &project.library;
    let mut batches: Vec<DrawBatch<'static>> = Vec::new();
    let mut mn = Vec3::splat(f32::MAX);
    let mut mx = Vec3::splat(f32::MIN);

    let mut push = |mesh: &'static re_core::mesh::Mesh, material: &'static Material, world: Mat4| {
        batches.push(DrawBatch { mesh, material, world, sprite: None });
        for p in &mesh.positions {
            let q = world.transform_point3(Vec3::from_array(*p));
            mn = mn.min(q);
            mx = mx.max(q);
        }
    };

    project.scene.walk(|id, node| {
        if !node.visible {
            return;
        }
        let world = project.scene.world_transform(id).to_mat4();
        match &node.kind {
            NodeKind::MeshInstance { mesh, material, .. } => {
                if let Some(m) = lib.meshes.get(mesh) {
                    let mat = material
                        .as_ref()
                        .and_then(|mid| lib.materials.get(mid))
                        .unwrap_or(fallback);
                    push(m, mat, world);
                }
            }
            NodeKind::TileMapInstance { tilemap } => {
                let Some(tm) = lib.tilemaps.get(tilemap) else { return };
                let Some(ts) = lib.tilesets.get(&tm.tileset) else { return };
                for cell in &tm.cells {
                    let Some(tile) = ts.tiles.get(cell.tile as usize) else { continue };
                    let Some(m) = lib.meshes.get(&tile.mesh) else { continue };
                    let mat = tile
                        .material
                        .as_ref()
                        .and_then(|mid| lib.materials.get(mid))
                        .unwrap_or(fallback);
                    let cell_world =
                        world * ts.cell_transform(cell.x, cell.z).to_mat4() * tile.offset.to_mat4();
                    push(m, mat, cell_world);
                }
            }
            _ => {}
        }
    });

    if mn.x > mx.x {
        (batches, (Vec3::ZERO, Vec3::ONE))
    } else {
        (batches, (mn, mx))
    }
}
