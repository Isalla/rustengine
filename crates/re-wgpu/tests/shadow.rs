//! Beweis für M5 Shadow 2.0: der GPU-Schattenpass ist die Zug-fuer-Zug-
//! Spiegelung des CPU-Kerns `re_render::shadow`. Tor 1: der `d-shadow`-
//! Knoten steckt im Graphen-Plan und der Executor folgt ihm. Tor 2: die
//! Shadow-Map enthält dieselben Tiefen wie die analytische Projektion
//! des `fit_cascade`-Ergebnisses (Sonden: Würfeldecke, Boden, freie
//! Luft). Tor 3: der Depth-Compare-Sampler liefert pro Bodenpixel
//! denselben Schattenfaktor wie `pcf_box3x3` gegen dieselbe Map — in
//! Kern- und Vollschatten auf Float-Toleranz, im Penumbra-Band mit der
//! einen Tap Toleranz, die Hardware-Mischung vs. CPU-Mischung erlaubt.
//! Tor 4: Kernschatten existiert und ist dunkel, zwei Frames bitidentisch.
//! Läuft headless auf dem Pi und in der CI (Software-Adapter llvmpipe).

use re_core::mesh::Mesh;
use re_render::{
    cascade_for_depth, depth_bias, fit_cascade, pcf_box3x3, project_shadow, Camera, CascadeFit,
    CascadeSetup, GBufferLayout, GfxBackend, ToneCurve,
};
use re_wgpu::{
    DeferredBatch, DeferredHdr, DeferredShadow, HdrReadback, WgpuBackend, LIGHT_DIR,
};

/// Kleine, feste Schatten-Konfiguration: zwei Kaskaden bis 25 Einheiten,
/// 1024er-Map — groß genug für scharfe Kanten, klein genug für llvmpipe.
const SHADOW: DeferredShadow = DeferredShadow {
    map_size: 1024,
    cascades: 2,
    lambda: 0.6,
    range: 25.0,
    bias_constant: 0.0015,
    bias_slope: 0.006,
};

/// 0.6-Würfel um den Ursprung mit harten Face-Normalen (identisch zu
/// tests/deferred.rs — dieselbe Geometrie, dieselbe Kanalkonstanz).
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

/// Horizontaler Boden in der Ebene y = 0 (Welt-Matrix schiebt ihn auf -1).
fn floor() -> Mesh {
    let h = 6.0f32;
    Mesh {
        positions: vec![[-h, 0.0, -h], [h, 0.0, -h], [h, 0.0, h], [-h, 0.0, h]],
        normals: Some(vec![[0.0, 1.0, 0.0]; 4]),
        uvs: None,
        indices: Some(vec![0, 1, 2, 0, 2, 3]),
        ..Default::default()
    }
}

fn hdr_px(rb: &HdrReadback, x: u32, y: u32, k: usize) -> f32 {
    rb.pixel(x, y)[k]
}

/// Sichttiefe (laengs Blick−Z) eines Welt-Punkts — dieselbe Groesse,
/// die der Shader aus der Depth rekonstruiert und der Kern sucht.
fn view_depth(cam: &Camera, p: glam::Vec3) -> f32 {
    -(cam.view.transform_point3(p)).z
}

/// CPU-Schattenfaktor: Kaskade, Projektion, Bias, PCF — alles Kern.
fn cpu_visibility(
    bounds: &[f32],
    fits: &[CascadeFit],
    maps: &[Vec<f32>],
    world: glam::Vec3,
    d: f32,
    normal: glam::Vec3,
    fall: glam::Vec3,
) -> f32 {
    let Some(c) = cascade_for_depth(bounds, d) else { return 1.0 };
    let q = project_shadow(&fits[c], world);
    if q[0] < 0.0 || q[0] > 1.0 || q[1] < 0.0 || q[1] > 1.0 {
        return 1.0; // außerhalb der Map — wie im Shader: kein Eintrag
    }
    let bias = depth_bias(normal, fall, SHADOW.bias_constant, SHADOW.bias_slope);
    pcf_box3x3(&maps[c], SHADOW.map_size as usize, [q[0], q[1]], q[2] - bias)
}

