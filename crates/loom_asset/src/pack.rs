//! One file instead of a tree — ADR 0089.
//!
//! **A shipped game should not be a directory of loose files.** A build tree is
//! fine to develop against and wrong to hand to someone: 341 files that can be
//! half-copied, individually corrupted, or edited by accident, with no way to
//! tell which happened. A pack is one file that is either there or not.
//!
//! The format is deliberately dull — a magic, a version, an index, then the
//! bytes:
//!
//! ```text
//! "LOOMPACK"  u32 version  u32 count
//! count x { u16 key_len, key, u64 offset, u64 length }
//! blobs
//! ```
//!
//! No compression, because the distribution archive already compresses and
//! decompressing twice to read one mesh is work nobody asked for. No memory
//! map, because a positional read per asset is simpler and assets load once.
//! Each read opens its own handle, so the pack is thread-safe by construction
//! rather than by a lock.
//!
//! **Keys are relative to the assets root**, so `assets/meshes/x.obj` is
//! `meshes/x.obj`. That is what lets a scene keep saying
//! `../meshes/gleamsprat_chrome.obj` whether or not it was packed: the path is
//! normalised, the `assets` component is found, and what follows is the key.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

const MAGIC: &[u8; 8] = b"LOOMPACK";
const VERSION: u32 = 1;

/// An opened pack: where it is, and what is in it.
pub struct Pack {
    path: PathBuf,
    index: BTreeMap<String, (u64, u64)>,
}

impl Pack {
    /// Read a pack's index. The blobs stay on disk.
    ///
    /// # Errors
    /// If the file will not open, is not a pack, or is a version this build
    /// does not know.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let mut magic = [0_u8; 8];
        file.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not a loom pack", path.display()),
            ));
        }
        let version = read_u32(&mut file)?;
        if version != VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is pack version {version}, this build reads {VERSION}", path.display()),
            ));
        }

        let count = read_u32(&mut file)?;
        let mut index = BTreeMap::new();
        for _ in 0..count {
            let key_len = read_u16(&mut file)? as usize;
            let mut key = vec![0_u8; key_len];
            file.read_exact(&mut key)?;
            let key = String::from_utf8(key)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let offset = read_u64(&mut file)?;
            let length = read_u64(&mut file)?;
            index.insert(key, (offset, length));
        }

        Ok(Self { path: path.to_path_buf(), index })
    }

    /// One entry's bytes, or `None` if the pack does not hold that key.
    ///
    /// # Errors
    /// If the pack cannot be re-opened or the blob will not read.
    pub fn read(&self, key: &str) -> Option<std::io::Result<Vec<u8>>> {
        let &(offset, length) = self.index.get(key)?;
        Some((|| {
            let mut file = std::fs::File::open(&self.path)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut bytes = vec![0_u8; usize::try_from(length).unwrap_or(0)];
            file.read_exact(&mut bytes)?;
            Ok(bytes)
        })())
    }

    /// How many entries it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether it holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

/// Pack every file under `root` into `out`, and report what went in.
///
/// # Errors
/// If the tree cannot be walked or the pack cannot be written.
pub fn write(root: &Path, out: &Path) -> std::io::Result<(usize, u64)> {
    let mut entries = Vec::new();
    collect(root, root, &mut entries)?;
    // Sorted, so packing the same tree twice gives the same bytes. A build
    // output that changes without its input changing is one nobody can check.
    entries.sort();

    let mut index_len = 8 + 4 + 4;
    for (key, _) in &entries {
        index_len += 2 + key.len() + 8 + 8;
    }

    let mut file = std::io::BufWriter::new(std::fs::File::create(out)?);
    file.write_all(MAGIC)?;
    file.write_all(&VERSION.to_le_bytes())?;
    file.write_all(&u32::try_from(entries.len()).unwrap_or(u32::MAX).to_le_bytes())?;

    let mut offset = index_len as u64;
    let mut total = 0_u64;
    for (key, path) in &entries {
        let length = std::fs::metadata(path)?.len();
        file.write_all(&u16::try_from(key.len()).unwrap_or(u16::MAX).to_le_bytes())?;
        file.write_all(key.as_bytes())?;
        file.write_all(&offset.to_le_bytes())?;
        file.write_all(&length.to_le_bytes())?;
        offset += length;
        total += length;
    }
    for (_, path) in &entries {
        let mut source = std::fs::File::open(path)?;
        std::io::copy(&mut source, &mut file)?;
    }
    file.flush()?;
    Ok((entries.len(), total))
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(root, &path, out)?;
        } else if let Ok(rest) = path.strip_prefix(root) {
            let key = rest
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect::<Vec<_>>()
                .join("/");
            out.push((key, path.clone()));
        }
    }
    Ok(())
}

