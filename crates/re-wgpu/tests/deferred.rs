//! Beweis für ADR-0003 Box 1: der GPU-G-Buffer (MRT, WGSL) liefert
//! dieselben Pixel wie die CPU-Referenz `GBufferLayout::pack()`, und
//! der Lighting-Pass dieselben Farben wie `unpack()` + Lambert auf der
//! CPU. Laedt den Software-Adapter (llvmpipe) — laeuft headless auf dem
//! Pi und in der CI.

use re_core::mesh::Mesh;
use re_render::{linear_to_srgb, Camera, GBufferLayout, GfxBackend, Surface};
use re_wgpu::{DeferredBatch, WgpuBackend};

fn quad(normal: [f32; 3]) -> Mesh {
    Mesh {
        positions: vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]],
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
    // Orthografisch: deterministische Pixel-Abdeckung fuer die
    // Tiefen-Proben (Mitte = Wuerfel, Rand = Flaeche). Die Face-
    // Normalen des Wuerfels sind konstant pro Dreieck, damit ist die
    // Interpolation egal — die G-Buffer-Kanale matchen exakt.
    let cam = Camera::orthographic(
        glam::Vec3::new(0.0, 0.0, 2.0),
        glam::Vec3::ZERO,
        glam::Vec3::Y,
        2.0,
        1.0,
    );

    let bg = quad([0.0, 0.0, 1.0]);
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
            world: glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, -1.0)),
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

    be.render_deferred(&cam, &batches).expect("render_deferred");
    let gb = be.read_gbuffer().expect("read_gbuffer");
    let (fw, fh, final_rgba) = be.read_deferred_rgba().expect("readback final");
    assert_eq!((fw, fh), (64, 64));

    let layout = GBufferLayout::deferred_v1();
    let light = glam::Vec3::new(0.45, 0.82, 0.35).normalize();

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

    // (2) Lighting: GPU-Farbe == CPU-Referenz aus denselben Bytes.
    for (x, y) in [(32u32, 32u32), (4, 4), (60, 60)] {
        let vel = gb.velocity(x, y);
        let surf = layout.unpack(&[
            gb.t0_f32(x, y),
            gb.t1_f32(x, y),
            [vel[0], vel[1], 0.0, 0.0],
        ]);
        let ndl = surf.normal.dot(light).max(0.0);
        let i = ((y * fw + x) * 4) as usize;
        for k in 0..3 {
            let want = linear_to_srgb(surf.albedo[k] * (0.1 + 0.9 * ndl));
            let got = final_rgba[i + k] as f32 / 255.0;
            assert!(
                (got - want).abs() <= TOL,
                "final({x},{y})[{k}]: gpu {got} vs cpu {want}"
            );
        }
        assert_eq!(final_rgba[i + 3], 255, "Alpha deckend");
    }

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
