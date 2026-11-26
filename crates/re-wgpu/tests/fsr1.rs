//! Beweis für M5 Box 3 (Upscaling, Pi-Pfad M13): die GPU-Kette
//! EASU + RCAS in `deferred.wgsl` liefert dieselben Display-Pixel wie
//! die CPU-Referenz `re_render::fsr` (Port von AMD ffx_fsr1.h) über
//! denselben tone-gemappten Internal-Frame — und FSR1 schlägt beim
//! 2×-Hochskalieren die naive bilineare Skalierung gegen den nativ
//! gerenderten Referenzframe (Qualitätstor: näher am Native-Bild).
//! Läuft headless auf dem Pi und in der CI (Software-Adapter llvmpipe).

use re_core::mesh::Mesh;
use re_render::{fsr1, Camera, GfxBackend, ToneCurve};
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

/// 0.6-Wuerfel mit harten Kanten (Face-Normalen) — wie im Deferred-Test.
fn cube() -> Mesh {
    let c = 0.3f32;
    let mut pos: Vec<[f32; 3]> = Vec::with_capacity(24);
    let mut nrm: Vec<[f32; 3]> = Vec::with_capacity(24);
    let mut idx: Vec<u32> = Vec::with_capacity(36);
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

/// Toleranz: ein Rasterpunkt (8-Bit) plus Rundungspuffer — dieselbe
/// Latte wie im Deferred-Test (FMA-Kontraktion auf der GPU kostet
/// Rechenpfad-Ulps, die 8-Bit-Quantisierung schluckt).
const TOL: f32 = 1.5 / 255.0;

/// RGBA8-Bytes -> display-kodierte RGB-Werte (der FSR1-Eingabevertrag).
fn rgb(bytes: &[u8]) -> Vec<[f32; 3]> {
    bytes
        .chunks_exact(4)
        .map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0])
        .collect()
}

/// Mittlere Absolutdifferenz (MAE) zweier RGBA8-Bilder über RGB.
fn mae(a: &[u8], b: &[u8]) -> f32 {
    assert_eq!(a.len(), b.len());
    let mut sum = 0.0f64;
    let mut n = 0u64;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for k in 0..3 {
            sum += (pa[k] as f64 - pb[k] as f64).abs();
            n += 1;
        }
    }
    (sum / n as f64) as f32
}

