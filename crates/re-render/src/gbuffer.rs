//! G-Buffer-Bausteine für die Deferred-Pipeline (M5, Box 1).
//!
//! Deferred Rendering schreibt Szeneeigenschaften in mehrere Targets
//! (G-Buffer) und beleuchtet sie später im Screen-Space. Der knausrigste
//! Kanal sind die Oberflächennormalen: drei floats wären Verschwendung.
//! Die oktaedrische Kodierung packt eine Einheitsrichtung verlustarm in
//! zwei — Standard in modernen Deferred-Engines.
//!
//! Diese Datei enthält die Kodierung als backend-neutrale Referenz-Mathe;
//! der WGSL-Post-/Lighting-Pass spiegelt sie Kanal-für-Kanal, damit CPU
//! (Picking, Editor) und GPU dieselbe Repräsentation sehen.
//!
//! Dazu kommt das Kanal-Layout als Daten (ADR-0003): [`GBufferLayout`]
//! beschreibt die Target-Belegung (`deferred_v1`, `visibility_v1`),
//! [`GBufferLayout::pack`] und [`GBufferLayout::unpack`] sind die
//! CPU-Referenz für Geometrie- und Lighting-Pass.

use glam::Vec3;

/// Richtungsvektor → oktaedrische UV in `0..=1` (zwei Kanäle statt drei).
///
/// Die Richtung wird auf den Oktaeder projiziert; die untere Hemisphäre
/// wird nach außen gefaltet. Der Nullvektor gilt als `+Z` (gerade von der
/// Kamera weg ist der harmloseste Fall).
pub fn octahedral_encode(n: Vec3) -> [f32; 2] {
    let sum = n.x.abs() + n.y.abs() + n.z.abs();
    if sum <= f32::EPSILON {
        return [0.5, 0.5]; // +Z
    }
    let p = n / sum;
    let mut e = [p.x, p.y];
    if p.z < 0.0 {
        let sx = if e[0] >= 0.0 { 1.0 } else { -1.0 };
        let sy = if e[1] >= 0.0 { 1.0 } else { -1.0 };
        e = [
            (1.0 - e[1].abs()) * sx,
            (1.0 - e[0].abs()) * sy,
        ];
    }
    [e[0] * 0.5 + 0.5, e[1] * 0.5 + 0.5]
}

/// Oktaedrische UV (`0..=1`) → normierter Richtungsvektor.
///
/// Umkehrung von [`octahedral_encode`]; Werte außerhalb `0..=1` werden
/// auf das Quadrat geklemmt (robust gegen Rundung/Bilinear-Filterung).
pub fn octahedral_decode(uv: [f32; 2]) -> Vec3 {
    let e = [uv[0].clamp(0.0, 1.0) * 2.0 - 1.0, uv[1].clamp(0.0, 1.0) * 2.0 - 1.0];
    let z = 1.0 - e[0].abs() - e[1].abs();
    let v = if z >= 0.0 {
        Vec3::new(e[0], e[1], z)
    } else {
        let sx = if e[0] >= 0.0 { 1.0 } else { -1.0 };
        let sy = if e[1] >= 0.0 { 1.0 } else { -1.0 };
        Vec3::new((1.0 - e[1].abs()) * sx, (1.0 - e[0].abs()) * sy, z)
    };
    // Auf dem Oktaeder-Rand (|x|+|y| = 1) ist die Länge 1; sonst > 0.
    let len = v.length();
    if len <= f32::EPSILON {
        Vec3::Z
    } else {
        v / len
    }
}

/// Quantisiere kodierte UV auf `bits` Bit pro Kanal (z. B. 8 Bit = ein
/// R8G8-Target) — hilft, Präzisionsbudgets durchzurechnen.
pub fn quantize_uv(uv: [f32; 2], bits: u32) -> [f32; 2] {
    let steps = (1u32 << bits) - 1;
    uv.map(|c| (c.clamp(0.0, 1.0) * steps as f32).round() / steps as f32)
}

