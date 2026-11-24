use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($name:ident, $prefix:literal) => {
        #[derive(
            Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Debug,
        )]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0)
            }
        }
    };
}

id_type!(NodeId, "n");
id_type!(MeshId, "m");
id_type!(SkeletonId, "s");
id_type!(AnimationId, "a");
id_type!(MaterialId, "mat");
id_type!(TileSetId, "ts");
id_type!(TileMapId, "tm");