/// Bilineares Hochskalieren von RGBA8-Bytes (sw×sh -> dw×dh) mit
/// Halbpixel-Zentren — der naive Vergleichspfad fuer das Qualitaetstor.
fn bilinear(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let at = |x: u32, y: u32, k: usize| -> f32 {
        src[((y.min(sh - 1) * sw + x.min(sw - 1)) * 4 + k as u32) as usize] as f32
    };
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let mut out = Vec::with_capacity((dw * dh * 4) as usize);
    for y in 0..dh {
        for x in 0..dw {
            let u = (x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5;
            let v = (y as f32 + 0.5) * sh as f32 / dh as f32 - 0.5;
            let (x0, y0) = (u.floor().max(0.0) as u32, v.floor().max(0.0) as u32);
            let (fx, fy) = (u - x0 as f32, v - y0 as f32);
            for k in 0..4 {
                let a = lerp(at(x0, y0, k), at(x0 + 1, y0, k), fx);
                let b = lerp(at(x0, y0 + 1, k), at(x0 + 1, y0 + 1, k), fx);
                out.push(lerp(a, b, fy).round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    out
}

#[test]
fn fsr1_gpu_kette_matcht_cpu_referenz_und_schlaegt_bilinear() {
    let mut be = WgpuBackend::new();
    if be.init().is_err() {
        eprintln!("kein Adapter — Test uebersprungen");
        return;
    }
    // Display 64×64, intern 32×32 (scale 2) — Pi-Formel in Kleinformat.
    be.resize(64, 64);
    let cam = Camera::perspective(
        glam::Vec3::new(0.0, 0.0, 2.0),
        glam::Vec3::ZERO,
        glam::Vec3::Y,
        std::f32::consts::FRAC_PI_3,
        1.0,
    );
    let bg = quad([0.0, 0.0, 1.0], 30.0);
    let cb = cube();
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
    let upscale = DeferredUpscale { scale: 2, sharpness_stops: 2.0 };

    // ── (1) Tor: GPU-Display == CPU easu+rcas ueber demselben Internal-Frame.
    let batches_up = [
        DeferredBatch {
            mesh: &bg,
            world: glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, -20.0)),
            albedo: [0.2, 0.6, 0.4],
            metallic: 0.0,
            roughness: 0.7,
            material_id: 9,
        },
        DeferredBatch {
            mesh: &cb,
            world: glam::Mat4::IDENTITY,
            albedo: [0.5, 0.5, 0.5],
            metallic: 0.0,
            roughness: 0.37,
            material_id: 173,
        },
    ];
    be.render_deferred(&cam, &batches_up, hdr, &lights, Some(upscale), None)
        .expect("render_deferred mit Upscale");
    let (iw, ih, internal) = be.read_deferred_internal().expect("read internal");
    assert_eq!((iw, ih), (32, 32), "interner Frame muss display/scale sein");
    let (dw, dh, display) = be.read_deferred_rgba().expect("read display");
    assert_eq!((dw, dh), (64, 64), "Display muss die Backend-Groesse sein");

    // CPU-Referenz: exakt dieselben display-kodierten Bytes als Eingabe,
    // inklusive des 8-Bit-Zwischenpuffers nach EASU (fsr1 gespiegelt).
    let src = rgb(&internal);
    let want = fsr1(&src, 32, 32, 64, 64, upscale.sharpness_stops);
    for (i, p) in want.iter().enumerate() {
        for k in 0..3 {
            let got = display[i * 4 + k] as f32 / 255.0;
            assert!(
                (got - p[k]).abs() <= TOL,
                "fsr({},{})[{k}]: gpu {got} vs cpu {}",
                i as u32 % 64,
                i as u32 / 64,
                p[k]
            );
        }
        assert_eq!(display[i * 4 + 3], 255, "Alpha deckend");
    }

    // ── (2) Qualitaetstor: FSR1 naeher am nativen 64×64-Frame als bilinear.
    let mut nat = WgpuBackend::new();
    nat.init().expect("Adapter fuer Native-Frame");
    nat.resize(64, 64);
    let batches_native = [
        DeferredBatch {
            mesh: &bg,
            world: glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, -20.0)),
            albedo: [0.2, 0.6, 0.4],
            metallic: 0.0,
            roughness: 0.7,
            material_id: 9,
        },
        DeferredBatch {
            mesh: &cb,
            world: glam::Mat4::IDENTITY,
            albedo: [0.5, 0.5, 0.5],
            metallic: 0.0,
            roughness: 0.37,
            material_id: 173,
        },
    ];
    nat.render_deferred(&cam, &batches_native, hdr, &lights, None, None)
        .expect("render_deferred nativ");
    let (nw, nh, native) = nat.read_deferred_rgba().expect("read native");
    assert_eq!((nw, nh), (64, 64));

    let bilin = bilinear(&internal, iw, ih, dw, dh);
    let m_fsr = mae(&display, &native);
    let m_bilin = mae(&bilin, &native);
    assert!(
        m_fsr < m_bilin,
        "FSR1 muss naeher am nativen Frame liegen als bilinear: fsr {m_fsr} vs bilin {m_bilin}"
    );

    // ── (3) Sichtbeleg (opt-in): PNG nur, wenn ein Pfad gesetzt ist.
    if let Ok(path) = std::env::var("RE_FSR_PNG") {
        be.export_deferred_png(&path).expect("PNG-Export");
        eprintln!("PNG nach {path}");
    }
}
