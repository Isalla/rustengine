//! Beweis für ADR-0003 Box 1 + HDR-Aufsatz + geclusterte Lichter: der
//! GPU-G-Buffer (MRT, WGSL) liefert dieselben Pixel wie die CPU-
//! Referenz `GBufferLayout::pack()`; der Compute-Cull spiegelt
//! `cull_lights()` Index für Index (Listen-Vergleich über alle
//! Cluster); der Lighting-Pass schreibt ein HDR-Float-Ziel, das Werte
//! über 1.0 erhält (CPU-Vergleich in Float-Präzision); der
//! Tone-Mapping-Pass endet bei denselben Display-Farben wie
//! `HdrPipeline::process()`. Läuft headless auf dem Pi und in der CI
//! (Software-Adapter llvmpipe).

use re_core::mesh::Mesh;
use re_render::{
    cull_lights, Camera, ClusterGrid, GBufferLayout, GfxBackend, HdrPipeline, SphereLight,
    Surface, ToneCurve,
};
use re_wgpu::{
    DeferredBatch, DeferredHdr, DeferredLight, WgpuBackend, CLUSTER_CAP, CLUSTER_SLICES,
    CLUSTER_TILE,
};

fn quad(normal: [f32; 3], half: f32) -> Mesh {
    Mesh {
        positions: vec![
            [-half, -half, 0.0],
            [half, -half, 0.0],
            [half, half, 0.0],
            [-half, half, 0.0],
        ],
        normals: Some(vec![normal; 4]),
        uvs: Some(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
        indices: Some(vec![0, 1, 2, 0, 2, 3]),
        ..Default::default()
    }
}

/// 0.6-Wuerfel um den Ursprung mit harten Kanten: 24 Vertices, pro
/// Face eine konstante Flaechen-Normale. (Ein Wuerfel mit geteilten
/// Eck-Normalen — diagonal (±,±,+)/√3 — interpoliert ueber die Face
/// *richtig* zur naechsten Ecke hin; die Mitte liegt dann auf einem
/// der beiden Dreiecke und kippt von der fehlenden Ecke weg. Das ist
/// korrekte Geometrie, aber nicht die flache Kanal-Referenz. Mit
/// Face-Normalen ist die Interpolation konstant und der Kanal-Diff
/// wird exakt — so baut man harte Kanten ohnehin.)
fn cube() -> Mesh {
    let c = 0.3f32;
    let mut pos: Vec<[f32; 3]> = Vec::with_capacity(24);
    let mut nrm: Vec<[f32; 3]> = Vec::with_capacity(24);
    let mut idx: Vec<u32> = Vec::with_capacity(36);
    // (Normale, 4 Ecken im Uhrzeigersinn von aussen — Cull ist aus,
    // die Reihenfolge nur fuer die Vollstaendigkeit relevant)
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        ([0.0, 0.0, 1.0], [[-c, -c, c], [c, -c, c], [c, c, c], [-c, c, c]]),
        ([0.0, 0.0, -1.0], [[c, -c, -c], [-c, -c, -c], [-c, c, -c], [c, c, -c]]),
        ([1.0, 0.0, 0.0], [[c, -c, c], [c, -c, -c], [c, c, -c], [c, c, c]]),
        ([-1.0, 0.0, 0.0], [[-c, -c, -c], [-c, -c, c], [-c, c, c], [-c, c, -c]]),
        ([0.0, 1.0, 0.0], [[-c, c, c], [c, c, c], [c, c, -c], [-c, c, -c]]),
        ([0.0, -1.0, 0.0], [[-c, -c, -c], [c, -c, -c], [c, -c, c], [-c, -c, c]]),
    ];
    for (n, corners) in faces {
        let base = pos.len() as u32;
        pos.extend_from_slice(&corners);
        nrm.extend_from_slice(&[n; 4]);
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh { positions: pos, normals: Some(nrm), uvs: None, indices: Some(idx), ..Default::default() }
}

/// Toleranz: ein Rasterpunkt (8-Bit) plus Rundungspuffer.
const TOL: f32 = 1.5 / 255.0;

#[test]
fn gpu_gbuffer_und_lighting_matchen_cpu_referenz() {
    let mut be = WgpuBackend::new();
    if be.init().is_err() {
        eprintln!("kein Adapter — Test uebersprungen");
        return;
    }
    be.resize(64, 64);
    // Perspektivkamera: die Cluster-Mathematik (cluster_view_aabb,
    // slice_of) ist perspektivisch definiert — Ortho-Cluster gaebe es
    // nicht. Der Wuerfel (0.6-Kante) fuellt die Mitte, die Flaeche in
    // 22 Einheiten Entfernung deckt den Rest des Frustums. Die
    // Proben-Pixel liegen bewusst in verschiedenen Kacheln UND
    // Tiefenscheiben: Wuerfel (Sichttiefe 1.7) in Scheibe 1, Flaeche
    // (22.0) in Scheibe 2 — eine falsche Scheibenwahl des Shaders
    // aendert die Licht-Liste und damit das Pixel.
    let cam = Camera::perspective(
        glam::Vec3::new(0.0, 0.0, 2.0),
        glam::Vec3::ZERO,
        glam::Vec3::Y,
        std::f32::consts::FRAC_PI_3,
        1.0,
    );

    let bg = quad([0.0, 0.0, 1.0], 30.0);
    let cb = cube();
    let s_quad = Surface {
        albedo: [0.2, 0.6, 0.4],
        metallic: 0.0,
        roughness: 0.7,
        normal: glam::Vec3::new(0.0, 0.0, 1.0),
        material_id: 9,
        velocity: [0.0, 0.0],
    };
    let s_cube = Surface {
        albedo: [0.5, 0.5, 0.5],
        metallic: 0.0,
        roughness: 0.37,
        normal: glam::Vec3::new(0.0, 0.0, 1.0),
        material_id: 173,
        velocity: [0.0, 0.0],
    };
    let batches = [
        DeferredBatch {
            mesh: &bg,
            world: glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, -20.0)),
            albedo: s_quad.albedo,
            metallic: s_quad.metallic,
            roughness: s_quad.roughness,
            material_id: s_quad.material_id,
        },
        DeferredBatch {
            mesh: &cb,
            world: glam::Mat4::IDENTITY,
            albedo: s_cube.albedo,
            metallic: s_cube.metallic,
            roughness: s_cube.roughness,
            material_id: s_cube.material_id,
        },
    ];

    // Zwei Punktlichter, so gesetzt, dass jeder Proben-Pixel genau
    // eine andere Licht-Liste sieht: A erreicht nur den Wuerfel
    // (Scheibe 1), B nur die Flaeche links oben (Kachel (0,0),
    // Scheibe 2), die Ecke rechts unten sieht keines.
    let lights = [
        DeferredLight {
            position: [0.0, 0.0, 1.0],
            range: 1.0,
            color: [1.0, 0.6, 0.3],
            intensity: 8.0,
        },
        DeferredLight {
            position: [-10.9, 10.9, -19.5],
            range: 1.0,
            color: [0.3, 0.5, 1.0],
            intensity: 8.0,
        },
    ];

    let hdr = DeferredHdr { exposure: 1.0, light_intensity: 5.0, curve: ToneCurve::Aces };
    be.render_deferred(&cam, &batches, hdr, &lights, None, None).expect("render_deferred");
    let gb = be.read_gbuffer().expect("read_gbuffer");
    let hdr_rb = be.read_hdr().expect("read_hdr");
    let depth = be.read_deferred_depth().expect("read_deferred_depth");
    let (fw, fh, final_rgba) = be.read_deferred_rgba().expect("readback final");
    assert_eq!((fw, fh), (64, 64));

    let layout = GBufferLayout::deferred_v1();
    let light = glam::Vec3::new(0.45, 0.82, 0.35).normalize();
    let pipe = HdrPipeline { exposure: hdr.exposure, curve: hdr.curve };

    // ── Cluster-Referenz auf der CPU: derselbe Kern, dieselben Werte ──
    let (near, far) = cam.depth_range().expect("Perspektivkamera");
    let grid = ClusterGrid::for_render_size([64, 64], CLUSTER_TILE, CLUSTER_SLICES, near, far);
    let spheres: Vec<SphereLight> =
        lights.iter().map(|l| SphereLight::new(glam::Vec3::from(l.position), l.range)).collect();
    let lists = cull_lights(&grid, &cam.view, &cam.proj, &spheres);

    // Differenzierungs-Beweis (CPU-Seite): die drei Proben-Pixel
    // liegen in verschiedenen Clustern mit verschiedenen Listen.
    let c_mid = grid.locate(32, 32, CLUSTER_TILE, 1.7).expect("Wuerfel-Cluster");
    let c_edge = grid.locate(4, 4, CLUSTER_TILE, 22.0).expect("Flaechen-Cluster");
    let c_corner = grid.locate(60, 60, CLUSTER_TILE, 22.0).expect("Ecken-Cluster");
    assert_ne!(c_mid, c_edge, "Wuerfel und Flaeche liegen in verschiedenen Clustern");
    assert_eq!(lists[c_mid as usize], vec![0], "Mitte sieht nur Licht A");
    assert_eq!(lists[c_edge as usize], vec![1], "Rand sieht nur Licht B");
    assert!(lists[c_corner as usize].is_empty(), "gegenueberliegende Ecke sieht keines");

    // (0) GPU-Culling == CPU-Referenz, Index fuer Index: der
    // Compute-Pass hat cluster_view_aabb + sphere_intersects_aabb
    // exakt gespiegelt — alle Cluster, nicht nur die Proben-Pixel.
    let gpu_lists = be.read_cluster_lights().expect("read_cluster_lights");
    for (c, want) in lists.iter().enumerate() {
        let base = c * CLUSTER_CAP as usize;
        let capped = want.len().min((CLUSTER_CAP - 1) as usize);
        assert_eq!(gpu_lists[base] as usize, capped, "cluster {c}: Anzahl");
        assert_eq!(
            &gpu_lists[base + 1..base + 1 + capped],
            &want[..capped],
            "cluster {c}: Indizes"
        );
    }

    // (1) G-Buffer: GPU-Pixel == CPU-pack(), Kanal fuer Kanal.
    for (label, x, y, s) in [
        ("cube", 32u32, 32u32, &s_cube),
        ("quad", 4u32, 4u32, &s_quad),
    ] {
        let want = layout.pack(s);
        let g0 = gb.t0_f32(x, y);
        let g1 = gb.t1_f32(x, y);
        for k in 0..4 {
            assert!(
                (g0[k] - want[0][k]).abs() <= TOL,
                "{label} t0[{k}]: gpu {g0:?} vs cpu {:?}",
                want[0]
            );
            assert!(
                (g1[k] - want[1][k]).abs() <= TOL,
                "{label} t1[{k}]: gpu {g1:?} vs cpu {:?}",
                want[1]
            );
        }
        let vel = gb.velocity(x, y);
        assert_eq!(vel, [0.0, 0.0], "{label} Velocity muss 0 sein (statisch)");
    }

    // (2) HDR-Ziel + Tone-Mapping: GPU == CPU-Referenz aus denselben
    // G-Buffer-Bytes und derselben Depth. Der Float-Kanal beweist
    // echtes HDR: Werte ueber 1.0 ueberleben das Lighting-Ziel und
    // werden erst vom Tone-Mapper gestaucht — ein 8-Bit-Ziel koennte
    // das nicht. Die Cluster-Summe der CPU trifft nur dann dieselben
    // Werte, wenn der Shader denselben Cluster gewaehlt hat.
    let mut hdr_max = 0.0f32;
    let pa = cam.proj.x_axis.x;
    let pb = cam.proj.y_axis.y;
    for (x, y) in [(32u32, 32u32), (4, 4), (60, 60)] {
        let vel = gb.velocity(x, y);
        let surf = layout.unpack(&[
            gb.t0_f32(x, y),
            gb.t1_f32(x, y),
            [vel[0], vel[1], 0.0, 0.0],
        ]);
        let ndl = surf.normal.dot(light).max(0.0);
        // Gespiegelte Shader-Mathe: Depth -> Sichttiefe -> View-Punkt
        // -> Cluster -> Licht-Summe (Lambert + quadratischer Falloff).
        let z = depth[(y * fw + x) as usize];
        let mut sum = [0.0f32; 3];
        if z < 1.0 {
            let d = near * far / (far - z * (far - near));
            let ndc_x = 2.0 * (x as f32 + 0.5) / fw as f32 - 1.0;
            let ndc_y = 1.0 - 2.0 * (y as f32 + 0.5) / fh as f32;
            let p = glam::Vec3::new(ndc_x * d / pa, ndc_y * d / pb, -d);
            if let Some(sl) = grid.slice_of(d) {
                let tx = (x / CLUSTER_TILE).min(grid.tiles_x - 1);
                let ty = (y / CLUSTER_TILE).min(grid.tiles_y - 1);
                for &li in &lists[grid.index(tx, ty, sl) as usize] {
                    let l = &lights[li as usize];
                    let lv = cam.view.transform_point3(glam::Vec3::from(l.position));
                    let v = lv - p;
                    let dist = v.length();
                    let t = (1.0 - dist / l.range).clamp(0.0, 1.0);
                    let att = t * t;
                    if att > 0.0 {
                        let ndl2 = surf.normal.dot(v / dist.max(1e-7)).max(0.0);
                        for k in 0..3 {
                            sum[k] += l.color[k] * l.intensity * att * ndl2;
                        }
                    }
                }
            }
        }
        let linear: [f32; 3] = std::array::from_fn(|k| {
            surf.albedo[k] * ((0.1 + 0.9 * ndl) + sum[k]) * hdr.light_intensity
        });
        let gpu_hdr = hdr_rb.pixel(x, y);
        for k in 0..3 {
            assert!(
                (gpu_hdr[k] - linear[k]).abs() <= 1e-4,
                "hdr({x},{y})[{k}]: gpu {} vs cpu {}",
                gpu_hdr[k],
                linear[k]
            );
            hdr_max = hdr_max.max(gpu_hdr[k]);
        }
        // Tonemapped + sRGB (der Display-Weg aus hdr.rs, identische Kurve).
        let want = pipe.process(linear);
        let i = ((y * fw + x) * 4) as usize;
        for k in 0..3 {
            let got = final_rgba[i + k] as f32 / 255.0;
            assert!(
                (got - want[k]).abs() <= TOL,
                "final({x},{y})[{k}]: gpu {got} vs cpu {}",
                want[k]
            );
        }
        assert_eq!(final_rgba[i + 3], 255, "Alpha deckend");
    }
    assert!(hdr_max > 1.0, "HDR-Beweis: mindestens ein Kanal muss ueber 1.0 liegen (max {hdr_max})");

    // (3) Tiefen-Beweis: in der Mitte gewinnt der Wuerfel (andere ID).
    assert_eq!(gb.t1_f32(32, 32)[3] * 255.0, 173.0, "Mitte muss den Wuerfel zeigen");
    assert_eq!(gb.t1_f32(4, 4)[3] * 255.0, 9.0, "Rand muss die Flaeche zeigen");

    // (4) Sichtbeleg (opt-in): PNG nur, wenn ein Pfad gesetzt ist —
    // CI-Laeufe schreiben keine Dateien.
    if let Ok(path) = std::env::var("RE_DEFERRED_PNG") {
        be.export_deferred_png(&path).expect("PNG-Export");
        eprintln!("PNG nach {path}");
    }
}
