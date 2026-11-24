//! Headless isometric sprite baker: rasterizes a node subtree into RGBA
//! sprite frames (classic 2:1-ish game isometric) with no window and no GPU.
//!
//! One frame per azimuth is rendered with an orthographic camera at the given
//! pitch (true isometric = 35.264°), painter-sorted, Lambert-shaded to match
//! the viewer's look, supersampled and box-filtered for crisp sprite edges.
//! `anim_frames` > 1 additionally steps the procedural motion
//! ([`crate::motion`]) through one full cycle: the sheet becomes a grid of
//! `views` rows × `anim_frames` columns, each row a seamless animation loop
//! of one azimuth. `anim_frames` = 1 keeps the classic horizontal strip.
//! Encode the result with `re_io::png::write_png_rgba8`.

use anyhow::{bail, Result};
use glam::{Mat4, Vec3};

use crate::id::NodeId;
use crate::material::Material;
use crate::mesh::Mesh;
use crate::ops::{self, Pose};
use crate::project::Project;
use crate::scene::NodeKind;

/// How to bake. Defaults: four frames (N/E/S/W-ish), true isometric pitch,
/// 128 px sprites, 2× supersampling, shaded, one still frame per view.
#[derive(Clone, Debug)]
pub struct BakeOptions {
    /// Edge length of one sprite frame in pixels (8..=1024).
    pub frame_px: u32,
    /// Supersampling factor per axis (1..=4); 2 gives nice sprite edges.
    pub oversample: u32,
    /// One azimuth per output frame, degrees around +Y (0 = camera at +Z).
    pub azimuths: Vec<f32>,
    /// Camera elevation above the horizon; 35.264° = true isometric.
    pub pitch_deg: f32,
    /// Fraction of the frame kept free around the object (0..=0.4).
    pub margin: f32,
    /// false = flat base colors, true = Lambert shading like the viewer.
    pub shaded: bool,
    /// Motion cycle samples per azimuth (1..=24). 1 = sprite at rest;
    /// > 1 bakes the procedural motion as a strip: waves/sway/bob run one
    /// full loop cycle (`2π` of loop time, independent of `speed` — pick the
    /// playback fps in your engine instead).
    pub anim_frames: u32,
    /// Keyframe clip to bake: with `Some`, every animation column samples
    /// this clip at `t = duration·k/anim_frames` (rigs are skinned, cf.
    /// `ops::sample_pose`) instead of the static procedural motion loop.
    /// Set `looping = true` on the clip for a seamless sprite loop.
    pub animation: Option<crate::id::AnimationId>,
}

impl Default for BakeOptions {
    fn default() -> Self {
        Self {
            frame_px: 128,
            oversample: 2,
            azimuths: vec![45.0, 135.0, 225.0, 315.0],
            pitch_deg: 35.264,
            margin: 0.08,
            shaded: true,
            anim_frames: 1,
            animation: None,
        }
    }
}

/// A finished sprite sheet. With `anim_frames` = 1 a horizontal strip of
/// `views()` images; otherwise a grid of `views()` rows (one per azimuth)
/// × `anim()` columns (one per motion sample), row-major in memory.
#[derive(Clone, Debug, PartialEq)]
pub struct BakedSheet {
    pub frame_px: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Azimuth views baked (= sheet rows when animated).
    pub views: usize,
    /// Motion samples per view (1 = still strip).
    pub anim: usize,
}

