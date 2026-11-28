//! Wavefront-OBJ-Import/Export: Fächerung, negative Indizes, MTL,
//! Gruppen und die Runde über Export → Import.

use re_core::mesh::Mesh;
use re_core::project::Project;
use re_core::scene::{Node, NodeKind};
use re_io::obj;
use std::fs;
use std::path::PathBuf;

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("re_obj_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

const TRI: &str = "\
v 0 0 0
v 1 0 0
v 0 1 0
f 1 2 3
";

#[test]
fn vier_ecke_werden_gefächert_und_uv_gespiegelt() {
    let d = tmpdir("fan");
    let src = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
vt 0 0
vt 1 0
vt 1 1
vt 0 1
vn 0 0 1
f 1/1/1 2/2/1 3/3/1 4/4/1
";
    fs::write(d.join("quad.obj"), src).unwrap();
    let p = obj::import(&d.join("quad.obj")).unwrap();
    assert_eq!(p.library.meshes.len(), 1);
    let m = p.library.meshes.values().next().unwrap();
    assert_eq!(m.positions.len(), 4, "Ecken dedupliziert");
    let ix = m.indices.clone().unwrap();
    assert_eq!(ix.len(), 6, "Viereck → zwei Dreiecke");
    // OBJ-v zeigt nach oben links → Engine spiegelt die y-Achse.
    let uvs = m.uvs.clone().unwrap();
    assert!((uvs[0][1] - 1.0).abs() < 1e-6, "vt (0,0) wird (0,1)");
    let ns = m.normals.clone().unwrap();
    assert_eq!(ns[0], [0.0, 0.0, 1.0]);
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn negative_indizes_zählen_vom_listenende() {
    let d = tmpdir("neg");
    let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
f -3 -2 -1
";
    fs::write(d.join("n.obj"), src).unwrap();
    let p = obj::import(&d.join("n.obj")).unwrap();
    let m = p.library.meshes.values().next().unwrap();
    let ix = m.indices.clone().unwrap();
    assert_eq!(ix, vec![0, 1, 2], "relativ aufgelöst auf absolut");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn ohne_vn_entstehen_flächennormalen() {
    let d = tmpdir("flat");
    fs::write(d.join("t.obj"), TRI).unwrap();
    let p = obj::import(&d.join("t.obj")).unwrap();
    let m = p.library.meshes.values().next().unwrap();
    let n = m.normals.clone().unwrap()[0];
    // Ebene in xy → Normale längs z.
    assert!(n[2].abs() > 0.9, "z-Anteil {n:?}");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn mtl_bindet_farbe_glanz_und_textur() {
    let d = tmpdir("mtl");
    fs::write(
        d.join("mat.mtl"),
        "newmtl Paint\nKd 0.2 0.4 0.6\nNs 25\nd 0.5\nmap_Kd tex.png\n",
    )
    .unwrap();
    let src = format!("mtllib mat.mtl\nusemtl Paint\n{TRI}");
    fs::write(d.join("m.obj"), src).unwrap();
    let p = obj::import(&d.join("m.obj")).unwrap();
    assert_eq!(p.library.materials.len(), 1);
    let mat = p.library.materials.values().next().unwrap();
    assert!((mat.base_color[0] - 0.2).abs() < 1e-6);
    assert!((mat.base_color[3] - 0.5).abs() < 1e-6, "d → Alpha");
    assert!((mat.roughness - 0.75).abs() < 1e-6, "Ns 25 → rauh 0.75");
    assert_eq!(mat.albedo_texture_id.as_deref(), Some("tex.png"));
    let node = p.scene.nodes.values().next().unwrap();
    matches!(&node.kind, NodeKind::MeshInstance { material: Some(_), .. });
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn gruppen_werden_eigene_meshes_mit_eigenen_knoten() {
    let d = tmpdir("grp");
    let src = format!(
        "g Alpha\n{TRI}g Beta\nv 5 0 0\nv 6 0 0\nv 5 1 0\nf 4 5 6\n"
    );
    fs::write(d.join("g.obj"), src).unwrap();
    let p = obj::import(&d.join("g.obj")).unwrap();
    assert_eq!(p.library.meshes.len(), 2);
    let mesh_names: Vec<String> = p.library.meshes.values().map(|m| m.name.clone()).collect();
    assert!(mesh_names.contains(&"Alpha".to_string()));
    assert!(mesh_names.contains(&"Beta".to_string()));
    assert_eq!(p.scene.nodes.len(), 2);
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn fehler_ohne_flächen_oder_mit_zu_wenig_ecken() {
    let d = tmpdir("err");
    fs::write(d.join("empty.obj"), "v 0 0 0\nvn 0 1 0\n").unwrap();
    assert!(obj::import(&d.join("empty.obj")).is_err(), "keine Flächen");
    fs::write(d.join("bad.obj"), "v 0 0 0\nv 1 0 0\nf 1 2\n").unwrap();
    assert!(obj::import(&d.join("bad.obj")).is_err(), "Zwei-Eck-Polygon");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn export_import_runde_erhält_geometrie_und_material() {
    let d = tmpdir("rt");
    let mut p = Project::new("rt");
    let mut mesh = Mesh {
        name: "Tri".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
        normals: Some(vec![[0.0, 0.0, 1.0]; 3]),
        uvs: Some(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
        indices: Some(vec![0, 1, 2]),
        skin: None,
        stamp: Default::default(),
    };
    mesh.touch();
    let mid = p.library.add_mesh(mesh);
    let mat = re_core::material::Material {
        name: "Rot".into(),
        base_color: [0.9, 0.1, 0.2, 1.0],
        roughness: 0.35,
        ..Default::default()
    };
    let mtd = p.library.add_material(mat);
    p.scene.insert(
        None,
        Node {
            name: "T".into(),
            kind: NodeKind::MeshInstance {
                mesh: mid,
                material: Some(mtd),
                skeleton_node: None,
            },
            ..Default::default()
        },
    );
    let path = d.join("rt.obj");
    obj::export(&p, &path).unwrap();
    assert!(d.join("rt.mtl").exists(), "MTL wird daneben gelegt");

    let back = obj::import(&path).unwrap();
    assert_eq!(back.library.meshes.len(), 1);
    assert_eq!(back.library.materials.len(), 1);
    let m = back.library.meshes.values().next().unwrap();
    assert_eq!(m.positions.len(), 3);
    assert!((m.positions[1][0] - 1.0).abs() < 1e-6);
    assert!((m.positions[2][1] - 2.0).abs() < 1e-6);
    let mat = back.library.materials.values().next().unwrap();
    assert!((mat.base_color[0] - 0.9).abs() < 1e-5, "Kd-Runde");
    assert!((mat.roughness - 0.35).abs() < 1e-5, "Ns-Runde");
    let _ = fs::remove_dir_all(&d);
}
