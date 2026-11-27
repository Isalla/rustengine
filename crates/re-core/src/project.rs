use crate::animation::Animation;
use crate::id::{AnimationId, MaterialId, MeshId, SkeletonId, TileMapId, TileSetId};
use crate::material::Material;
use crate::mesh::Mesh;
use crate::scene::Scene;
use crate::skeleton::Skeleton;
use crate::tileset::{TileMap, TileSet};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Reusable asset store: everything a scene node can reference.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Library {
    pub meshes: BTreeMap<MeshId, Mesh>,
    pub skeletons: BTreeMap<SkeletonId, Skeleton>,
    pub animations: BTreeMap<AnimationId, Animation>,
    pub materials: BTreeMap<MaterialId, Material>,
    #[serde(default)]
    pub tilesets: BTreeMap<TileSetId, TileSet>,
    #[serde(default)]
    pub tilemaps: BTreeMap<TileMapId, TileMap>,
    pub(crate) next_mesh: u32,
    pub(crate) next_skeleton: u32,
    pub(crate) next_animation: u32,
    pub(crate) next_material: u32,
    #[serde(default)]
    pub(crate) next_tileset: u32,
    #[serde(default)]
    pub(crate) next_tilemap: u32,
}

impl Library {
    pub fn add_mesh(&mut self, mut mesh: Mesh) -> MeshId {
        let id = MeshId(self.next_mesh);
        self.next_mesh += 1;
        if mesh.name.is_empty() {
            mesh.name = format!("Mesh_{}", id);
        }
        self.meshes.insert(id, mesh);
        id
    }

    pub fn add_skeleton(&mut self, mut sk: Skeleton) -> SkeletonId {
        let id = SkeletonId(self.next_skeleton);
        self.next_skeleton += 1;
        if sk.name.is_empty() {
            sk.name = format!("Skeleton_{}", id);
        }
        self.skeletons.insert(id, sk);
        id
    }

    pub fn add_animation(&mut self, mut anim: Animation) -> AnimationId {
        let id = AnimationId(self.next_animation);
        self.next_animation += 1;
        if anim.name.is_empty() {
            anim.name = format!("Animation_{}", id);
        }
        self.animations.insert(id, anim);
        id
    }

    pub fn add_material(&mut self, mut mat: Material) -> MaterialId {
        let id = MaterialId(self.next_material);
        self.next_material += 1;
        if mat.name.is_empty() {
            mat.name = format!("Material_{}", id);
        }
        self.materials.insert(id, mat);
        id
    }

    pub fn add_tileset(&mut self, mut ts: TileSet) -> TileSetId {
        let id = TileSetId(self.next_tileset);
        self.next_tileset += 1;
        if ts.name.is_empty() {
            ts.name = format!("TileSet_{}", id);
        }
        self.tilesets.insert(id, ts);
        id
    }

    pub fn add_tilemap(&mut self, mut tm: TileMap) -> TileMapId {
        let id = TileMapId(self.next_tilemap);
        self.next_tilemap += 1;
        if tm.name.is_empty() {
            tm.name = format!("TileMap_{}", id);
        }
        self.tilemaps.insert(id, tm);
        id
    }

    /// Copy an asset under a fresh id (so instances can be edited independently).
    pub fn duplicate_mesh(&mut self, id: MeshId) -> Result<MeshId> {
        let m = self.meshes.get(&id).context("mesh not found")?.clone();
        Ok(self.add_mesh(m))
    }

    pub fn duplicate_skeleton(&mut self, id: SkeletonId) -> Result<SkeletonId> {
        let mut s = self.skeletons.get(&id).context("skeleton not found")?.clone();
        s.name = format!("{} (copy)", s.name);
        Ok(self.add_skeleton(s))
    }

    pub fn duplicate_animation(&mut self, id: AnimationId) -> Result<AnimationId> {
        let mut a = self.animations.get(&id).context("animation not found")?.clone();
        a.name = format!("{} (copy)", a.name);
        Ok(self.add_animation(a))
    }
}

