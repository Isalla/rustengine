//! End-to-end: build a skinned, animated humanoid -> export GLB/glTF -> re-import.

use re_core::animation::{ChannelPath, KeyValue};
use re_core::ops;
use re_core::primitives::{self, PrimitiveKind};
use re_core::project::Project;
use re_core::scene::NodeKind;
use re_core::skeleton::Skeleton;
use re_core::transform::Transform;

fn demo_project() -> Project {
    let mut p = Project::new("demo");

    // skeleton instance
    let (skel_node, _skel_id) = ops::add_humanoid(&mut p, None, "Rig", );

    // torso-ish mesh on the rig, bound automatically
    let torso = primitives::generate(PrimitiveKind::Capsule, 0.18, 0.85, 12);
    let torso_id = p.library.add_mesh(torso);
    let torso_node = p.scene.insert(
        None,
        re_core::scene::Node {
            name: "Body".into(),
            transform: Transform {
                translation: [0.0, 1.05, 0.0],
                ..Default::default()
            },
            kind: NodeKind::MeshInstance {
                mesh: torso_id,
                material: None,
                skeleton_node: None,
            },
            ..Default::default()
        },
    );
    ops::auto_bind(&mut p, torso_node, skel_node).expect("auto bind");

    // a reusable walk animation addressing joints BY NAME
    let anim = ops::create_animation(&mut p, "wave");
    p.library.animations.get_mut(&anim).unwrap().looping = true;
    use glam::{Quat, Vec3};
    let rot = |angle: f32| KeyValue::Quat(Quat::from_rotation_z(angle).to_array());
    ops::add_keyframe_joint(&mut p, anim, "LeftArm", ChannelPath::Rotation, 0.0, rot(0.0)).unwrap();
    ops::add_keyframe_joint(
        &mut p,
        anim,
        "LeftArm",
        ChannelPath::Rotation,
        0.5,
        rot(0.7),
    )
    .unwrap();
    ops::add_keyframe_joint(&mut p, anim, "LeftArm", ChannelPath::Rotation, 1.0, rot(0.0)).unwrap();
    // also animate the rig root node itself (translation loop)
    ops::add_keyframe_node(
        &mut p,
        anim,
        skel_node,
        ChannelPath::Translation,
        0.0,
        KeyValue::Vec3(Vec3::ZERO.to_array()),
    )
    .unwrap();
    ops::add_keyframe_node(
        &mut p,
        anim,
        skel_node,
        ChannelPath::Translation,
        1.0,
        KeyValue::Vec3(Vec3::ZERO.to_array()),
    )
    .unwrap();

    p
}

