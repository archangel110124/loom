//! Everything derived from scene text: meshes, draw calls, pick boxes, bounds.
//!
//! This exists so the derivation can be **re-run**. Until now it happened once
//! in `open_scene` and the results were moved into the window, which meant the
//! viewport showed the scene as it was when the window opened and nothing
//! after — neither the human's own edits nor the agent's writes.
//!
//! One type, rebuilt from text, is what makes the viewer a live view of the
//! file rather than a screenshot of it.

use std::collections::BTreeMap;

use loom_ecs::World;
use loom_render::glam::Vec3;
use loom_render::Object;
use loom_scene::Scene;

/// One node's worth of difference between two versions of a scene.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
}

/// What happened to a node. Deliberately coarse: the human wants to know where
/// to look, and the transaction label already says why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Removed,
    Moved,
    Edited,
    MovedAndEdited,
}

impl ChangeKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Moved => "moved",
            Self::Edited => "edited",
            Self::MovedAndEdited => "moved + edited",
        }
    }
}

/// The renderable form of a scene.
pub struct SceneView {
    pub scene: Scene,
    pub objects: Vec<Object>,
    /// Kept rather than consumed, so play mode can turn a *simulated* world
    /// into draw calls with the same meshes the authored one uses.
    library: crate::MeshLibrary,
    /// Kept for the same reason as `library`: play mode redraws a simulated
    /// world and must dress it in the authored scene's materials.
    materials: crate::materials::MaterialLibrary,
    /// World AABB per node, for picking and framing.
    pub picks: BTreeMap<String, loom_scene::place::Bounds>,
    /// Centre and radius of everything.
    pub bounds: (Vec3, f32),
    /// Node paths in scene order — the hierarchy panel's rows.
    pub paths: Vec<String>,
    /// Asset aliases this scene resolved — the asset browser's rows.
    pub assets: Vec<String>,
    /// Identity of the *mesh set*. Equal keys mean the GPU buffers are still
    /// valid, so a transform edit costs no re-upload while a new asset or a
    /// re-baked voxel volume does.
    pub mesh_key: u64,
    /// The resolved hierarchy. Kept because anything that edits a node's
    /// transform needs its parent's global to get from world space — where the
    /// gizmo and the camera work — into the local space the file stores.
    world: World,
}

impl SceneView {
    /// Derive everything from scene text.
    ///
    /// # Errors
    /// The validation errors, as JSON, when the text is not a valid scene.
    pub fn build(text: &str, base: &std::path::Path) -> Result<Self, String> {
        Self::build_cached(text, base, &mut crate::VoxelCache::default())
    }

    /// As [`Self::build`], reusing voxel meshes whose recipe has not changed.
    ///
    /// # Errors
    /// The validation errors, as JSON, when the text is not a valid scene.
    pub fn build_cached(
        text: &str,
        base: &std::path::Path,
        cache: &mut crate::VoxelCache,
    ) -> Result<Self, String> {
        let scene = Scene::parse(text).map_err(|errors| {
            serde_json::to_string_pretty(&serde_json::json!({ "errors": errors }))
                .unwrap_or_else(|_| "invalid scene".to_owned())
        })?;
        let world = World::from_scene(&scene);
        let library = crate::MeshLibrary::with_cache(&scene, base, cache);
        let materials = crate::materials::MaterialLibrary::for_scene(&world, &scene, base);

        let objects = crate::world_to_objects(&world, &library, &materials);
        let picks = crate::node_bounds(&world, &library);
        let bounds = crate::scene_bounds(&picks);
        let paths = scene.nodes().iter().map(|n| n.path.clone()).collect();
        let mesh_key = library.key();
        let assets = library.names().map(str::to_owned).collect();

        Ok(Self {
            scene,
            objects,
            library,
            materials,
            picks,
            bounds,
            paths,
            assets,
            mesh_key,
            world,
        })
    }

    /// The textures this scene's materials name, for the renderer to upload.
    #[must_use]
    pub fn textures(&self) -> &[loom_asset::Texture] {
        &self.materials.textures
    }

