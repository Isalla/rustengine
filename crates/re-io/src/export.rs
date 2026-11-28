//! glTF 2.0 / GLB writer built directly on `gltf-json`.
//!
//! Semantics:
//! - `SkeletonInstance` nodes are expanded into real glTF node hierarchies
//!   (one joint node per bone, bind-pose TRS) plus a `Skin` with inverse bind
//!   matrices. Meshes keep skin weights (`JOINTS_0`/`WEIGHTS_0`) — nothing is
//!   baked, so DCC tools can edit the rig.
//! - Every animation asset is emitted once per skeleton instance in the scene,
//!   resolving joint-name targets to that instance's joint nodes
//!   (retargeting happens at export time).

use anyhow::{bail, Context, Result};
use glam::Mat4;
use gltf_json as json;
use gltf_json::validation::Checked::Valid;
use gltf_json::validation::USize64;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use re_core::animation::{ChannelPath, Interpolation, KeyValue, Target};
use re_core::id::{MaterialId, MeshId, NodeId, SkeletonId};
use re_core::project::Project;
use re_core::scene::NodeKind;
use re_core::transform::Transform;

type JointKey = (NodeId, usize); // skeleton-instance node, joint index

// ------------------------------------------------------------- ctors
// Most gltf-json structs do not implement Default; keep it explicit.

fn mk_buffer(byte_length: usize, uri: Option<String>) -> json::Buffer {
    json::Buffer {
        byte_length: USize64::from(byte_length),
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        uri,
    }
}

fn mk_view(buffer: json::Index<json::Buffer>, byte_length: usize, byte_offset: usize, target: json::buffer::Target) -> json::buffer::View {
    json::buffer::View {
        buffer,
        byte_length: USize64::from(byte_length),
        byte_offset: Some(USize64::from(byte_offset)),
        byte_stride: None,
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        target: Some(Valid(target)),
    }
}

fn mk_accessor(
    view: json::Index<json::buffer::View>,
    count: usize,
    ct: json::accessor::ComponentType,
    ty: json::accessor::Type,
    min: Option<json::Value>,
    max: Option<json::Value>,
) -> json::Accessor {
    json::Accessor {
        buffer_view: Some(view),
        byte_offset: Some(USize64(0)),
        count: USize64::from(count),
        component_type: Valid(json::accessor::GenericComponentType(ct)),
        extensions: Default::default(),
        extras: Default::default(),
        type_: Valid(ty),
        min,
        max,
        name: None,
        normalized: false,
        sparse: None,
    }
}

pub(crate) fn export_gltf(p: &Project, path: &Path) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("glb")
        .to_ascii_lowercase();
    if ext != "glb" && ext != "gltf" {
        bail!("extension must be .glb or .gltf, got '{ext}'");
    }
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir).with_context(|| format!("create dir {}", dir.display()))?;
        }
    }
    let glb = ext == "glb";

    let mut root = json::Root::default();
    let buffer = root.push(mk_buffer(
        0, // fixed up after buffers are written
        if glb {
            None
        } else {
            Some(format!(
                "{}.bin",
                path.file_stem().and_then(|s| s.to_str()).unwrap_or("scene")
            ))
        },
    ));

    let export_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut ex = Exporter {
        root,
        buffer,
        bytes: Vec::new(),
        node_of: HashMap::new(),
        joint_node: HashMap::new(),
        root_joints_of: HashMap::new(),
        skin_of: HashMap::new(),
        instances: Vec::new(),
        material_of: HashMap::new(),
        mesh_of: HashMap::new(),
        export_dir,
    };

    // Pre-pass: all skeleton instances (joint nodes + skins), independent of
    // traversal order so mesh nodes can reference skins in any order.
    p.scene.walk(|id, node| {
        if let NodeKind::SkeletonInstance { skeleton } = node.kind {
            ex.instances.push((id, skeleton));
        }
    });
    for (inst, skel) in ex.instances.clone() {
        ex.build_skeleton_instance(p, inst, skel)?;
    }

    // Pass 1: node hierarchy.
    let mut scene_nodes = Vec::new();
    for &r in &p.scene.roots {
        if let Some(idx) = ex.export_node(p, r)? {
            scene_nodes.push(json::Index::new(idx as u32));
        }
    }
    ex.root.push(json::Scene {
        extensions: Default::default(),
        extras: Default::default(),
        name: Some(p.name.clone()),
        nodes: scene_nodes,
    });

    // Pass 2: animations.
    ex.export_animations(p);

    ex.root.asset = json::Asset {
        copyright: None,
        generator: Some(format!("WorldBuilder {}", env!("CARGO_PKG_VERSION"))),
        min_version: None,
        version: "2.0".into(),
        extensions: Default::default(),
        extras: Default::default(),
    };

    let bl = ex.bytes.len();
    if let Some(buf) = ex.root.buffers.get_mut(buffer.value()) {
        buf.byte_length = USize64::from(bl);
    }

    if glb {
        write_glb(&ex.root, &ex.bytes, path)?;
    } else {
        let writer = fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
        json::serialize::to_writer_pretty(writer, &ex.root).context("write gltf json")?;
        let bin_path = path.with_extension("bin");
        fs::write(&bin_path, &ex.bytes)
            .with_context(|| format!("write {}", bin_path.display()))?;
    }
    Ok(())
}

