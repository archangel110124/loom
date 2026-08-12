//! Prefab instancing and override resolution — format spec §5.
//!
//! **The consuming file stores a source reference plus explicit deltas, never
//! a copy.** That is the whole point: editing a prefab has to update every
//! placed instance, which is impossible the moment a placement contains a copy
//! of what it placed.
//!
//! # Resolution produces an ordinary scene
//!
//! [`resolve`] returns a `Scene` with no prefab keys left in it — instances
//! replaced by the nodes they stood for, overrides folded in. Every existing
//! consumer (the renderer, `measure`, physics, the ECS) therefore understands
//! prefabs without knowing they exist, and `unpack` is this same code writing
//! its result back to the file rather than keeping it in memory.
//!
//! The flattened scene is a *derived artifact*. Byte-identical round-trip is a
//! property of the source file, which resolution never touches, and comments
//! in a prefab do not survive into a scene that instanced it — there is no
//! sensible place to put them and no one to read them there.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::scene::{Node, Scene, SceneError};

/// The prefabs available to resolve against, keyed by `id`.
///
/// **By `id`, not by alias.** An alias is file-local: two scenes may call the
/// same prefab different things, and the same word may mean different prefabs
/// in different files. Keying a shared library by alias would make a prefab's
/// meaning depend on which file happened to load first.
#[derive(Debug, Clone, Default)]
pub struct Library {
    by_id: BTreeMap<String, Scene>,
}

