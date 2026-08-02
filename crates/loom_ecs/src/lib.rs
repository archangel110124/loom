//! Entity storage, transform propagation, and the fixed-timestep loop.
//!
//! Determinism is the whole point of this crate (brief §7.5). It is a
//! *verification* concern here, not a networking one: `loom sim --assert` is
//! one of the two feedback channels the agent has, and a non-deterministic
//! simulation makes every assertion flaky — which trains the agent to ignore
//! failures. That is worse than having no assertions at all.
//!
//! So: no `HashMap` iteration, no `thread_rng`, no wall clock. `clippy.toml`
//! enforces all three mechanically rather than by vigilance.

use std::collections::BTreeMap;

use loom_scene::components::Transform;

/// A handle to an entity.
///
/// Generational: reusing a slot bumps the generation, so a stale handle fails
/// loudly instead of silently addressing whatever now lives there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    #[must_use]
    pub fn index(self) -> u32 {
        self.index
    }
}

/// Where a camera sits and what it looks at, resolved from its node.
///
/// Deliberately plain arrays and not the renderer's `Camera`: `loom_ecs` must
/// not depend on `loom_render`, which would drag `ash` into every crate that
/// simulates anything (the dependency rule in CLAUDE.md). The CLI converts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraView {
    /// World position of the eye.
    pub eye: [f32; 3],
    /// A point one metre in front of the eye, along the node's −Z.
    pub target: [f32; 3],
    pub fov_y_degrees: f32,
}

/// Dense per-entity storage for one component type.
///
/// `ponytail:` `Vec<Option<T>>` indexed by entity, not archetype storage.
/// Iteration walks holes where an entity lacks the component, so a query over
/// a sparse component costs O(entities) rather than O(matches). At blockout
/// scale that is nothing.
///
/// Upgrade path when it stops being nothing — profile first, at M10 when voxel
/// chunks arrive: group entities by component set and store each group's
/// components contiguously (design doc §2.5). `World`'s public API is the seam
/// and does not change, because callers only ever see spawn/get/set.
#[derive(Debug, Clone)]
struct Storage<T> {
    items: Vec<Option<T>>,
}

// Hand-written: `#[derive(Default)]` would demand `T: Default`, which an empty
// storage plainly does not need.
impl<T> Default for Storage<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<T> Storage<T> {
    fn insert(&mut self, entity: Entity, value: T) {
        let index = entity.index as usize;
        if index >= self.items.len() {
            self.items.resize_with(index + 1, || None);
        }
        self.items[index] = Some(value);
    }

    fn get(&self, entity: Entity) -> Option<&T> {
        self.items.get(entity.index as usize)?.as_ref()
    }

    fn get_mut(&mut self, entity: Entity) -> Option<&mut T> {
        self.items.get_mut(entity.index as usize)?.as_mut()
    }
}

/// A transform resolved into world space by [`World::propagate_transforms`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobalTransform {
    /// Column-major 4x4, matching the render path's convention.
    pub matrix: [f32; 16],
}

impl Default for GlobalTransform {
    fn default() -> Self {
        Self {
            matrix: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        }
    }
}

/// The simulation world.
///
/// Only the components M3 needs. New types get a field here until there are
/// enough to justify type-erased storage — which is the point at which
/// archetypes become worth building.
#[derive(Debug, Default, Clone)]
pub struct World {
    generations: Vec<u32>,
    alive: Vec<bool>,
    free: Vec<u32>,

