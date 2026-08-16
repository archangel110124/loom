//! Expanding a scene's prefab instances before a command reads it.
//!
//! The loading itself lives in `loom_scene::prefab` — `library_for` reads the
//! declared files, `resolve` expands against them. This is the thin layer that
//! says *when*: which commands need it, and what they do with the warnings.
//!
//! **Going through here is a correctness requirement, not tidiness.** Before
//! S4 the parser refused `prefab` outright, because a key it does not
//! understand is a key it *ignores*: the instance node arrived with no
//! components at all, drew nothing, lit nothing, and the scene validated
//! clean. Now that the parser accepts the key, a command that reads a scene
//! and skips resolution reintroduces exactly that bug.

use std::path::Path;

use loom_scene::prefab;
use loom_scene::{Scene, SceneError};

/// Expand a parsed scene for *consumption*, discarding the warnings.
///
/// For the read-only commands — render, sim, measure — which want a scene to
/// look at and have no channel for a warning. `validate` uses
/// [`for_reading_with_warnings`] and reports them, which is where an author is
/// actually looking.
///
/// **A scene with no prefabs is returned unchanged**, so routing a command
/// through here costs nothing.
///
/// # Errors
/// Whatever loading or resolution rejected.
pub(crate) fn for_reading(scene: &Scene, path: &Path) -> Result<Scene, Vec<SceneError>> {
    for_reading_with_warnings(scene, path).map(|(scene, _)| scene)
}

/// As [`for_reading`], for a caller that holds the scene's *directory* rather
/// than its file path.
///
/// `prefab::library_for` takes the scene file and calls `.parent()` on it;
/// `SceneView` has already taken that parent and kept only the base, because
/// every asset path it resolves is relative to it. Re-attaching a filename
/// recovers exactly what `library_for` wants **without inventing a second
/// resolution rule** — which is the thing to avoid here, since a prefab that
/// resolved differently in the editor than on the CLI would be far worse than
/// the bug this exists to fix.
///
/// # Errors
/// Whatever loading or resolution rejected.
pub(crate) fn for_reading_in_dir(scene: &Scene, base: &Path) -> Result<Scene, Vec<SceneError>> {
    for_reading(scene, &base.join("scene.loom"))
}

/// As [`for_reading`], keeping the orphaned-override warnings.
///
/// # Errors
/// Whatever loading or resolution rejected.
pub(crate) fn for_reading_with_warnings(
    scene: &Scene,
    path: &Path,
) -> Result<(Scene, Vec<SceneError>), Vec<SceneError>> {
    if scene.prefabs().is_empty() {
        return Ok((scene.clone(), Vec::new()));
    }
    let library = prefab::library_for(scene, path)?;
    prefab::resolve(scene, &library).map(|resolved| (resolved.scene, resolved.warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Write a prefab and a scene that places it twice, and load the pair off
    /// disk exactly as a command does.
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
    fn load(path: &Path) -> Result<(Scene, Vec<SceneError>), Vec<SceneError>> {
        let src = std::fs::read_to_string(path).expect("read scene");
        let scene = Scene::parse(&src)?;
        for_reading_with_warnings(&scene, path)
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

        let (resolved, warnings) = load(&scene).expect("loads");

        let paths: Vec<&str> = resolved.nodes().iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, ["Room", "Room/A", "Room/B"]);
        assert_eq!(resolved.nodes()[1].components["Light"]["intensity"], 100.0);
        assert_eq!(resolved.nodes()[2].components["Light"]["intensity"], 7.0);
        assert!(warnings.is_empty(), "{warnings:?}");
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
        let (resolved, _) = load(&scene).expect("still loads");

        assert_eq!(
            resolved.nodes()[1].components["Light"]["intensity"],
            400.0,
            "the un-overridden instance followed the edit"
        );
        assert_eq!(
            resolved.nodes()[2].components["Light"]["intensity"],
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
