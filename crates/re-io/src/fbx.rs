//! Binary FBX (Autodesk, version 7.4 and compatible) reader built on the
//! `fbxcel` crate's data-tree API.
//!
//! Import only — export stays on glTF/OBJ. The layout below was verified
//! against real exporter output (Blender FBX IO, Maya, 3ds Max via the
//! ufbx regression corpus); the semantics we rely on:
//!
//! - Every object node under `Objects` carries `[id, "Name\0\x01Class", sub]`;
//!   `sub` is the type (`Mesh`, `Skin`, `Cluster`, `Material`, ...).
//! - `Connections` lines `C: "<ptype>", subject, owner [, label]` mean
//!   "subject belongs to / is driven by owner" — for `OO` model lines the
//!   subject is the child and the owner the parent (`owner == 0` marks the
//!   scene root); animation targets use `OP` with the property label.
//! - Units: `GlobalSettings` `"-unit"` gives the length unit (missing =
//!   treat as metres); `UpAxis` 1 = Y (native), 2 = Z (wrapped in a
//!   −90°-X fix-up root node).
//! - Times: `KeyTime` arrays are KTime ticks, 46186158000 per second.
//!
//! Mapping mirrors [`crate::import`]: node tree + meshes/materials first,
//! then `Skin` deformers become `Skeleton` assets with hidden original
//! bone nodes plus a `SkeletonInstance`, cluster `Indexes`/`Weights`
//! become per-vertex skin weights (top-4, normalized), and each
//! `AnimationStack` (take) becomes one `Animation` clip with
//! joint-targeted channels.
//!
//! Known limitations (deliberate): blend shapes are skipped, FBX cubic
//! key interpolation is reduced to linear, rotations are composed in XYZ
//! order, and whole-file compressed FBX (some 7.7+ exports) is rejected —
//! `fbxcel` only decodes per-array zlib compression.

use anyhow::{anyhow, bail, Context, Result};
use fbxcel::low::v7400::AttributeValue;
use fbxcel::tree::any::AnyTree;
use fbxcel::tree::v7400::{NodeHandle as Handle, Tree};
use glam::{Mat4, Quat};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use re_core::animation::{Animation, Channel, ChannelPath, Interpolation, KeyValue, Target};
use re_core::id::{MaterialId, MeshId, NodeId, SkeletonId};
use re_core::mesh::{Mesh as WMesh, Skin};
use re_core::project::Project;
use re_core::scene::{Node as WNode, NodeKind};
use re_core::skeleton::Skeleton;
use re_core::transform::Transform;

/// KTime ticks per second (fixed constant across all FBX versions).
const KTIME_PER_SECOND: f64 = 4_618_615_8000.0;

pub(crate) fn import_fbx(path: &Path) -> Result<Project> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let tree = match AnyTree::from_seekable_reader(BufReader::new(file)) {
        Ok(AnyTree::V7400(_version, tree, _footer)) => tree,
        // `from_seekable_reader` only ever yields V7400 or an error today.
        Ok(_) => unreachable!("fbxcel 0.9 has a single parser version"),
        Err(e) => bail!(
            "FBX {}: {e} — unterstützt wird binäres FBX ab 7.4 (kein ASCII-FBX, \
             kein vollständig komprimiertes FBX)",
            path.display()
        ),
    };

    let mut p = Project::new(
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("imported"),
    );
    let mut imp = Importer::default();
    imp.collect_objects(&tree);
    imp.collect_connections(&tree);
    imp.read_settings(&tree);

    // Z-up files (Maya default) get one fix-up node above every scene root.
    let root_parent = if imp.up_axis == 2 {
        let q = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        Some(p.scene.insert(
            None,
            WNode {
                name: "FBX (Z-up fix)".into(),
                transform: Transform {
                    translation: [0.0; 3],
                    rotation: [q.x, q.y, q.z, q.w],
                    scale: [1.0; 3],
                },
                kind: NodeKind::Empty,
                ..Default::default()
            },
        ))
    } else {
        None
    };

    // Pass 1: node hierarchy + mesh/material assets.
    let roots: Vec<i64> = imp
        .conns
        .iter()
        .filter(|c| c.ptype == "OO" && c.owner == 0)
        .map(|c| c.subject)
        .filter(|id| imp.kind(*id) == Some("Model"))
        .collect();
    for root in roots {
        imp.convert_model(root, root_parent, &mut p, 0)?;
    }

    // Passes 2–3: skins and weights; pass 4: animation takes.
    imp.build_skins(&mut p)?;
    imp.attach_weights(&mut p);
    imp.build_animations(&mut p);

    Ok(p)
}

/// One parsed `Connections` line: "subject belongs to owner".
#[derive(Debug, Clone)]
struct Conn {
    ptype: String,
    subject: i64,
    owner: i64,
    label: String,
}