    names: Storage<String>,
    local: Storage<Transform>,
    global: Storage<GlobalTransform>,
    parents: Storage<Entity>,
    /// Tag component: presence is the whole payload.
    renderable: Storage<()>,
    /// The `VoxelVolume` component, verbatim, for whoever needs to rebuild the
    /// field. Carried rather than baked here so `loom_ecs` stays free of a
    /// dependency on the voxel crate; physics and rendering each bake their
    /// own from the same recipe (never-do #11: the recipe, never the voxels).
    voxel_recipe: Storage<serde_json::Value>,
    /// Authored `BoxCollider` half-extents, when a node declares them.
    collider: Storage<[f32; 3]>,
    /// The asset alias a node's `MeshRenderer` names.
    mesh_asset: Storage<String>,
    /// The `ParticleEmitter` component, verbatim. Carried for the same reason
    /// as `material`: resolving it needs crates this one must not depend on.
    emitter: Storage<serde_json::Value>,
    /// The `CharacterController` component, verbatim. Carried rather than
    /// resolved for the same reason as `material`: turning it into a capsule
    /// needs `loom_physics`, and this crate does not depend on it.
    character: Storage<serde_json::Value>,
    /// The `Hud` component, verbatim. Resolving it needs egui, which lives
    /// several crates away.
    hud: Storage<serde_json::Value>,
    /// The `Blast` component, verbatim. Carried for the same reason as the
    /// rest: setting it off needs `loom_physics`, which this crate must not
    /// depend on.
    blast: Storage<serde_json::Value>,
    /// The `Material` component, verbatim. Carried rather than resolved here
    /// for the same reason as `voxel_recipe`: resolving it needs the asset and
    /// render crates, and this one depends on neither.
    material: Storage<serde_json::Value>,
    /// Scene path, so callers can address an entity the way a `.loom` file does.
    paths: Storage<String>,
    /// The `.rhai` file a node's `Script` component names.
    scripts: Storage<String>,
    /// The `.rhai` file a node's `GameRules` component names. Separate from
    /// `scripts` because it is a different entry point with a different
    /// contract, and one node could legitimately carry both.
    rules: Storage<String>,
    /// `RigidBody`: whether it falls, and its mass.
    bodies: Storage<(bool, f32)>,
    /// Field of view for every node carrying an active `Camera`. Resolved on
    /// load rather than carried verbatim like `material`: it is two numbers
    /// and needs no crate this one lacks.
    camera_fov: Storage<f32>,
    /// The camera the view comes from, chosen once when the scene loads.
    ///
    /// Cached because this is read *per frame* and the alternative is a scan
    /// over every entity to find the one node in a hundred thousand that has
    /// a camera. Only which node is fixed here — where it is pointing is read
    /// live from its global transform, so a camera parented to a moving head
    /// still moves.
    active_camera: Option<Entity>,
    /// Insertion order. Iterated instead of a `HashMap` so every traversal is
    /// reproducible — the most common source of "works on my machine"
    /// nondeterminism in Rust engines, and it hides for months (§7.5).
    order: Vec<Entity>,
}

impl World {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an entity, reusing a free slot when one exists.
    pub fn spawn(&mut self) -> Entity {
        if let Some(index) = self.free.pop() {
            let slot = index as usize;
            self.generations[slot] += 1;
            self.alive[slot] = true;
            let entity = Entity {
                index,
                generation: self.generations[slot],
            };
            self.order.push(entity);
            return entity;
        }

        let index = u32::try_from(self.generations.len()).unwrap_or(u32::MAX);
        self.generations.push(0);
        self.alive.push(true);
        let entity = Entity {
            index,
            generation: 0,
        };
        self.order.push(entity);
        entity
    }

    /// Whether a handle still refers to a live entity.
    #[must_use]
    pub fn is_alive(&self, entity: Entity) -> bool {
        let slot = entity.index as usize;
        self.alive.get(slot).copied().unwrap_or(false)
            && self.generations.get(slot) == Some(&entity.generation)
    }

    /// Remove an entity and free its slot.
    pub fn despawn(&mut self, entity: Entity) {
        if !self.is_alive(entity) {
            return;
        }
        let slot = entity.index as usize;
        self.alive[slot] = false;
        self.free.push(entity.index);
        self.order.retain(|e| *e != entity);
    }

    /// Entities in creation order. Deterministic by construction.
    #[must_use]
    pub fn entities(&self) -> &[Entity] {
        &self.order
    }

    pub fn set_name(&mut self, entity: Entity, name: impl Into<String>) {
        self.names.insert(entity, name.into());
    }

    #[must_use]
    pub fn name(&self, entity: Entity) -> Option<&str> {
        self.names.get(entity).map(String::as_str)
    }

    pub fn set_transform(&mut self, entity: Entity, transform: Transform) {
        self.local.insert(entity, transform);
        self.global.insert(entity, GlobalTransform::default());
    }

