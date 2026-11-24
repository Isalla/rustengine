use crate::id::{MaterialId, MeshId, NodeId, SkeletonId, TileMapId};
use crate::transform::Transform;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    Empty,
    MeshInstance {
        mesh: MeshId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        material: Option<MaterialId>,
        /// Skeleton-instance node this mesh is skinned to (for display/export).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skeleton_node: Option<NodeId>,
    },
    /// Instantiates a skeleton asset; its joints form a virtual hierarchy.
    SkeletonInstance { skeleton: SkeletonId },
    /// Painted grid of tiles; expands to one mesh placement per cell.
    TileMapInstance { tilemap: TileMapId },
    Camera { fov_deg: f32 },
    Light { intensity: f32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub transform: Transform,
    pub visible: bool,
    pub kind: NodeKind,
    /// Procedural motion evaluated at draw time (viewer only; glTF export skips it).
    #[serde(default, skip_serializing_if = "crate::motion::Motion::is_none")]
    pub motion: crate::motion::Motion,
}

impl Default for Node {
    fn default() -> Self {
        Self {
            name: String::new(),
            parent: None,
            children: Vec::new(),
            transform: Transform::default(),
            visible: true,
            kind: NodeKind::Empty,
            motion: crate::motion::Motion::None,
        }
    }
}

/// Flat node map. Roots have `parent == None`; `roots` keeps stable ordering.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub nodes: BTreeMap<NodeId, Node>,
    pub roots: Vec<NodeId>,
    pub(crate) next_node: u32,
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc_node_id(&mut self) -> NodeId {
        let id = NodeId(self.next_node);
        self.next_node += 1;
        id
    }

    /// Insert a node under `parent` (`None` = scene root). Returns the id.
    pub fn insert(&mut self, parent: Option<NodeId>, node: Node) -> NodeId {
        let id = self.alloc_node_id();
        let mut node = node;
        node.parent = parent;
        if let Some(p) = parent {
            if let Some(pn) = self.nodes.get_mut(&p) {
                pn.children.push(id);
            }
        } else {
            self.roots.push(id);
        }
        self.nodes.insert(id, node);
        id
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    pub fn remove(&mut self, id: NodeId) -> Option<Node> {
        let node = self.nodes.remove(&id)?;
        // detach from parent
        match node.parent {
            Some(p) => {
                if let Some(pn) = self.nodes.get_mut(&p) {
                    pn.children.retain(|c| *c != id);
                }
            }
            None => self.roots.retain(|r| *r != id),
        }
        // orphan children become roots
        for c in &node.children {
            if let Some(cn) = self.nodes.get_mut(c) {
                cn.parent = None;
            }
            self.roots.push(*c);
        }
        Some(node)
    }

    /// Remove a node and its whole subtree.
    pub fn remove_subtree(&mut self, id: NodeId) {
        let children: Vec<NodeId> = self
            .get(id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        for c in children {
            self.remove_subtree(c);
        }
        self.remove(id);
    }

    /// World transform = product of TRS from root down to this node.
    pub fn world_transform(&self, id: NodeId) -> Transform {
        let Some(node) = self.get(id) else {
            return Transform::default();
        };
        match node.parent {
            Some(p) => self.world_transform(p).then(&node.transform),
            None => node.transform,
        }
    }

    /// Depth-first pre-order walk from roots.
    pub fn walk(&self, mut f: impl FnMut(NodeId, &Node)) {
        fn rec(scene: &Scene, id: NodeId, f: &mut impl FnMut(NodeId, &Node)) {
            if let Some(n) = scene.get(id) {
                f(id, n);
                for c in n.children.clone() {
                    rec(scene, c, f);
                }
            }
        }
        for r in self.roots.clone() {
            rec(self, r, &mut f);
        }
    }
}