#[derive(Default)]
struct Importer<'a> {
    /// object id -> node name ("Model", "Geometry", "Deformer", ...)
    kinds: HashMap<i64, &'a str>,
    /// object id -> type string (attr[2]: "Mesh", "Skin", "Cluster", ...)
    subs: HashMap<i64, String>,
    /// object id -> display name (attr[1] before the "\0\x01" separator)
    names: HashMap<i64, String>,
    /// object id -> tree node, for property lookups
    objs: HashMap<i64, Handle<'a>>,
    conns: Vec<Conn>,

    /// length-unit factor to metres
    unit: f64,
    /// GlobalSettings UpAxis (1 = Y, 2 = Z)
    up_axis: i64,

    our_node: HashMap<i64, NodeId>,
    world_of: HashMap<i64, Transform>,
    parent_model: HashMap<i64, i64>,
    /// model id -> mesh asset produced in pass 1
    model_mesh: HashMap<i64, MeshId>,
    /// material node-attribute id -> cached material asset
    mat_assets: HashMap<i64, MaterialId>,

    /// skin deformer id -> (skeleton asset, instance node, cluster id -> joint index)
    skin_map: HashMap<i64, (SkeletonId, NodeId, HashMap<i64, usize>)>,
    /// bone model id -> (skin id, joint index) for animation retargeting
    joints_of_node: HashMap<i64, (i64, usize)>,
}

// ------------------------------------------------------------------ attrs

fn a_str<'a>(h: &Handle<'a>, i: usize) -> Option<&'a str> {
    match h.attributes().get(i) {
        Some(AttributeValue::String(s)) => Some(s),
        _ => None,
    }
}

fn a_i64(h: &Handle<'_>, i: usize) -> Option<i64> {
    match h.attributes().get(i) {
        Some(AttributeValue::I64(v)) => Some(*v),
        Some(AttributeValue::I32(v)) => Some(*v as i64),
        _ => None,
    }
}

/// Array attribute widened to f64 (`f`/`d` arrays both accepted).
fn a_arr_f64(h: &Handle<'_>, i: usize) -> Option<Vec<f64>> {
    match h.attributes().get(i) {
        Some(AttributeValue::ArrF64(v)) => Some(v.clone()),
        Some(AttributeValue::ArrF32(v)) => Some(v.iter().map(|x| *x as f64).collect()),
        _ => None,
    }
}

fn a_arr_i32(h: &Handle<'_>, i: usize) -> Option<Vec<i32>> {
    match h.attributes().get(i) {
        Some(AttributeValue::ArrI32(v)) => Some(v.clone()),
        Some(AttributeValue::ArrI64(v)) => Some(v.iter().map(|x| *x as i32).collect()),
        _ => None,
    }
}

fn a_arr_i64(h: &Handle<'_>, i: usize) -> Option<Vec<i64>> {
    match h.attributes().get(i) {
        Some(AttributeValue::ArrI64(v)) => Some(v.clone()),
        Some(AttributeValue::ArrI32(v)) => Some(v.iter().map(|x| *x as i64).collect()),
        _ => None,
    }
}

/// Direct child with the exact node name; object values live in attr 0.
fn child<'a>(h: &Handle<'a>, name: &str) -> Option<Handle<'a>> {
    h.children().find(|c| c.name() == name)
}

/// `Properties70` map: property name -> value attributes (header skipped).
fn props70<'h>(h: &Handle<'h>) -> HashMap<&'h str, &'h [AttributeValue]> {
    let mut m = HashMap::new();
    if let Some(p70) = child(h, "Properties70") {
        for e in p70.children() {
            if e.name() != "P" || e.attributes().len() < 5 {
                continue;
            }
            if let Some(AttributeValue::String(name)) = e.attributes().first() {
                m.insert(name.as_str(), &e.attributes()[4..]);
            }
        }
    }
    m
}