/// The document: scene graph plus asset library. Saved as pretty JSON
/// (`.wbproj`) so projects stay diff-able and hand-editable.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub scene: Scene,
    pub library: Library,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            scene: Scene::new(),
            library: Library::default(),
        }
    }

    /// Serialization used by [`Self::save`]; the GUI compares it against the
    /// last saved state to detect unsaved changes (close dialog).
    pub fn fingerprint(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("create dir {}", dir.display()))?;
            }
        }
        let json = serde_json::to_string_pretty(self).context("serialize project")?;
        std::fs::write(path, json)
            .with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        let p: Project = serde_json::from_str(&text)
            .with_context(|| format!("parse {}", path.display()))?;
        if p.scene.nodes.is_empty() && p.library.meshes.is_empty() && p.name.is_empty() {
            bail!("file does not look like a WorldBuilder project");
        }
        p.validate_structure()
            .with_context(|| format!("validate {}", path.display()))?;
        Ok(p)
    }

    /// Structural sanity check for loaded files: parent/child cycles in the
    /// scene and joint parent cycles in skeletons would send the recursive
    /// helpers (`world_transform`, `walk`, `global_transform`) into endless
    /// recursion. Reject such files at load time instead of crashing later.
    pub fn validate_structure(&self) -> Result<()> {
        use crate::id::NodeId;
        use std::collections::BTreeSet;
        let nodes = &self.scene.nodes;
        // Parent chains must terminate (a cycle revisits a node).
        for (id, node) in nodes {
            let mut cur = node.parent;
            let mut steps = 0usize;
            while let Some(p) = cur {
                if p == *id || steps > nodes.len() {
                    bail!("scene node {id}: parent cycle");
                }
                match nodes.get(&p) {
                    Some(n) => cur = n.parent,
                    None => break, // dangling parent: treated as root boundary
                }
                steps += 1;
            }
        }
        // Children must form a forest over the roots: no cycles, no sharing
        // (a shared child would be walked twice and could hide a cycle).
        let mut visited: BTreeSet<NodeId> = BTreeSet::new();
        let mut stack: Vec<NodeId> = self.scene.roots.clone();
        while let Some(id) = stack.pop() {
            if !visited.insert(id) {
                bail!("scene node {id}: reachable twice (cycle or shared child)");
            }
            if let Some(n) = nodes.get(&id) {
                stack.extend(n.children.iter().copied());
            }
        }
        // Joint parent chains must terminate as well.
        for (sid, skel) in &self.library.skeletons {
            let n = skel.joints.len();
            for j in 0..n {
                let mut cur = skel.joints[j].parent;
                let mut steps = 0usize;
                while let Some(p) = cur {
                    if p == j || steps > n {
                        bail!("skeleton {sid}: joint parent cycle");
                    }
                    if p >= n {
                        break;
                    }
                    cur = skel.joints[p].parent;
                    steps += 1;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Node;
    use crate::transform::Transform;

    #[test]
    fn validate_struktur_erkennt_zyklen() {
        // Parent cycle: A is B's parent and B is A's parent.
        let mut p = Project::new("z");
        let a = p.scene.insert(None, Node { name: "A".into(), ..Default::default() });
        let b = p.scene.insert(Some(a), Node { name: "B".into(), ..Default::default() });
        p.scene.get_mut(a).unwrap().parent = Some(b);
        assert!(p.validate_structure().is_err(), "parent cycle must be rejected");

        // Children cycle without parent backlink, reachable from a root.
        let mut q = Project::new("z");
        let x = q.scene.insert(None, Node { name: "X".into(), ..Default::default() });
        let y = q.scene.insert(None, Node { name: "Y".into(), ..Default::default() });
        q.scene.get_mut(x).unwrap().children.push(y);
        q.scene.get_mut(y).unwrap().children.push(x);
        assert!(q.validate_structure().is_err(), "children cycle must be rejected");

        // Joint parent cycle in a skeleton.
        let mut s = Skeleton::new("s");
        s.joints.push(crate::skeleton::Joint { name: "j0".into(), parent: Some(1), local: Transform::default() });
        s.joints.push(crate::skeleton::Joint { name: "j1".into(), parent: Some(0), local: Transform::default() });
        let mut r = Project::new("z");
        r.library.skeletons.insert(crate::id::SkeletonId(7), s);
        assert!(r.validate_structure().is_err(), "joint cycle must be rejected");

        // A clean project passes.
        let clean = Project::new("z");
        assert!(clean.validate_structure().is_ok());
    }

    #[test]
    fn roundtrip_json() {
        let mut p = Project::new("test");
        let mesh = crate::primitives::cube(1.0, 1.0, 1.0);
        let m = p.library.add_mesh(mesh);
        let skel = p.library.add_skeleton(Skeleton::humanoid("rig"));
        let sk_node = p.scene.insert(None, Node {
            name: "rig".into(),
            kind: crate::scene::NodeKind::SkeletonInstance { skeleton: skel },
            ..Default::default()
        });
        p.scene.insert(None, Node {
            name: "box".into(),
            transform: Transform {
                translation: [1.0, 0.5, 0.0],
                ..Default::default()
            },
            kind: crate::scene::NodeKind::MeshInstance {
                mesh: m,
                material: None,
                skeleton_node: Some(sk_node),
            },
            ..Default::default()
        });
        let json = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
