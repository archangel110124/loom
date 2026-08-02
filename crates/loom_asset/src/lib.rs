//! Asset identity, import, and the procedural primitive library.
//!
//! Unity's `.meta` sidecar idea is right and its Editor-only asset database is
//! the trap (design doc §2.6). So identity is a UUID in a sidecar, paths are
//! advisory, and the UUID→artifact map is generated on **every** build and
//! shipped — no parallel system, no surprise at ship time.

pub mod mesh;
pub mod packed;
pub mod meta;
pub mod primitives;
pub mod texture;

pub use mesh::{Mesh, Vertex};
pub use texture::{ColorSpace, Texture};
pub use packed::{PackedBounds, PackedVertex};
pub use meta::{AssetId, Manifest, Meta};

/// Why an import failed.
#[derive(Debug)]
pub enum AssetError {
    Io(std::io::Error),
    /// The file parsed but is not something we can use.
    Unsupported(String),
    Gltf(String),
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::Gltf(e) => write!(f, "glTF import failed: {e}"),
        }
    }
}

impl std::error::Error for AssetError {}