#[test]
fn glb_roundtrip() {
    let dir = std::env::temp_dir().join(format!("wbtest_glb_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = demo_project();
    let glb = dir.join("demo.glb");
    re_io::export(&p, &glb).expect("export glb");

    // re-import
    let q = re_io::import(&glb).expect("import glb");

    // one skeleton with all humanoid joints
    assert_eq!(q.library.skeletons.len(), 1, "skeleton count");
    let skel = q.library.skeletons.values().next().unwrap();
    assert_eq!(skel.joints.len(), 25, "joint count, got {}", skel.joints.len());
    assert!(skel.joint_by_name("Hips").is_some());
    assert!(skel.joint_by_name("LeftArm").is_some());
    assert!(skel.joint_by_name("RightToeBase").is_some());

    // the re-imported skin must still exist on a mesh
    let skinned = q
        .library
        .meshes
        .values()
        .filter(|m| m.skin.is_some())
        .count();
    assert!(skinned >= 1, "skinned meshes lost");
    let m = q.library.meshes.values().find(|m| m.skin.is_some()).unwrap();
    let skin = m.skin.as_ref().unwrap();
    assert_eq!(skin.joints.len(), m.positions.len());
    for w in &skin.weights {
        let sum: f32 = w.iter().sum();
        assert!((sum - 1.0).abs() < 1e-3 || sum == 0.0, "weights not normalized");
    }

    // the wave animation survived with a joint-name target
    assert!(!q.library.animations.is_empty(), "animations lost");
    let a = q.library.animations.values().next().unwrap();
    let has_joint_target = a
        .channels
        .iter()
        .any(|c| matches!(&c.target, re_core::animation::Target::Joint(n) if n == "LeftArm"));
    assert!(has_joint_target, "retargetable joint target lost: {:?}", a.channels.iter().map(|c| &c.target).collect::<Vec<_>>());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn gltf_text_roundtrip() {
    let dir = std::env::temp_dir().join(format!("wbtest_gltf_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = demo_project();
    let gltf = dir.join("demo.gltf");
    re_io::export(&p, &gltf).expect("export gltf");
    assert!(dir.join("demo.bin").exists(), "missing .bin");

    let q = re_io::import(&gltf).expect("import gltf");
    assert_eq!(q.library.skeletons.len(), 1);
    let skel = q.library.skeletons.values().next().unwrap();
    assert_eq!(skel.joints.len(), 25);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn pose_sampling_consistency() {
    // sample the pose at t=0.5 of "wave": LeftArm should be rotated ~0.7 rad
    let p = demo_project();
    let anim_id = *p.library.animations.keys().next().unwrap();
    let anim = p.library.animations.get(&anim_id).unwrap();
    let skel_node = p
        .scene
        .nodes
        .values()
        .position(|n| matches!(n.kind, NodeKind::SkeletonInstance { .. }))
        .map(|_pos| {
            let mut ids: Vec<_> = p.scene.nodes.keys().copied().collect();
            ids.sort();
            // find the skeleton instance node id
            for id in &ids {
                if matches!(p.scene.get(*id).unwrap().kind, NodeKind::SkeletonInstance { .. }) {
                    return *id;
                }
            }
            ids[0]
        })
        .unwrap();
    let pose = ops::sample_pose(&p, skel_node, anim, 0.5);
    let skel = Skeleton::humanoid("x"); // for joint index lookup only
    let _ = skel;
    let left_arm_idx = {
        let sid = ops::skeleton_of(&p, skel_node).unwrap();
        p.library.skeletons.get(&sid).unwrap().joint_by_name("LeftArm").unwrap()
    };
    let t = pose.joints.get(&left_arm_idx).expect("LeftArm pose");
    let q = glam::Quat::from_array(t.rotation);
    let angle = 2.0 * q.w.max(-1.0).min(1.0).abs().acos();
    assert!((angle - 0.7).abs() < 0.05, "rotation angle {angle}");
}

#[test]
fn relative_clip_bakes_to_absolute_on_export() {
    use re_core::animation::{Channel, ChannelPath, KeyValue, Space, Target};
    use re_core::id::NodeId;

    let dir = std::env::temp_dir().join(format!("wbtest_rel_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // humanoid + a relative bob: authored once, must survive a format round trip
    let mut p = Project::new("bob");
    let (_skel_node, _) = ops::add_humanoid(&mut p, None, "Rig");
    let anim = ops::create_animation(&mut p, "bob");
    {
        let a = p.library.animations.get_mut(&anim).unwrap();
        let mut ch = Channel::new(Target::Joint("Hips".into()), ChannelPath::Translation);
        ch.space = Space::RelativeToBind;
        ch.set_key(0.0, KeyValue::Vec3([0.0, 0.0, 0.0]));
        ch.set_key(1.0, KeyValue::Vec3([0.0, 0.2, 0.0]));
        a.channels.push(ch);
    }
    let glb = dir.join("bob.glb");
    re_io::export(&p, &glb).expect("export");
    let q = re_io::import(&glb).expect("import");

    // find the imported rig instance + the bob clip
    let inst = q
        .scene
        .nodes
        .keys()
        .copied()
        .find(|id| matches!(q.scene.get(*id).unwrap().kind, NodeKind::SkeletonInstance { .. }))
        .expect("rig instance");
    let clip = q
        .library
        .animations
        .values()
        .find(|a| a.name.contains("bob"))
        .expect("bob clip");
    assert!(
        clip.channels
            .iter()
            .all(|c| c.space == Space::Absolute && !matches!(&c.target, Target::Node(NodeId(u32::MAX)))),
        "exported channels must be absolute"
    );

    // hips: bind 1.0 + baked 0.2 offset = 1.2
    let pose = ops::sample_pose(&q, inst, clip, 1.0);
    let sid = ops::skeleton_of(&q, inst).unwrap();
    let skel = q.library.skeletons.get(&sid).unwrap();
    let hips = skel.joint_by_name("Hips").unwrap();
    let g = ops::joint_global(skel, &pose, hips);
    assert!(
        (g.translation[1] - 1.2).abs() < 1e-4,
        "baked bob lost: {:?}",
        g.translation
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Tilemaps have no glTF equivalent: export expands every painted cell into
/// a node that REUSES the tile's mesh, and the re-imported scene must show
/// that structure (correct count, shared mesh assets).
#[test]
fn tilemap_expands_to_cells_on_export() {
    let dir = std::env::temp_dir().join(format!("wbtest_tiles_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let mut p = Project::new("tiles");
    let kit = ops::create_tileset(&mut p, "kit", 1.0, 1.0);
    let floor = ops::add_tile_primitive(
        &mut p, kit, "floor", PrimitiveKind::Plane, 1.0, 1.0, 1,
        Transform::default(), None,
    )
    .unwrap();
    let wall = ops::add_tile_primitive(
        &mut p, kit, "wall", PrimitiveKind::Box, 0.5, 0.5, 2,
        Transform { translation: [0.5, 0.5, 0.5], ..Default::default() }, None,
    )
    .unwrap();
    let (_node, map) =
        ops::create_tilemap(&mut p, "island", kit, None, Transform::default()).unwrap();
    ops::tilemap_fill_rect(&mut p, map, 0, 0, 4, 4, floor).unwrap(); // 25 cells
    ops::tilemap_fill_rect(&mut p, map, 0, 0, 2, 0, wall).unwrap(); // overwrite 3

    let glb = dir.join("tiles.glb");
    re_io::export(&p, &glb).expect("export");
    let q = re_io::import(&glb).expect("import");

    // 1 tilemap node + 25 expanded cells; unique meshes: floor + wall = 2
    let mut mesh_nodes = 0;
    let mut used_meshes = std::collections::BTreeSet::new();
    q.scene.walk(|_, n| {
        if let NodeKind::MeshInstance { mesh, .. } = &n.kind {
            mesh_nodes += 1;
            used_meshes.insert(mesh.0);
        }
    });
    assert_eq!(mesh_nodes, 25, "one glTF node per painted cell");
    assert_eq!(used_meshes.len(), 2, "cells must share the two tile meshes");

    // world position of a cell: floor at (3,1) sits at x=3, z=1
    let mut seen = false;
    q.scene.walk(|_, n| {
        if let NodeKind::MeshInstance { mesh, .. } = &n.kind {
            if n.name.contains("@3,1") {
                assert_eq!(n.transform.translation, [3.0, 0.0, 1.0]);
                let _ = mesh;
                seen = true;
            }
        }
    });
    assert!(seen, "cell name/transform from expansion missing");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn texture_escape_paths_are_skipped() {
    use re_core::material::Material;
    use re_core::scene::Node;

    let dir = std::env::temp_dir().join(format!("re_tex_{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    // Any bytes work: the exporter embeds the file without decoding it.
    std::fs::write(dir.join("sub/tex.png"), b"\x89PNG fake").unwrap();

    let mut p = Project::new("tex");
    let mesh_id = p.library.add_mesh(primitives::generate(PrimitiveKind::Box, 1.0, 1.0, 1));
    let mut bad = Material::default();
    bad.name = "bad".into();
    bad.albedo_texture_id = Some("../../escape.png".into());
    bad.normal_texture_id = Some("/etc/hostname".into());
    let bad_id = p.library.add_material(bad);
    let mut good = Material::default();
    good.name = "good".into();
    good.albedo_texture_id = Some("sub/tex.png".into());
    let good_id = p.library.add_material(good);
    for (name, m) in [("Bad", bad_id), ("Good", good_id)] {
        p.scene.insert(
            None,
            Node {
                name: name.into(),
                transform: Transform::default(),
                kind: NodeKind::MeshInstance { mesh: mesh_id, material: Some(m), skeleton_node: None },
                ..Default::default()
            },
        );
    }

    let out = dir.join("scene.gltf");
    re_io::export(&p, &out).expect("export");
    let text = std::fs::read_to_string(&out).unwrap();
    // Exactly one image (the legit relative path); escaping paths embed nothing.
    assert_eq!(
        text.matches("\"mimeType\"").count(),
        1,
        "escaped texture paths must be skipped, legit relative path embedded"
    );

    std::fs::remove_dir_all(&dir).ok();
}