impl Library {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: impl Into<String>, scene: Scene) {
        self.by_id.insert(id.into(), scene);
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Scene> {
        self.by_id.get(id)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// A flattened scene, plus everything that was survivable but worth saying.
#[derive(Debug)]
pub struct Resolved {
    /// The scene with every instance expanded and no prefab keys remaining.
    pub scene: Scene,
    /// Overrides that pointed at something the prefab no longer has.
    ///
    /// **Warnings, not errors, and the file is untouched** (§5). A prefab that
    /// renamed a child should not make twenty scenes fail to load, and it
    /// certainly should not silently discard what the author wrote — Unity's
    /// handling of this is a known pain point and reproducing it would be a
    /// choice.
    pub warnings: Vec<SceneError>,
}

/// Expand every prefab instance in `scene`.
///
/// # Errors
/// Structural problems that make the scene meaningless: a prefab that is not
/// in the library, or an instancing cycle.
pub fn resolve(scene: &Scene, library: &Library) -> Result<Resolved, Vec<SceneError>> {
    let mut assets = AssetMerge::new(scene);
    let mut warnings = Vec::new();
    let mut stack = Vec::new();

    let nodes = expand(scene, library, None, &mut stack, &mut assets, &mut warnings)?;

    let text = emit(scene, &nodes, &assets);
    let flattened = Scene::parse(&text)?;
    Ok(Resolved { scene: flattened, warnings })
}

/// Load every prefab a scene declares, and every prefab *those* declare.
///
/// `scene_path` locates the consuming file; `path` hints resolve relative to
/// the directory holding it, and a nested prefab relative to its own file — so
/// a prefab that instances another keeps working wherever it is placed from.
///
/// **`Scene::parse` stays pure and this does the I/O**, which is the split that
/// lets the parser be tested without a filesystem while `apply_to_file` and
/// the CLI still get prefabs resolved from disk.
///
/// # Errors
/// A declared prefab whose file cannot be read, or that does not parse.
pub fn library_for(
    scene: &Scene,
    scene_path: &std::path::Path,
) -> Result<Library, Vec<SceneError>> {
    let base = scene_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut library = Library::new();
    let mut seen = BTreeSet::new();
    collect(scene, base, &mut library, &mut seen)?;
    Ok(library)
}

/// Depth-first, guarded by `seen` on the prefab id. The guard is for repeated
/// work, not for cycles: a cycle is a *scene* error with a path to report, and
/// swallowing it here would turn a nameable mistake into a missing prefab.
fn collect(
    scene: &Scene,
    base: &std::path::Path,
    library: &mut Library,
    seen: &mut BTreeSet<String>,
) -> Result<(), Vec<SceneError>> {
    for decl in scene.prefabs() {
        if !seen.insert(decl.id.clone()) {
            continue;
        }

        // The path is a hint for *finding* the file, once. Identity stays the
        // id, which is what the library is keyed by — so moving a prefab
        // breaks one hint rather than every reference to it.
        let candidate = std::path::Path::new(&decl.path);
        let file = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            base.join(candidate)
        };

        let text = std::fs::read_to_string(&file).map_err(|e| {
            let mut err = SceneError::new("io_error", "");
            err.constraint = format!("{}: {e}", file.display());
            err.field = "prefab".to_owned();
            err.value = Value::from(decl.key.clone());
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

        let nested = file.parent().unwrap_or(base).to_path_buf();
        collect(&parsed, &nested, library, seen)?;
        library.insert(decl.id, parsed);
    }
    Ok(())
}

/// A node's transform as TOML, or `None` when it is identity.
///
/// **Not `serde_json::to_value` plus the generic converter**, for two reasons
/// that both show up in a written file.
///
/// A `Transform` holds `f32`; JSON holds `f64`. Widening `1.4_f32` gives
/// `1.399999976158142`, and writing that back replaces the author's number
/// with noise — the format spec is explicit that the authored value is the
/// source of truth. Going through `f32::to_string` emits the shortest decimal
/// that round-trips, which is `1.4`.
///
/// And defaults are omitted (§4), so an unpacked node does not sprout
/// `rot_euler = [0.0, 0.0, 0.0]` and `scale = [1.0, 1.0, 1.0]` it never had.
pub(crate) fn transform_toml(
    transform: &crate::components::Transform,
) -> Option<toml_edit::InlineTable> {
    let default = crate::components::Transform::default();
    let mut table = toml_edit::InlineTable::new();

    let mut put = |key: &str, value: [f32; 3]| {
        let mut array = toml_edit::Array::new();
        for component in value {
            // `to_string` on the f32, re-read as f64: the decimal is the
            // shortest that identifies the f32, and it parses exactly.
            let shortest = component.to_string().parse::<f64>().unwrap_or(f64::from(component));
            array.push(shortest);
        }
        table.insert(key, array.into());
    };

    if transform.pos != default.pos {
        put("pos", transform.pos);
    }
    if transform.rot_euler != default.rot_euler {
        put("rot_euler", transform.rot_euler);
    }
    if transform.scale != default.scale {
        put("scale", transform.scale);
    }

    if table.is_empty() { None } else { Some(table) }
}

/// One instance's contents, ready to be written into the consuming file.
#[derive(Debug)]
pub struct Unpacked {
    /// The concrete nodes, the first being the instance itself.
    pub nodes: Vec<Node>,
    /// Asset declarations the consuming file does not have yet.
    ///
    /// A prefab's aliases are file-local, so unpacking has to bring the
    /// declarations along or the nodes reference names nothing declares.
    pub assets: Vec<crate::PrefabDecl>,
    /// Overrides that pointed at nothing — reported, never silently dropped.
    pub warnings: Vec<SceneError>,
}

/// Expand a single instance, for `unpack`.
///
/// Separate from [`resolve`] because unpacking is surgery on one node: the
/// rest of the file keeps its instances, its comments and its formatting, and
/// only this one becomes concrete.
///
/// # Errors
/// The node is not an instance, its prefab is missing, or it is in a cycle.
pub fn expand_instance(
    scene: &Scene,
    instance_path: &str,
    library: &Library,
) -> Result<Unpacked, Vec<SceneError>> {
    let Some(node) = scene.nodes().iter().find(|n| n.path == instance_path) else {
        let mut err = SceneError::new("unknown_node", instance_path);
        err.constraint = "a node in this scene".to_owned();
        return Err(vec![err]);
    };
    let Some(alias) = node.prefab.as_deref() else {
        let mut err = SceneError::new("not_a_prefab_instance", instance_path);
        err.constraint = "a node with `prefab = \"...\"`".to_owned();
        err.hint = Some("only an instance has anything to unpack".to_owned());
        return Err(vec![err]);
    };

    let mut assets = AssetMerge::new(scene);
    let before = assets.order.len();
    let mut warnings = Vec::new();
    let mut stack = Vec::new();

    let id = scene.prefab_id(alias).unwrap_or_default();
    let Some(source) = library.get(&id) else {
        let mut err = SceneError::new("prefab_not_loaded", instance_path);
        err.field = "prefab".to_owned();
        err.value = Value::from(alias);
        err.constraint = id;
        return Err(vec![err]);
    };

    stack.push(id);
    let mut nodes =
        expand(source, library, Some(instance_path), &mut stack, &mut assets, &mut warnings)?;

    if let Some(root) = nodes.first_mut() {
        root.transform.clone_from(&node.transform);
        (root.name, root.parent) = split_path(instance_path);
    }
    apply_overrides(&node.overrides, instance_path, &mut nodes, &mut warnings);

    let gained = assets.order[before..]
        .iter()
        .map(|(key, id, path)| crate::PrefabDecl {
            key: key.clone(),
            id: id.clone(),
            path: path.clone(),
        })
        .collect();
    Ok(Unpacked { nodes, assets: gained, warnings })
}

/// Expand one scene's nodes, recursing into instances.
///
/// `prefix` is the path the scene's root takes in the output. `None` means
/// this is the top-level scene and paths are unchanged.
fn expand(
    scene: &Scene,
    library: &Library,
    prefix: Option<&str>,
    stack: &mut Vec<String>,
    assets: &mut AssetMerge,
    warnings: &mut Vec<SceneError>,
) -> Result<Vec<Node>, Vec<SceneError>> {
    let mut out: Vec<Node> = Vec::new();
    let scene_root = scene.nodes().first().map(|n| n.path.clone()).unwrap_or_default();

    for node in scene.nodes() {
        // Where this node lands in the output tree.
        let path = match prefix {
            None => node.path.clone(),
            Some(prefix) => reparent_path(&node.path, &scene_root, prefix),
        };

        let Some(alias) = node.prefab.as_deref() else {
            let mut placed = node.clone();
            placed.path.clone_from(&path);
            (placed.name, placed.parent) = split_path(&path);
            // A prefab's own asset aliases are file-local and may collide with
            // the consuming scene's. Rewriting them here is what makes two
            // files that both call something "wood" instancable together.
            if prefix.is_some() {
                assets.rewrite(scene, &mut placed.components);
            }
            out.push(placed);
            continue;
        };

        let Some(id) = scene.prefab_id(alias) else {
            // Unreachable through `Scene::parse`, which rejects an alias with
            // no declaration — but `resolve` is public and a caller could hand
            // us a scene built another way.
            let mut err = SceneError::new("unresolved_prefab", &path);
            err.field = "prefab".to_owned();
            err.value = Value::from(alias);
            return Err(vec![err]);
        };

        if let Some(at) = stack.iter().position(|seen| *seen == id) {
            let mut err = SceneError::new("prefab_cycle", &path);
            err.value = Value::from(alias);
            // The whole cycle, not just the repeated id: "a includes b
            // includes a" is fixable, "a is in a cycle" is a search.
            let mut cycle: Vec<String> = stack[at..].to_vec();
            cycle.push(id.clone());
            err.constraint = cycle.join(" -> ");
            err.hint = Some("a prefab cannot contain itself, at any depth".to_owned());
            return Err(vec![err]);
        }

        let Some(source) = library.get(&id) else {
            let mut err = SceneError::new("prefab_not_loaded", &path);
            err.field = "prefab".to_owned();
            err.value = Value::from(alias);
            err.constraint = id.clone();
            err.hint = Some(format!(
                "the prefab `{alias}` (id {id}) is declared but was not \
                 supplied to `resolve`. Its `path` is the hint for finding it."
            ));
            return Err(vec![err]);
        };

        stack.push(id);
        let mut subtree = expand(source, library, Some(&path), stack, assets, warnings)?;
        stack.pop();

        // **The instance's transform wins, whole.** A prefab root's own
        // transform is its authoring origin, not a placement; keeping it would
        // make an instance land somewhere other than where the file says.
        if let Some(root) = subtree.first_mut() {
            root.transform.clone_from(&node.transform);
            (root.name, root.parent) = split_path(&path);
        }

        apply_overrides(&node.overrides, &path, &mut subtree, warnings);
        out.append(&mut subtree);
    }

    Ok(out)
}

/// Fold a node's override map into the subtree it instanced.
fn apply_overrides(
    overrides: &BTreeMap<String, Value>,
    instance: &str,
    subtree: &mut [Node],
    warnings: &mut Vec<SceneError>,
) {
    for (key, value) in overrides {
        let (child, type_name, field) = split_override_key(key);
        let target = match child {
            None => instance.to_owned(),
            Some(child) => format!("{instance}/{child}"),
        };

        let Some(node) = subtree.iter_mut().find(|n| n.path == target) else {
            warnings.push(orphan(instance, key, value, &format!("no node at `{target}`")));
            continue;
        };
        let Some(component) = node.components.get_mut(type_name) else {
            warnings.push(orphan(
                instance,
                key,
                value,
                &format!("`{target}` has no `{type_name}` component"),
            ));
            continue;
        };

        set_field(component, field, value.clone());
    }
}

/// An override that pointed at something no longer there.
fn orphan(instance: &str, key: &str, value: &Value, why: &str) -> SceneError {
    let mut err = SceneError::new("orphaned_override", instance);
    err.field = key.to_owned();
    err.value = value.clone();
    err.constraint = why.to_owned();
    err.hint = Some(
        "the prefab changed under this instance. The value is kept in the \
         file — retarget it, or delete the override deliberately."
            .to_owned(),
    );
    err
}

/// Split `Child/Path::Type.field.sub` into its three parts.
///
/// The grammar is validated at parse time, so this is total: anything that got
/// here is well-formed.
fn split_override_key(key: &str) -> (Option<&str>, &str, &str) {
    let (child, rest) = match key.split_once("::") {
        Some((child, rest)) => (Some(child), rest),
        None => (None, key),
    };
    let (type_name, field) = rest.split_once('.').unwrap_or((rest, ""));
    (child, type_name, field)
}

/// Write `value` at a dotted path inside a component, creating tables as
/// needed — a field left at its default is omitted from the file (§4), so the
/// path it lives at often does not exist yet.
fn set_field(component: &mut Value, field: &str, value: Value) {
    let mut cursor = component;
    let mut segments = field.split('.').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            if let Some(map) = cursor.as_object_mut() {
                map.insert(segment.to_owned(), value);
            }
            return;
        }
        if !cursor.is_object() {
            *cursor = Value::Object(serde_json::Map::new());
        }
        cursor = cursor
            .as_object_mut()
            .map(|map| map.entry(segment.to_owned()).or_insert(Value::Null))
            .expect("just ensured an object");
    }
}

/// Re-root a path: `Lamp/Bulb` under prefix `Office/Desk` is
/// `Office/Desk/Bulb`.
fn reparent_path(path: &str, root: &str, prefix: &str) -> String {
    match path.strip_prefix(root) {
        Some("") => prefix.to_owned(),
        Some(rest) => format!("{prefix}{rest}"),
        None => format!("{prefix}/{path}"),
    }
}

/// A path's last segment and everything before it.
fn split_path(path: &str) -> (String, Option<String>) {
    match path.rsplit_once('/') {
        Some((parent, name)) => (name.to_owned(), Some(parent.to_owned())),
        None => (path.to_owned(), None),
    }
}

/// Merges the asset declarations of every scene that contributes nodes.
///
/// **Two files may use the same alias for different assets**, and the same
/// asset under different aliases. Identity is the `id`, so the merge is keyed
/// by it: an id already present keeps its alias, and an alias already taken by
/// a *different* id gets a suffix.
struct AssetMerge {
    /// Final alias per asset id.
    alias_of: BTreeMap<String, String>,
    taken: BTreeSet<String>,
    /// Declarations to emit, in insertion order.
    order: Vec<(String, String, String)>,
}

impl AssetMerge {
    fn new(root: &Scene) -> Self {
        let mut merge =
            Self { alias_of: BTreeMap::new(), taken: BTreeSet::new(), order: Vec::new() };
        // The consuming scene's own aliases are kept exactly as written, so a
        // file that instances nothing is unchanged by the merge.
        for asset in root.assets() {
            merge.alias_of.insert(asset.id.clone(), asset.key.clone());
            merge.taken.insert(asset.key.clone());
            merge.order.push((asset.key, asset.id, asset.path));
        }
        merge
    }

