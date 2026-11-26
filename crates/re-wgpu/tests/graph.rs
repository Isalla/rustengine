//! Beweis für M5 Box 5 (Render-Graph): die Deferred-Kette läuft nicht
//! mehr als Code-Zettel, sondern der Executor folgt dem kompilierten
//! Graphen (`re_render::graph`). Tor 1: die ausgeführte Pass-Folge ist
//! genau der Plan — inklusive der Sortierer-Entscheidung `d-cull` vor
//! `d-geometry` (beide unabhängig, alphabetischer Tie-Break). Tor 2:
//! ohne Upscale fehlen EASU/RCAS im Plan und in der Ausführung. Tor 3:
//! gleiche Deklaration, gleiches Bild — der Plan ist deterministisch
//! und die Pixel bleiben dieselben (die schweren Pixel-Diffs gegen
//! die CPU-Referenz liefern deferred.rs und fsr1.rs).
//! Läuft headless auf dem Pi und in der CI (Software-Adapter llvmpipe).

use re_core::mesh::Mesh;
use re_render::{Camera, GfxBackend, ToneCurve};
use re_wgpu::{DeferredBatch, DeferredHdr, DeferredLight, DeferredUpscale, WgpuBackend};

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

#[test]
fn executor_folgt_dem_kompilierten_graphen() {
    let mut be = WgpuBackend::new();
    if be.init().is_err() {
        eprintln!("kein Adapter — Test uebersprungen");
        return;
    }
    be.resize(64, 64);
    let cam = Camera::perspective(
        glam::Vec3::new(0.0, 0.0, 2.0),
        glam::Vec3::ZERO,
        glam::Vec3::Y,
        std::f32::consts::FRAC_PI_3,
        1.0,
    );
    let bg = quad([0.0, 0.0, 1.0], 30.0);
    let lights = [DeferredLight {
        position: [0.0, 0.0, 1.0],
        range: 1.0,
        color: [1.0, 0.6, 0.3],
        intensity: 8.0,
    }];
    let hdr = DeferredHdr { exposure: 1.0, light_intensity: 5.0, curve: ToneCurve::Aces };
    let upscale = DeferredUpscale { scale: 2, sharpness_stops: 2.0 };
    let batches = [DeferredBatch {
        mesh: &bg,
        world: glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, -20.0)),
        albedo: [0.2, 0.6, 0.4],
        metallic: 0.0,
        roughness: 0.7,
        material_id: 9,
    }];

    // ── Tor 1: mit Upscale ist der Plan die sechsköpfige Kette — und
    // der Executor hat sie genau so ausgeführt (Spur aus dem Frame).
    be.render_deferred(&cam, &batches, hdr, &lights, Some(upscale), None)
        .expect("render_deferred mit Upscale");
    assert_eq!(
        be.deferred_pass_order().expect("Pass-Folge"),
        ["d-cull", "d-geometry", "d-light", "d-tonemap", "d-easu", "d-rcas"],
        "der Executor muss dem kompilierten Graphen folgen"
    );
    // Die Pässe haben wirklich gearbeitet: RCAS hat ein Display-Bild
    // mit Inhalt hingelegt (kein Vollschwarz).
    let (dw, dh, display) = be.read_deferred_rgba().expect("read display");
    assert_eq!((dw, dh), (64, 64));
    assert!(
        display.chunks_exact(4).any(|p| (p[0] | p[1] | p[2]) > 0),
        "der Upscale-Pfad muss ein Bild liefern"
    );

    // ── Tor 2: ohne Upscale endet die Kette beim Tone-Mapping —
    // dieselbe Deklaration, andere Konfiguration, kürzerer Plan.
    be.render_deferred(&cam, &batches, hdr, &lights, None, None).expect("render nativ");
    assert_eq!(
        be.deferred_pass_order().expect("Pass-Folge nativ"),
        ["d-cull", "d-geometry", "d-light", "d-tonemap"],
        "ohne Upscale dürfen EASU/RCAS nicht laufen"
    );

    // ── Tor 3: Determinismus — gleicher Plan, gleiches Bild.
    let (_, _, first) = be.read_deferred_rgba().expect("read nativ 1");
    be.render_deferred(&cam, &batches, hdr, &lights, None, None).expect("render nativ 2");
    assert_eq!(
        be.deferred_pass_order().expect("Pass-Folge nativ 2"),
        ["d-cull", "d-geometry", "d-light", "d-tonemap"]
    );
    let (_, _, second) = be.read_deferred_rgba().expect("read nativ 2");
    assert_eq!(first, second, "zwei Frames nach demselben Plan müssen identisch sein");

    // ── Sichtbeleg (opt-in): PNG nur, wenn ein Pfad gesetzt ist.
    if let Ok(path) = std::env::var("RE_GRAPH_PNG") {
        be.export_deferred_png(&path).expect("PNG-Export");
        eprintln!("PNG nach {path}");
    }
}