struct Exporter {
    root: json::Root,
    buffer: json::Index<json::Buffer>,
    bytes: Vec<u8>,
    node_of: HashMap<NodeId, usize>,
    joint_node: HashMap<JointKey, usize>,
    /// instance node -> gltf node indices of the skeleton's root joints
    root_joints_of: HashMap<NodeId, Vec<usize>>,
    skin_of: HashMap<NodeId, usize>,
    instances: Vec<(NodeId, SkeletonId)>,
    material_of: HashMap<MaterialId, usize>,
    /// (mesh asset, material) -> glTF mesh index. Tilemap cells and repeated
    /// instances share one glTF mesh instead of duplicating the buffers.
    mesh_of: HashMap<(u32, Option<u32>), usize>,
    /// Base directory for resolving texture paths.
    export_dir: std::path::PathBuf,
}

impl Exporter {
    // ------------------------------------------------------------ buffer

    fn push_view(&mut self, target: json::buffer::Target, data: &[u8]) -> usize {
        while self.bytes.len() % 4 != 0 {
            self.bytes.push(0);
        }
        let offset = self.bytes.len();
        self.bytes.extend_from_slice(data);
        self.root
            .push(mk_view(self.buffer, data.len(), offset, target))
            .value()
    }

    fn push_accessor(
        &mut self,
        view: usize,
        count: usize,
        ct: json::accessor::ComponentType,
        ty: json::accessor::Type,
        min: Option<json::Value>,
        max: Option<json::Value>,
    ) -> usize {
        self.root
            .push(mk_accessor(
                json::Index::new(view as u32),
                count,
                ct,
                ty,
                min,
                max,
            ))
            .value()
    }

    fn add_f32(
        &mut self,
        floats: &[f32],
        ty: json::accessor::Type,
        count: usize,
        minmax: Option<([f32; 3], [f32; 3])>,
    ) -> usize {
        let view = self.push_view(json::buffer::Target::ArrayBuffer, f32_bytes(floats));
        let (min, max) = match minmax {
            Some((a, b)) => (
                Some(json::Value::from(a.to_vec())),
                Some(json::Value::from(b.to_vec())),
            ),
            None => (None, None),
        };
        self.push_accessor(view, count, json::accessor::ComponentType::F32, ty, min, max)
    }

    fn add_times(&mut self, times: &[f32]) -> usize {
        let view = self.push_view(json::buffer::Target::ArrayBuffer, f32_bytes(times));
        let min = times.first().copied();
        let max = times.last().copied();
        self.push_accessor(
            view,
            times.len(),
            json::accessor::ComponentType::F32,
            json::accessor::Type::Scalar,
            min.map(|v| json::Value::from(vec![v])),
            max.map(|v| json::Value::from(vec![v])),
        )
    }

    fn add_mat4s(&mut self, mats: &[Mat4]) -> usize {
        let mut data: Vec<f32> = Vec::with_capacity(mats.len() * 16);
        for m in mats {
            data.extend_from_slice(&m.to_cols_array());
        }
        let view = self.push_view(json::buffer::Target::ArrayBuffer, f32_bytes(&data));
        self.push_accessor(
            view,
            mats.len(),
            json::accessor::ComponentType::F32,
            json::accessor::Type::Mat4,
            None,
            None,
        )
    }