/// The pack this process reads from, if it found one.
static MOUNTED: OnceLock<Option<Pack>> = OnceLock::new();

/// Mount `assets.pack` from beside the running executable, if it is there.
///
/// **Silent when there is none**, which is the developing case: a repository
/// checkout has a loose `assets/` tree and wants it. Called once at startup;
/// later calls do nothing.
pub fn mount_beside_exe() {
    MOUNTED.get_or_init(|| {
        let beside = std::env::current_exe().ok()?.parent()?.join("assets.pack");
        Pack::open(&beside).ok()
    });
}

/// A path's key, if it names something under an `assets` directory.
///
/// Normalised lexically first, so `games/../prefabs/x.loom` is
/// `prefabs/x.loom` — the scene format is full of those and they must not miss.
#[must_use]
pub fn key_for(path: &Path) -> Option<String> {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                parts.pop();
            }
            Component::CurDir => {}
            Component::Normal(part) => parts.push(part),
            Component::RootDir | Component::Prefix(_) => parts.clear(),
        }
    }
    // The *last* `assets`, so a checkout living in a directory called `assets`
    // does not swallow the whole path.
    let at = parts.iter().rposition(|part| *part == "assets")?;
    let key = parts[at + 1..]
        .iter()
        .filter_map(|part| part.to_str())
        .collect::<Vec<_>>()
        .join("/");
    (!key.is_empty()).then_some(key)
}

/// Read a file, from the mounted pack if it holds it and from the filesystem
/// if not.
///
/// # Errors
/// Whatever the pack or the filesystem says.
pub fn read_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    if let Some(pack) = MOUNTED.get().and_then(Option::as_ref)
        && let Some(key) = key_for(path)
        && let Some(bytes) = pack.read(&key)
    {
        return bytes;
    }
    // **Falls through to the filesystem on purpose.** A checkout has no pack
    // and wants its loose tree; a packed build that is missing one key should
    // say so in the language of the file that is not there.
    std::fs::read(path)
}

/// The same, as text.
///
/// # Errors
/// Whatever [`read_bytes`] says, or that the bytes are not UTF-8.
pub fn read_text(path: &Path) -> std::io::Result<String> {
    let bytes = read_bytes(path)?;
    String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn read_u16(file: &mut std::fs::File) -> std::io::Result<u16> {
    let mut buf = [0_u8; 2];
    file.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32(file: &mut std::fs::File) -> std::io::Result<u32> {
    let mut buf = [0_u8; 4];
    file.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(file: &mut std::fs::File) -> std::io::Result<u64> {
    let mut buf = [0_u8; 8];
    file.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_relative_to_the_assets_root() {
        assert_eq!(
            key_for(Path::new("/opt/deeper/assets/games/deeper_demo.loom")).as_deref(),
            Some("games/deeper_demo.loom")
        );
    }

    /// **The scene format is full of `..`.** A prefab declares
    /// `../prefabs/jib_vi.loom` against a base of `assets/games`, and if that
    /// did not normalise the pack would miss every prefab in the game.
    #[test]
    fn a_parent_hop_is_resolved_before_the_key_is_cut() {
        assert_eq!(
            key_for(Path::new("assets/games/../prefabs/jib_vi.loom")).as_deref(),
            Some("prefabs/jib_vi.loom")
        );
    }

    #[test]
    fn a_path_outside_any_assets_directory_has_no_key() {
        assert_eq!(key_for(Path::new("/etc/passwd")), None);
    }

    /// A pack written and read back holds what went in, byte for byte.
    #[test]
    fn a_pack_round_trips() {
        let root = std::env::temp_dir().join("loom-pack-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("meshes")).expect("temp dir");
        std::fs::write(root.join("meshes/a.obj"), b"v 0 0 0\n").expect("write");
        std::fs::write(root.join("b.loom"), b"[scene]\n").expect("write");

        let out = root.join("test.pack");
        let (count, bytes) = write(&root, &out).expect("write pack");
        assert_eq!(count, 2);
        assert_eq!(bytes, 8 + 8);

        let pack = Pack::open(&out).expect("open pack");
        assert_eq!(pack.len(), 2);
        assert_eq!(
            pack.read("meshes/a.obj").expect("present").expect("reads"),
            b"v 0 0 0\n"
        );
        assert_eq!(pack.read("b.loom").expect("present").expect("reads"), b"[scene]\n");
        assert!(pack.read("nothing.obj").is_none());

        let _ = std::fs::remove_dir_all(&root);
    }
}
