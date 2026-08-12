//! Loading a scene with its prefabs resolved.
//!
//! **`loom_scene` deliberately cannot read files.** It depends on nothing else
//! in the workspace and `Scene::parse` takes a string, which is what makes it
//! testable without a filesystem and keeps `cargo check` fast. Resolution
//! therefore takes a [`Library`] someone else filled — and filling it from
//! disk is this module's whole job.
//!
//! **Every production load goes through here**, and that is a correctness
//! requirement rather than tidiness. Before S4 the parser refused `prefab`
//! outright, because a key it does not understand is a key it *ignores*: the
//! instance node arrived with no components at all, drew nothing, lit nothing,
//! and the scene validated clean. Now that the parser accepts the key, a load
//! path that skips resolution reintroduces exactly that bug.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use loom_scene::prefab::{self, Library};
use loom_scene::{Scene, SceneError};

/// Expand an already-parsed scene, reading its prefabs from disk.
///
/// Takes a parsed scene rather than a path because every caller already has
/// the text — commands read it to report a version token, and the editor polls
/// it — so a read-it-again entry point would be a second way to do the same
/// thing with its own chance of drifting.
///
/// `path` locates the scene; prefab `path` hints resolve relative to the
/// directory holding it.
///
/// # Errors
/// Parse or validation failures of any prefab it pulls in, a prefab file that
/// cannot be read, or an instancing cycle.
pub(crate) fn resolve_from(scene: &Scene, path: &Path) -> Result<prefab::Resolved, Vec<SceneError>> {
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut library = Library::new();
    let mut seen = BTreeSet::new();
    collect(scene, base, &mut library, &mut seen)?;
    prefab::resolve(scene, &library)
}

/// Expand a parsed scene for *consumption*, discarding the warnings.
///
/// For the read-only commands — render, sim, measure — which want a scene to
/// look at and have no channel for a warning. `validate` uses
/// [`resolve_from`] directly and reports them, which is where an author is
/// actually looking for them.
///
/// **A scene with no prefabs is returned unchanged**, so routing a command
/// through here costs nothing and closes the hole where an unexpanded
/// instance reaches a consumer as a node with no components.
///
/// # Errors
/// Whatever resolution rejected.
pub(crate) fn for_reading(scene: &Scene, path: &Path) -> Result<Scene, Vec<SceneError>> {
    for_reading_with_warnings(scene, path).map(|(scene, _)| scene)
}

/// As [`for_reading`], keeping the orphaned-override warnings.
///
/// # Errors
/// Whatever resolution rejected.
pub(crate) fn for_reading_with_warnings(
    scene: &Scene,
    path: &Path,
) -> Result<(Scene, Vec<SceneError>), Vec<SceneError>> {
    if scene.prefabs().is_empty() {
        return Ok((scene.clone(), Vec::new()));
    }
    resolve_from(scene, path).map(|resolved| (resolved.scene, resolved.warnings))
}

/// Load every prefab a scene declares, and every prefab *those* declare.
///
/// Depth-first, guarded by `seen` on the prefab id. The guard is for repeated
/// work, not for cycles: a cycle is a *scene* error with a path to report, and
/// swallowing it here would turn a nameable mistake into a missing prefab.
fn collect(
    scene: &Scene,
    base: &Path,
    library: &mut Library,
    seen: &mut BTreeSet<String>,
) -> Result<(), Vec<SceneError>> {
    for decl in scene.prefabs() {
        if !seen.insert(decl.id.clone()) {
            continue;
        }

        // The path is a hint for *finding* the file, once. Identity stays the
        // id, which is what the library is keyed by (§3, the Unity lesson) —
        // so moving a prefab breaks one hint rather than every reference.
        let file = resolve_path(base, &decl.path);
        let text = std::fs::read_to_string(&file).map_err(|e| {
            let mut err = io_error(&file, &e.to_string());
            err.field = "prefab".to_owned();
            err.value = serde_json::Value::from(decl.key.clone());
            err.hint = Some(format!(
                "the prefab `{}` (id {}) declares `path = \"{}\"`, which does \
                 not read from {}.",
                decl.key,
                decl.id,
                decl.path,
                base.display()
            ));
            vec![err]
        })?;
        let parsed = Scene::parse(&text)?;

        // Nested prefabs are loaded relative to *their own* file, so a prefab
        // that instances another keeps working wherever it is placed from.
        let nested_base = file.parent().unwrap_or(base).to_path_buf();
        collect(&parsed, &nested_base, library, seen)?;
        library.insert(decl.id, parsed);
    }
    Ok(())
}

