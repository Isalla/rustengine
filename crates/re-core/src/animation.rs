use crate::id::NodeId;
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Which TRS component a channel animates.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelPath {
    Translation,
    Rotation,
    Scale,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interpolation {
    Step,
    #[default]
    Linear,
    /// Accepted for import round-trips; sampled as linear for now.
    Cubic,
}

/// Animated value: 3 components for Translation/Scale, quaternion `[x,y,z,w]` for Rotation.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum KeyValue {
    Vec3([f32; 3]),
    Quat([f32; 4]),
}

impl KeyValue {
    pub fn lerp(&self, other: &KeyValue, t: f32) -> KeyValue {
        match (self, other) {
            (KeyValue::Vec3(a), KeyValue::Vec3(b)) => {
                KeyValue::Vec3(Vec3::from_array(*a).lerp(Vec3::from_array(*b), t).to_array())
            }
            (KeyValue::Quat(a), KeyValue::Quat(b)) => {
                let qa = Quat::from_array(*a);
                let qb = Quat::from_array(*b);
                KeyValue::Quat(qa.slerp(qb, t).to_array())
            }
            _ => *self,
        }
    }

    pub fn default_for(path: ChannelPath) -> KeyValue {
        match path {
            ChannelPath::Translation => KeyValue::Vec3([0.0, 0.0, 0.0]),
            ChannelPath::Rotation => KeyValue::Quat([0.0, 0.0, 0.0, 1.0]),
            ChannelPath::Scale => KeyValue::Vec3([1.0, 1.0, 1.0]),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub time: f32,
    pub value: KeyValue,
}

/// What a channel drives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Target {
    /// A concrete scene node.
    Node(NodeId),
    /// A joint by name — resolved against whatever skeleton instance the
    /// animation plays on. This is what makes animations reusable across rigs.
    Joint(String),
}

/// Space a keyframe value lives in.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Space {
    /// The value *is* the local TRS component (glTF behaviour). Translation
    /// keys therefore replace the bind-pose offset of the joint.
    #[default]
    Absolute,
    /// The value is applied on top of the bind/rest local transform
    /// (`bind * delta`). Lets one clip drive rigs whose joints sit at
    /// different heights or scales. Baked to absolute on glTF export.
    RelativeToBind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub target: Target,
    pub path: ChannelPath,
    #[serde(default)]
    pub interpolation: Interpolation,
    /// Absolute by default; older project files stay loadable.
    #[serde(default)]
    pub space: Space,
    /// Sorted by time.
    pub keys: Vec<Keyframe>,
}

impl Channel {
    pub fn new(target: Target, path: ChannelPath) -> Self {
        Self {
            target,
            path,
            interpolation: Interpolation::default(),
            space: Space::default(),
            keys: Vec::new(),
        }
    }

    /// Insert or replace a keyframe at (nearly) the same time, keeping order.
    pub fn set_key(&mut self, time: f32, value: KeyValue) {
        let pos = self.keys.partition_point(|k| k.time < time - 1e-6);
        if pos < self.keys.len() && (self.keys[pos].time - time).abs() < 1e-5 {
            self.keys[pos] = Keyframe { time, value };
        } else {
            self.keys.insert(pos, Keyframe { time, value });
        }
    }

    /// Sample the channel value at time `t` (clamped to the key range).
    pub fn sample(&self, t: f32) -> Option<KeyValue> {
        let keys = &self.keys;
        match keys.len() {
            0 => None,
            1 => Some(keys[0].value),
            _ => {
                if t <= keys[0].time {
                    return Some(keys[0].value);
                }
                let last = keys.len() - 1;
                if t >= keys[last].time {
                    return Some(keys[last].value);
                }
                let i = keys.partition_point(|k| k.time <= t).min(last) - 1;
                let a = keys[i];
                let b = keys[i + 1];
                match self.interpolation {
                    Interpolation::Step => Some(a.value),
                    _ => {
                        let span = b.time - a.time;
                        let f = if span > 0.0 { (t - a.time) / span } else { 0.0 };
                        Some(a.value.lerp(&b.value, f))
                    }
                }
            }
        }
    }
}

/// A named, reusable animation clip: a set of TRS channels over time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Animation {
    pub name: String,
    pub channels: Vec<Channel>,
    /// Explicit loop flag (viewer/export may honour it).
    #[serde(default)]
    pub looping: bool,
}

impl Animation {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            channels: Vec::new(),
            looping: false,
        }
    }

    pub fn duration(&self) -> f32 {
        self.channels
            .iter()
            .filter_map(|c| c.keys.last().map(|k| k.time))
            .fold(0.0f32, f32::max)
    }

    /// Find the channel for a target/path pair.
    pub fn channel(&self, target: &Target, path: ChannelPath) -> Option<&Channel> {
        self.channels
            .iter()
            .find(|c| &c.target == target && c.path == path)
    }

    pub fn channel_mut(&mut self, target: &Target, path: ChannelPath) -> Option<&mut Channel> {
        self.channels
            .iter_mut()
            .find(|c| &c.target == target && c.path == path)
    }

    /// Get or create the channel for a target/path pair.
    pub fn get_or_create(&mut self, target: Target, path: ChannelPath) -> &mut Channel {
        let idx = match self
            .channels
            .iter()
            .position(|c| c.target == target && c.path == path)
        {
            Some(i) => i,
            None => {
                self.channels.push(Channel::new(target, path));
                self.channels.len() - 1
            }
        };
        &mut self.channels[idx]
    }

    /// Sample every channel at time `t`.
    pub fn sample(&self, t: f32) -> Vec<(&Channel, KeyValue)> {
        let t = if self.looping && self.duration() > 0.0 {
            t % self.duration()
        } else {
            t
        };
        self.channels
            .iter()
            .filter_map(|c| c.sample(t).map(|v| (c, v)))
            .collect()
    }
}