    #[must_use]
    pub fn transform(&self, entity: Entity) -> Option<&Transform> {
        self.local.get(entity)
    }

    pub fn transform_mut(&mut self, entity: Entity) -> Option<&mut Transform> {
        self.local.get_mut(entity)
    }

    pub fn set_parent(&mut self, child: Entity, parent: Entity) {
        self.parents.insert(child, parent);
    }

    #[must_use]
    pub fn parent(&self, entity: Entity) -> Option<Entity> {
        self.parents.get(entity).copied()
    }

    #[must_use]
    pub fn global_transform(&self, entity: Entity) -> Option<&GlobalTransform> {
        self.global.get(entity)
    }

    /// Resolve `Transform` into `GlobalTransform` for every entity.
    ///
    /// Entities are visited in creation order and parents are created before
    /// children (the scene format's forward-reference rule guarantees it), so
    /// one pass suffices — no sort, no recursion, no revisiting.
    pub fn propagate_transforms(&mut self) {
        let mut resolved: BTreeMap<u32, [f32; 16]> = BTreeMap::new();

        for entity in self.order.clone() {
            let Some(local) = self.local.get(entity) else {
                continue;
            };
            let matrix = compose(local);
            let world = match self.parents.get(entity).copied() {
                // A parent with no transform of its own is treated as identity
                // rather than skipped, so a bare grouping node does not
                // silently drop its whole subtree out of the world.
                Some(parent) => resolved
                    .get(&parent.index)
                    .map_or(matrix, |parent_matrix| multiply(*parent_matrix, matrix)),
                None => matrix,
            };
            resolved.insert(entity.index, world);
            self.global.insert(entity, GlobalTransform { matrix: world });
        }
    }

    /// Build a world from a parsed scene.
    ///
    /// This is where the scene tree becomes runtime state: authors and agents
    /// see Godot, the CPU sees flat storage (design doc §2.2). Nodes are
    /// declared parents-first, so parents already exist when children arrive.
    #[must_use]
    pub fn from_scene(scene: &loom_scene::Scene) -> Self {
        let mut world = Self::new();
        let mut by_path: BTreeMap<&str, Entity> = BTreeMap::new();

        for node in scene.nodes() {
            let entity = world.spawn();
            world.set_name(entity, node.name.clone());
            world.set_transform(entity, node.transform.clone());
            if let Some(parent) = node.parent.as_deref().and_then(|p| by_path.get(p)) {
                world.set_parent(entity, *parent);
            }
            world.paths.insert(entity, node.path.clone());
            if let Some(body) = node.components.get("RigidBody") {
                let dynamic = body.get("dynamic").and_then(|d| d.as_bool()).unwrap_or(false);
                #[allow(clippy::cast_possible_truncation)]
                let mass = body.get("mass").and_then(|m| m.as_f64()).unwrap_or(1.0) as f32;
                world.bodies.insert(entity, (dynamic, mass));
            }
            if let Some(path) = node
                .components
                .get("Script")
                .and_then(|s| s.get("path"))
                .and_then(|p| p.as_str())
            {
                world.scripts.insert(entity, path.to_owned());
            }
            by_path.insert(node.path.as_str(), entity);
            if let Some(half) = node
                .components
                .get("BoxCollider")
                .and_then(|c| c.get("half_extents"))
                .and_then(serde_json::Value::as_array)
            {
                #[allow(clippy::cast_possible_truncation)]
                let values: Vec<f32> = half
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect();
                if let [x, y, z] = values[..] {
                    world.collider.insert(entity, [x, y, z]);
                }
            }
            if let Some(camera) = node.components.get("Camera") {
                // Absent `active` means active: a camera is written to be
                // used, and the flag exists to switch a spare one *off*.
                let active = camera
                    .get("active")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true);
                if active {
                    #[allow(clippy::cast_possible_truncation)]
                    let fov = camera
                        .get("fov_y_degrees")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(60.0) as f32;
                    world.camera_fov.insert(entity, fov);
                    // First active one wins, in file order. Deterministic and
                    // explainable; picking "the last" would mean appending a
                    // node silently stole the view.
                    world.active_camera.get_or_insert(entity);
                }
            }
            if let Some(character) = node.components.get("CharacterController") {
                world.character.insert(entity, character.clone());
            }
            if let Some(path) = node
                .components
                .get("GameRules")
                .and_then(|s| s.get("path"))
                .and_then(|p| p.as_str())
            {
                world.rules.insert(entity, path.to_owned());
            }
            if let Some(hud) = node.components.get("Hud") {
                world.hud.insert(entity, hud.clone());
            }
            if let Some(blast) = node.components.get("Blast") {
                world.blast.insert(entity, blast.clone());
            }
            if let Some(emitter) = node.components.get("ParticleEmitter") {
                world.emitter.insert(entity, emitter.clone());
            }
            if let Some(material) = node.components.get("Material") {
                world.material.insert(entity, material.clone());
            }
            if let Some(volume) = node.components.get("VoxelVolume") {
                world.mark_renderable(entity);
                world.voxel_recipe.insert(entity, volume.clone());
            }
            if let Some(renderer) = node.components.get("MeshRenderer") {
                world.mark_renderable(entity);
                if let Some(asset) = renderer
                    .get("mesh")
                    .and_then(|m| m.get("asset"))
                    .and_then(|a| a.as_str())
                {
                    world.mesh_asset.insert(entity, asset.to_owned());
                }
            }
        }

