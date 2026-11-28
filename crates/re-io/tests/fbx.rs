//! Binär-FBX-Import (7.4–7.7): Mesh, Skin, Animation und die
//! sauberen Ablehnungspfade. Die Fixtures sind echte Exporte aus dem
//! ufbx-Regressionkatalog (MIT) — siehe `data/fbx/ATTRIBUTION.md`.

use re_core::project::Project;
use re_core::scene::NodeKind;
use std::fs;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/fbx")
        .join(name)
}

/// FBX-Import braucht einen echten Pfad mit Endung — kurz in ein
/// prozess-private Temp-Verzeichnis kopieren.
fn import(name: &str) -> anyhow::Result<Project> {
    let dir = std::env::temp_dir().join(format!("re_fbx_{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let dst = dir.join(name);
    fs::copy(fixture(name), &dst).unwrap();
    re_io::import(&dst)
}

fn bbox(mesh: &re_core::mesh::Mesh) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &mesh.positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    (lo, hi)
}

#[test]
fn cube_7400_wird_mesh_mit_dreifach_faechern() {
    let p = import("blender_272_cube_7400_binary.fbx").expect("Import sollte klappen");
    assert_eq!(p.library.meshes.len(), 1);
    let mesh = p.library.meshes.values().next().unwrap();
    assert_eq!(mesh.positions.len(), 8);
    // 6 Vierecke -> 12 Dreiecke
    assert_eq!(mesh.indices.as_deref().map(<[u32]>::len), Some(36));
    assert_eq!(mesh.normals.as_ref().map(|n| n.len()), Some(8));
    let (lo, hi) = bbox(mesh);
    for i in 0..3 {
        assert!((lo[i] + 1.0).abs() < 1e-3, "lo[{i}] = {}", lo[i]);
        assert!((hi[i] - 1.0).abs() < 1e-3, "hi[{i}] = {}", hi[i]);
    }
    // keine Haut, keine Takes, ein Mesh-Knoten "Cube"
    assert!(mesh.skin.is_none());
    assert!(p.library.animations.is_empty());
    let node = p
        .scene
        .nodes.values()
        .find(|n| matches!(n.kind, NodeKind::MeshInstance { .. }))
        .expect("MeshInstance-Knoten");
    assert_eq!(node.name, "Cube");
}

#[test]
fn skin_7400_wird_skeleton_gewichtet_und_knochen_versteckt() {
    let p = import("blender_293_half_skinned_7400_binary.fbx").expect("Import");
    assert_eq!(p.library.skeletons.len(), 1);
    let skel = p.library.skeletons.values().next().unwrap();
    assert_eq!(skel.joints.len(), 1);
    assert_eq!(skel.joints[0].name, "Bone");

    let mesh = p
        .library
        .meshes
        .values()
        .find(|m| m.skin.is_some())
        .expect("Mesh mit Skin");
    let skin = mesh.skin.as_ref().unwrap();
    // alle vier Eckpunktemachen den einzigen Joint fest (gewichtet normalisiert)
    assert_eq!(skin.joints.len(), 4);
    for j in &skin.joints {
        assert_eq!(*j, [0, 0, 0, 0]);
    }
    for w in &skin.weights {
        assert!((w[0] - 1.0).abs() < 1e-5, "w = {w:?}");
        assert!((w[1] + w[2] + w[3]).abs() < 1e-5);
    }

    // Originalknochen ausgeblendet, Instanz-Knoten vorhanden
    let bone = p.scene.nodes.values().find(|n| n.name == "Bone").expect("Bone-Knoten");
    assert!(!bone.visible, "Originalknochen muss ausgeblendet sein");
    assert!(p
        .scene
        .nodes.values()
        .any(|n| matches!(n.kind, NodeKind::SkeletonInstance { .. })));
    assert!(p.scene.nodes.values().any(|n| {
        matches!(n.kind, NodeKind::MeshInstance { skeleton_node: Some(_), .. })
    }));
}

#[test]
fn animation_7500_wird_clip_mit_kanaelen() {
    let p = import("maya_transform_animation_7500_binary.fbx").expect("Import");
    assert_eq!(p.library.animations.len(), 1);
    let clip = p.library.animations.values().next().unwrap();
    // Translation + Rotation + Scaling mindestens (Visibility wird verworfen)
    assert!(clip.channels.len() >= 3, "kanaele = {}", clip.channels.len());
    for ch in &clip.channels {
        assert!(!ch.keys.is_empty());
        assert_eq!(ch.keys.first().unwrap().time, 0.0, "Clip beginnt bei 0");
        match ch.path {
            re_core::animation::ChannelPath::Rotation => {
                let Some(re_core::animation::KeyValue::Quat(q)) = ch.keys.last().map(|k| k.value) else {
                    panic!("Rotationskanal muss Quaternions führen");
                };
                let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
                assert!((len - 1.0).abs() < 1e-3, "Quaternion nicht normiert: {q:?}");
            }
            _ => {
                let Some(re_core::animation::KeyValue::Vec3(v)) = ch.keys.last().map(|k| k.value) else {
                    panic!("Vektorkanal erwartet");
                };
                assert!(v.iter().all(|x| x.is_finite()));
            }
        }
        assert!(matches!(ch.target, re_core::animation::Target::Node(_)));
    }
    let dur = clip.channels.iter().flat_map(|c| c.keys.iter()).map(|k| k.time).fold(0.0f32, f32::max);
    assert!(dur > 0.5, "Animationsdauer plausibel? {dur}");
}

#[test]
fn skin_7700_mit_64_bit_offseten_laeuft() {
    let p = import("maya_advanced_skinned_pivot_7700_binary.fbx").expect("Import");
    assert!(!p.library.skeletons.is_empty(), "Skelett aus 7700er Skin");
    assert!(p.library.meshes.values().any(|m| m.skin.is_some()));
    assert!(p.library.meshes.values().all(|m| !m.positions.is_empty()));
}

#[test]
fn fbx_unter_7_4_wird_sauber_abgelehnt() {
    let err = import("max_transformed_skin_6100_binary.fbx").unwrap_err().to_string();
    assert!(err.contains("7.4"), "Fehlermeldung nennt die Untergrenze: {err}");
}

#[test]
fn ascii_fbx_wird_sauber_abgelehnt() {
    let dir = std::env::temp_dir().join(format!("re_fbx_ascii_{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let dst = dir.join("alt.fbx");
    fs::write(&dst, "FBX;5.0.0\nK: 0,0, \"KDFCTemplateInfo::Name\", \"KString\", \"\", \"\", \"\"\n").unwrap();
    let err = re_io::import(&dst).unwrap_err().to_string();
    assert!(err.contains("7.4"), "Fehlermeldung nennt die Untergrenze: {err}");
}

#[test]
fn abgeschnittene_datei_ohne_panic() {
    let dir = std::env::temp_dir().join(format!("re_fbx_trunc_{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let dst = dir.join("abgebrochen.fbx");
    let full = fs::read(fixture("blender_272_cube_7400_binary.fbx")).unwrap();
    fs::write(&dst, &full[..full.len() / 3]).unwrap();
    assert!(re_io::import(&dst).is_err());
}