    fn add_indices(&mut self, ints: &[u32], fits16: bool) -> usize {
        let view = if fits16 {
            let data: Vec<u16> = ints.iter().map(|v| *v as u16).collect();
            self.push_view(json::buffer::Target::ElementArrayBuffer, u16_bytes(&data))
        } else {
            let bytes: Vec<u8> = ints.iter().flat_map(|v| v.to_le_bytes()).collect();
            self.push_view(json::buffer::Target::ElementArrayBuffer, &bytes)
        };
        self.push_accessor(
            view,
            ints.len(),
            if fits16 {
                json::accessor::ComponentType::U16
            } else {
                json::accessor::ComponentType::U32
            },
            json::accessor::Type::Scalar,
            None,
            None,
        )
    }

    fn add_u16x4(&mut self, data: &[[u16; 4]]) -> usize {
        let flat: Vec<u16> = data.iter().flatten().copied().collect();
        let view = self.push_view(json::buffer::Target::ArrayBuffer, u16_bytes(&flat));
        self.push_accessor(
            view,
            data.len(),
            json::accessor::ComponentType::U16,
            json::accessor::Type::Vec4,
            None,
            None,
        )
    }

    // ------------------------------------------------------------ pieces

    fn node_transform(t: &Transform) -> json::Node {
        json::Node {
            translation: Some(t.translation),
            rotation: Some(json::scene::UnitQuaternion(t.rotation)),
            scale: Some(t.scale),
            ..Default::default()
        }
    }

