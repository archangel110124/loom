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
    /// The asset alias a node's `MeshRenderer` names.
    mesh_asset: Storage<String>,
    /// Scene path, so callers can address an entity the way a `.loom` file does.
    paths: Storage<String>,
    /// The `.rhai` file a node's `Script` component names.
    scripts: Storage<String>,
    /// `RigidBody`: whether it falls, and its mass.
    bodies: Storage<(bool, f32)>,
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