        world.propagate_transforms();
        world
    }

    /// The viewpoint this scene authored, if it authored one.
    ///
    /// O(1): the node was chosen at load, and this reads its already-computed
    /// global transform. Cheap enough to call every frame, which is the point
    /// — a camera parented to a moving player has to be re-read every frame or
    /// it does not follow.
    ///
    /// `None` when the scene has no camera, and also when the one it has has
    /// been scaled to nothing. Both mean "fall back to framing the bounds",
    /// which shows the scene; a NaN view direction would show nothing and say
    /// nothing about why.
    #[must_use]
    pub fn active_camera(&self) -> Option<CameraView> {
        let entity = self.active_camera?;
        if !self.is_alive(entity) {
            return None;
        }
        let fov_y_degrees = *self.camera_fov.get(entity)?;
        let m = self.global.get(entity)?.matrix;

        // Column-major, so the third column is the node's local +Z in world
        // space. The scene format's forward is −Z (`Transform`'s docs), and
        // the column carries the node's scale, so it needs normalising: the
        // direction is the payload and its length is not.
        let axis = [-m[8], -m[9], -m[10]];
        let length = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
        if !length.is_finite() || length < 1e-6 {
            return None;
        }
        let eye = [m[12], m[13], m[14]];
        Some(CameraView {
            eye,
            target: [
                eye[0] + axis[0] / length,
                eye[1] + axis[1] / length,
                eye[2] + axis[2] / length,
            ],
            fov_y_degrees,
        })
    }

    /// The node the view comes from, for whoever needs to *turn* it rather
    /// than only read it — mouse look writes pitch onto this node.
    #[must_use]
    pub fn active_camera_entity(&self) -> Option<Entity> {
        self.active_camera.filter(|e| self.is_alive(*e))
    }

    /// The first node carrying a `CharacterController`.
    ///
    /// "The player" is not a concept the engine has. This is the nearest
    /// honest thing: the character a human driving the scene would possess.
    #[must_use]
    pub fn first_character(&self) -> Option<Entity> {
        self.order
            .iter()
            .find(|e| self.character.get(**e).is_some())
            .copied()
    }

    /// The `CharacterController` a node declares, if any.
    #[must_use]
    pub fn character(&self, entity: Entity) -> Option<&serde_json::Value> {
        self.character.get(entity)
    }

    /// The `.rhai` file this node's `GameRules` names, if it has one.
    #[must_use]
    pub fn rules_path(&self, entity: Entity) -> Option<&str> {
        self.rules.get(entity).map(String::as_str)
    }

    /// Every node's world path and position, for the rules script.
    ///
    /// Built fresh rather than cached: a rules script runs after everything
    /// has moved, so a cache would be one tick stale exactly where it matters.
    #[must_use]
    pub fn positions(&self) -> Vec<(String, [f32; 3])> {
        self.order
            .iter()
            .filter_map(|e| {
                let path = self.paths.get(*e)?;
                let m = self.global.get(*e)?.matrix;
                Some((path.clone(), [m[12], m[13], m[14]]))
            })
            .collect()
    }

    /// Every `Hud` element in the scene, in file order — which is also the
    /// order they draw, so an author can put one in front of another.
    #[must_use]
    pub fn hud_elements(&self) -> Vec<&serde_json::Value> {
        self.order.iter().filter_map(|e| self.hud.get(*e)).collect()
    }

    /// The `Blast` a node declares, if any.
    #[must_use]
    pub fn blast(&self, entity: Entity) -> Option<&serde_json::Value> {
        self.blast.get(entity)
    }

    /// The `ParticleEmitter` a node declares, if any.
    #[must_use]
    pub fn emitter(&self, entity: Entity) -> Option<&serde_json::Value> {
        self.emitter.get(entity)
    }

    /// The `Material` component a node declares, if any.
    #[must_use]
    pub fn material(&self, entity: Entity) -> Option<&serde_json::Value> {
        self.material.get(entity)
    }

    /// Authored collider half-extents, if the node declares a `BoxCollider`.
    ///
    /// Physics used to derive every collider from `transform.scale` and never
    /// look at this, so a documented, schema-validated component silently did
    /// nothing — a node could declare a collider twice the size of its mesh and
    /// collide as the mesh.
    #[must_use]
    pub fn collider_half_extents(&self, entity: Entity) -> Option<[f32; 3]> {
        self.collider.get(entity).copied()
    }

    /// This entity's `VoxelVolume` recipe, if it has one.
    #[must_use]
    pub fn voxel_recipe(&self, entity: Entity) -> Option<&serde_json::Value> {
        self.voxel_recipe.get(entity)
    }

    /// Flag an entity as having geometry to draw.
    pub fn mark_renderable(&mut self, entity: Entity) {
        self.renderable.insert(entity, ());
    }

    /// Whether this entity draws anything.
    #[must_use]
    pub fn is_renderable(&self, entity: Entity) -> bool {
        self.renderable.get(entity).is_some()
    }

    /// This entity's scene path, e.g. `Office/Desk`.
    #[must_use]
    pub fn path(&self, entity: Entity) -> Option<&str> {
        self.paths.get(entity).map(String::as_str)
    }

    /// Whether this entity falls under gravity.
    #[must_use]
    pub fn is_dynamic(&self, entity: Entity) -> bool {
        self.bodies.get(entity).is_some_and(|(d, _)| *d)
    }

    /// This entity's mass, or 1.0 when it has no `RigidBody`.
    #[must_use]
    pub fn body_mass(&self, entity: Entity) -> f32 {
        self.bodies.get(entity).map_or(1.0, |(_, m)| *m)
    }

    /// The `.rhai` file this entity runs, if any.
    #[must_use]
    pub fn script_path(&self, entity: Entity) -> Option<&str> {
        self.scripts.get(entity).map(String::as_str)
    }

    /// The asset alias this entity's mesh comes from.
    #[must_use]
    pub fn mesh_asset(&self, entity: Entity) -> Option<&str> {
        self.mesh_asset.get(entity).map(String::as_str)
    }

    /// A hash of everything the simulation can observe.
    ///
    /// FNV-1a over floats' **bit patterns**, in entity order. Explicit rather
    /// than `DefaultHasher` because that is not stable across Rust versions,
    /// and a determinism check that changes meaning on a toolchain bump is
    /// worse than none. Floats hash by bits, so `0.0` and `-0.0` differ —
    /// correct, since they are different states even though they compare equal.
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let eat = |hash: &mut u64, bytes: &[u8]| {
            for byte in bytes {
                *hash ^= u64::from(*byte);
                *hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        };

        for entity in &self.order {
            eat(&mut hash, &entity.index.to_le_bytes());
            eat(&mut hash, &entity.generation.to_le_bytes());
            if let Some(name) = self.names.get(*entity) {
                eat(&mut hash, name.as_bytes());
            }
            if let Some(global) = self.global.get(*entity) {
                for value in global.matrix {
                    eat(&mut hash, &value.to_bits().to_le_bytes());
                }
            }
        }
        hash
    }
}