    /// The material table this scene's objects index into.
    #[must_use]
    pub fn material_table(&self) -> &[loom_render::MaterialData] {
        &self.materials.materials
    }

    /// The authored world, for anything that has to read components the draw
    /// list does not carry — particle emitters, for one.
    #[must_use]
    pub fn world(&self) -> &World {
        &self.world
    }

    /// The geometry this scene needs, for the renderer to upload.
    #[must_use]
    pub fn meshes(&self) -> &[loom_asset::Mesh] {
        self.library.meshes()
    }

    /// Draw calls for a world that is not the authored one — play mode's
    /// simulated transforms, drawn with the authored scene's meshes.
    #[must_use]
    pub fn objects_of(&self, world: &World) -> Vec<Object> {
        crate::world_to_objects(world, &self.library, &self.materials)
    }

    /// Which nodes differ between two versions of a scene, and how.
    ///
    /// The point of an AI-native editor is that somebody else is authoring the
    /// file while you watch it. "The scene changed on disk" is true and
    /// useless; *what* changed is the thing a human needs, and the scene is
    /// structured text, so it can be said exactly.
    #[must_use]
    pub fn changes_from(&self, previous: &[loom_scene::Node]) -> Vec<Change> {
        let mut out = Vec::new();
        for node in self.scene.nodes() {
            match previous.iter().find(|n| n.path == node.path) {
                None => out.push(Change {
                    path: node.path.clone(),
                    kind: ChangeKind::Added,
                }),
                Some(before) => {
                    let moved = before.transform.pos != node.transform.pos
                        || before.transform.rot_euler != node.transform.rot_euler
                        || before.transform.scale != node.transform.scale;
                    let recomposed = before.components != node.components;
                    if moved || recomposed {
                        out.push(Change {
                            path: node.path.clone(),
                            kind: if moved && recomposed {
                                ChangeKind::MovedAndEdited
                            } else if moved {
                                ChangeKind::Moved
                            } else {
                                ChangeKind::Edited
                            },
                        });
                    }
                }
            }
        }
        for node in previous {
            if !self.scene.nodes().iter().any(|n| n.path == node.path) {
                out.push(Change {
                    path: node.path.clone(),
                    kind: ChangeKind::Removed,
                });
            }
        }
        out
    }

    /// The transform that takes a point from world space into `path`'s parent's
    /// space — identity when the node is at the root.
    ///
    /// A gizmo drags along a world axis and a node stores a *local* transform.
    /// Without this the two are the same only for root-level nodes; under a
    /// turned or scaled parent the node moved in the wrong direction and by the
    /// wrong amount.
    #[must_use]
    pub fn parent_inverse(&self, path: &str) -> loom_render::glam::Mat4 {
        use loom_render::glam::Mat4;
        self.world
            .entities()
            .iter()
            .find(|e| self.world.path(**e) == Some(path))
            .and_then(|e| self.world.parent(*e))
            .and_then(|parent| self.world.global_transform(parent))
            // One helper, so all three places that need a parent's inverse
            // agree about what "not invertible" means.
            .map_or(Mat4::IDENTITY, |g| {
                crate::play::invertible_parent(Mat4::from_cols_array(&g.matrix))
            })
    }

    /// The viewpoint this scene authors, if it authors one.
    #[must_use]
    pub fn camera(&self) -> Option<loom_ecs::CameraView> {
        self.world.active_camera()
    }

    /// The world AABB of a node, if it draws anything.
    #[must_use]
    pub fn node_bounds(&self, path: &str) -> Option<&loom_scene::place::Bounds> {
        self.picks.get(path)
    }