    /// The alias `local` takes in the output, declaring it if it is new.
    fn adopt(&mut self, source: &Scene, local: &str) -> Option<String> {
        let decl = source.assets().into_iter().find(|a| a.key == local)?;
        if let Some(existing) = self.alias_of.get(&decl.id) {
            return Some(existing.clone());
        }

        let mut alias = decl.key.clone();
        let mut n = 1;
        while self.taken.contains(&alias) {
            n += 1;
            alias = format!("{}_{n}", decl.key);
        }
        self.taken.insert(alias.clone());
        self.alias_of.insert(decl.id.clone(), alias.clone());
        self.order.push((alias.clone(), decl.id, decl.path));
        Some(alias)
    }

    /// Rewrite every `{ asset = "..." }` reference in a prefab's components.
    fn rewrite(&mut self, source: &Scene, components: &mut BTreeMap<String, Value>) {
        for value in components.values_mut() {
            rewrite_asset_refs(value, &mut |local| self.adopt(source, local));
        }
    }
}

/// Walk a component's JSON, remapping every asset reference.
///
/// `{ asset = "alias" }` is the format's one spelling for a reference (§3), so
/// an object with an `asset` string is the thing to rewrite wherever it sits.
fn rewrite_asset_refs(value: &mut Value, remap: &mut impl FnMut(&str) -> Option<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(alias)) = map.get("asset")
                && let Some(replacement) = remap(alias)
            {
                map.insert("asset".to_owned(), Value::String(replacement));
                return;
            }
            for item in map.values_mut() {
                rewrite_asset_refs(item, remap);
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite_asset_refs(item, remap);
            }
        }
        _ => {}
    }
}