/// Compose a `Transform` into a column-major 4x4.
///
/// Intrinsic Y-X-Z euler in degrees, matching `docs/format/README.md` §1.1.
/// Written out rather than pulled from a math crate so `loom_ecs` stays free of
/// a linear-algebra dependency it would need only here.
fn compose(t: &Transform) -> [f32; 16] {
    let [rx, ry, rz] = t.rot_euler;
    let (sx, cx) = rx.to_radians().sin_cos();
    let (sy, cy) = ry.to_radians().sin_cos();
    let (sz, cz) = rz.to_radians().sin_cos();

    // R = Ry * Rx * Rz (intrinsic Y-X-Z), written column-major below.
    let m00 = cy * cz + sy * sx * sz;
    let m01 = cx * sz;
    let m02 = -sy * cz + cy * sx * sz;
    let m10 = -cy * sz + sy * sx * cz;
    let m11 = cx * cz;
    let m12 = sy * sz + cy * sx * cz;
    let m20 = sy * cx;
    let m21 = -sx;
    let m22 = cy * cx;

    let [px, py, pz] = t.pos;
    let [kx, ky, kz] = t.scale;

    [
        m00 * kx, m01 * kx, m02 * kx, 0.0, //
        m10 * ky, m11 * ky, m12 * ky, 0.0, //
        m20 * kz, m21 * kz, m22 * kz, 0.0, //
        px, py, pz, 1.0,
    ]
}