fn p_num(map: &HashMap<&str, &[AttributeValue]>, key: &str) -> Option<f64> {
    match map.get(key)?.first() {
        Some(AttributeValue::F64(v)) => Some(*v),
        Some(AttributeValue::F32(v)) => Some(*v as f64),
        Some(AttributeValue::I32(v)) => Some(*v as f64),
        Some(AttributeValue::I64(v)) => Some(*v as f64),
        Some(AttributeValue::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn p_vec3(map: &HashMap<&str, &[AttributeValue]>, key: &str) -> Option<[f64; 3]> {
    let v = map.get(key)?;
    let g = |i: usize| match v.get(i)? {
        AttributeValue::F64(x) => Some(*x),
        AttributeValue::F32(x) => Some(*x as f64),
        AttributeValue::I32(x) => Some(*x as f64),
        AttributeValue::I64(x) => Some(*x as f64),
        _ => None,
    };
    Some([g(0)?, g(1)?, g(2)?])
}

fn p_str<'m>(map: &'m HashMap<&str, &[AttributeValue]>, key: &str) -> Option<&'m str> {
    match map.get(key)?.first()? {
        AttributeValue::String(s) => Some(s),
        _ => None,
    }
}

impl<'a> Importer<'a> {
    fn obj_name(&self, id: i64) -> String {
        self.names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("object_{id}"))
    }

    fn kind(&self, id: i64) -> Option<&'a str> {
        self.kinds.get(&id).copied()
    }

    fn sub(&self, id: i64) -> Option<&str> {
        self.subs.get(&id).map(|s| s.as_str())
    }

    fn collect_objects(&mut self, tree: &'a Tree) {
        let Some(objects) = child(&tree.root(), "Objects") else {
            return;
        };
        for obj in objects.children() {
            let Some(id) = a_i64(&obj, 0) else { continue };
            self.kinds.insert(id, obj.name());
            self.subs.insert(id, a_str(&obj, 2).unwrap_or("").to_string());
            let raw = a_str(&obj, 1).unwrap_or("");
            // display name = part before the "\0\x01" class separator
            let name = raw.split('\0').next().unwrap_or("");
            let name = if name.is_empty() { raw } else { name };
            self.names.insert(id, name.to_string());
            self.objs.insert(id, obj);
        }
    }

    fn collect_connections(&mut self, tree: &'a Tree) {
        let Some(conns) = child(&tree.root(), "Connections") else {
            return;
        };
        for c in conns.children() {
            if c.name() != "C" {
                continue;
            }
            let (Some(ptype), Some(subject), Some(owner)) = (
                a_str(&c, 0).map(|s| s.to_string()),
                a_i64(&c, 1),
                a_i64(&c, 2),
            ) else {
                continue;
            };
            self.conns.push(Conn {
                ptype,
                subject,
                owner,
                label: a_str(&c, 3).unwrap_or("").to_string(),
            });
        }
    }

    fn read_settings(&mut self, tree: &'a Tree) {
        // Defaults: metre, Y-up. "-unit" overrides the factor when present.
        self.unit = 1.0;
        self.up_axis = 1;
        let Some(gs) = child(&tree.root(), "GlobalSettings") else {
            return;
        };
        let m = props70(&gs);
        if let Some(unit) = p_str(&m, "-unit") {
            self.unit = match unit.trim() {
                "mm" => 0.001,
                "cm" => 0.01,
                "m" => 1.0,
                "km" => 1000.0,
                "in" => 0.0254,
                "ft" | "'" => 0.3048,
                "yd" => 0.9144,
                _ => 1.0,
            };
        }
        if let Some(axis) = p_num(&m, "UpAxis") {
            self.up_axis = axis as i64;
        }
    }

    /// Transform from `Lcl Translation/Rotation/Scaling` (translation
    /// scaled to metres, rotation degrees -> quaternion in XYZ order).
    fn lcl_transform(&self, h: &Handle<'_>) -> Transform {
        let m = props70(h);
        let t = p_vec3(&m, "Lcl Translation").unwrap_or([0.0; 3]);
        let r = p_vec3(&m, "Lcl Rotation").unwrap_or([0.0; 3]);
        let mut s = p_vec3(&m, "Lcl Scaling").unwrap_or([1.0; 3]);
        for v in &mut s {
            if *v == 0.0 {
                *v = 1.0; // Maya quirk: zero scale means "unset"
            }
        }
        let q = Quat::from_euler(
            glam::EulerRot::XYZ,
            (r[0] as f32).to_radians(),
            (r[1] as f32).to_radians(),
            (r[2] as f32).to_radians(),
        );
        Transform {
            translation: [
                (t[0] * self.unit) as f32,
                (t[1] * self.unit) as f32,
                (t[2] * self.unit) as f32,
            ],
            rotation: [q.x, q.y, q.z, q.w],
            scale: [s[0] as f32, s[1] as f32, s[2] as f32],
        }
    }

    /// Owners of `subject` with the given connection type + label filter.
    fn owners(&self, subject: i64, ptype: &str, label: Option<&str>) -> Vec<i64> {
        self.conns
            .iter()
            .filter(|c| {
                c.subject == subject
                    && c.ptype == ptype
                    && label.map_or(true, |l| c.label == l)
            })
            .map(|c| c.owner)
            .collect()
    }

    fn subjects(&self, owner: i64, ptype: &str) -> Vec<i64> {
        self.conns
            .iter()
            .filter(|c| c.owner == owner && c.ptype == ptype)
            .map(|c| c.subject)
            .collect()
    }

    // ------------------------------------------------------------- pass 1

    fn convert_model(
        &mut self,
        id: i64,
        parent: Option<NodeId>,
        p: &mut Project,
        depth: u32,
    ) -> Result<()> {
        if depth > 128 || self.our_node.contains_key(&id) {
            // cycle or duplicate parenting — import the model only once
            return Ok(());
        }
        let h = match self.objs.get(&id) {
            Some(h) => h.clone(),
            None => return Ok(()),
        };
        let local = self.lcl_transform(&h);
        let world = match parent {
            Some(pn) => p.scene.world_transform(pn).then(&local),
            None => local,
        };
        self.world_of.insert(id, world);

        let kind = if self.sub(id) == Some("Mesh") {
            let mesh_id = self.build_mesh(id, p)?;
            let mat = self.material_for_model(id, p);
            self.model_mesh.insert(id, mesh_id);
            NodeKind::MeshInstance {
                mesh: mesh_id,
                material: mat,
                skeleton_node: None,
            }
        } else {
            NodeKind::Empty
        };
        let node_id = p.scene.insert(
            parent,
            WNode {
                name: self.obj_name(id),
                transform: local,
                kind,
                ..Default::default()
            },
        );
        self.our_node.insert(id, node_id);

        // child models: subjects of OO connections owned by this model
        for kid in self.subjects(id, "OO") {
            if self.kind(kid) == Some("Model") {
                self.parent_model.insert(kid, id);
                self.convert_model(kid, Some(node_id), p, depth + 1)?;
            }
        }
        Ok(())
    }

    /// Mesh asset for a Mesh model, built from its attached Geometry.
    fn build_mesh(&mut self, model_id: i64, p: &mut Project) -> Result<MeshId> {
        let geo_id = self
            .subjects(model_id, "OO")
            .into_iter()
            .find(|g| self.kind(*g) == Some("Geometry"))
            .ok_or_else(|| anyhow!("FBX mesh model {} has no Geometry", self.obj_name(model_id)))?;
        let geo = self
            .objs
            .get(&geo_id)
            .cloned()
            .ok_or_else(|| anyhow!("FBX geometry {geo_id} missing"))?;

        let verts = child(&geo, "Vertices")
            .and_then(|v| a_arr_f64(&v, 0))
            .ok_or_else(|| anyhow!("FBX geometry {} without Vertices", self.obj_name(geo_id)))?;
        let positions: Vec<[f32; 3]> = verts
            .chunks_exact(3)
            .map(|c| {
                [
                    (c[0] * self.unit) as f32,
                    (c[1] * self.unit) as f32,
                    (c[2] * self.unit) as f32,
                ]
            })
            .collect();
        if positions.is_empty() {
            bail!("FBX geometry {} has no vertices", self.obj_name(geo_id));
        }

        // PolygonVertexIndex: a negative value closes the polygon (!v = last).
        let pvi = child(&geo, "PolygonVertexIndex")
            .and_then(|c| a_arr_i32(&c, 0))
            .unwrap_or_default();
        let mut indices: Vec<u32> = Vec::new();
        let mut poly: Vec<u32> = Vec::new();
        for v in &pvi {
            if *v < 0 {
                poly.push((!v) as u32);
                for i in 1..poly.len().saturating_sub(1) {
                    indices.extend_from_slice(&[poly[0], poly[i], poly[i + 1]]);
                }
                poly.clear();
            } else {
                poly.push(*v as u32);
            }
        }

        let mut wmesh = WMesh {
            name: self.obj_name(geo_id),
            positions,
            normals: self.layer_vec3(&geo, "LayerElementNormal"),
            uvs: self.layer_vec2(&geo),
            indices: (!indices.is_empty()).then_some(indices),
            ..Default::default()
        };
        if wmesh.normals.is_none() {
            wmesh.compute_normals();
        }
        Ok(p.library.add_mesh(wmesh))
    }

    /// First `LayerElementNormal` collapsed to per-vertex normals.
    /// ByPolygon/ByEdge mappings are dropped (the caller recomputes).
    fn layer_vec3(&self, geo: &Handle<'_>, element: &str) -> Option<Vec<[f32; 3]>> {
        let el = child(geo, element)?;
        let n_verts = child(geo, "Vertices")
            .and_then(|v| a_arr_f64(&v, 0))
            .map(|v| v.len() / 3)?;
        let flat = child(&el, "Normals").and_then(|c| a_arr_f64(&c, 0))?;
        let values: Vec<[f32; 3]> = flat
            .chunks_exact(3)
            .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
            .collect();
        if values.is_empty() {
            return None;
        }
        let mapping = child(&el, "MappingInformationType")
            .and_then(|c| a_str(&c, 0).map(|s| s.to_string()))
            .unwrap_or_else(|| "ByPolygonVertex".into());
        if mapping == "AllSame" {
            return Some(vec![values[0]; n_verts]);
        }
        if mapping != "ByPolygonVertex" && mapping != "ByVertex" {
            return None;
        }
        Some(collapse_corners(geo, &values, n_verts))
    }

    /// First `LayerElementUV` collapsed to per-vertex UVs (UVIndex-aware).
    fn layer_vec2(&self, geo: &Handle<'_>) -> Option<Vec<[f32; 2]>> {
        let el = child(geo, "LayerElementUV")?;
        let n_verts = child(geo, "Vertices")
            .and_then(|v| a_arr_f64(&v, 0))
            .map(|v| v.len() / 3)?;
        let table = child(&el, "UV").and_then(|c| a_arr_f64(&c, 0))?;
        let table: Vec<[f32; 2]> = table
            .chunks_exact(2)
            .map(|c| [c[0] as f32, c[1] as f32])
            .collect();
        if table.is_empty() {
            return None;
        }
        let corners: Vec<[f32; 2]> = match child(&el, "UVIndex").and_then(|c| a_arr_i32(&c, 0)) {
            Some(ids) => ids
                .iter()
                .map(|i| table.get(*i as usize).copied().unwrap_or([0.0; 2]))
                .collect(),
            None => table,
        };
        let mapping = child(&el, "MappingInformationType")
            .and_then(|c| a_str(&c, 0).map(|s| s.to_string()))
            .unwrap_or_else(|| "ByPolygonVertex".into());
        if mapping == "AllSame" {
            return Some(vec![corners[0]; n_verts]);
        }
        if mapping != "ByPolygonVertex" && mapping != "ByPolygon" {
            return None;
        }
        Some(collapse_corners(geo, &corners, n_verts))
    }

    /// Material for a mesh model: NodeAttribute(Material) owned by model.
    fn material_for_model(&mut self, model_id: i64, p: &mut Project) -> Option<MaterialId> {
        let na = self
            .subjects(model_id, "OO")
            .into_iter()
            .find(|o| self.kind(*o) == Some("NodeAttribute") && self.sub(*o) == Some("Material"))?;
        if let Some(id) = self.mat_assets.get(&na) {
            return Some(*id);
        }
        let h = self.objs.get(&na)?.clone();
        let m = props70(&h);
        let diff = p_vec3(&m, "DiffuseColor").unwrap_or([0.8; 3]);
        let dfac = p_num(&m, "DiffuseFactor").unwrap_or(1.0).clamp(0.0, 1.0) as f32;
        let alpha = (p_num(&m, "Opacity").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0) as f32;
        let emis = p_vec3(&m, "EmissiveColor").unwrap_or([0.0; 3]);
        let shin = p_num(&m, "Shininess").unwrap_or(0.0);
        let double_sided = p_num(&m, "DoubleSided").unwrap_or(1.0) != 0.0;
        // FBX predates PBR: no metallic channel, the classic specular
        // exponent maps to a roughness estimate only.
        let mat = re_core::material::Material {
            name: self.obj_name(na),
            base_color: [
                (diff[0] as f32) * dfac,
                (diff[1] as f32) * dfac,
                (diff[2] as f32) * dfac,
                alpha,
            ],
            metallic: 0.0,
            roughness: (1.0 - (shin as f32 / 128.0).clamp(0.0, 1.0)).clamp(0.05, 1.0),
            emissive: [emis[0] as f32, emis[1] as f32, emis[2] as f32],
            double_sided,
            albedo_texture_id: self.texture_file_of(na, "DiffuseColor"),
            normal_texture_id: self
                .texture_file_of(na, "NormalMap")
                .or_else(|| self.texture_file_of(na, "Bump")),
        };
        let id = p.library.add_material(mat);
        self.mat_assets.insert(na, id);
        Some(id)
    }

    /// Relative (or plain) filename of the texture linked to a material
    /// node attribute under the given usage label.
    fn texture_file_of(&self, na: i64, usage: &str) -> Option<String> {
        let tex = self
            .conns
            .iter()
            .find(|c| {
                c.owner == na
                    && c.ptype == "OO"
                    && c.label == usage
                    && self.kind(c.subject) == Some("Texture")
            })?
            .subject;
        let h = self.objs.get(&tex)?;
        let rel = child(h, "RelativeFilename").and_then(|c| a_str(&c, 0).map(|s| s.to_string()))?;
        if !rel.trim().is_empty() {
            return Some(rel);
        }
        let full = child(h, "FileName").and_then(|c| a_str(&c, 0).map(|s| s.to_string()))?;
        Path::new(&full)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
    }

    // ------------------------------------------------------------- skins

    fn build_skins(&mut self, p: &mut Project) -> Result<()> {
        let mut skin_ids: Vec<i64> = self
            .objs
            .keys()
            .filter(|id| self.kind(**id) == Some("Deformer") && self.sub(**id) == Some("Skin"))
            .copied()
            .collect();
        skin_ids.sort();
        for skin in skin_ids {
            let Some(geo) = self
                .owners(skin, "OO", None)
                .into_iter()
                .find(|g| self.kind(*g) == Some("Geometry"))
            else {
                continue;
            };
            let model = self
                .owners(geo, "OO", None)
                .into_iter()
                .find(|s| self.kind(*s) == Some("Model"));
            let clusters = self.clusters(skin);
            if clusters.is_empty() {
                continue;
            }
            // bone model per cluster (empty entries keep cluster alignment)
            let bones: Vec<Option<i64>> = clusters
                .iter()
                .map(|c| {
                    self.subjects(*c, "OO")
                        .into_iter()
                        .find(|s| self.kind(*s) == Some("Model"))
                })
                .collect();
            if bones.iter().all(Option::is_none) {
                continue;
            }

            // instance attachment: scene node of the root bone's parent
            let root_bone = bones.iter().copied().flatten().next();
            let instance_parent = root_bone
                .and_then(|b| self.parent_model.get(&b).copied())
                .and_then(|pm| self.our_node.get(&pm).copied());

            let mut skel = Skeleton::new(self.obj_name(skin));
            let mut cluster_to_joint: HashMap<i64, usize> = HashMap::new();
            let base_inv = model
                .and_then(|m| self.world_of.get(&m).cloned())
                .map(|t| t.to_mat4().inverse())
                .unwrap_or(Mat4::IDENTITY);
            for (ci, cluster) in clusters.iter().enumerate() {
                let Some(bone) = bones[ci] else { continue };
                // nearest same-skin joint ancestor defines the hierarchy parent
                let mut anc = self.parent_model.get(&bone).copied();
                let mut parent_ji = None;
                let mut anc_inv = base_inv;
                while let Some(a) = anc {
                    if let Some((s, pai)) = self.joints_of_node.get(&a) {
                        if *s == skin {
                            parent_ji = Some(*pai);
                            anc_inv = self
                                .world_of
                                .get(&a)
                                .map(|t| t.to_mat4().inverse())
                                .unwrap_or(Mat4::IDENTITY);
                            break;
                        }
                    }
                    anc = self.parent_model.get(&a).copied();
                }
                let world = self
                    .world_of
                    .get(&bone)
                    .map(|t| t.to_mat4())
                    .unwrap_or(Mat4::IDENTITY);
                let local = Transform::from_mat4(&(anc_inv * world)).unwrap_or_default();
                let ji = skel.add_joint(self.obj_name(bone), parent_ji, local);
                cluster_to_joint.insert(*cluster, ji);
                self.joints_of_node.insert(bone, (skin, ji));
            }
            if skel.joints.is_empty() {
                continue;
            }

            let skel_id = p.library.add_skeleton(skel);
            let rig_name = root_bone
                .map(|b| self.obj_name(b))
                .unwrap_or_else(|| "skin".into());
            let inst = p.scene.insert(
                instance_parent,
                WNode {
                    name: format!("{rig_name} (rig)"),
                    kind: NodeKind::SkeletonInstance { skeleton: skel_id },
                    ..Default::default()
                },
            );
            // hide the original bone nodes; their transforms live in the skeleton
            for b in bones.iter().flatten() {
                if let Some(&nid) = self.our_node.get(b) {
                    if let Some(n) = p.scene.get_mut(nid) {
                        n.visible = false;
                    }
                }
            }
            self.skin_map.insert(skin, (skel_id, inst, cluster_to_joint));
        }
        Ok(())
    }

    /// Cluster deformers of a skin, in connection (file) order, deduped.
    fn clusters(&self, skin: i64) -> Vec<i64> {
        let mut out: Vec<i64> = Vec::new();
        for s in self.subjects(skin, "OO") {
            if self.kind(s) == Some("Deformer")
                && self.sub(s) == Some("Cluster")
                && !out.contains(&s)
            {
                out.push(s);
            }
        }
        out
    }

    fn attach_weights(&mut self, p: &mut Project) {
        let mut skins: Vec<i64> = self.skin_map.keys().copied().collect();
        skins.sort();
        for skin in skins {
            let (skel_id, inst, cluster_to_joint) = self.skin_map[&skin].clone();
            let Some(geo) = self
                .owners(skin, "OO", None)
                .into_iter()
                .find(|g| self.kind(*g) == Some("Geometry"))
            else {
                continue;
            };
            let mut models: Vec<i64> = self
                .owners(geo, "OO", None)
                .into_iter()
                .filter(|s| self.model_mesh.contains_key(s))
                .collect();
            models.sort();
            models.dedup();
            let clusters: Vec<i64> = self
                .clusters(skin)
                .into_iter()
                .filter(|c| cluster_to_joint.contains_key(c))
                .collect();
            if models.is_empty() || clusters.is_empty() {
                continue;
            }

            // per-vertex influence lists gathered from all clusters
            let n_verts = p
                .library
                .meshes
                .get(&self.model_mesh[&models[0]])
                .map(|m| m.positions.len())
                .unwrap_or(0);
            let mut acc: Vec<Vec<(u16, f32)>> = vec![Vec::new(); n_verts];
            for cluster in &clusters {
                let Some(&ji) = cluster_to_joint.get(cluster) else { continue };
                let Some(h) = self.objs.get(cluster) else { continue };
                let (Some(idx), Some(w)) = (
                    child(h, "Indexes").and_then(|c| a_arr_i32(&c, 0)),
                    child(h, "Weights").and_then(|c| a_arr_f64(&c, 0)),
                ) else {
                    continue;
                };
                for (vi, wt) in idx.iter().zip(w.iter()) {
                    if *wt <= 1e-6 || ji > u16::MAX as usize {
                        continue;
                    }
                    if let Some(list) = acc.get_mut(*vi as usize) {
                        list.push((ji as u16, *wt as f32));
                    }
                }
            }
            let mut joints = Vec::with_capacity(n_verts);
            let mut weights = Vec::with_capacity(n_verts);
            for mut list in acc {
                // keep the four heaviest influences, normalized
                list.sort_by(|a, b| b.1.total_cmp(&a.1));
                list.truncate(4);
                let sum: f32 = list.iter().map(|(_, w)| *w).sum();
                if list.is_empty() || sum <= 1e-9 {
                    joints.push([0; 4]);
                    weights.push([1.0, 0.0, 0.0, 0.0]);
                    continue;
                }
                let mut j = [0u16; 4];
                let mut w4 = [0.0f32; 4];
                for (i, (jj, ww)) in list.into_iter().enumerate() {
                    j[i] = jj;
                    w4[i] = ww / sum;
                }
                joints.push(j);
                weights.push(w4);
            }

            for model in models {
                let Some(&mid) = self.model_mesh.get(&model) else { continue };
                let node_id = self.our_node[&model];
                // shared geometry with several skinned instances: private copy
                let target = if p.library.meshes.get(&mid).and_then(|m| m.skin.as_ref()).is_some() {
                    match p.library.duplicate_mesh(mid) {
                        Ok(new_id) => {
                            if let Some(NodeKind::MeshInstance { mesh, .. }) =
                                p.scene.get_mut(node_id).map(|n| &mut n.kind)
                            {
                                *mesh = new_id;
                            }
                            new_id
                        }
                        Err(_) => continue,
                    }
                } else {
                    mid
                };
                if let Some(m) = p.library.meshes.get_mut(&target) {
                    m.skin = Some(Skin {
                        skeleton: skel_id,
                        joints: joints.clone(),
                        weights: weights.clone(),
                    });
                }
                if let Some(NodeKind::MeshInstance { skeleton_node, .. }) =
                    p.scene.get_mut(node_id).map(|n| &mut n.kind)
                {
                    *skeleton_node = Some(inst);
                }
            }
        }
    }

    // -------------------------------------------------------- animations

    fn build_animations(&mut self, p: &mut Project) {
        let mut stacks: Vec<i64> = self
            .objs
            .keys()
            .filter(|id| self.kind(**id) == Some("AnimationStack"))
            .copied()
            .collect();
        stacks.sort();
        for stack in stacks {
            let mut clip = Animation::new(self.obj_name(stack));
            // Maya inserts AnimationLayer nodes between stack and curve nodes.
            let mut nodes: Vec<i64> = Vec::new();
            let direct: Vec<i64> = self
                .subjects(stack, "OP")
                .into_iter()
                .chain(self.subjects(stack, "OO"))
                .collect();
            for s in direct {
                match self.kind(s) {
                    Some("AnimationCurveNode") => nodes.push(s),
                    Some("AnimationLayer") => {
                        for x in self
                            .subjects(s, "OP")
                            .into_iter()
                            .chain(self.subjects(s, "OO"))
                        {
                            if self.kind(x) == Some("AnimationCurveNode") && !nodes.contains(&x) {
                                nodes.push(x);
                            }
                        }
                    }
                    _ => {}
                }
            }
            for cnode in nodes {
                // the OP line to a Model carries the property label
                let Some((model, prop)) = self
                    .owners(cnode, "OP", None)
                    .into_iter()
                    .find(|o| self.kind(*o) == Some("Model"))
                    .map(|m| {
                        let label = self
                            .conns
                            .iter()
                            .find(|c| c.subject == cnode && c.owner == m)
                            .map(|c| c.label.clone())
                            .unwrap_or_default();
                        (m, label)
                    })
                else {
                    continue;
                };
                let path = match prop.as_str() {
                    "Lcl Translation" => ChannelPath::Translation,
                    "Lcl Rotation" => ChannelPath::Rotation,
                    "Lcl Scaling" => ChannelPath::Scale,
                    _ => continue, // Visibility, material props, ...: skipped
                };
                // component curves of this node: labels "d|X" / "X" / "d" ...
                let mut curves: Vec<AxisCurve> = Vec::new();
                for cid in self.subjects(cnode, "OP") {
                    if self.kind(cid) != Some("AnimationCurve") {
                        continue;
                    }
                    let label = self
                        .conns
                        .iter()
                        .find(|c| c.subject == cid && c.owner == cnode)
                        .map(|c| c.label.clone())
                        .unwrap_or_default();
                    let axis = match label.rsplit('|').next().unwrap_or("X") {
                        "Y" => 1usize,
                        "Z" => 2usize,
                        _ => 0usize,
                    };
                    let Some(h) = self.objs.get(&cid) else { continue };
                    // exporters vary: Maya writes the plain-float variant
                    let (Some(times), values) = (
                        child(h, "KeyTime").and_then(|c| a_arr_i64(&c, 0)),
                        child(h, "KeyValue")
                            .and_then(|c| a_arr_f64(&c, 0))
                            .or_else(|| child(h, "KeyValueFloat").and_then(|c| a_arr_f64(&c, 0)))
                            .unwrap_or_default(),
                    ) else {
                        continue;
                    };
                    if times.len() != values.len() || times.is_empty() {
                        continue;
                    }
                    curves.push(AxisCurve {
                        axis,
                        times: times.iter().map(|t| *t as f64 / KTIME_PER_SECOND).collect(),
                        values,
                    });
                }
                if curves.is_empty() {
                    continue;
                }
                let t0 = curves
                    .iter()
                    .flat_map(|c| c.times.iter().copied())
                    .fold(f64::INFINITY, f64::min);
                // reference timeline: the axis with the most keys
                let rt = curves
                    .iter()
                    .max_by_key(|c| c.times.len())
                    .map(|c| c.times.clone())
                    .unwrap_or_default();
                let mut ch = Channel::new(Target::Node(NodeId(0)), path);
                ch.interpolation = Interpolation::Linear;
                for i in 0..rt.len() {
                    let v = [
                        curve_value(&curves, 0, &rt, i),
                        curve_value(&curves, 1, &rt, i),
                        curve_value(&curves, 2, &rt, i),
                    ];
                    let kv = match path {
                        ChannelPath::Translation => {
                            KeyValue::Vec3([
                                (v[0] * self.unit) as f32,
                                (v[1] * self.unit) as f32,
                                (v[2] * self.unit) as f32,
                            ])
                        }
                        ChannelPath::Rotation => {
                            // FBX euler in degrees; the engine stores quaternions
                            let q = Quat::from_euler(
                                glam::EulerRot::XYZ,
                                (v[0] as f32).to_radians(),
                                (v[1] as f32).to_radians(),
                                (v[2] as f32).to_radians(),
                            );
                            KeyValue::Quat([q.x, q.y, q.z, q.w])
                        }
                        ChannelPath::Scale => KeyValue::Vec3([
                            if v[0] == 0.0 { 1.0 } else { v[0] as f32 },
                            if v[1] == 0.0 { 1.0 } else { v[1] as f32 },
                            if v[2] == 0.0 { 1.0 } else { v[2] as f32 },
                        ]),
                    };
                    ch.set_key((rt[i] - t0).max(0.0) as f32, kv);
                }
                if ch.keys.is_empty() {
                    continue;
                }
                ch.target = match self.joints_of_node.get(&model) {
                    Some((s, ji)) => {
                        let (skel_id, _, _) = self.skin_map[s];
                        let name = p
                            .library
                            .skeletons
                            .get(&skel_id)
                            .and_then(|sk| sk.joints.get(*ji))
                            .map(|j| j.name.clone())
                            .unwrap_or_else(|| format!("joint_{ji}"));
                        Target::Joint(name)
                    }
                    None => match self.our_node.get(&model) {
                        Some(n) => Target::Node(*n),
                        None => continue,
                    },
                };
                clip.channels.push(ch);
            }
            if !clip.channels.is_empty() {
                p.library.add_animation(clip);
            }
        }
    }
}