/// Emit the flattened scene as `.loom` text.
fn emit(source: &Scene, nodes: &[Node], assets: &AssetMerge) -> String {
    let mut out = String::from("[scene]\nformat = 1\n");
    if let Some(id) = source.scene_id() {
        out.push_str(&format!("id = {}\n", toml_edit::Value::from(id)));
    }

    for (key, id, path) in &assets.order {
        out.push_str("\n[[asset]]\n");
        out.push_str(&format!("key = {}\n", toml_edit::Value::from(key.clone())));
        out.push_str(&format!("id = {}\n", toml_edit::Value::from(id.clone())));
        out.push_str(&format!("path = {}\n", toml_edit::Value::from(path.clone())));
    }

    for node in nodes {
        out.push_str("\n[[node]]\n");
        out.push_str(&format!("name = {}\n", toml_edit::Value::from(node.name.clone())));
        if let Some(parent) = &node.parent {
            out.push_str(&format!("parent = {}\n", toml_edit::Value::from(parent.clone())));
        }
        if let Some(transform) = transform_toml(&node.transform) {
            out.push_str(&format!("transform = {transform}\n"));
        }
        for (type_name, data) in &node.components {
            let Some(toml_edit::Value::InlineTable(table)) = json_to_toml(data) else {
                continue;
            };
            out.push_str(&format!("\n  [node.components.{type_name}]\n"));
            for (key, value) in table.iter() {
                out.push_str(&format!("  {key} = {value}\n"));
            }
        }
    }
    out
}