/// Column-major 4x4 multiply: `a * b`.
fn multiply(a: [f32; 16], b: [f32; 16]) -> [f32; 16] {
    let mut out = [0.0_f32; 16];
    for column in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[column * 4 + k];
            }
            out[column * 4 + row] = sum;
        }
    }
    out
}

/// Fixed-timestep accumulator.
///
/// The simulation never sees a variable `dt` (locked decision). Render
/// interpolates between the last two states using [`Self::alpha`].
#[derive(Debug, Clone, Copy)]
pub struct FixedTimestep {
    step: f64,
    accumulated: f64,
    max_steps: u32,
    /// Ticks elapsed. Part of the simulation state, so it is reproducible.
    pub tick: u64,
}

impl FixedTimestep {
    /// A timestep of `hz` ticks per second.
    #[must_use]
    pub fn new(hz: f64) -> Self {
        Self {
            step: 1.0 / hz,
            accumulated: 0.0,
            max_steps: 8,
            tick: 0,
        }
    }

    /// Feed elapsed wall time; get back how many fixed steps to run.
    ///
    /// The wall clock is read by the *caller*, never inside simulation code
    /// (never-do #8). Elapsed time is an argument precisely so a headless run
    /// can pass an exact value and be reproducible.
    pub fn advance(&mut self, elapsed_seconds: f64) -> u32 {
        self.accumulated += elapsed_seconds;
        let mut steps = 0;
        while self.accumulated >= self.step && steps < self.max_steps {
            self.accumulated -= self.step;
            steps += 1;
            self.tick += 1;
        }
        if steps == self.max_steps {
            // Drop the backlog rather than chasing it. Without this clamp one
            // long frame spirals: more steps take longer, which accumulates
            // more time, which runs more steps.
            self.accumulated = 0.0;
        }
        steps
    }

    /// How far between the previous and next tick the renderer should
    /// interpolate: 0.0 at the last tick, approaching 1.0 before the next.
    #[must_use]
    pub fn alpha(&self) -> f32 {
        #[allow(clippy::cast_possible_truncation)]
        {
            (self.accumulated / self.step) as f32
        }
    }