impl BakedSheet {
    pub fn views(&self) -> usize {
        self.views.max(1)
    }
    pub fn anim(&self) -> usize {
        self.anim.max(1)
    }
    /// Total number of images on the sheet.
    pub fn frames(&self) -> usize {
        self.views() * self.anim()
    }
    /// Grid size in cells: the classic strip (`anim() == 1`) is a single row of
    /// `views` columns; an animation is `views` rows × `anim` columns.
    fn grid(&self) -> (usize, usize) {
        if self.anim() == 1 {
            (1, self.views())
        } else {
            (self.views(), self.anim())
        }
    }
    /// Pixels of the cell at view row `view`, animation column `frame`, copied
    /// out of the row-major sheet image. For the classic strip every view is a
    /// column of the single row, so pass `frame = 0`.
    pub fn frame_rgba_at(&self, view: usize, frame: usize) -> Option<Vec<u8>> {
        if view >= self.views() || frame >= self.anim() {
            return None;
        }
        let px = self.frame_px as usize;
        let (rows, cols) = self.grid();
        let (r, c) = if self.anim() == 1 { (0, view) } else { (view, frame) };
        if r >= rows || c >= cols {
            return None;
        }
        let w = self.width as usize;
        let mut out = Vec::with_capacity(px * px * 4);
        for y in 0..px {
            let off = ((r * px + y) * w + c * px) * 4;
            out.extend_from_slice(&self.rgba[off..off + px * 4]);
        }
        Some(out)
    }
    /// Linear frame index over the row-major grid (strip-compatible).
    pub fn frame_rgba(&self, i: usize) -> Option<Vec<u8>> {
        let (rows, cols) = self.grid();
        let (r, c) = (i / cols, i % cols);
        if r >= rows {
            return None;
        }
        if self.anim() == 1 {
            self.frame_rgba_at(c, 0)
        } else {
            self.frame_rgba_at(r, c)
        }
    }
}

struct WorldTri {
    pts: [Vec3; 3],
    color: [u8; 4],
}