/// JSON to TOML for values that came *from* TOML, so nothing is unrepresentable.
///
/// `None` for the cases TOML has no answer for. They cannot arise from a
/// parsed scene, and inventing a value for them is how a field quietly becomes
/// the wrong type.
fn json_to_toml(value: &Value) -> Option<toml_edit::Value> {
    Some(match value {
        Value::Bool(b) => (*b).into(),
        // Integer-ness is load-bearing: TOML distinguishes 4 from 4.0 and the
        // voxel reader takes `chunks` through `as_u64`.
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into()
            } else {
                n.as_f64()?.into()
            }
        }
        Value::String(s) => s.as_str().into(),
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(json_to_toml(item)?);
            }
            array.into()
        }
        Value::Object(map) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, item) in map {
                table.insert(key, json_to_toml(item)?);
            }
            table.into()
        }
        Value::Null => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAMP_ID: &str = "3f1c9a20-77bd-4e11-9c02-51ad6e7b8c44";

    /// A prefab: a root with a Light, and a child that carries its own.
    fn lamp() -> Scene {
        Scene::parse(
            "[scene]\nformat = 1\nid = \"11111111-1111-4111-8111-111111111111\"\n\
             \n[[node]]\nname = \"Lamp\"\n\n  [node.components.Light]\n  intensity = 100.0\n\
             \n[[node]]\nname = \"Bulb\"\nparent = \"Lamp\"\n\
             transform = { pos = [0.0, 0.5, 0.0] }\n\n  [node.components.Light]\n  \
             intensity = 5.0\n",
        )
        .expect("the lamp prefab is valid")
    }

    /// A scene placing `count` lamps, with `extra` appended to the last one.
    fn placing(count: usize, extra: &str) -> String {
        let mut out = String::from(
            "[scene]\nformat = 1\nid = \"22222222-2222-4222-8222-222222222222\"\n\n\
             [[prefab]]\nkey = \"lamp\"\nid = \"",
        );
        out.push_str(LAMP_ID);
        out.push_str("\"\npath = \"lamp.loom\"\n\n[[node]]\nname = \"Office\"\n");
        for i in 0..count {
            out.push_str(&format!(
                "\n[[node]]\nname = \"Lamp{i}\"\nparent = \"Office\"\nprefab = \"lamp\"\n\
                 transform = {{ pos = [{i}.0, 0.0, 0.0] }}\n"
            ));
        }
        out.push_str(extra);
        out
    }

    fn library() -> Library {
        let mut library = Library::new();
        library.insert(LAMP_ID, lamp());
        library
    }

    fn intensity(resolved: &Resolved, path: &str) -> f64 {
        resolved
            .scene
            .nodes()
            .iter()
            .find(|n| n.path == path)
            .unwrap_or_else(|| panic!("no node at {path}"))
            .components["Light"]["intensity"]
            .as_f64()
            .expect("a number")
    }

    /// **The S4 exit criterion, first half.** Twenty placed instances, and
    /// editing the prefab moves all twenty — because not one of them contains
    /// a copy of it.
    #[test]
    fn editing_the_prefab_updates_every_instance() {
        let scene = Scene::parse(&placing(20, "")).expect("valid");

        let before = resolve(&scene, &library()).expect("resolves");
        let lit = (0..20).filter(|i| intensity(&before, &format!("Office/Lamp{i}")) == 100.0).count();
        assert_eq!(lit, 20, "every instance takes the prefab's value");

        // Edit the prefab: one file, one number.
        let mut brighter = Library::new();
        brighter.insert(
            LAMP_ID,
            Scene::parse(
                &lamp().to_loom_string().replace("intensity = 100.0", "intensity = 250.0"),
            )
            .expect("still valid"),
        );

        let after = resolve(&scene, &brighter).expect("resolves");
        let relit = (0..20).filter(|i| intensity(&after, &format!("Office/Lamp{i}")) == 250.0).count();
        assert_eq!(relit, 20, "all twenty followed the edit");
    }

    /// **The S4 exit criterion, second half.** A per-instance override is not
    /// washed away by an edit to the prefab.
    #[test]
    fn per_instance_overrides_survive_a_prefab_edit() {
        let scene = Scene::parse(&placing(3, "\n  [node.overrides]\n  \"Light.intensity\" = 42.0\n"))
            .expect("valid");

        let resolved = resolve(&scene, &library()).expect("resolves");

        assert_eq!(intensity(&resolved, "Office/Lamp2"), 42.0, "the override held");
        assert_eq!(intensity(&resolved, "Office/Lamp0"), 100.0, "its siblings did not");
    }

    /// The instanced sub-tree comes along, re-rooted under the instance.
    #[test]
    fn the_prefabs_children_are_grafted_under_the_instance() {
        let scene = Scene::parse(&placing(1, "")).expect("valid");

        let resolved = resolve(&scene, &library()).expect("resolves");

        let paths: Vec<&str> = resolved.scene.nodes().iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, ["Office", "Office/Lamp0", "Office/Lamp0/Bulb"]);
    }

    /// `Child/Path::Type.field` reaches inside the instanced sub-tree.
    #[test]
    fn an_override_can_target_a_child_of_the_instance() {
        let scene =
            Scene::parse(&placing(1, "\n  [node.overrides]\n  \"Bulb::Light.intensity\" = 9.0\n"))
                .expect("valid");

        let resolved = resolve(&scene, &library()).expect("resolves");

        assert_eq!(intensity(&resolved, "Office/Lamp0/Bulb"), 9.0);
        assert_eq!(intensity(&resolved, "Office/Lamp0"), 100.0, "the root is untouched");
        assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    }

    /// The instance's transform is its placement and wins whole. The prefab
    /// root's own transform is an authoring origin, not a position.
    #[test]
    fn the_instance_transform_places_the_prefab() {
        let scene = Scene::parse(&placing(2, "")).expect("valid");

        let resolved = resolve(&scene, &library()).expect("resolves");

        let second = resolved
            .scene
            .nodes()
            .iter()
            .find(|n| n.path == "Office/Lamp1")
            .expect("the second instance");
        assert_eq!(second.transform.pos, [1.0, 0.0, 0.0]);
        // And a child keeps its own local transform from the prefab.
        let bulb = resolved
            .scene
            .nodes()
            .iter()
            .find(|n| n.path == "Office/Lamp1/Bulb")
            .expect("its bulb");
        assert_eq!(bulb.transform.pos, [0.0, 0.5, 0.0]);
    }

    /// **Never a silent drop** (§5). An override whose target the prefab no
    /// longer has is a warning naming it, and the scene still loads.
    #[test]
    fn an_override_with_no_target_warns_and_the_value_is_kept() {
        let source = placing(1, "\n  [node.overrides]\n  \"Flicker.enabled\" = true\n");
        let scene = Scene::parse(&source).expect("valid");

        let resolved = resolve(&scene, &library()).expect("an orphan is survivable");

        assert_eq!(resolved.warnings.len(), 1, "{:?}", resolved.warnings);
        let warning = &resolved.warnings[0];
        assert_eq!(warning.error, "orphaned_override");
        assert_eq!(warning.field, "Flicker.enabled");
        assert_eq!(warning.value, Value::Bool(true));
        // Preserved means preserved: the source file is not rewritten.
        assert!(scene.to_loom_string().contains("\"Flicker.enabled\" = true"));
    }

    /// A prefab that contains itself, at any depth, is a load error naming the
    /// whole cycle rather than a stack overflow.
    #[test]
    fn an_instancing_cycle_is_reported_with_its_path() {
        let recursive = Scene::parse(&format!(
            "[scene]\nformat = 1\n\n[[prefab]]\nkey = \"self\"\nid = \"{LAMP_ID}\"\n\
             path = \"lamp.loom\"\n\n[[node]]\nname = \"Lamp\"\n\n\
             [[node]]\nname = \"Inner\"\nparent = \"Lamp\"\nprefab = \"self\"\n"
        ))
        .expect("the file itself is well-formed");
        let mut library = Library::new();
        library.insert(LAMP_ID, recursive);
        let scene = Scene::parse(&placing(1, "")).expect("valid");

        let errors = resolve(&scene, &library).expect_err("a cycle must not resolve");

        assert_eq!(errors[0].error, "prefab_cycle");
        assert!(errors[0].constraint.contains("->"), "name the whole cycle: {:?}", errors[0]);
    }

    /// A declared prefab that was never handed to `resolve` is an error that
    /// says so, not a node that silently has no components.
    #[test]
    fn a_prefab_missing_from_the_library_is_named() {
        let scene = Scene::parse(&placing(1, "")).expect("valid");

        let errors = resolve(&scene, &Library::new()).expect_err("nothing to resolve against");

        assert_eq!(errors[0].error, "prefab_not_loaded");
        assert!(errors[0].hint.as_deref().unwrap_or_default().contains("lamp"));
    }

    /// A scene with no instances resolves to itself — same nodes, same order.
    #[test]
    fn a_scene_without_prefabs_is_unchanged_by_resolution() {
        let scene = Scene::parse(&placing(0, "")).expect("valid");

        let resolved = resolve(&scene, &Library::new()).expect("nothing to do");

        assert_eq!(resolved.scene.nodes(), scene.nodes());
    }

    /// **Two files, one word, two meanings.** A prefab's asset aliases are
    /// file-local; flattening two files that both say "wood" must not make one
    /// of them draw the other's texture. Identity is the id, so the merge keys
    /// on it and renames the loser.
    #[test]
    fn colliding_asset_aliases_are_kept_apart() {
        let prefab = Scene::parse(
            "[scene]\nformat = 1\n\n[[asset]]\nkey = \"wood\"\n\
             id = \"aaaaaaaa-0000-4000-8000-000000000001\"\npath = \"oak.png\"\n\n\
             [[node]]\nname = \"Crate\"\n\n  [node.components.Material]\n  \
             albedo_map = { asset = \"wood\" }\n",
        )
        .expect("valid prefab");
        let mut library = Library::new();
        library.insert(LAMP_ID, prefab);

        let scene = Scene::parse(&format!(
            "[scene]\nformat = 1\n\n[[asset]]\nkey = \"wood\"\n\
             id = \"bbbbbbbb-0000-4000-8000-000000000002\"\npath = \"pine.png\"\n\n\
             [[prefab]]\nkey = \"crate\"\nid = \"{LAMP_ID}\"\npath = \"crate.loom\"\n\n\
             [[node]]\nname = \"Room\"\n\n[[node]]\nname = \"Box\"\nparent = \"Room\"\n\
             prefab = \"crate\"\n"
        ))
        .expect("valid scene");

        let resolved = resolve(&scene, &library).expect("resolves");

        let placed = resolved
            .scene
            .nodes()
            .iter()
            .find(|n| n.path == "Room/Box")
            .expect("the instance");
        let alias = placed.components["Material"]["albedo_map"]["asset"].as_str().expect("an alias");
        assert_ne!(alias, "wood", "the prefab's oak must not become the scene's pine");
        assert_eq!(
            resolved.scene.asset_path(alias),
            Some("oak.png"),
            "and it must still point at its own texture"
        );
        assert_eq!(
            resolved.scene.asset_path("wood"),
            Some("pine.png"),
            "the consuming scene keeps its own alias untouched"
        );
    }

    /// The same asset under two aliases is declared once, not twice.
    #[test]
    fn the_same_asset_id_is_shared_rather_than_duplicated() {
        let prefab = Scene::parse(
            "[scene]\nformat = 1\n\n[[asset]]\nkey = \"timber\"\n\
             id = \"aaaaaaaa-0000-4000-8000-000000000001\"\npath = \"oak.png\"\n\n\
             [[node]]\nname = \"Crate\"\n\n  [node.components.Material]\n  \
             albedo_map = { asset = \"timber\" }\n",
        )
        .expect("valid prefab");
        let mut library = Library::new();
        library.insert(LAMP_ID, prefab);

        let scene = Scene::parse(&format!(
            "[scene]\nformat = 1\n\n[[asset]]\nkey = \"wood\"\n\
             id = \"aaaaaaaa-0000-4000-8000-000000000001\"\npath = \"oak.png\"\n\n\
             [[prefab]]\nkey = \"crate\"\nid = \"{LAMP_ID}\"\npath = \"crate.loom\"\n\n\
             [[node]]\nname = \"Room\"\n\n[[node]]\nname = \"Box\"\nparent = \"Room\"\n\
             prefab = \"crate\"\n"
        ))
        .expect("valid scene");

        let resolved = resolve(&scene, &library).expect("resolves");

        assert_eq!(resolved.scene.assets().len(), 1, "one id, one declaration");
        let placed = resolved
            .scene
            .nodes()
            .iter()
            .find(|n| n.path == "Room/Box")
            .expect("the instance");
        assert_eq!(placed.components["Material"]["albedo_map"]["asset"], "wood");
    }
}
