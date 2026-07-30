//! Asset identity: `.meta` sidecars, content hashes, and the runtime manifest.
//!
//! Design doc §2.6. Unity's sidecar idea is right; its Editor-only asset
//! database is the trap — a shipped build cannot resolve a GUID and needs a
//! parallel system bolted on. So the manifest is generated on **every** build
//! and shipped, and runtime resolution works identically in both.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AssetError;

/// A stable asset identity.
///
/// Survives rename, move, and re-import. It is the *only* identity that
/// appears in a `.loom` file; paths are advisory (`docs/format/README.md` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AssetId(pub Uuid);

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The sidecar written next to an imported file as `<file>.meta`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// Identity. Generated once and never regenerated — that is the whole point.
    pub id: AssetId,
    /// BLAKE3 of the source file, hex.
    ///
    /// Same hash function as scene version tokens (`docs/format/README.md` §8):
    /// one hash for the project, chosen once.
    pub content_hash: String,
    /// What produced this. Informational; lets a future importer detect that
    /// an asset needs re-importing because the rules changed, not the bytes.
    pub importer: String,
}

impl Meta {
    /// Load an existing sidecar, or create one for `path`.
    ///
    /// **Never regenerates an existing id.** Transplanting a `.meta` onto a
    /// replacement file is how a broken reference gets fixed (design doc §1.1),
    /// and that only works if import leaves the id alone.
    ///
    /// # Errors
    /// [`AssetError::Io`] if the file or its sidecar cannot be read or written.
    pub fn load_or_create(path: &Path) -> Result<Self, AssetError> {
        let sidecar = meta_path(path);
        let bytes = std::fs::read(path).map_err(AssetError::Io)?;
        let content_hash = blake3::hash(&bytes).to_hex().to_string();

        if let Ok(text) = std::fs::read_to_string(&sidecar)
            && let Ok(mut existing) = serde_json::from_str::<Self>(&text)
        {
            // The hash follows the bytes; the id does not.
            existing.content_hash = content_hash;
            std::fs::write(&sidecar, to_json(&existing)).map_err(AssetError::Io)?;
            return Ok(existing);
        }

        let meta = Self {
            id: AssetId(Uuid::new_v4()),
            content_hash,
            importer: "gltf".to_owned(),
        };
        std::fs::write(&sidecar, to_json(&meta)).map_err(AssetError::Io)?;
        Ok(meta)
    }
}

/// `<file>.meta`, alongside the file — Unity's convention, and it survives the
/// file being renamed only because both move together.
#[must_use]
pub fn meta_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".meta");
    PathBuf::from(s)
}

/// UUID → artifact, generated on every build and shipped with the game.
///
/// This is Unity's AssetPack, except automatic and non-optional. Making it
/// non-optional is the entire fix for their Editor-only database problem.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    entries: BTreeMap<AssetId, Entry>,
}

/// One manifest row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Source path. Advisory — for humans and for re-import, never for lookup.
    pub source: String,
    pub content_hash: String,
}

impl Manifest {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: AssetId, source: impl Into<String>, content_hash: impl Into<String>) {
        self.entries.insert(
            id,
            Entry {
                source: source.into(),
                content_hash: content_hash.into(),
            },
        );
    }

    #[must_use]
    pub fn get(&self, id: AssetId) -> Option<&Entry> {
        self.entries.get(&id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Write the manifest.
    ///
    /// JSON, not a binary blob, despite the brief calling it `manifest.bin`.
    /// It is small, it is generated, and a readable manifest is debuggable —
    /// binary buys nothing until it is large enough to measure.
    /// `ponytail:` swap to a binary format when load time is actually a
    /// problem; `BTreeMap` keeps the output byte-stable either way.
    ///
    /// # Errors
    /// [`AssetError::Io`] if the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), AssetError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(AssetError::Io)?;
        }
        std::fs::write(path, to_json(self)).map_err(AssetError::Io)
    }

    /// Read a manifest back.
    ///
    /// # Errors
    /// [`AssetError::Io`] if it cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Self, AssetError> {
        let text = std::fs::read_to_string(path).map_err(AssetError::Io)?;
        serde_json::from_str(&text)
            .map_err(|e| AssetError::Io(std::io::Error::other(e.to_string())))
    }
}

fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("loom_asset_test_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// **The property the whole design rests on.** Re-importing must not mint
    /// a new id, or every reference in every scene breaks on the next import.
    #[test]
    fn reimporting_keeps_the_same_id() {
        let dir = scratch("stable_id");
        let asset = dir.join("thing.bin");
        std::fs::write(&asset, b"original").unwrap();

        let first = Meta::load_or_create(&asset).unwrap();
        let second = Meta::load_or_create(&asset).unwrap();

        assert_eq!(first.id, second.id, "id must survive re-import");
    }

    /// The hash follows the bytes even though the id does not — that is what
    /// lets the import cache know the artifact is stale.
    #[test]
    fn editing_the_file_changes_the_hash_but_not_the_id() {
        let dir = scratch("hash_follows");
        let asset = dir.join("thing.bin");
        std::fs::write(&asset, b"before").unwrap();
        let first = Meta::load_or_create(&asset).unwrap();

        std::fs::write(&asset, b"after").unwrap();
        let second = Meta::load_or_create(&asset).unwrap();

        assert_eq!(first.id, second.id);
        assert_ne!(first.content_hash, second.content_hash);
    }

    #[test]
    fn the_sidecar_sits_next_to_the_file() {
        assert_eq!(
            meta_path(Path::new("assets/props/desk.glb")),
            PathBuf::from("assets/props/desk.glb.meta")
        );
    }

    #[test]
    fn a_manifest_round_trips() {
        let dir = scratch("manifest");
        let path = dir.join("manifest.json");
        let id = AssetId(Uuid::new_v4());

        let mut manifest = Manifest::new();
        manifest.insert(id, "assets/props/desk.glb", "abc123");
        manifest.save(&path).unwrap();

        let loaded = Manifest::load(&path).unwrap();
        assert_eq!(loaded.get(id).unwrap().source, "assets/props/desk.glb");
        assert_eq!(loaded, manifest);
    }

    /// Byte-stable output: a manifest regenerated from the same inputs must be
    /// identical, or every build churns the file and the diff is noise.
    #[test]
    fn manifest_output_is_byte_stable() {
        let ids: Vec<AssetId> = (0..8).map(|_| AssetId(Uuid::new_v4())).collect();

        let build = || {
            let mut m = Manifest::new();
            // Inserted in a different order each time; BTreeMap sorts them.
            for id in ids.iter().rev() {
                m.insert(*id, "a.glb", "hash");
            }
            to_json(&m)
        };
        let forward = {
            let mut m = Manifest::new();
            for id in &ids {
                m.insert(*id, "a.glb", "hash");
            }
            to_json(&m)
        };

        assert_eq!(build(), forward, "insertion order must not matter");
    }
}