    fn material(&mut self, p: &Project, id: MaterialId) -> usize {
        if let Some(i) = self.material_of.get(&id) {
            return *i;
        }
        let m = p.library.materials.get(&id).cloned().unwrap_or_default();

        // Load textures if specified.
        let base_color_texture = m
            .albedo_texture_id
            .as_ref()
            .and_then(|path| self.load_texture(path));
        let normal_texture = m
            .normal_texture_id
            .as_ref()
            .and_then(|path| self.load_texture(path));

        let idx = self
            .root
            .push(json::Material {
                name: Some(m.name.clone()),
                double_sided: m.double_sided,
                emissive_factor: json::material::EmissiveFactor(clamp3(m.emissive)),
                normal_texture: normal_texture.map(|t| json::material::NormalTexture {
                    index: json::Index::new(t as u32),
                    scale: 1.0,
                    tex_coord: 0,
                    extensions: None,
                    extras: Default::default(),
                }),
                pbr_metallic_roughness: json::material::PbrMetallicRoughness {
                    base_color_factor: json::material::PbrBaseColorFactor(clamp4(m.base_color)),
                    metallic_factor: json::material::StrengthFactor(m.metallic.clamp(0.0, 1.0)),
                    roughness_factor: json::material::StrengthFactor(m.roughness.clamp(0.0, 1.0)),
                    base_color_texture: base_color_texture.map(|t| json::texture::Info {
                        index: json::Index::new(t as u32),
                        tex_coord: 0,
                        extensions: None,
                        extras: Default::default(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            })
            .value();
        self.material_of.insert(id, idx);
        idx
    }

    /// Load an image file and create a glTF texture. Returns the texture index.
    fn load_texture(&mut self, path: &str) -> Option<usize> {
        // Security: texture references come from project data (a loaded
        // .wbproj may be untrusted). Only plain relative paths are allowed -
        // absolute paths and `..` components could smuggle any file on disk
        // into the exported .glb.
        let rel = Path::new(path);
        if rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return None;
        }
        // Resolve path relative to the export file's parent directory.
        let full_path = self.export_dir.join(rel);
        let data = std::fs::read(&full_path).ok()?;
        let mime = if path.to_lowercase().ends_with(".png") {
            "image/png"
        } else if path.to_lowercase().ends_with(".jpg") || path.to_lowercase().ends_with(".jpeg") {
            "image/jpeg"
        } else {
            "image/png"
        };

        // Create image.
        let view = self.push_view(json::buffer::Target::ArrayBuffer, &data);
        let image_idx = self
            .root
            .push(json::Image {
                buffer_view: Some(json::Index::new(view as u32)),
                mime_type: Some(json::image::MimeType(mime.to_string())),
                uri: None,
                name: None,
                extensions: Default::default(),
                extras: Default::default(),
            })
            .value();

        // Create texture.
        let texture_idx = self
            .root
            .push(json::Texture {
                sampler: None,
                source: json::Index::new(image_idx as u32),
                name: None,
                extensions: Default::default(),
                extras: Default::default(),
            })
            .value();

        Some(texture_idx)
    }

    /// glTF mesh (single primitive) from a wb mesh asset.
    fn mesh(&mut self, p: &Project, mesh_id: MeshId, material: Option<MaterialId>) -> Result<usize> {
        let key = (mesh_id.0, material.map(|m| m.0));
        if let Some(&idx) = self.mesh_of.get(&key) {
            return Ok(idx);
        }
        let m = p
            .library
            .meshes
            .get(&mesh_id)
            .cloned()
            .context("mesh asset vanished")?;

        let (mn, mx) = m.aabb();
        let mut attributes: BTreeMap<
            json::validation::Checked<json::mesh::Semantic>,
            json::Index<json::Accessor>,
        > = BTreeMap::new();
        let flat3 = |v: &[[f32; 3]]| -> Vec<f32> { v.iter().flatten().copied().collect() };

        let positions =
            self.add_f32(&flat3(&m.positions), json::accessor::Type::Vec3, m.positions.len(), Some((mn, mx)));
        attributes.insert(Valid(json::mesh::Semantic::Positions), json::Index::new(positions as u32));

        if let Some(n) = &m.normals {
            let a = self.add_f32(&flat3(n), json::accessor::Type::Vec3, n.len(), None);
            attributes.insert(Valid(json::mesh::Semantic::Normals), json::Index::new(a as u32));
        }
        if let Some(uv) = &m.uvs {
            let flat: Vec<f32> = uv.iter().flatten().copied().collect();
            let a = self.add_f32(&flat, json::accessor::Type::Vec2, uv.len(), None);
            attributes.insert(Valid(json::mesh::Semantic::TexCoords(0)), json::Index::new(a as u32));
        }
        if let Some(skin_d) = &m.skin {
            let j = self.add_u16x4(&skin_d.joints);
            attributes.insert(Valid(json::mesh::Semantic::Joints(0)), json::Index::new(j as u32));
            let flat: Vec<f32> = skin_d.weights.iter().flatten().copied().collect();
            let w = self.add_f32(&flat, json::accessor::Type::Vec4, skin_d.weights.len(), None);
            attributes.insert(Valid(json::mesh::Semantic::Weights(0)), json::Index::new(w as u32));
        }

        let indices = m
            .indices
            .clone()
            .unwrap_or_else(|| (0..m.positions.len() as u32).collect());
        let fits16 = m.positions.len() <= u16::MAX as usize;
        let idx = self.add_indices(&indices, fits16);
        let mat_index = match material {
            Some(mid) => {
                let i = self.material(p, mid);
                Some(json::Index::new(i as u32))
            }
            None => None,
        };

        let mesh_idx = self
            .root
            .push(json::Mesh {
                extensions: Default::default(),
                extras: Default::default(),
                name: Some(m.name.clone()),
                primitives: vec![json::mesh::Primitive {
                    attributes,
                    extensions: Default::default(),
                    extras: Default::default(),
                    indices: Some(json::Index::new(idx as u32)),
                    material: mat_index,
                    mode: Valid(json::mesh::Mode::Triangles),
                    targets: None,
                }],
                weights: None,
            })
            .value();
        self.mesh_of.insert(key, mesh_idx);
        Ok(mesh_idx)
    }

    // ------------------------------------------------------------ skins

    /// Create glTF joint nodes + skin for one skeleton instance (pre-pass).
    fn build_skeleton_instance(
        &mut self,
        p: &Project,
        inst: NodeId,
        skel_id: SkeletonId,
    ) -> Result<()> {
        let skel = p
            .library
            .skeletons
            .get(&skel_id)
            .with_context(|| format!("skeleton asset {skel_id} missing"))?
            .clone();

        let mut joint_indices = vec![usize::MAX; skel.joints.len()];
        for ji in 0..skel.joints.len() {
            let mut jn = Self::node_transform(&skel.joints[ji].local);
            jn.name = Some(skel.joints[ji].name.clone());
            let idx = self.root.push(jn).value();
            joint_indices[ji] = idx;
            self.joint_node.insert((inst, ji), idx);
        }
        // Hierarchy: the instance node adopts root joints; joints parent each other.
        let mut roots = Vec::new();
        for ji in 0..skel.joints.len() {
            let child = json::Index::new(joint_indices[ji] as u32);
            match skel.joints[ji].parent {
                Some(parent) => {
                    self.root.nodes[joint_indices[parent]]
                        .children
                        .get_or_insert_with(Vec::new)
                        .push(child);
                }
                None => roots.push(joint_indices[ji]),
            }
        }
        self.root_joints_of.insert(inst, roots);

        let base = p.scene.world_transform(inst).to_mat4();
        let binds: Vec<Mat4> = (0..skel.joints.len())
            .map(|ji| (base * skel.global_transform(ji).to_mat4()).inverse())
            .collect();
        let ibm = self.add_mat4s(&binds);
        let root_joint = skel
            .joints
            .iter()
            .position(|j| j.parent.is_none())
            .map(|ji| json::Index::new(joint_indices[ji] as u32));

        let skin = self
            .root
            .push(json::Skin {
                extensions: Default::default(),
                extras: Default::default(),
                inverse_bind_matrices: Some(json::Index::new(ibm as u32)),
                joints: joint_indices
                    .iter()
                    .map(|i| json::Index::new(*i as u32))
                    .collect(),
                name: Some(skel.name.clone()),
                skeleton: root_joint,
            })
            .value();
        self.skin_of.insert(inst, skin);
        Ok(())
    }

    // -------------------------------------------------------------- scene

    fn export_node(&mut self, p: &Project, id: NodeId) -> Result<Option<usize>> {
        let node = p.scene.get(id).context("node vanished")?;
        if !node.visible {
            return Ok(None);
        }
        let mut out = Self::node_transform(&node.transform);
        out.name = Some(node.name.clone());

        let mut children: Vec<json::Index<json::scene::Node>> = Vec::new();

        match &node.kind {
            NodeKind::SkeletonInstance { .. } => {
                if let Some(roots) = self.root_joints_of.get(&id).cloned() {
                    children.extend(roots.iter().map(|i| json::Index::new(*i as u32)));
                }
            }
            NodeKind::MeshInstance {
                mesh,
                material,
                skeleton_node,
            } => {
                let (mesh, material, skeleton_node) = (*mesh, *material, *skeleton_node);
                let skinned = match skeleton_node {
                    Some(sn) => {
                        let has_skin = p
                            .library
                            .meshes
                            .get(&mesh)
                            .and_then(|m| m.skin.as_ref())
                            .is_some();
                        has_skin.then(|| self.skin_of.get(&sn).copied()).flatten()
                    }
                    None => None,
                };
                let m_idx = self.mesh(p, mesh, material)?;
                out.mesh = Some(json::Index::new(m_idx as u32));
                if let Some(skin) = skinned {
                    out.skin = Some(json::Index::new(skin as u32));
                }
            }
            NodeKind::TileMapInstance { tilemap } => {
                // glTF has no tilemaps: expand each cell into a child node
                // that reuses the tile's mesh asset (glTF nodes may share
                // meshes, so this stays compact).
                let map = p
                    .library
                    .tilemaps
                    .get(tilemap)
                    .context("tilemap asset missing")?
                    .clone();
                let ts = p
                    .library
                    .tilesets
                    .get(&map.tileset)
                    .context("tileset asset missing")?
                    .clone();
                for c in &map.cells {
                    let Some(tile) = ts.tiles.get(c.tile as usize) else {
                        continue;
                    };
                    let cell = ts.cell_transform(c.x, c.z).to_mat4() * tile.offset.to_mat4();
                    let Some(local) = Transform::from_mat4(&cell) else {
                        continue;
                    };
                    let mut child = Self::node_transform(&local);
                    child.name = Some(format!("{}@{},{}", node.name, c.x, c.z));
                    let m_idx = self.mesh(p, tile.mesh, tile.material)?;
                    child.mesh = Some(json::Index::new(m_idx as u32));
                    let ci = self.root.push(child).value();
                    children.push(json::Index::new(ci as u32));
                }
            }
            _ => {}
        }

        let kids: Vec<NodeId> = node.children.clone();
        for c in kids {
            if let Some(ci) = self.export_node(p, c)? {
                children.push(json::Index::new(ci as u32));
            }
        }
        if !children.is_empty() {
            out.children = Some(children);
        }
        let idx = self.root.push(out).value();
        self.node_of.insert(id, idx);
        Ok(Some(idx))
    }

    // --------------------------------------------------------- animations

    fn export_animations(&mut self, p: &Project) {
        let anim_ids: Vec<_> = p.library.animations.keys().copied().collect();

        for anim_id in anim_ids {
            let Some(anim) = p.library.animations.get(&anim_id).cloned() else {
                continue;
            };
            if anim.channels.is_empty() {
                continue;
            }

            // (animation name, [(exported gltf node index, channel)])
            let mut emissions: Vec<(String, Vec<(usize, re_core::animation::Channel)>)> = Vec::new();

            for &(inst_node, skel_id) in &self.instances {
                let Some(skel) = p.library.skeletons.get(&skel_id) else {
                    continue;
                };
                let resolved: Vec<_> = anim
                    .channels
                    .iter()
                    .filter_map(|ch| {
                        let (node_idx, bind) = match &ch.target {
                            Target::Joint(name) => {
                                let ji = skel.joint_by_name(name)?;
                                (*self.joint_node.get(&(inst_node, ji))?, skel.joints[ji].local)
                            }
                            Target::Node(n) if *n == inst_node => {
                                let t = p.scene.get(*n).map(|x| x.transform)?;
                                (*self.node_of.get(n)?, t)
                            }
                            Target::Node(_) => return None,
                        };
                        Some((node_idx, bake_channel(ch, &bind)))
                    })
                    .collect();
                if resolved.is_empty() {
                    continue;
                }
                let name = if self.instances.len() > 1 {
                    format!(
                        "{} [{}]",
                        anim.name,
                        p.scene.get(inst_node).map(|n| n.name.as_str()).unwrap_or("?")
                    )
                } else {
                    anim.name.clone()
                };
                emissions.push((name, resolved));
            }

            // Channels targeting plain nodes (not skeleton instances).
            let plain: Vec<(usize, re_core::animation::Channel)> = anim
                .channels
                .iter()
                .filter_map(|ch| match &ch.target {
                    Target::Node(n) => {
                        let bind = p.scene.get(*n)?.transform;
                        Some((*self.node_of.get(n)?, bake_channel(ch, &bind)))
                    }
                    Target::Joint(_) => None,
                })
                .filter(|(_, ch)| {
                    !matches!(&ch.target, Target::Node(n) if self.instances.iter().any(|(i, _)| i == n))
                })
                .collect();
            if !plain.is_empty() {
                emissions.push((anim.name.clone(), plain));
            }

            for (name, channels) in emissions {
                self.emit_animation(&name, &channels);
            }
        }
    }

    fn emit_animation(&mut self, name: &str, channels: &[(usize, re_core::animation::Channel)]) {
        let mut samplers: Vec<json::animation::Sampler> = Vec::new();
        let mut gl_channels = Vec::new();

        for (node_idx, ch) in channels {
            if ch.keys.is_empty() {
                continue;
            }
            let times: Vec<f32> = ch.keys.iter().map(|k| k.time.max(0.0)).collect();
            let input = self.add_times(&times);

            let output = match ch.path {
                ChannelPath::Translation | ChannelPath::Scale => {
                    let flat: Vec<f32> = ch
                        .keys
                        .iter()
                        .filter_map(|k| match k.value {
                            KeyValue::Vec3(v) => Some(v.to_vec()),
                            _ => None,
                        })
                        .flatten()
                        .collect();
                    if flat.len() != times.len() * 3 {
                        continue;
                    }
                    self.add_f32(&flat, json::accessor::Type::Vec3, times.len(), None)
                }
                ChannelPath::Rotation => {
                    let flat: Vec<f32> = ch
                        .keys
                        .iter()
                        .filter_map(|k| match k.value {
                            KeyValue::Quat(v) => Some(v.to_vec()),
                            _ => None,
                        })
                        .flatten()
                        .collect();
                    if flat.len() != times.len() * 4 {
                        continue;
                    }
                    self.add_f32(&flat, json::accessor::Type::Vec4, times.len(), None)
                }
            };

            let path = match ch.path {
                ChannelPath::Translation => json::animation::Property::Translation,
                ChannelPath::Rotation => json::animation::Property::Rotation,
                ChannelPath::Scale => json::animation::Property::Scale,
            };
            let interp = match ch.interpolation {
                Interpolation::Step => json::animation::Interpolation::Step,
                _ => json::animation::Interpolation::Linear,
            };

            samplers.push(json::animation::Sampler {
                extensions: Default::default(),
                extras: Default::default(),
                input: json::Index::new(input as u32),
                interpolation: Valid(interp),
                output: json::Index::new(output as u32),
            });
            gl_channels.push(json::animation::Channel {
                extensions: Default::default(),
                extras: Default::default(),
                sampler: json::Index::new((samplers.len() - 1) as u32),
                target: json::animation::Target {
                    extensions: Default::default(),
                    extras: Default::default(),
                    node: json::Index::new(*node_idx as u32),
                    path: Valid(path),
                },
            });
        }

        if gl_channels.is_empty() {
            return;
        }
        self.root.push(json::animation::Animation {
            extensions: Default::default(),
            extras: Default::default(),
            channels: gl_channels,
            name: Some(name.to_string()),
            samplers,
        });
    }
}

// ------------------------------------------------------------------- glb

fn write_glb(root: &json::Root, bin: &[u8], path: &Path) -> Result<()> {
    let mut json_bytes = json::serialize::to_string(root)
        .context("serialize gltf json")?
        .into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let mut bin_pad = bin.to_vec();
    while bin_pad.len() % 4 != 0 {
        bin_pad.push(0);
    }

    let has_bin = !bin_pad.is_empty();
    let mut total = 12 + 8 + json_bytes.len();
    if has_bin {
        total += 8 + bin_pad.len();
    }

    let mut out: Vec<u8> = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    if has_bin {
        out.extend_from_slice(&(bin_pad.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin_pad);
    }
    fs::write(path, out).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

// ----------------------------------------------------------------- utils

/// glTF has no additive animation space, so `RelativeToBind` channels are baked
/// into absolute local values on export: every key becomes `bind * delta`.
fn bake_channel(ch: &re_core::animation::Channel, bind: &Transform) -> re_core::animation::Channel {
    let mut out = ch.clone();
    if ch.space == re_core::animation::Space::Absolute {
        return out;
    }
    out.space = re_core::animation::Space::Absolute;
    for k in &mut out.keys {
        let mut delta = Transform::default();
        match ch.path {
            ChannelPath::Translation => {
                if let KeyValue::Vec3(v) = k.value {
                    delta.translation = v;
                }
            }
            ChannelPath::Rotation => {
                if let KeyValue::Quat(q) = k.value {
                    delta.rotation = q;
                }
            }
            ChannelPath::Scale => {
                if let KeyValue::Vec3(s) = k.value {
                    delta.scale = s;
                }
            }
        }
        let abs = bind.then(&delta);
        k.value = match ch.path {
            ChannelPath::Translation => KeyValue::Vec3(abs.translation),
            ChannelPath::Rotation => KeyValue::Quat(abs.rotation),
            ChannelPath::Scale => KeyValue::Vec3(abs.scale),
        };
    }
    out
}

fn f32_bytes(data: &[f32]) -> &[u8] {
    // SAFETY: plain f32 slice reinterpreted as bytes; alignment irrelevant for reads.
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4) }
}

fn u16_bytes(data: &[u16]) -> &[u8] {
    // SAFETY: see f32_bytes.
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2) }
}

fn clamp3(v: [f32; 3]) -> [f32; 3] {
    [v[0].clamp(0.0, 1.0), v[1].clamp(0.0, 1.0), v[2].clamp(0.0, 1.0)]
}

fn clamp4(v: [f32; 4]) -> [f32; 4] {
    [
        v[0].clamp(0.0, 1.0),
        v[1].clamp(0.0, 1.0),
        v[2].clamp(0.0, 1.0),
        v[3].clamp(0.0, 1.0),
    ]
}