// ── Layout-Spec (ADR-0003) ────────────────────────────────────────────
//
// Die Kanal-Belegung des G-Buffers als DATEN, nicht als hartkodierte
// Shader-Annahme: Backends und Render-Graph lesen dieses Layout, der
// CPU-Referenz-Pack/Unpack-Pfad spiegelt es, und der WGSL-Geometrie-Pass
// muss später exakt dieselbe Belegung schreiben (Pixel-Diff-Test).

/// Backend-neutrale Texturformate, die das G-Buffer-Layout braucht.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GfxFormat {
    /// 4×8 Bit unorm, linear 0..1 — quantisiert.
    Rgba8Unorm,
    /// 2×16 Bit Float — signed (Velocity), keine Quantisierung.
    Rg16Float,
    /// 1×32 Bit Uint — Visibility-ID (ADR-0003, zweite Quelle).
    R32Uint,
    /// Tiefen-Attachment.
    Depth32Float,
}

impl GfxFormat {
    /// Speicherbedarf pro Pixel in Bytes.
    pub fn bytes_per_pixel(&self) -> u32 {
        match self {
            GfxFormat::Rgba8Unorm | GfxFormat::Rg16Float | GfxFormat::R32Uint | GfxFormat::Depth32Float => 4,
        }
    }

    /// Nutzbare Kanäle des Formats.
    pub fn channel_count(&self) -> usize {
        match self {
            GfxFormat::Rgba8Unorm => 4,
            GfxFormat::Rg16Float => 2,
            GfxFormat::R32Uint | GfxFormat::Depth32Float => 1,
        }
    }

    /// 8-Bit-unorm-Formate quantisieren auf 1/255; Float-Formate nicht.
    fn is_quantized8(&self) -> bool {
        matches!(self, GfxFormat::Rgba8Unorm)
    }
}

/// Semantischer Inhalt eines Kanals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelSemantic {
    AlbedoR,
    AlbedoG,
    AlbedoB,
    Metallic,
    NormalOctX,
    NormalOctY,
    Roughness,
    MaterialId,
    VelocityX,
    VelocityY,
    /// Gepackte (Instance, Primitive)-ID des Visibility Buffers.
    VisibilityId,
    Unused,
}

/// Ein Farb-Target des Layouts: Format + Belegung seiner Kanäle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GBufferTarget {
    pub format: GfxFormat,
    pub channels: [ChannelSemantic; 4],
}

/// Vollständiges G-Buffer-Layout: Farb-Targets + Tiefen-Attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GBufferLayout {
    pub targets: Vec<GBufferTarget>,
    pub depth: GfxFormat,
}

impl GBufferLayout {
    /// Das MRT-Layout aus ADR-0003 Box 1: Albedo+Metallic, oktaedrische
    /// Normalen+Roughness+Material-ID, Velocity (RG16F), Tiefe.
    /// 16 Byte/Pixel — der Wert, gegen den der Visibility-Pfad antritt.
    pub fn deferred_v1() -> Self {
        Self {
            targets: vec![
                GBufferTarget {
                    format: GfxFormat::Rgba8Unorm,
                    channels: [
                        ChannelSemantic::AlbedoR,
                        ChannelSemantic::AlbedoG,
                        ChannelSemantic::AlbedoB,
                        ChannelSemantic::Metallic,
                    ],
                },
                GBufferTarget {
                    format: GfxFormat::Rgba8Unorm,
                    channels: [
                        ChannelSemantic::NormalOctX,
                        ChannelSemantic::NormalOctY,
                        ChannelSemantic::Roughness,
                        ChannelSemantic::MaterialId,
                    ],
                },
                GBufferTarget {
                    format: GfxFormat::Rg16Float,
                    channels: [
                        ChannelSemantic::VelocityX,
                        ChannelSemantic::VelocityY,
                        ChannelSemantic::Unused,
                        ChannelSemantic::Unused,
                    ],
                },
            ],
            depth: GfxFormat::Depth32Float,
        }
    }