struct AxisCurve {
    axis: usize,
    times: Vec<f64>,
    values: Vec<f64>,
}

/// Value of `axis` at reference key `i`: direct if the curve owns that key
/// index, otherwise linearly resampled onto the reference times.
fn curve_value(curves: &[AxisCurve], axis: usize, rt: &[f64], i: usize) -> f64 {
    for c in curves {
        if c.axis != axis {
            continue;
        }
        if c.times.len() == rt.len() {
            return c.values.get(i).copied().unwrap_or(0.0);
        }
        let x = rt[i];
        let mut lo = 0usize;
        let mut hi = c.times.len() - 1;
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if c.times[mid] <= x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        if x <= c.times[lo] {
            return c.values[lo];
        }
        if x >= c.times[hi] {
            return c.values[hi];
        }
        let f = (x - c.times[lo]) / (c.times[hi] - c.times[lo]).max(1e-12);
        return c.values[lo] + (c.values[hi] - c.values[lo]) * f;
    }
    0.0
}

/// Collapse a per-polygon-corner attribute to one value per vertex
/// (first corner wins, following `PolygonVertexIndex`).
fn collapse_corners<T: Copy + Default>(geo: &Handle<'_>, corners: &[T], n_verts: usize) -> Vec<T> {
    let mut out = vec![T::default(); n_verts];
    let mut seen = vec![false; n_verts];
    let pvi = child(geo, "PolygonVertexIndex").and_then(|c| a_arr_i32(&c, 0));
    match pvi {
        Some(pvi) => {
            for (corner, v) in pvi.iter().enumerate() {
                let vi = if *v < 0 { (!*v) as usize } else { *v as usize };
                if vi < n_verts && !seen[vi] {
                    if let Some(val) = corners.get(corner) {
                        out[vi] = *val;
                        seen[vi] = true;
                    }
                }
            }
        }
        None => {
            // no topology: assume a plain 1:1 vertex layout
            for (o, c) in out.iter_mut().zip(corners.iter()) {
                *o = *c;
            }
        }
    }
    out
}
