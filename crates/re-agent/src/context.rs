//! Szenen-Kondensat für Prompts: die KI soll verstehen, WAS im Projekt
//! liegt, ohne 500 KB JSON in den Kontext zu kippen.

use re_core::project::Project;
use re_core::scene::NodeKind;

/// Kompakte Zusammenfassung eines Projekts (Hunderte Tokens statt Megabytes).
#[derive(Clone, Debug, Default)]
pub struct SceneDigest {
    pub project_name: String,
    pub nodes: usize,
    pub meshes: usize,
    pub materials: usize,
    pub tilesets: usize,
    pub tilemaps: usize,
    pub skeletons: usize,
    /// Eine Zeile pro Knoten: "name — MeshInstance(m3)", gekappt.
    pub node_lines: Vec<String>,
}

const MAX_LINES: usize = 200;

impl SceneDigest {
    pub fn from_project(p: &Project) -> Self {
        let mut d = SceneDigest {
            project_name: p.name.clone(),
            nodes: p.scene.nodes.len(),
            meshes: p.library.meshes.len(),
            materials: p.library.materials.len(),
            tilesets: p.library.tilesets.len(),
            tilemaps: p.library.tilemaps.len(),
            skeletons: p.library.skeletons.len(),
            node_lines: Vec::new(),
        };
        p.scene.walk(|_id, node| {
            if d.node_lines.len() >= MAX_LINES {
                return;
            }
            let kind = match &node.kind {
                NodeKind::Empty => "Empty".to_string(),
                NodeKind::MeshInstance { mesh, material, .. } => match material {
                    Some(m) => format!("MeshInstance({mesh} / {m})"),
                    None => format!("MeshInstance({mesh})"),
                },
                NodeKind::SkeletonInstance { skeleton } => format!("SkeletonInstance({skeleton})"),
                NodeKind::TileMapInstance { tilemap } => format!("TileMapInstance({tilemap})"),
                NodeKind::Camera { fov_deg } => format!("Camera({fov_deg}°)"),
                NodeKind::Light { intensity } => format!("Light({intensity})"),
            };
            let vis = if node.visible { "" } else { " [versteckt]" };
            d.node_lines.push(format!("{} — {kind}{vis}", node.name));
        });
        d
    }

    /// Als Prompt-Abschnitt ("Die Szene enthält …").
    pub fn prompt_block(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "Projekt '{}': {} Knoten, {} Meshes, {} Materialien, {} Tilesets, {} Tilemaps, {} Skeletons.",
            self.project_name,
            self.nodes,
            self.meshes,
            self.materials,
            self.tilesets,
            self.tilemaps,
            self.skeletons
        );
        for line in &self.node_lines {
            let _ = writeln!(s, "  {line}");
        }
        if self.nodes > self.node_lines.len() {
            let _ = writeln!(s, "  … ({} weitere Knoten)", self.nodes - self.node_lines.len());
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use re_core::material::Material;
    use re_core::mesh::Mesh;
    use re_core::project::Project;
    use re_core::scene::{Node, NodeKind};
    use re_core::tileset::{Tile, TileCell, TileMap, TileSet};
    use re_core::transform::Transform;

    #[test]
    fn digest_zaehlt_und_benennt() {
        let mut p = Project::new("testmap");
        let mesh = p.library.add_mesh(Mesh::default());
        let mat = p.library.add_material(Material::default());
        let mut ts = TileSet::new("rasen", 2.0, 2.0);
        let tile = Tile {
            name: "gras".into(),
            mesh,
            offset: Transform::default(),
            material: Some(mat),
            motion: re_core::motion::Motion::None,
        };
        ts.tiles.push(tile);
        let tsid = p.library.add_tileset(ts);
        let mut tm = TileMap::new("boden", tsid);
        tm.cells.push(TileCell { x: 0, z: 0, tile: 0 });
        let tmid = p.library.add_tilemap(tm);

        p.scene.insert(None, Node { name: "Fels".into(), kind: NodeKind::MeshInstance { mesh, material: Some(mat), skeleton_node: None }, ..Default::default() });
        p.scene.insert(None, Node { name: "Boden".into(), kind: NodeKind::TileMapInstance { tilemap: tmid }, ..Default::default() });

        let d = SceneDigest::from_project(&p);
        assert_eq!(d.meshes, 1);
        assert_eq!(d.tilemaps, 1);
        let block = d.prompt_block();
        assert!(block.contains("Fels"));
        assert!(block.contains("Boden"));
        assert!(block.contains("testmap"));
    }
}