    /// Das Visibility-Buffer-Layout aus ADR-0003 (Desktop-Zweitquelle):
    /// eine gepackte ID + Tiefe — 8 Byte/Pixel statt 16.
    pub fn visibility_v1() -> Self {
        Self {
            targets: vec![GBufferTarget {
                format: GfxFormat::R32Uint,
                channels: [
                    ChannelSemantic::VisibilityId,
                    ChannelSemantic::Unused,
                    ChannelSemantic::Unused,
                    ChannelSemantic::Unused,
                ],
            }],
            depth: GfxFormat::Depth32Float,
        }
    }

    /// Gesamtbedarf pro Pixel: alle Farb-Targets plus Tiefe.
    pub fn bytes_per_pixel(&self) -> u32 {
        self.targets.iter().map(|t| t.format.bytes_per_pixel()).sum::<u32>()
            + self.depth.bytes_per_pixel()
    }

    /// Fundstelle eines Semantik-Kanals: `(target, kanal)`.
    pub fn find(&self, sem: ChannelSemantic) -> Option<(usize, usize)> {
        self.targets.iter().enumerate().find_map(|(t, tgt)| {
            tgt.channels
                .iter()
                .position(|c| *c == sem)
                .map(|c| (t, c))
        })
    }

    /// Prüft das Layout in sich: keine doppelten Semantiken, keine
    /// Kanäle über der Formatbreite, vollständige Paare (Albedo dreier,
    /// Normal-X mit Normal-Y, Velocity-X mit Velocity-Y).
    pub fn validate(&self) -> Result<(), String> {
        let mut seen: Vec<ChannelSemantic> = Vec::new();
        for (t, tgt) in self.targets.iter().enumerate() {
            for (c, sem) in tgt.channels.iter().enumerate() {
                if *sem == ChannelSemantic::Unused {
                    continue;
                }
                if c >= tgt.format.channel_count() {
                    return Err(format!("Target {t}: Kanal {c} über Formatbreite {}", tgt.format.channel_count()));
                }
                if seen.contains(sem) {
                    return Err(format!("doppelte Semantik {sem:?}"));
                }
                seen.push(*sem);
            }
        }
        let has = |s| seen.contains(&s);
        let albedo_count = [ChannelSemantic::AlbedoR, ChannelSemantic::AlbedoG, ChannelSemantic::AlbedoB]
            .iter()
            .filter(|s| has(**s))
            .count();
        if !(albedo_count == 0 || albedo_count == 3) {
            return Err(format!("Albedo unvollständig ({albedo_count}/3 Kanälen)"));
        }
        if has(ChannelSemantic::NormalOctX) != has(ChannelSemantic::NormalOctY) {
            return Err("Normal-Oktaeder nur halb vorhanden".into());
        }
        if has(ChannelSemantic::VelocityX) != has(ChannelSemantic::VelocityY) {
            return Err("Velocity nur halb vorhanden".into());
        }
        Ok(())
    }

    /// CPU-Referenz des Geometrie-Passes: Oberfläche → normierte
    /// Kanalwerte je Target, pro 8-Bit-Format quantisiert. Der WGSL-Pass
    /// muss später dieselben Werte liefern.
    pub fn pack(&self, s: &Surface) -> Vec<[f32; 4]> {
        let oct = octahedral_encode(s.normal);
        let mut out = vec![[0.0f32; 4]; self.targets.len()];
        for (t, tgt) in self.targets.iter().enumerate() {
            for (c, sem) in tgt.channels.iter().enumerate() {
                let v = match sem {
                    ChannelSemantic::AlbedoR => s.albedo[0],
                    ChannelSemantic::AlbedoG => s.albedo[1],
                    ChannelSemantic::AlbedoB => s.albedo[2],
                    ChannelSemantic::Metallic => s.metallic,
                    ChannelSemantic::NormalOctX => oct[0],
                    ChannelSemantic::NormalOctY => oct[1],
                    ChannelSemantic::Roughness => s.roughness,
                    ChannelSemantic::MaterialId => s.material_id as f32 / 255.0,
                    ChannelSemantic::VelocityX => s.velocity[0],
                    ChannelSemantic::VelocityY => s.velocity[1],
                    _ => 0.0,
                };
                out[t][c] = if tgt.format.is_quantized8() { q8(v) } else { v };
            }
        }
        out
    }