fn resolve_path(base: &Path, hint: &str) -> PathBuf {
    let candidate = Path::new(hint);
    if candidate.is_absolute() { candidate.to_path_buf() } else { base.join(candidate) }
}

fn io_error(path: &Path, message: &str) -> SceneError {
    SceneError::external("io_error", &format!("{}: {message}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a prefab and a scene that places it twice, and load the pair off
    /// disk exactly as the CLI does.
    fn fixture(dir: &Path) -> PathBuf {
        std::fs::write(
            dir.join("lamp.loom"),
            "[scene]\nformat = 1\n\n[[node]]\nname = \"Lamp\"\n\n  \
             [node.components.Light]\n  intensity = 100.0\n",
        )
        .expect("write prefab");

        let scene = dir.join("room.loom");
        std::fs::write(
            &scene,
            "[scene]\nformat = 1\n\n[[prefab]]\nkey = \"lamp\"\n\
             id = \"9a7c1e40-2b8d-4f16-9053-6c1ea4b7d820\"\npath = \"lamp.loom\"\n\n\
             [[node]]\nname = \"Room\"\n\n\
             [[node]]\nname = \"A\"\nparent = \"Room\"\nprefab = \"lamp\"\n\n\
             [[node]]\nname = \"B\"\nparent = \"Room\"\nprefab = \"lamp\"\n\n  \
             [node.overrides]\n  \"Light.intensity\" = 7.0\n",
        )
        .expect("write scene");
        scene
    }

    /// Read and expand, the way a command does.
    fn load(path: &Path) -> Result<prefab::Resolved, Vec<SceneError>> {
        let src = std::fs::read_to_string(path).expect("read scene");
        let scene = Scene::parse(&src)?;
        resolve_from(&scene, path)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("loom-prefab-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// The end-to-end shape: a prefab in its own file, instanced twice, one
    /// instance overridden.
    #[test]
    fn a_scene_loads_with_its_prefabs_expanded() {
        let dir = scratch("expand");
        let scene = fixture(&dir);

        let resolved = load(&scene).expect("loads");

        let paths: Vec<&str> = resolved.scene.nodes().iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, ["Room", "Room/A", "Room/B"]);
        assert_eq!(resolved.scene.nodes()[1].components["Light"]["intensity"], 100.0);
        assert_eq!(resolved.scene.nodes()[2].components["Light"]["intensity"], 7.0);
        assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    }

    /// **Editing the prefab file moves every instance**, which is the whole
    /// point and the thing a copy-on-place design cannot do.
    #[test]
    fn editing_the_prefab_file_updates_the_instances() {
        let dir = scratch("edit");
        let scene = fixture(&dir);

        std::fs::write(
            dir.join("lamp.loom"),
            "[scene]\nformat = 1\n\n[[node]]\nname = \"Lamp\"\n\n  \
             [node.components.Light]\n  intensity = 400.0\n",
        )
        .expect("edit prefab");
        let resolved = load(&scene).expect("still loads");

        assert_eq!(
            resolved.scene.nodes()[1].components["Light"]["intensity"],
            400.0,
            "the un-overridden instance followed the edit"
        );
        assert_eq!(
            resolved.scene.nodes()[2].components["Light"]["intensity"],
            7.0,
            "and the overridden one kept its own value"
        );
    }

    /// A `path` that does not read names the prefab, the hint and the
    /// directory — the three things needed to fix it.
    #[test]
    fn a_missing_prefab_file_says_which_and_where() {
        let dir = scratch("missing");
        let scene = fixture(&dir);
        std::fs::remove_file(dir.join("lamp.loom")).expect("remove prefab");

        let errors = load(&scene).expect_err("the prefab is gone");

        assert_eq!(errors[0].error, "io_error");
        let hint = errors[0].hint.as_deref().unwrap_or_default();
        assert!(hint.contains("lamp") && hint.contains("lamp.loom"), "{hint}");
    }
}