    #[must_use]
    pub fn step_seconds(&self) -> f64 {
        self.step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(pos: [f32; 3]) -> Transform {
        Transform {
            pos,
            ..Transform::default()
        }
    }

    #[test]
    fn a_stale_handle_is_not_alive_after_the_slot_is_reused() {
        let mut world = World::new();
        let first = world.spawn();
        world.despawn(first);
        let second = world.spawn();

        assert_eq!(first.index(), second.index(), "slot was reused");
        assert!(!world.is_alive(first), "stale handle must fail loudly");
        assert!(world.is_alive(second));
    }

    #[test]
    fn entities_iterate_in_creation_order() {
        let mut world = World::new();
        let entities: Vec<Entity> = (0..8).map(|_| world.spawn()).collect();

        assert_eq!(world.entities(), entities.as_slice());
    }

    #[test]
    fn a_child_inherits_its_parents_translation() {
        let mut world = World::new();
        let parent = world.spawn();
        world.set_transform(parent, at([10.0, 0.0, 0.0]));
        let child = world.spawn();
        world.set_transform(child, at([1.0, 2.0, 0.0]));
        world.set_parent(child, parent);

        world.propagate_transforms();

        let global = world.global_transform(child).expect("child has a global");
        // Translation is the last column of a column-major matrix.
        assert_eq!([global.matrix[12], global.matrix[13]], [11.0, 2.0]);
    }

    /// Non-uniform parent scale multiplies through to children — the authoring
    /// trap `assets/test/blockout.loom` documents.
    #[test]
    fn a_child_inherits_its_parents_scale() {
        let mut world = World::new();
        let parent = world.spawn();
        world.set_transform(
            parent,
            Transform {
                scale: [1.0, 0.4, 1.0],
                ..Transform::default()
            },
        );
        let child = world.spawn();
        world.set_transform(child, at([0.0, 1.4, 0.0]));
        world.set_parent(child, parent);

        world.propagate_transforms();

        let global = world.global_transform(child).unwrap();
        assert!(
            (global.matrix[13] - 0.56).abs() < 1e-6,
            "1.4 * 0.4 = 0.56, got {}",
            global.matrix[13]
        );
    }

    /// **The M3 exit criterion.** The same scene simulated twice must produce
    /// identical state hashes.
    #[test]
    fn identical_worlds_produce_identical_state_hashes() {
        fn build() -> World {
            let mut world = World::new();
            let root = world.spawn();
            world.set_name(root, "Room");
            world.set_transform(root, Transform::default());
            for i in 0..32 {
                let child = world.spawn();
                world.set_name(child, format!("Box{i}"));
                #[allow(clippy::cast_precision_loss)]
                world.set_transform(child, at([i as f32 * 0.5, 1.0, -2.0]));
                world.set_parent(child, root);
            }
            world.propagate_transforms();
            world
        }

        assert_eq!(build().state_hash(), build().state_hash());
    }

    #[test]
    fn a_changed_transform_changes_the_state_hash() {
        let mut world = World::new();
        let entity = world.spawn();
        world.set_transform(entity, at([0.0, 0.0, 0.0]));
        world.propagate_transforms();
        let before = world.state_hash();

        world.transform_mut(entity).unwrap().pos[1] = 1.0;
        world.propagate_transforms();

        assert_ne!(before, world.state_hash(), "the hash must notice");
    }

    /// Build a world from scene text, so these exercise the whole path an
    /// authored camera actually takes rather than a hand-populated storage.
    fn world_from(nodes: &str) -> World {
        let src = format!(
            "[scene]\nformat = 1\nid = \"7f3a1c22-5d80-4e11-9b6a-2c4e08f5d913\"\n\n{nodes}"
        );
        World::from_scene(&loom_scene::Scene::parse(&src).expect("scene parses"))
    }

    /// Within a hair, per component. Camera maths goes through sin/cos, so
    /// exact equality would fail on a value that is right.
    #[track_caller]
    fn close(actual: [f32; 3], expected: [f32; 3]) {
        for axis in 0..3 {
            assert!(
                (actual[axis] - expected[axis]).abs() < 1e-5,
                "{actual:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn a_scene_with_no_camera_has_no_view() {
        let world = world_from("[[node]]\nname = \"Box\"\n");

        assert!(world.active_camera().is_none(), "auto-framing must stay");
    }

    #[test]
    fn an_unrotated_camera_looks_down_negative_z() {
        let world = world_from(
            "[[node]]\nname = \"Eye\"\ntransform = { pos = [0.0, 2.0, 5.0] }\n\
             [node.components.Camera]\n",
        );

        let view = world.active_camera().expect("the scene declares one");
        close(view.eye, [0.0, 2.0, 5.0]);
        close(view.target, [0.0, 2.0, 4.0]);
    }

    /// Catches reading the wrong matrix column, or the wrong sign of the
    /// right one — both of which still produce a plausible-looking camera.
    #[test]
    fn a_yawed_camera_looks_where_it_is_turned() {
        let world = world_from(
            "[[node]]\nname = \"Eye\"\ntransform = { rot_euler = [0.0, 90.0, 0.0] }\n\
             [node.components.Camera]\n",
        );

        let view = world.active_camera().expect("the scene declares one");
        close(view.target, [-1.0, 0.0, 0.0]);
    }

    #[test]
    fn a_pitched_camera_looks_up() {
        let world = world_from(
            "[[node]]\nname = \"Eye\"\ntransform = { rot_euler = [30.0, 0.0, 0.0] }\n\
             [node.components.Camera]\n",
        );

        let view = world.active_camera().expect("the scene declares one");
        close(view.target, [0.0, 0.5, -30.0_f32.to_radians().cos()]);
    }

    /// The whole point of putting the camera on a node: parent it to a head
    /// and it rides along. Fails if the local transform is read instead of
    /// the propagated one.
    #[test]
    fn a_camera_rides_its_parent() {
        let world = world_from(
            "[[node]]\nname = \"Player\"\ntransform = { pos = [10.0, 0.0, 0.0] }\n\n\
             [[node]]\nname = \"Eye\"\nparent = \"Player\"\n\
             transform = { pos = [0.0, 1.7, 0.0] }\n\
             [node.components.Camera]\n",
        );

        let view = world.active_camera().expect("the scene declares one");
        close(view.eye, [10.0, 1.7, 0.0]);
    }

    /// A scaled node must not stretch the step from eye to target: the
    /// direction is the payload, its length is not. Fails without a normalize.
    #[test]
    fn scale_does_not_change_where_a_camera_looks() {
        let world = world_from(
            "[[node]]\nname = \"Eye\"\ntransform = { scale = [4.0, 4.0, 4.0] }\n\
             [node.components.Camera]\n",
        );

        let view = world.active_camera().expect("the scene declares one");
        close(view.target, [0.0, 0.0, -1.0]);
    }

    #[test]
    fn an_inactive_camera_is_passed_over_for_the_next_one() {
        let world = world_from(
            "[[node]]\nname = \"Stage\"\n\n\
             [[node]]\nname = \"Spare\"\nparent = \"Stage\"\n\
             transform = { pos = [0.0, 0.0, 99.0] }\n\
             [node.components.Camera]\nactive = false\n\n\
             [[node]]\nname = \"Eye\"\nparent = \"Stage\"\n\
             transform = { pos = [0.0, 0.0, 7.0] }\n\
             [node.components.Camera]\nfov_y_degrees = 75.0\n",
        );

        let view = world.active_camera().expect("the second one is active");
        close(view.eye, [0.0, 0.0, 7.0]);
        assert!((view.fov_y_degrees - 75.0).abs() < f32::EPSILON, "authored fov");
    }

    /// A zero-scaled node has no direction to look in. Reporting no camera
    /// falls back to auto-framing, which shows the scene; a NaN target
    /// renders a blank image with no clue why.
    #[test]
    fn a_camera_with_no_direction_is_ignored() {
        let world = world_from(
            "[[node]]\nname = \"Eye\"\ntransform = { scale = [0.0, 0.0, 0.0] }\n\
             [node.components.Camera]\n",
        );

        assert!(world.active_camera().is_none(), "degenerate, not NaN");
    }

    #[test]
    fn the_timestep_runs_whole_steps_only() {
        let mut clock = FixedTimestep::new(60.0);

        assert_eq!(clock.advance(0.008), 0, "less than one step");
        assert_eq!(clock.advance(0.009), 1, "0.017s total crosses 1/60");
        assert_eq!(clock.tick, 1);
    }

    /// A long stall must not spiral: more steps take longer, which accumulates
    /// more time, which runs more steps.
    #[test]
    fn a_long_stall_is_clamped_rather_than_spiralling() {
        let mut clock = FixedTimestep::new(60.0);

        let steps = clock.advance(10.0);

        assert_eq!(steps, 8, "clamped to max_steps");
        assert!(clock.alpha().abs() < f32::EPSILON, "backlog dropped");
    }
}