    /// CPU-Referenz des Lighting-Passes: Targets → Oberfläche
    /// (die abstrakte `surface_at`-Abfrage aus ADR-0003).
    pub fn unpack(&self, targets: &[[f32; 4]]) -> Surface {
        let get = |sem: ChannelSemantic| -> f32 {
            self.find(sem)
                .and_then(|(t, c)| targets.get(t).map(|row| row[c]))
                .unwrap_or(0.0)
        };
        Surface {
            albedo: [
                get(ChannelSemantic::AlbedoR),
                get(ChannelSemantic::AlbedoG),
                get(ChannelSemantic::AlbedoB),
            ],
            metallic: get(ChannelSemantic::Metallic),
            roughness: get(ChannelSemantic::Roughness),
            normal: octahedral_decode([get(ChannelSemantic::NormalOctX), get(ChannelSemantic::NormalOctY)]),
            material_id: (get(ChannelSemantic::MaterialId) * 255.0).round() as u8,
            velocity: [get(ChannelSemantic::VelocityX), get(ChannelSemantic::VelocityY)],
        }
    }
}

/// Rundung auf 8-Bit-unorm-Raster.
fn q8(v: f32) -> f32 {
    (v.clamp(0.0, 1.0) * 255.0).round() / 255.0
}

/// Oberflächen-Daten eines Pixels — was der Geometrie-Pass schreibt und
/// der Lighting-Pass über [`GBufferLayout::unpack`] liest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    /// Lineare Albedo 0..1.
    pub albedo: [f32; 3],
    pub metallic: f32,
    pub roughness: f32,
    /// Einheitsnormale; wird oktaedrisch kodiert.
    pub normal: Vec3,
    pub material_id: u8,
    /// Bewegungsvektor in Pixeln/Frame (für TAA und Upscaler).
    pub velocity: [f32; 2],
}

impl Default for Surface {
    fn default() -> Self {
        Self {
            albedo: [0.5, 0.5, 0.5],
            metallic: 0.0,
            roughness: 0.5,
            normal: Vec3::Z,
            material_id: 0,
            velocity: [0.0, 0.0],
        }
    }
}

/// Packe Instance- und Primitive-ID in eine Visibility-ID (je 16 Bit).
pub fn pack_visibility_id(instance: u16, primitive: u16) -> u32 {
    ((instance as u32) << 16) | primitive as u32
}

