//! re-core: scene graph, meshes, skeletons, animations and the asset library
//! for WorldBuilder. Everything here is headless and serialization friendly.

pub mod animation;
pub mod id;
pub mod iso;
pub mod material;
pub mod mesh;
pub mod motion;
pub mod ops;
pub mod optimize;
pub mod primitives;
pub mod project;
pub mod scene;
pub mod skeleton;
pub mod terrain;
pub mod tilebuild;
pub mod tileset;
pub mod transform;

pub use animation::{Animation, Channel, ChannelPath, Interpolation, KeyValue, Keyframe, Space, Target};
pub use id::{AnimationId, MaterialId, MeshId, NodeId, SkeletonId};
pub use material::Material;
pub use mesh::{Mesh, Skin};
pub use project::Project;
pub use scene::{Node, NodeKind, Scene};
pub use skeleton::{Joint, Skeleton};
pub use tileset::{Tile, TileCell, TileMap, TileSet};
pub use transform::Transform;