#[test]
fn gpu_schattenpass_mirrored_den_cpu_kern() {
    let mut be = WgpuBackend::new();
    if be.init().is_err() {
        eprintln!("kein Adapter — Test uebersprungen");
        return;
    }
    be.resize(128, 128);
    let cam = Camera::perspective(
        glam::Vec3::new(0.0, 1.5, 5.5),
        glam::Vec3::new(0.0, -0.4, 0.0),
        glam::Vec3::Y,
        std::f32::consts::FRAC_PI_3,
        1.0,
    );
    let floor_mesh = floor();
    let cube_mesh = cube();
    let batches = [
        DeferredBatch {
            mesh: &floor_mesh,
            world: glam::Mat4::from_translation(glam::Vec3::new(0.0, -1.0, 0.0)),
            albedo: [0.8, 0.8, 0.8],
            metallic: 0.0,
            roughness: 0.9,
            material_id: 1,
        },
        DeferredBatch {
            mesh: &cube_mesh,
            world: glam::Mat4::IDENTITY,
            albedo: [0.7, 0.25, 0.2],
            metallic: 0.0,
            roughness: 0.6,
            material_id: 2,
        },
    ];
    let hdr = DeferredHdr { exposure: 1.0, light_intensity: 1.0, curve: ToneCurve::Aces };

    be.render_deferred(&cam, &batches, hdr, &[], None, Some(SHADOW))
        .expect("Deferred-Frame mit Schatten");

    // ── Tor 1: der Schatten-Knoten ist Teil des Plans und lief. ────────
    assert_eq!(
        be.deferred_pass_order().expect("Pass-Folge"),
        ["d-cull", "d-geometry", "d-shadow", "d-light", "d-tonemap"],
        "der d-shadow-Knoten muss im kompilierten Plan laufen"
    );

    // Dieselben Kern-Aufrufe, die render() pro Frame tat.
    let (near, far) = cam.depth_range().expect("Perspektive");
    let setup = CascadeSetup::new(SHADOW.cascades, near, far.min(SHADOW.range), SHADOW.lambda);
    let bounds = setup.boundaries();
    let fall = -glam::Vec3::from(LIGHT_DIR).normalize();
    let fits: Vec<CascadeFit> = setup
        .cascade_ranges()
        .iter()
        .map(|&r| fit_cascade(r, &cam.view, &cam.proj, fall, SHADOW.map_size, true))
        .collect();

    // ── Tor 2: Sonden in der Map — analytische Tiefen des Fits. ────────
    let probe = |map: &[f32], fit: &CascadeFit, p: glam::Vec3| -> (f32, f32) {
        let q = project_shadow(fit, p);
        let s = SHADOW.map_size as usize;
        let tx = (q[0] * s as f32 - 0.5).round().clamp(0.0, s as f32 - 1.0) as usize;
        let ty = (q[1] * s as f32 - 0.5).round().clamp(0.0, s as f32 - 1.0) as usize;
        (q[2], map[ty * s + tx])
    };
    let (size, map0) = be.read_deferred_shadow_map(0).expect("Map Kaskade 0");
    let (_, map1) = be.read_deferred_shadow_map(1).expect("Map Kaskade 1");
    assert_eq!(size, SHADOW.map_size);
    let maps = vec![map0, map1];

    // Wuerfeldecke (0, 0.3, 0): liegt in Kaskade 0, Tiefe = Fit-Wert.
    let top = glam::Vec3::new(0.0, 0.3, 0.0);
    let c_top = cascade_for_depth(&bounds, view_depth(&cam, top)).expect("Decke in Reichweite");
    let (want, got) = probe(&maps[c_top], &fits[c_top], top);
    assert!(got < 1.0, "Decke muss in die Map schreiben (Kaskade {c_top})");
    assert!((want - got).abs() <= 5e-3, "Deckentiefe {got} vs analytisch {want}");
    // Bodenpunkt (2, -1, 1.5): ebenfalls echte Tiefe des Bodens.
    let fp = glam::Vec3::new(2.0, -1.0, 1.5);
    let c_fp = cascade_for_depth(&bounds, view_depth(&cam, fp)).expect("Boden in Reichweite");
    let (want, got) = probe(&maps[c_fp], &fits[c_fp], fp);
    assert!((want - got).abs() <= 5e-3, "Bodentiefe {got} vs analytisch {want}");
    // Freie Luft, deren Lichtstrahl auch den Boden verfehlt (tritt bei
    // x ≈ −7 aus, Boden endet bei −6): nichts gezeichnet -> 1.0.
    let air = glam::Vec3::new(-5.5, 2.0, 0.0);
    let c_air = cascade_for_depth(&bounds, view_depth(&cam, air)).expect("Luft in Reichweite");
    let q_air = project_shadow(&fits[c_air], air);
    assert!(q_air[0] > 0.05 && q_air[0] < 0.95 && q_air[1] > 0.05 && q_air[1] < 0.95,
        "Luftsonde muss innerhalb der Map liegen: {q_air:?}");
    let (_, got) = probe(&maps[c_air], &fits[c_air], air);
    assert_eq!(got, 1.0, "freie Luft darf keinen Tiefeneintrag haben");

    // ── Tor 3 + 4: jeder Bodenpixel gegen den CPU-Kern. ────────────────
    let gb = be.read_gbuffer().expect("read_gbuffer");
    let depth = be.read_deferred_depth().expect("read_depth");
    let hdr_rb = be.read_hdr().expect("read_hdr");
    let layout = GBufferLayout::deferred_v1();
    let (fw, fh) = (hdr_rb.width, hdr_rb.height);
    let ivp = cam.view_proj().inverse();
    let toward = -fall;
    let mut umbra = 0usize;
    let mut lit = 0usize;
    let mut umbra_sum = 0.0f64;
    let mut lit_sum = 0.0f64;
    for y in 0..fh {
        for x in 0..fw {
            let z = depth[(y * fw + x) as usize];
            if z >= 1.0 {
                continue;
            }
            // Welt-Rekonstruktion wie im Shader: Clip -> inv_view_proj.
            let ndc = glam::Vec4::new(
                2.0 * (x as f32 + 0.5) / fw as f32 - 1.0,
                1.0 - 2.0 * (y as f32 + 0.5) / fh as f32,
                z,
                1.0,
            );
            let h = ivp * ndc;
            let world = h.truncate() / h.w;
            if (world.y + 1.0).abs() > 0.02 {
                continue; // nur der Boden ist Empfaenger
            }
            let d = near * far / (far - z * (far - near));
            let vel = gb.velocity(x, y);
            let surf = layout.unpack(&[gb.t0_f32(x, y), gb.t1_f32(x, y), [vel[0], vel[1], 0.0, 0.0]]);
            let normal = surf.normal;
            let vis = cpu_visibility(&bounds, &fits, &maps, world, d, normal, fall);
            let ndl = normal.dot(toward).max(0.0);
            for k in 0..3 {
                let expected = surf.albedo[k] * (0.1 + 0.9 * ndl * vis);
                let got = hdr_px(&hdr_rb, x, y, k);
                // Kernschatten/Volllicht: alle neun Taps stimmen, Hardware
                // und CPU mischen dasselbe Ergebnis. Penumbra: eine Tap
                // Differenz (1/9) ist durch die Mischungsreihenfolge erlaubt.
                let tol = if vis == 0.0 || vis == 1.0 {
                    2e-3
                } else {
                    0.9 * surf.albedo[k] * ndl / 9.0 + 2e-3
                };
                assert!(
                    (got - expected).abs() <= tol,
                    "Bodenpixel ({x},{y}) Kanal {k}: gpu {got} vs Kern {expected} (vis {vis})"
                );
            }
            let lum = hdr_px(&hdr_rb, x, y, 0) as f64;
            if vis == 0.0 {
                umbra += 1;
                umbra_sum += lum;
            } else if vis == 1.0 {
                lit += 1;
                lit_sum += lum;
            }
        }
    }
    assert!(umbra >= 50, "Kernschatten fehlt: {umbra} Pixel");
    assert!(lit >= 50, "beleuchteter Boden fehlt: {lit} Pixel");
    // Der Schatten dunkelt wirklich ab: Kernschatten deutlich dunkler.
    assert!(
        umbra_sum / (umbra as f64) < 0.5 * lit_sum / (lit as f64),
        "Kernschatten {umbra_sum}/{umbra} nicht dunkler als Licht {lit_sum}/{lit}"
    );

    // ── Tor 4b: Determinismus — zweiter Frame bitidentisch. ────────────
    be.render_deferred(&cam, &batches, hdr, &[], None, Some(SHADOW))
        .expect("zweiter Schatten-Frame");
    let again = be.read_hdr().expect("read_hdr 2");
    for y in 0..fh {
        for x in 0..fw {
            assert_eq!(
                again.pixel(x, y).map(f32::to_bits),
                hdr_rb.pixel(x, y).map(f32::to_bits),
                "zwei Frames müssen bitidentisch sein ({x},{y})"
            );
        }
    }

    // ── Sichtbeleg (opt-in): PNG nur, wenn ein Pfad gesetzt ist. ───────
    if let Ok(path) = std::env::var("RE_SHADOW_PNG") {
        be.export_deferred_png(&path).expect("PNG-Export");
        eprintln!("PNG nach {path}");
    }
}