/// Umkehrung von [`pack_visibility_id`].
pub fn unpack_visibility_id(id: u32) -> (u16, u16) {
    ((id >> 16) as u16, id as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn angle_deg(a: Vec3, b: Vec3) -> f32 {
        a.normalize().dot(b.normalize()).clamp(-1.0, 1.0).acos().to_degrees()
    }

    #[test]
    fn achsen_runden_exakt() {
        let achsen = [
            Vec3::X,
            -Vec3::X,
            Vec3::Y,
            -Vec3::Y,
            Vec3::Z,
            -Vec3::Z,
        ];
        for a in achsen {
            let back = octahedral_decode(octahedral_encode(a));
            assert!(angle_deg(a, back) < 1e-3, "{a:?} -> {back:?}");
        }
    }

    #[test]
    fn diagonale_runden_dicht() {
        let richtungen = [
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, 0.2, -0.7),
            Vec3::new(0.0, -1.0, -1.0),
            Vec3::new(0.3, 0.3, -0.05),
            Vec3::new(-0.9, 0.1, 0.42),
        ];
        for r in richtungen {
            let back = octahedral_decode(octahedral_encode(r));
            assert!(angle_deg(r, back) < 1e-3, "{r:?} -> {back:?}");
        }
    }

    #[test]
    fn kodierte_uv_in_null_eins() {
        for i in 0..64u32 {
            let f = i as f32 / 64.0;
            let n = Vec3::new((f * 7.0).sin(), (f * 11.0).cos(), (f * 3.0).sin() - 0.3);
            let uv = octahedral_encode(n);
            assert!(
                uv[0] >= 0.0 && uv[0] <= 1.0 && uv[1] >= 0.0 && uv[1] <= 1.0,
                "{n:?} -> {uv:?}"
            );
        }
    }

    #[test]
    fn nullvektor_ist_plus_z() {
        assert_eq!(octahedral_encode(Vec3::ZERO), [0.5, 0.5]);
        assert_eq!(octahedral_decode([0.5, 0.5]), Vec3::Z);
    }

    #[test]
    fn encode_decode_encode_idempotent() {
        let n = Vec3::new(0.4, -0.8, 0.35);
        let uv1 = octahedral_encode(n);
        let uv2 = octahedral_encode(octahedral_decode(uv1));
        assert!((uv1[0] - uv2[0]).abs() < 1e-5 && (uv1[1] - uv2[1]).abs() < 1e-5);
    }

    #[test]
    fn quantisierung_8bit_haelt_winkel_klein() {
        // Das ist die Kern-Aussage: 2×8 Bit reichen für Normalen.
        let mut worst = 0.0f32;
        for i in 0..64u32 {
            let f = i as f32 / 64.0;
            let n = Vec3::new((f * 13.0).sin(), (f * 5.0).cos(), (f * 9.0).sin());
            let q = quantize_uv(octahedral_encode(n), 8);
            worst = worst.max(angle_deg(n, octahedral_decode(q)));
        }
        assert!(worst < 3.0, "8-Bit-Quantisierung: max {worst}°");
    }

    #[test]
    fn decode_klemmt_ausserhalb() {
        // Bilinear gefilterte Werte können knapp über 1 liegen — kein Crash,
        // kein NaN.
        let v = octahedral_decode([1.02, -0.01]);
        assert!(v.is_finite());
        assert!((v.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn deferred_layout_struktur_und_bandbreite() {
        let l = GBufferLayout::deferred_v1();
        assert_eq!(l.targets.len(), 3);
        assert!(l.validate().is_ok());
        // ADR-0003-Zahl: 16 Byte/Pixel (3 Farb-Targets + Tiefe).
        assert_eq!(l.bytes_per_pixel(), 16);
        // Fundstellen wie spezifiziert.
        assert_eq!(l.find(ChannelSemantic::AlbedoR), Some((0, 0)));
        assert_eq!(l.find(ChannelSemantic::Metallic), Some((0, 3)));
        assert_eq!(l.find(ChannelSemantic::NormalOctX), Some((1, 0)));
        assert_eq!(l.find(ChannelSemantic::Roughness), Some((1, 2)));
        assert_eq!(l.find(ChannelSemantic::MaterialId), Some((1, 3)));
        assert_eq!(l.find(ChannelSemantic::VelocityX), Some((2, 0)));
        assert_eq!(l.find(ChannelSemantic::VisibilityId), None);
    }

    #[test]
    fn visibility_layout_halbe_bandbreite() {
        let v = GBufferLayout::visibility_v1();
        assert!(v.validate().is_ok());
        // ADR-0003-Zahl: 8 Byte/Pixel — die Hälfte des G-Buffers.
        assert_eq!(v.bytes_per_pixel(), 8);
        assert_eq!(v.find(ChannelSemantic::VisibilityId), Some((0, 0)));
    }

    #[test]
    fn validierung_findet_fehler() {
        // Doppelte Semantik.
        let mut l = GBufferLayout::deferred_v1();
        l.targets[1].channels[3] = ChannelSemantic::Roughness;
        assert!(l.validate().is_err(), "doppeltes Roughness muss fehln");
        // Kanal über Formatbreite (Rg16F hat nur 2 Kanäle).
        let mut l2 = GBufferLayout::deferred_v1();
        l2.targets[2].channels[2] = ChannelSemantic::Metallic;
        assert!(l2.validate().is_err(), "Kanal 2 in RG-Format muss fehln");
        // Unvollständige Albedo.
        let mut l3 = GBufferLayout::deferred_v1();
        l3.targets[0].channels[1] = ChannelSemantic::Unused;
        assert!(l3.validate().is_err(), "2/3 Albedo muss fehln");
        // Halber Oktaeder: Normal-Y entfernt, Normal-X bleibt allein.
        let mut l4 = GBufferLayout::deferred_v1();
        l4.targets[1].channels[1] = ChannelSemantic::Unused;
        assert!(l4.validate().is_err(), "Normal-X ohne Y muss fehln");
        // Halbe Velocity.
        let mut l5 = GBufferLayout::deferred_v1();
        l5.targets[2].channels[1] = ChannelSemantic::Unused;
        assert!(l5.validate().is_err(), "Velocity-X ohne Y muss fehln");
    }

    #[test]
    fn pack_unpack_roundtrip_haelt_praezisionsbudget() {
        let l = GBufferLayout::deferred_v1();
        let s = Surface {
            albedo: [0.8, 0.25, 0.5],
            metallic: 1.0,
            roughness: 0.37,
            normal: Vec3::new(0.4, -0.8, 0.35).normalize(),
            material_id: 173,
            velocity: [-12.75, 3.5],
        };
        let back = l.unpack(&l.pack(&s));
        // 8-Bit-Kanäle: Fehler <= ein Rasterpunkt.
        for i in 0..3 {
            assert!((back.albedo[i] - s.albedo[i]).abs() <= 1.0 / 255.0 + 1e-6, "albedo {i}");
        }
        assert!((back.metallic - s.metallic).abs() <= 1.0 / 255.0 + 1e-6);
        assert!((back.roughness - s.roughness).abs() <= 1.0 / 255.0 + 1e-6);
        // Normalen: 8-Bit-Oktaeder-Winkel (wie im Quantisierungs-Test < 3°).
        let dot = back.normal.dot(s.normal).clamp(-1.0, 1.0);
        let winkel = dot.acos().to_degrees();
        assert!(winkel < 3.0, "Normalenwinkel {winkel}°");
        // Material-ID und Velocity (16F) exakt.
        assert_eq!(back.material_id, s.material_id);
        assert_eq!(back.velocity, s.velocity);
    }

    #[test]
    fn quantisierung_entspricht_dem_format() {
        let l = GBufferLayout::deferred_v1();
        let mut s = Surface::default();
        s.albedo = [0.5001, 0.0, 0.0]; // knapp über einem Rasterpunkt
        s.velocity = [123.456, -0.001]; // Float-Format: unquantisiert
        let packed = l.pack(&s);
        assert_eq!(packed[0][0], 128.0 / 255.0, "Albedo muss auf 8 Bit rastern");
        assert_eq!(packed[2][0], 123.456, "Velocity (RG16F-Spec) darf nicht rastern");
    }

    #[test]
    fn visibility_id_rundlauf() {
        for (inst, prim) in [(0u16, 0u16), (1, 2), (65535, 65535), (4096, 1234)] {
            let id = pack_visibility_id(inst, prim);
            assert_eq!(unpack_visibility_id(id), (inst, prim));
        }
        // Instanz und Primitive überschneiden sich nicht im 32-Bit-Wort.
        assert_eq!(pack_visibility_id(1, 0), 1 << 16);
        assert_eq!(pack_visibility_id(0, 1), 1);
    }
}