    /// A node's local transform, read back from the scene rather than tracked
    /// separately — a second copy of the truth is a second answer.
    #[must_use]
    pub fn transform_of(&self, path: &str) -> Option<&loom_scene::components::Transform> {
        self.scene
            .nodes()
            .iter()
            .find(|n| n.path == path)
            .map(|n| &n.transform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = r#"
[scene]
format = 1
id = "0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f"

[[node]]
name = "Room"

[[node]]
name = "Desk"
parent = "Room"
transform = { pos = [0.0, 0.0, 0.0] }

  [node.components.MeshRenderer]
  mesh = { asset = "box" }
"#;

    fn view(text: &str) -> SceneView {
        SceneView::build(text, std::path::Path::new(".")).expect("valid scene")
    }

    fn translation(view: &SceneView) -> Vec3 {
        view.objects[0].model.w_axis.truncate()
    }

    /// The point of the whole module: an edit to the text — from a human or
    /// from the agent — changes what gets drawn.
    #[test]
    fn a_moved_node_moves_its_object() {
        let before = view(SCENE);
        let after = view(&SCENE.replace("pos = [0.0, 0.0, 0.0]", "pos = [5.0, 0.0, 0.0]"));

        assert_eq!(translation(&before).x, 0.0);
        assert_eq!(translation(&after).x, 5.0, "the viewport must follow the file");
    }

    #[test]
    fn a_spawned_node_appears_in_the_view() {
        let before = view(SCENE);
        let after = view(&format!(
            "{SCENE}\n[[node]]\nname = \"Lamp\"\nparent = \"Room\"\n\n  [node.components.MeshRenderer]\n  mesh = {{ asset = \"box\" }}\n"
        ));

        assert_eq!(after.objects.len(), before.objects.len() + 1);
        assert!(after.paths.contains(&"Room/Lamp".to_owned()));
        assert!(after.picks.contains_key("Room/Lamp"));
    }

    /// Moving something must not force a GPU re-upload — otherwise every frame
    /// of a slider drag stalls the device.
    #[test]
    fn moving_a_node_leaves_the_mesh_key_alone() {
        let before = view(SCENE);
        let after = view(&SCENE.replace("pos = [0.0, 0.0, 0.0]", "pos = [5.0, 0.0, 0.0]"));

        assert_eq!(before.mesh_key, after.mesh_key);
    }

    /// A re-baked voxel volume produces different geometry under the same
    /// alias, so the key must notice.
    #[test]
    fn a_changed_voxel_op_list_changes_the_mesh_key() {
        let cave = std::fs::read_to_string("../../assets/test/cave.loom").expect("fixture");
        let base = std::path::Path::new("../../assets/test");

        let small = SceneView::build(&cave, base).expect("valid");
        // Bore a wider tunnel: same node, same alias, different surface.
        let large = SceneView::build(&cave.replacen("radius = 2.0", "radius = 3.0", 1), base)
            .expect("valid");

        assert_ne!(small.mesh_key, large.mesh_key, "new geometry, new buffers");
    }

    /// A gizmo drags along a world axis and the file stores a local transform.
    /// Under a parent turned 90 degrees about Y, a world +X drag has to become
    /// a purely local +Z one — that rotation carries local +Z onto world +X —
    /// and adding the travel straight to the local x moved the node sideways
    /// relative to what the handle was pointing at.
    #[test]
    fn the_parent_inverse_turns_a_world_drag_into_a_local_one() {
        let scene = r#"
[scene]
format = 1
id = "0f9c1a3e-4b2d-4c1a-9e7f-8a1b2c3d4e5f"

[[node]]
name = "Room"

[[node]]
name = "Rig"
parent = "Room"
transform = { rot_euler = [0.0, 90.0, 0.0] }

[[node]]
name = "Desk"
parent = "Room/Rig"

  [node.components.MeshRenderer]
  mesh = { asset = "box" }
"#;
        let view = view(scene);
        let world_x = loom_render::glam::Vec3::new(1.0, 0.0, 0.0);
        let local = view.parent_inverse("Room/Rig/Desk").transform_vector3(world_x);

        assert!(local.x.abs() < 1e-4, "no local x component: {local:?}");
        assert!((local.z - 1.0).abs() < 1e-4, "world +X is local +Z here: {local:?}");

        // A root-level node is its own parent space.
        let root = view.parent_inverse("Room").transform_vector3(world_x);
        assert!((root.x - 1.0).abs() < 1e-4, "identity at the root: {root:?}");
    }

    #[test]
    fn an_invalid_scene_is_an_error_not_a_panic() {
        assert!(SceneView::build("not a scene", std::path::Path::new(".")).is_err());
    }
}
