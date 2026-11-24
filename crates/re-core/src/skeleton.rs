use crate::transform::Transform;
use serde::{Deserialize, Serialize};

/// A single bone. `parent` is the index into `Skeleton::joints`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Joint {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
    /// Bind-pose transform relative to the parent joint.
    pub local: Transform,
}

/// A joint hierarchy in bind pose. Skeletons are reusable assets; scene nodes
/// instantiate them via `NodeKind::SkeletonInstance`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Skeleton {
    pub name: String,
    pub joints: Vec<Joint>,
}

impl Skeleton {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            joints: Vec::new(),
        }
    }

    /// Append a joint; returns its index.
    pub fn add_joint(&mut self, name: impl Into<String>, parent: Option<usize>, local: Transform) -> usize {
        let idx = self.joints.len();
        self.joints.push(Joint {
            name: name.into(),
            parent,
            local,
        });
        idx
    }

    pub fn joint_by_name(&self, name: &str) -> Option<usize> {
        self.joints.iter().position(|j| j.name == name)
    }

    /// Global (bind-pose) transform of a joint relative to the skeleton root space.
    pub fn global_transform(&self, joint: usize) -> Transform {
        let j = &self.joints[joint];
        match j.parent {
            Some(p) => self.global_transform(p).then(&j.local),
            None => j.local,
        }
    }

    /// Direct children of a joint (`None` = root level).
    pub fn children_of(&self, parent: Option<usize>) -> Vec<usize> {
        self.joints
            .iter()
            .enumerate()
            .filter(|(_, j)| j.parent == parent)
            .map(|(i, _)| i)
            .collect()
    }

    /// End of a humanoid rig roughly following common (Mixamo-style) naming,
    /// so imported animations can retarget by name. Bones point +Y in
    /// rest pose, meters.
    pub fn humanoid(name: impl Into<String>) -> Skeleton {
        use glam::Vec3;
        let mut sk = Skeleton::new(name);
        let t = |x, y, z| Transform::from_parts(Vec3::new(x, y, z), Default::default(), Vec3::ONE);

        let hips = sk.add_joint("Hips", None, t(0.0, 1.0, 0.0));
        let spine = sk.add_joint("Spine", Some(hips), t(0.0, 0.15, 0.0));
        let chest = sk.add_joint("Chest", Some(spine), t(0.0, 0.2, 0.0));
        let neck = sk.add_joint("Neck", Some(chest), t(0.0, 0.25, 0.0));
        sk.add_joint("Head", Some(neck), t(0.0, 0.1, 0.0));

        let l_shoulder = sk.add_joint("LeftShoulder", Some(chest), t(0.06, 0.2, 0.0));
        let l_arm = sk.add_joint("LeftArm", Some(l_shoulder), t(0.14, -0.02, 0.0));
        let l_fore = sk.add_joint("LeftForeArm", Some(l_arm), t(0.0, -0.28, 0.0));
        let l_hand = sk.add_joint("LeftHand", Some(l_fore), t(0.0, -0.26, 0.0));
        sk.add_joint("LeftHandThumb1", Some(l_hand), t(0.03, -0.05, 0.02));
        sk.add_joint("LeftHandIndex1", Some(l_hand), t(0.0, -0.09, 0.03));

        let r_shoulder = sk.add_joint("RightShoulder", Some(chest), t(-0.06, 0.2, 0.0));
        let r_arm = sk.add_joint("RightArm", Some(r_shoulder), t(-0.14, -0.02, 0.0));
        let r_fore = sk.add_joint("RightForeArm", Some(r_arm), t(0.0, -0.28, 0.0));
        let r_hand = sk.add_joint("RightHand", Some(r_fore), t(0.0, -0.26, 0.0));
        sk.add_joint("RightHandThumb1", Some(r_hand), t(-0.03, -0.05, 0.02));
        sk.add_joint("RightHandIndex1", Some(r_hand), t(0.0, -0.09, 0.03));

        let l_up = sk.add_joint("LeftUpLeg", Some(hips), t(0.09, -0.05, 0.0));
        let l_leg = sk.add_joint("LeftLeg", Some(l_up), t(0.0, -0.44, 0.0));
        let l_foot = sk.add_joint("LeftFoot", Some(l_leg), t(0.0, -0.42, 0.0));
        sk.add_joint("LeftToeBase", Some(l_foot), t(0.0, -0.06, 0.1));

        let r_up = sk.add_joint("RightUpLeg", Some(hips), t(-0.09, -0.05, 0.0));
        let r_leg = sk.add_joint("RightLeg", Some(r_up), t(0.0, -0.44, 0.0));
        let r_foot = sk.add_joint("RightFoot", Some(r_leg), t(0.0, -0.42, 0.0));
        sk.add_joint("RightToeBase", Some(r_foot), t(0.0, -0.06, 0.1));

        sk
    }
}