/// Linear RGBA -> 8 bit sRGB-ish color, keeping alpha (same curve as the viewer).
fn to_rgba8(rgba: [f32; 4]) -> [u8; 4] {
    let f = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.0031308 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round() as u8
    };
    [
        f(rgba[0]),
        f(rgba[1]),
        f(rgba[2]),
        (rgba[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

/// Collect world-space triangles of the subtree under `node` (mesh instances,
/// skinned meshes and expanded tilemaps). When `motion_t` is `Some`,
/// procedural motion is displaced by [`Motion::offset_loop`] at that loop
/// time (mesh nodes and kit tiles alike, shared phase so the whole sprite
/// loops coherently); `None` gathers the pose at rest. When `clip` is `Some`,
/// skinned meshes are posed by that keyframe animation at its time instead of
/// the bind pose (clip mode replaces the procedural motion sampling — bake()
/// passes `motion_t = None` then).
fn gather(
    p: &Project,
    node: NodeId,
    shaded: bool,
    motion_t: Option<f32>,
    clip: Option<(&crate::animation::Animation, f32)>,
    out: &mut Vec<WorldTri>,
) {
    let Some(n) = p.scene.get(node) else {
        return;
    };
    if !n.visible {
        return;
    }
    let light = Vec3::new(0.45, 0.82, 0.35).normalize();
    let world = p.scene.world_transform(node).to_mat4();
    match &n.kind {
        NodeKind::MeshInstance {
            mesh,
            material,
            skeleton_node,
        } => {
            if let Some(m) = p.library.meshes.get(mesh) {
                let skinned = if m.skin.is_some() {
                    skeleton_node.and_then(|sk| {
                        let pose = match clip {
                            Some((anim, t)) => ops::sample_pose(p, sk, anim, t),
                            None => Pose::default(),
                        };
                        ops::skinned_positions(p, *mesh, node, sk, &pose).ok()
                    })
                } else {
                    None
                };
                let pos: &[[f32; 3]] = skinned.as_deref().unwrap_or(&m.positions);
                let mo = motion_t.map(|_| n.motion).unwrap_or_default();
                push_tris(
                    m,
                    pos,
                    &world,
                    material.as_ref().and_then(|id| p.library.materials.get(id)),
                    shaded,
                    light,
                    mo,
                    motion_t,
                    out,
                );
            }
        }
        NodeKind::TileMapInstance { tilemap } => {
            let Some(map) = p.library.tilemaps.get(tilemap) else {
                return;
            };
            let Some(ts) = p.library.tilesets.get(&map.tileset) else {
                return;
            };
            for c in &map.cells {
                let Some(tile) = ts.tiles.get(c.tile as usize) else {
                    continue;
                };
                let Some(m) = p.library.meshes.get(&tile.mesh) else {
                    continue;
                };
                let w = world * ts.cell_transform(c.x, c.z).to_mat4() * tile.offset.to_mat4();
                let mo = motion_t.map(|_| tile.motion).unwrap_or_default();
                push_tris(
                    m,
                    &m.positions,
                    &w,
                    tile.material.as_ref().and_then(|id| p.library.materials.get(id)),
                    shaded,
                    light,
                    mo,
                    motion_t,
                    out,
                );
            }
        }
        _ => {}
    }
    let children = n.children.clone();
    for c in children {
        gather(p, c, shaded, motion_t, clip, out);
    }
}

#[allow(clippy::too_many_arguments)]
fn push_tris(
    m: &Mesh,
    pos: &[[f32; 3]],
    world: &Mat4,
    material: Option<&Material>,
    shaded: bool,
    light: Vec3,
    motion: crate::motion::Motion,
    motion_t: Option<f32>,
    out: &mut Vec<WorldTri>,
) {
    let base = material
        .map(|mt| to_rgba8(mt.base_color))
        .unwrap_or([176, 176, 176, 255]);
    let mut wp: Vec<Vec3> = pos
        .iter()
        .map(|q| world.transform_point3(Vec3::from_array(*q)))
        .collect();
    if let (false, Some(t)) = (motion.is_none(), motion_t) {
        // sway needs the object's own world height range
        let (mut y0, mut y1) = (f32::INFINITY, f32::NEG_INFINITY);
        for q in &wp {
            y0 = y0.min(q.y);
            y1 = y1.max(q.y);
        }
        for q in &mut wp {
            *q += motion.offset_loop(*q, y0, y1, 0.0, t);
        }
    }
    for t in m.iter_triangles() {
        let i = [t[0] as usize, t[1] as usize, t[2] as usize];
        if !i.iter().all(|k| *k < wp.len()) {
            continue;
        }
        let (a, b, c) = (wp[i[0]], wp[i[1]], wp[i[2]]);
        let color = if shaded {
            let nrm = (b - a).cross(c - a).normalize_or_zero();
            let l = 0.38 + 0.62 * nrm.dot(light).abs();
            [
                (base[0] as f32 * l).min(255.0) as u8,
                (base[1] as f32 * l).min(255.0) as u8,
                (base[2] as f32 * l).min(255.0) as u8,
                base[3],
            ]
        } else {
            base
        };
        out.push(WorldTri {
            pts: [a, b, c],
            color,
        });
    }
}

/// Bake the subtree under `root` into an RGBA sprite sheet: `views` rows
/// (one per azimuth) × `anim_frames` columns (motion samples of one loop).
pub fn bake(p: &Project, root: NodeId, opt: &BakeOptions) -> Result<BakedSheet> {
    if !p.scene.nodes.contains_key(&root) {
        bail!("node n{} not found", root.0);
    }
    if opt.azimuths.is_empty() {
        bail!("bake needs at least one azimuth");
    }
    let anim = opt.anim_frames.clamp(1, 24) as usize;
    // keyframe clip mode: columns sample the clip; procedural motion rests
    let clip = opt
        .animation
        .as_ref()
        .and_then(|id| p.library.animations.get(id))
        .map(|a| (a, a.duration().max(1e-4)));
    if opt.animation.is_some() && clip.is_none() {
        bail!(
            "animation a{} not found in library",
            opt.animation.map(|a| a.0).unwrap_or_default()
        );
    }
    // one triangle set per column; sample 0 is the pose at rest / clip start
    let mut frames: Vec<Vec<WorldTri>> = Vec::with_capacity(anim);
    for k in 0..anim {
        let frac = if anim == 1 { 0.0 } else { k as f32 / anim as f32 };
        let (motion_t, clip_t) = match &clip {
            Some((a, dur)) => (None, Some((*a, dur * frac))),
            None => (if anim == 1 { None } else { Some(std::f32::consts::TAU * frac) }, None),
        };
        let mut tris = Vec::new();
        gather(p, root, opt.shaded, motion_t, clip_t, &mut tris);
        frames.push(tris);
    }

    let frame = opt.frame_px.clamp(8, 1024);
    let os = opt.oversample.clamp(1, 4);
    let res = frame * os;
    // shared pivot from the union of all samples so the loop does not wander
    let mut s = Vec3::ZERO;
    let mut k = 0u32;
    for tris in &frames {
        for t in tris {
            for v in t.pts {
                s += v;
                k += 1;
            }
        }
    }
    let center = if k == 0 { Vec3::ZERO } else { s / k as f32 };

    let bytes = (frame * frame * 4) as usize;
    let views = opt.azimuths.len();
    // Sheet geometry (also what PNG/preview read as a row-major image):
    // anim == 1 → one row with a column per view (classic left-to-right strip);
    // anim > 1  → `views` rows × `anim` columns, frame k of a loop in row `vi`.
    let (rows, cols) = if anim == 1 { (1usize, views) } else { (views, anim) };
    let w_px = cols * frame as usize;
    let mut rgba = vec![0u8; w_px * rows * frame as usize * 4];
    let mut cell = vec![0u8; bytes];
    for (vi, &az) in opt.azimuths.iter().enumerate() {
        // one scale for the whole row: the loop must not zoom in and out
        let union_scale = (anim > 1).then(|| row_scale(&frames, center, az, opt.pitch_deg, res));
        for (ti, tris) in frames.iter().enumerate() {
            let hi = render_frame(tris, center, az, opt.pitch_deg, res, opt.margin, union_scale);
            downsample(&hi, os, &mut cell, frame);
            let (dr, dc) = if anim == 1 { (0usize, vi) } else { (vi, ti) };
            let stride = frame as usize * 4;
            for y in 0..frame as usize {
                let o = ((dr * frame as usize + y) * w_px + dc * frame as usize) * 4;
                rgba[o..o + stride].copy_from_slice(&cell[y * stride..(y + 1) * stride]);
            }
        }
    }
    Ok(BakedSheet {
        frame_px: frame,
        width: w_px as u32,
        height: (rows * frame as usize) as u32,
        rgba,
        views,
        anim,
    })
}

struct Rt {
    pts: [[f32; 2]; 3],
    prox: f32,
    color: [u8; 4],
}

/// Screen scale (px per world unit) that fits every animation sample of a
/// row into the frame — computed once per azimuth so the loop never rescales.
fn row_scale(frames: &[Vec<WorldTri>], center: Vec3, az_deg: f32, pitch_deg: f32, res: u32) -> f32 {
    let az = az_deg.to_radians();
    let pr = pitch_deg.to_radians().clamp(0.05, 1.5);
    let e = Vec3::new(az.sin() * pr.cos(), pr.sin(), az.cos() * pr.cos());
    let f = -e;
    let mut s = f.cross(Vec3::Y);
    if s.length_squared() < 1e-9 {
        s = Vec3::X;
    }
    let s = s.normalize();
    let u = s.cross(f).normalize();
    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for tris in frames {
        for t in tris {
            for v in t.pts {
                let d = v - center;
                let (x, y) = (d.dot(s), d.dot(u));
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
            }
        }
    }
    if maxx <= minx {
        return 1.0;
    }
    // 0.72 ≈ the legacy (1 - margin 0.28) use of the frame; bake() still
    // clamps to the per-frame fit, so nothing can overflow.
    (res as f32 * 0.72) / (maxx - minx).max(maxy - miny).max(1e-4)
}

/// One isometric frame at internal `res`; painter's algorithm, no depth buffer.
/// `scale` overrides the automatic fit (used to keep an animation row stable).
fn render_frame(
    tris: &[WorldTri],
    center: Vec3,
    az_deg: f32,
    pitch_deg: f32,
    res: u32,
    margin: f32,
    scale: Option<f32>,
) -> Vec<u8> {
    let mut buf = vec![0u8; (res * res * 4) as usize];
    if tris.is_empty() {
        return buf;
    }
    let az = az_deg.to_radians();
    let pr = pitch_deg.to_radians().clamp(0.05, 1.5);
    // direction from the subject towards the camera
    let e = Vec3::new(az.sin() * pr.cos(), pr.sin(), az.cos() * pr.cos());
    let f = -e;
    let mut s = f.cross(Vec3::Y);
    if s.length_squared() < 1e-9 {
        s = Vec3::X;
    }
    let s = s.normalize();
    let u = s.cross(f).normalize();

    let mut rts: Vec<Rt> = Vec::with_capacity(tris.len());
    let (mut minx, mut miny, mut maxx, mut maxy) =
        (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for t in tris {
        let pr_pts: [[f32; 2]; 3] = t.pts.map(|v| {
            let d = v - center;
            [d.dot(s), d.dot(u)]
        });
        for q in &pr_pts {
            minx = minx.min(q[0]);
            maxx = maxx.max(q[0]);
            miny = miny.min(q[1]);
            maxy = maxy.max(q[1]);
        }
        let centroid = (t.pts[0] + t.pts[1] + t.pts[2]) * (1.0 / 3.0);
        rts.push(Rt {
            pts: pr_pts,
            prox: (centroid - center).dot(e),
            color: t.color,
        });
    }

    let fit = (res as f32 * (1.0 - margin.clamp(0.0, 0.4)))
        / (maxx - minx).max(maxy - miny).max(1e-4);
    // With a row-wide union scale the frame belongs to an animation loop:
    // keep the shared pivot (coords are already relative to it) — re-centering
    // per frame would cancel the very motion the loop bakes. Without it
    // (classic strip) every view is framed independently as always.
    let shared_pivot = scale.is_some();
    let scale = scale.map_or(fit, |rs| rs.min(fit));
    let (cx, cy) = if shared_pivot {
        (0.0, 0.0)
    } else {
        ((minx + maxx) * 0.5, (miny + maxy) * 0.5)
    };
    for r in &mut rts {
        for q in &mut r.pts {
            q[0] = res as f32 * 0.5 + (q[0] - cx) * scale;
            q[1] = res as f32 * 0.5 - (q[1] - cy) * scale;
        }
    }

    // opaque pass first (far -> near), translucent blended on top
    let mut opaque: Vec<&Rt> = rts.iter().filter(|r| r.color[3] >= 250).collect();
    let mut blend: Vec<&Rt> = rts.iter().filter(|r| r.color[3] < 250).collect();
    opaque.sort_by(|a, b| a.prox.total_cmp(&b.prox));
    blend.sort_by(|a, b| a.prox.total_cmp(&b.prox));
    for r in &opaque {
        raster(&mut buf, res, &r.pts, r.color, false);
    }
    for r in &blend {
        raster(&mut buf, res, &r.pts, r.color, true);
    }
    buf
}

fn raster(buf: &mut [u8], res: u32, pts: &[[f32; 2]; 3], color: [u8; 4], alpha: bool) {
    let [a, b, c] = pts;
    let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if area.abs() < 1e-6 {
        return;
    }
    let minx = a[0].min(b[0]).min(c[0]).floor().max(0.0) as u32;
    let maxx = a[0].max(b[0]).max(c[0]).ceil().min(res as f32 - 1.0).max(0.0) as u32;
    let miny = a[1].min(b[1]).min(c[1]).floor().max(0.0) as u32;
    let maxy = a[1].max(b[1]).max(c[1]).ceil().min(res as f32 - 1.0).max(0.0) as u32;
    for py in miny..=maxy {
        for px in minx..=maxx {
            let x = px as f32 + 0.5;
            let y = py as f32 + 0.5;
            let w0 = (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
            let w1 = (c[0] - b[0]) * (y - b[1]) - (c[1] - b[1]) * (x - b[0]);
            let w2 = (a[0] - c[0]) * (y - c[1]) - (a[1] - c[1]) * (x - c[0]);
            let inside = (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0)
                || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0);
            if !inside {
                continue;
            }
            let i = ((py * res + px) as usize) * 4;
            if !alpha {
                buf[i..i + 4].copy_from_slice(&color);
            } else {
                let sa = color[3] as f32 / 255.0;
                let da = buf[i + 3] as f32 / 255.0;
                let oa = sa + da * (1.0 - sa);
                for k in 0..3 {
                    let sv = color[k] as f32;
                    let dv = buf[i + k] as f32;
                    buf[i + k] = if oa > 1e-6 {
                        ((sv * sa + dv * da * (1.0 - sa)) / oa).min(255.0).round() as u8
                    } else {
                        0
                    };
                }
                buf[i + 3] = (oa * 255.0).round() as u8;
            }
        }
    }
}

/// Box filter with alpha-weighted color averaging (straight alpha).
fn downsample(src: &[u8], os: u32, dst: &mut [u8], frame: u32) {
    if os == 1 {
        dst.copy_from_slice(src);
        return;
    }
    let res = frame * os;
    for fy in 0..frame {
        for fx in 0..frame {
            let (mut rs, mut gs, mut bs, mut asum) = (0f32, 0f32, 0f32, 0f32);
            for dy in 0..os {
                for dx in 0..os {
                    let sx = fx * os + dx;
                    let sy = fy * os + dy;
                    let i = ((sy * res + sx) as usize) * 4;
                    let a = src[i + 3] as f32 / 255.0;
                    rs += src[i] as f32 * a;
                    gs += src[i + 1] as f32 * a;
                    bs += src[i + 2] as f32 * a;
                    asum += a;
                }
            }
            let o = ((fy * frame + fx) as usize) * 4;
            let n = (os * os) as f32;
            if asum > 1e-5 {
                dst[o] = (rs / asum).min(255.0).round() as u8;
                dst[o + 1] = (gs / asum).min(255.0).round() as u8;
                dst[o + 2] = (bs / asum).min(255.0).round() as u8;
            }
            dst[o + 3] = (asum / n * 255.0).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops;
    use crate::primitives::PrimitiveKind;
    use crate::terrain;
    use crate::transform::Transform;
    use glam::Quat;

    #[test]
    fn box_bakes_an_opaque_silhouette_on_transparent_bg() {
        let mut p = Project::new("bake");
        let (node, _m) = ops::add_primitive(
            &mut p,
            None,
            "box",
            PrimitiveKind::Box,
            1.0,
            1.0,
            4,
            Transform::from_parts(Vec3::ZERO, Quat::IDENTITY, Vec3::ONE),
        );
        let opt = BakeOptions {
            frame_px: 32,
            oversample: 2,
            azimuths: vec![45.0, 225.0],
            ..Default::default()
        };
        let sheet = bake(&p, node, &opt).unwrap();
        assert_eq!(sheet.width, 64);
        assert_eq!(sheet.height, 32);
        assert_eq!(sheet.rgba.len(), (64 * 32 * 4) as usize);

        let f0 = sheet.frame_rgba(0).unwrap();
        let px = |x: u32, y: u32| &f0[((y * 32 + x) * 4) as usize..];
        // center filled, corners fully transparent
        assert!(px(16, 16)[3] > 200, "center must be covered");
        assert_eq!(px(0, 0)[3], 0, "corner must stay transparent");
        assert_eq!(px(31, 31)[3], 0, "corner must stay transparent");
        // second frame rendered too (not just black)
        let f1 = sheet.frame_rgba(1).unwrap();
        assert!(f1.iter().skip(3).step_by(4).any(|a| *a > 200));
    }

    #[test]
    fn bake_covers_terrain_tilemap_and_water() {
        let mut p = Project::new("bake2");
        let root = ops::add_node(&mut p, None, "root", Transform::default(), NodeKind::Empty);
        let (_t, _) =
            terrain::add_terrain(&mut p, Some(root), "Karte", 4.0, 4.0, 4, Transform::default());
        let (_w, _) = ops::add_primitive(
            &mut p,
            Some(root),
            "w",
            PrimitiveKind::Box,
            0.5,
            0.5,
            4,
            Transform::from_parts(Vec3::new(0.0, 0.3, 0.0), Quat::IDENTITY, Vec3::ONE),
        );
        let opt = BakeOptions {
            frame_px: 48,
            oversample: 1,
            azimuths: vec![45.0],
            ..Default::default()
        };
        let sheet = bake(&p, root, &opt).unwrap();
        let opaque = sheet.rgba.iter().skip(3).step_by(4).filter(|a| **a > 200).count();
        // a flat 4×4 m map seen at 35° fills roughly w·w·sin(pitch)/2 ≈ 460 px of 2304
        assert!(opaque > 250, "subtree bake should cover the frame, got {opaque}");
    }

    /// Anim-Bake: Bewegung wird als Loop-Serie gebacken – Zeilen = Ansichten,
    /// Spalten = Frames; Frames unterscheiden sich, Views bleiben stabil.
    #[test]
    fn animated_bake_loops_and_grid_layout() {
        let mut p = Project::new("anim");
        let (water, _, _) = crate::terrain::add_water(&mut p, None, "See", 6.0, 6.0, 0.0);
        // chunky ripple so the motion crosses pixel granularity at 64 px
        p.scene.get_mut(water).unwrap().motion = crate::motion::Motion::Waves {
            amp: 0.5,
            speed: 1.0,
        };

        let opt = BakeOptions {
            frame_px: 64,
            oversample: 1,
            azimuths: vec![45.0, 225.0],
            anim_frames: 8,
            ..Default::default()
        };
        let sheet = bake(&p, water, &opt).unwrap();
        assert_eq!(sheet.views(), 2);
        assert_eq!(sheet.anim(), 8);
        assert_eq!(sheet.frames(), 16);
        assert_eq!(sheet.width, 512);
        assert_eq!(sheet.height, 128);
        assert_eq!(sheet.rgba.len(), (512 * 128 * 4) as usize);

        // the waves must actually move between loop frames of one view
        let a = sheet.frame_rgba_at(0, 0).unwrap();
        let b = sheet.frame_rgba_at(0, 2).unwrap();
        assert_ne!(a, b, "loop frames of a moving lake must differ");
        // enough painted coverage in every cell (the lake is translucent,
        // so sample coverage, not opacity)
        for v in 0..2 {
            for f in 0..8 {
                let img = sheet.frame_rgba_at(v, f).unwrap();
                let covered = img.iter().skip(3).step_by(4).filter(|al| **al > 30).count();
                assert!(covered > 300, "frame {v}/{f} too empty: {covered} px");
            }
        }
        // out of range stays None
        assert_eq!(sheet.frame_rgba_at(2, 0), None);
        assert_eq!(sheet.frame_rgba_at(0, 8), None);

        // anim = 1 stays the classic strip: one row, no motion displacement
        let still = bake(
            &p,
            water,
            &BakeOptions {
                frame_px: 64,
                oversample: 1,
                azimuths: vec![45.0, 225.0],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(still.height, 64);
        assert_eq!(still.anim(), 1);
        let first = still.frame_rgba(0).unwrap().to_vec();
        let via_at = still.frame_rgba_at(0, 0).unwrap().to_vec();
        assert_eq!(first, via_at, "strip indexing stays compatible");
    }

    #[test]
    fn invisible_subtrees_are_skipped() {        let mut p = Project::new("bake3");
        let (node, _) = ops::add_primitive(
            &mut p,
            None,
            "box",
            PrimitiveKind::Box,
            1.0,
            1.0,
            4,
            Transform::default(),
        );
        p.scene.get_mut(node).unwrap().visible = false;
        let opt = BakeOptions {
            frame_px: 16,
            oversample: 1,
            azimuths: vec![0.0],
            ..Default::default()
        };
        let sheet = bake(&p, node, &opt).unwrap();
        assert!(sheet.rgba.iter().all(|b| *b == 0), "hidden node bakes nothing");
    }

    /// Clip-Back: bei `animation` samplen die Spalten den Keyframe-Clip und
    /// die Skins werden passend gepost – prozedurale Bewegung ruht.
    #[test]
    fn bake_clip_animation_poses_skins_per_column() {
        let mut p = Project::new("clip");
        let mut sk = crate::skeleton::Skeleton::new("rig");
        sk.add_joint("Hips", None, Transform::from_parts(Vec3::ZERO, Quat::IDENTITY, Vec3::ONE));
        let rig = p.library.add_skeleton(sk);

        let mut m = Mesh::default();
        m.name = "tri".into();
        m.positions = vec![[0.0, 0.0, 0.0], [0.8, 0.0, 0.0], [0.0, 0.8, 0.0]];
        m.skin = Some(crate::mesh::Skin {
            skeleton: rig,
            joints: vec![[0, 0, 0, 0]; 3],
            weights: vec![[1.0, 0.0, 0.0, 0.0]; 3],
        });
        let mesh = p.library.add_mesh(m);

        let hero = ops::add_node(
            &mut p,
            None,
            "hero",
            Transform::default(),
            NodeKind::SkeletonInstance { skeleton: rig },
        );
        ops::add_node(
            &mut p,
            Some(hero),
            "body",
            Transform::default(),
            NodeKind::MeshInstance { mesh, material: None, skeleton_node: Some(hero) },
        );

        let mut a = crate::animation::Animation::new("lift");
        a.looping = true;
        let mut ch = crate::animation::Channel::new(
            crate::animation::Target::Joint("Hips".into()),
            crate::animation::ChannelPath::Translation,
        );
        ch.space = crate::animation::Space::RelativeToBind;
        ch.set_key(0.0, crate::animation::KeyValue::Vec3([0.0, 0.0, 0.0]));
        ch.set_key(1.0, crate::animation::KeyValue::Vec3([0.0, 1.2, 0.0]));
        a.channels.push(ch);
        let anim = p.library.add_animation(a);

        let opt = BakeOptions {
            frame_px: 64,
            oversample: 1,
            azimuths: vec![45.0, 225.0],
            anim_frames: 4,
            animation: Some(anim),
            ..Default::default()
        };
        let sheet = bake(&p, hero, &opt).unwrap();
        assert_eq!(sheet.views(), 2);
        assert_eq!(sheet.anim(), 4);
        assert_eq!(sheet.width, 256);
        assert_eq!(sheet.height, 128);

        // die Hebe-Kurve verschiebt die Silhouette zwischen den Spalten
        let f0 = sheet.frame_rgba_at(0, 0).unwrap();
        let f2 = sheet.frame_rgba_at(0, 2).unwrap();
        assert_ne!(f0, f2, "clip columns must differ");
        let centroid = |f: &[u8]| -> f32 {
            let (mut s, mut n) = (0.0f32, 0.0f32);
            for y in 0..64usize {
                for x in 0..64usize {
                    let a = f[(y * 64 + x) * 4 + 3] as f32;
                    if a > 8.0 {
                        s += y as f32 * a;
                        n += a;
                    }
                }
            }
            if n == 0.0 {
                -1.0
            } else {
                s / n
            }
        };
        let (c0, c2) = (centroid(&f0), centroid(&f2));
        assert!(c0 >= 0.0 && c2 >= 0.0, "every column shows the sprite");
        assert!(
            (c0 - c2).abs() > 2.0,
            "the clip must move the sprite between columns (c0={c0} c2={c2})"
        );
    }
}
