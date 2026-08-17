//! `.loom` parse and serialize, per `docs/format/README.md`.
//!
//! The document is kept as a format-preserving DOM and re-emitted verbatim.
//! That is not an optimization — it is the reason an agent's write cannot
//! delete a human's comments, which `CLAUDE.md` never-do #15 calls the worst
//! bug class in this project. Reading is validated; writing preserves.

use std::collections::{BTreeMap, BTreeSet};

use loom_reflect::TypeRegistry;
use serde::Serialize;
use serde_json::Value;
use toml_edit::{DocumentMut, Item};

use crate::components;

/// The format version this build understands.
const FORMAT_VERSION: i64 = 1;

/// A validated scene, plus the exact bytes it was parsed from.
#[derive(Debug, Clone)]
pub struct Scene {
    doc: DocumentMut,
    nodes: Vec<Node>,
}

/// One node in the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Unique among siblings.
    pub name: String,
    /// Parent's path. `None` for the root.
    pub parent: Option<String>,
    /// Slash-separated path from the root, including it: `Office/Desk`.
    pub path: String,
    /// Local transform relative to the parent. Identity when omitted.
    ///
    /// Local, not world: composing the parent chain is transform propagation,
    /// which belongs to the ECS at M3. Keeping it local here means `loom_scene`
    /// needs no matrix math and no linear-algebra dependency.
    pub transform: components::Transform,
    /// Attached components, keyed by registered type name.
    ///
    /// Kept as JSON rather than typed fields so consumers can read any
    /// component without this struct growing one accessor per type — the same
    /// reason the registry is schema-driven.
    pub components: BTreeMap<String, Value>,
    /// File-local alias of the prefab this node instances (§5).
    ///
    /// Structural, like `parent`: it describes what the node *is*, not data
    /// attached to it. An instance carries no components of its own — they
    /// come from the prefab, and deviations go in `overrides`.
    pub prefab: Option<String>,
    /// Alias of the scene this one extends, on the root node only (§5).
    ///
    /// Godot's scene inheritance: the whole file starts from another and
    /// changes it. Unlike `prefab`, a node with `extends` **does** carry
    /// components — they are the changes, merged field by field over the base.
    pub extends: Option<String>,
    /// Per-instance deviations from the prefab, as a flat dotted map.
    ///
    /// `Light.intensity`, or `Child/Path::Light.intensity` to reach inside the
    /// instanced sub-tree. **Flat on purpose** — setting one override is a
    /// single map insert with no tree surgery, which is what makes it a
    /// one-line `SceneOp` rather than a structural edit.
    pub overrides: BTreeMap<String, Value>,
}

/// One `[[prefab]]` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefabDecl {
    /// File-local alias. What nodes in *this* file write.
    pub key: String,
    /// The real identity, stable across files and renames.
    pub id: String,
    /// A hint for humans (§3). Nothing resolves a reference through it — a
    /// loader uses it to *find* the file once, and identity stays the `id`.
    pub path: String,
}

/// A rejection, shaped per `docs/format/README.md` §6.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SceneError {
    /// Machine-readable code.
    pub error: String,
    /// Node path, or empty for a file-level problem.
    pub node: String,
    /// `TypeName.field`, or empty when not field-specific.
    pub field: String,
    /// What was supplied.
    pub value: Value,
    /// The bound or rule that was broken.
    pub constraint: String,
    /// Guidance, from the field's doc comment where there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl SceneError {
    /// Build one from outside the crate.
    ///
    /// A loader that cannot read a prefab file has a real §6-shaped error to
    /// report and no way to construct it otherwise — and inventing a second
    /// error type for "problems near a scene" would mean every caller
    /// matching on two.
    #[must_use]
    pub fn external(error: &str, constraint: &str) -> Self {
        let mut err = Self::new(error, "");
        err.constraint = constraint.to_owned();
        err
    }

    pub(crate) fn new(error: &str, node: &str) -> Self {
        Self {
            error: error.to_owned(),
            node: node.to_owned(),
            field: String::new(),
            value: Value::Null,
            constraint: String::new(),
            hint: None,
        }
    }
}

impl Scene {
    /// Parse and validate.
    ///
    /// Returns **every** problem found, not just the first — an agent that has
    /// to round-trip once per error is the retry loop §6 exists to avoid.
    /// Structural problems short-circuit component validation, because a node
    /// whose parent is unknown has no meaningful path to report against.
    ///
    /// # Errors
    /// One [`SceneError`] per violation.
    pub fn parse(src: &str) -> Result<Self, Vec<SceneError>> {
        let doc: DocumentMut = src.parse().map_err(|e: toml_edit::TomlError| {
            let mut err = SceneError::new("parse_error", "");
            err.constraint = e.to_string();
            vec![err]
        })?;

        check_format_version(&doc)?;
        let nodes = build_tree(&doc)?;

        let registry = components::registry();
        let errors = validate_components(&doc, &nodes, &registry);
        if errors.is_empty() {
            Ok(Self { doc, nodes })
        } else {
            Err(errors)
        }
    }

    /// The validated node tree, in declaration order (depth-first, parents
    /// before children — enforced by the forward-reference rule).
    #[must_use]
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// The advisory `path` of an `[[asset]]` entry, by its file-local alias.
    ///
    /// **Identity is the UUID, but this path IS resolved at runtime.** The
    /// comment here used to say nothing resolved a reference through it, while
    /// `loom_cli`'s mesh and texture loaders have always joined it onto the
    /// declaring scene's directory to find the file. ADR 0024 settled the
    /// contradiction in favour of the code and amended
    /// `docs/format/README.md` §3; this is that amendment reaching the source.
    ///
    /// Resolution is relative to the declaring scene file and to nothing else:
    /// no project-relative fallback and no search path.
    #[must_use]
    pub fn asset_path(&self, key: &str) -> Option<&str> {
        self.doc
            .get("asset")?
            .as_array_of_tables()?
            .iter()
            .find(|t| t.get("key").and_then(Item::as_str) == Some(key))?
            .get("path")?
            .as_str()
    }

    /// The `[[asset]]` declarations, in file order.
    #[must_use]
    pub fn assets(&self) -> Vec<PrefabDecl> {
        declarations(&self.doc, "asset")
    }

    /// The `[scene] id`, if the file carries one.
    #[must_use]
    pub fn scene_id(&self) -> Option<String> {
        self.doc.get("scene")?.get("id")?.as_str().map(str::to_owned)
    }

    /// The `[[prefab]]` declarations, in file order.
    ///
    /// **Identity is `id`, never the alias and never the path.** The alias is
    /// file-local — two scenes may call the same prefab different things, and
    /// the same word may mean different prefabs in different files — so a
    /// library of prefabs is keyed by `id` and each file's aliases are
    /// resolved through its own declarations (§3, the Unity lesson).
    #[must_use]
    pub fn prefabs(&self) -> Vec<PrefabDecl> {
        declarations(&self.doc, "prefab")
    }

    /// The `id` a file-local prefab alias refers to.
    #[must_use]
    pub fn prefab_id(&self, key: &str) -> Option<String> {
        self.prefabs().into_iter().find(|p| p.key == key).map(|p| p.id)
    }

    /// Serialize back to `.loom`.
    ///
    /// For an unmodified scene this is byte-identical to the input, comments
    /// and spacing included — that is the M1 exit criterion and it holds by
    /// construction rather than by careful re-emission.
    #[must_use]
    pub fn to_loom_string(&self) -> String {
        self.doc.to_string()
    }
}

impl std::fmt::Display for Scene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_loom_string())
    }
}

fn check_format_version(doc: &DocumentMut) -> Result<(), Vec<SceneError>> {
    let found = doc
        .get("scene")
        .and_then(|s| s.get("format"))
        .and_then(Item::as_integer);

    match found {
        Some(FORMAT_VERSION) => Ok(()),
        // A file from the future is refused, never parsed best-effort (§3).
        Some(other) => {
            let mut err = SceneError::new("format_version_unsupported", "");
            err.value = Value::from(other);
            err.constraint = FORMAT_VERSION.to_string();
            Err(vec![err])
        }
        None => {
            let mut err = SceneError::new("parse_error", "");
            err.constraint = "[scene] requires an integer `format` key".to_owned();
            Err(vec![err])
        }
    }
}

/// The `key`/`id`/`path` triples of an `[[asset]]` or `[[prefab]]` array.
///
/// One reader for both because they are the same shape by design (§3): a
/// file-local alias, the UUID that is the real identity, and an advisory path.
fn declarations(doc: &DocumentMut, table: &str) -> Vec<PrefabDecl> {
    let Some(entries) = doc.get(table).and_then(Item::as_array_of_tables) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            Some(PrefabDecl {
                key: entry.get("key")?.as_str()?.to_owned(),
                id: entry.get("id").and_then(Item::as_str).unwrap_or_default().to_owned(),
                path: entry.get("path").and_then(Item::as_str).unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

/// Resolve every node's path, enforcing the structural rules from §3.
fn build_tree(doc: &DocumentMut) -> Result<Vec<Node>, Vec<SceneError>> {
    let Some(entries) = doc.get("node").and_then(Item::as_array_of_tables) else {
        return Err(vec![SceneError::new("no_root", "")]);
    };

    let mut errors = Vec::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut known: BTreeSet<String> = BTreeSet::new();
    let mut children: BTreeSet<(String, String)> = BTreeSet::new();
    let mut root: Option<String> = None;

    // Whether this file inherits. A derived scene's nodes may be prefab
    // instances *in the base* without restating `prefab` here — the alias is
    // file-local and the derived file need not even declare it — so the
    // "overrides require prefab" rule is relaxed when the scene extends.
    let inherits = doc
        .get("node")
        .and_then(Item::as_array_of_tables)
        .is_some_and(|entries| entries.iter().any(|t| t.get("extends").is_some()));

    // Declared prefab aliases, for the resolution check below. Read straight
    // from the document rather than through `Scene::prefabs`, which needs a
    // `Scene` that does not exist until this function returns.
    let declared: BTreeSet<String> = doc
        .get("prefab")
        .and_then(Item::as_array_of_tables)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|t| t.get("key").and_then(Item::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    for table in entries {
        let Some(name) = table.get("name").and_then(Item::as_str) else {
            errors.push(SceneError::new("parse_error", ""));
            continue;
        };

        if name.is_empty() || name.contains('/') || name.trim() != name {
            let mut err = SceneError::new("parse_error", name);
            err.constraint = "name must be non-empty, untrimmed, and contain no `/`".to_owned();
            errors.push(err);
            continue;
        }

        let extends = table.get("extends").and_then(Item::as_str).map(str::to_owned);

        // **Only the root extends.** Inheritance is a property of the scene,
        // not of a node inside it — a mid-tree `extends` would be a prefab
        // instance spelled differently, and having two spellings for one thing
        // is how a format grows a dialect.
        if extends.is_some() && table.get("parent").is_some() {
            let mut err = SceneError::new("extends_on_a_child", name);
            err.field = "extends".to_owned();
            err.constraint = "the root node".to_owned();
            err.hint = Some(
                "`extends` makes the whole scene an extension of another. To \
                 bring one scene *into* another as a node, use \
                 `prefab = \"<alias>\"`."
                    .to_owned(),
            );
            errors.push(err);
        }

        let parent = table.get("parent").and_then(Item::as_str);
        let path = match parent {
            None => {
                if let Some(existing) = &root {
                    let mut err = SceneError::new("multiple_roots", name);
                    err.constraint = format!("`{existing}` is already the root");
                    errors.push(err);
                    continue;
                }
                root = Some(name.to_owned());
                name.to_owned()
            }
            Some(parent) => {
                // Forward references are rejected, which is what makes cycles
                // unrepresentable rather than merely detected (§3).
                if !known.contains(parent) {
                    let mut err = SceneError::new("unknown_parent", name);
                    err.value = Value::from(parent);
                    err.constraint = "parent must be a node declared earlier".to_owned();
                    errors.push(err);
                    continue;
                }
                format!("{parent}/{name}")
            }
        };

        let sibling_key = (parent.unwrap_or_default().to_owned(), name.to_owned());
        if !children.insert(sibling_key) {
            let mut err = SceneError::new("duplicate_sibling_name", &path);
            err.constraint = "sibling names must be unique".to_owned();
            errors.push(err);
            continue;
        }

        let transform = table
            .get("transform")
            .and_then(item_to_json)
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        let mut component_map = BTreeMap::new();
        if let Some(table_like) = table.get("components").and_then(Item::as_table_like) {
            for (type_name, item) in table_like.iter() {
                if let Some(value) = item_to_json(item) {
                    component_map.insert(type_name.to_owned(), value);
                }
            }
        }

        let prefab = table.get("prefab").and_then(Item::as_str).map(str::to_owned);

        // An unresolved alias names itself and lists what *is* declared — the
        // §3 rule for assets, and the difference between a fixable message and
        // a scavenger hunt.
        if let Some(alias) = prefab.as_ref().or(extends.as_ref())
            && !declared.contains(alias)
        {
            {
                let mut err = SceneError::new("unresolved_prefab", &path);
                err.field = "prefab".to_owned();
                err.value = Value::from(alias.clone());
                err.constraint = "a declared `[[prefab]]` key".to_owned();
                err.hint = Some(if declared.is_empty() {
                    "this file declares no prefabs. Add `[[prefab]]` with \
                     `key`, `id` and `path`."
                        .to_owned()
                } else {
                    format!(
                        "declared prefab keys: {}",
                        declared.iter().cloned().collect::<Vec<_>>().join(", ")
                    )
                });
                errors.push(err);
            }
        }

        // **An instance declares no components of its own.** Allowing both
        // would give a node two sources for the same component and no rule
        // about which wins — the override map is the one way to deviate.
        if prefab.is_some() && !component_map.is_empty() {
            let mut err = SceneError::new("prefab_instance_has_components", &path);
            err.constraint = "a prefab instance takes its components from the prefab".to_owned();
            err.hint = Some(
                "put the deviation in `[node.overrides]` as \
                 \"TypeName.field\" = value, or drop `prefab` to write the \
                 components directly."
                    .to_owned(),
            );
            errors.push(err);
        }

        let mut override_map = BTreeMap::new();
        if let Some(table_like) = table.get("overrides").and_then(Item::as_table_like) {
            if prefab.is_none() && !inherits {
                let mut err = SceneError::new("overrides_without_prefab", &path);
                err.constraint = "`overrides` requires `prefab`".to_owned();
                err.hint = Some(
                    "overrides are deviations from a prefab. A node with no \
                     prefab has nothing to deviate from — set the fields \
                     directly instead."
                        .to_owned(),
                );
                errors.push(err);
            }
            for (key, item) in table_like.iter() {
                if let Some(reason) = override_key_problem(key) {
                    let mut err = SceneError::new("malformed_override_key", &path);
                    err.field = key.to_owned();
                    err.constraint = reason;
                    err.hint = Some(
                        "an override key is `TypeName.field`, or \
                         `Child/Path::TypeName.field` to reach inside the \
                         instanced sub-tree."
                            .to_owned(),
                    );
                    errors.push(err);
                    continue;
                }
                if let Some(value) = item_to_json(item) {
                    override_map.insert(key.to_owned(), value);
                }
            }
        }

        known.insert(path.clone());
        nodes.push(Node {
            name: name.to_owned(),
            parent: parent.map(str::to_owned),
            path,
            transform,
            components: component_map,
            prefab,
            extends,
            overrides: override_map,
        });
    }

    if root.is_none() && errors.is_empty() {
        errors.push(SceneError::new("no_root", ""));
    }

    if errors.is_empty() {
        Ok(nodes)
    } else {
        Err(errors)
    }
}

/// Why an override key is not well-formed, or `None` if it is.
///
/// The grammar is `[<child path>::]<TypeName>.<field>`. Checked here rather
/// than at resolution because a malformed key is wrong in the file regardless
/// of which prefab it points at — and because a key that cannot be parsed
/// cannot be reported against a target later.
///
/// **Not checked here: whether the target exists.** That needs the prefab, and
/// a missing target is a warning with the value preserved (§5), never an
/// error — see `resolve`.
fn override_key_problem(key: &str) -> Option<String> {
    let (child, field_path) = match key.split_once("::") {
        Some((child, rest)) => (Some(child), rest),
        None => (None, key),
    };

    if let Some(child) = child {
        if child.is_empty() || child.starts_with('/') || child.ends_with('/') {
            return Some("the child path before `::` is empty or has a stray `/`".to_owned());
        }
        if child.split('/').any(|segment| segment.is_empty() || segment.trim() != segment) {
            return Some("every segment of the child path must be a non-empty name".to_owned());
        }
    }

    let Some((type_name, field)) = field_path.split_once('.') else {
        return Some("expected `TypeName.field`, with a `.` between them".to_owned());
    };
    if type_name.is_empty() {
        return Some("the component type before `.` is empty".to_owned());
    }
    if field.is_empty() {
        return Some("the field after `.` is empty".to_owned());
    }
    // Nested fields are addressed with further dots, so only the first split
    // matters — but an empty segment anywhere is still a typo.
    if field.split('.').any(str::is_empty) {
        return Some("a field segment between dots is empty".to_owned());
    }
    None
}

/// Check every component on every node against its registered schema.
fn validate_components(doc: &DocumentMut, nodes: &[Node], registry: &TypeRegistry) -> Vec<SceneError> {
    let Some(entries) = doc.get("node").and_then(Item::as_array_of_tables) else {
        return Vec::new();
    };

    let mut errors = Vec::new();
    // GPU emitters seen so far. Scene-wide rather than per-node, because "one
    // per scene" is a property of the file and no single node can see it.
    let mut gpu_emitters = 0_usize;
    for (table, node) in entries.iter().zip(nodes) {
        // `transform` is sugar for the Transform component (§1.1), but it used
        // to be the one field that skipped this pass — it went straight through
        // serde with `.ok().unwrap_or_default()`, so a guessed key name was
        // ignored and a malformed element threw away the whole transform,
        // rotation and scale included. Both produced a node at the origin and
        // `ok: true`. Routing it through the registry like everything else is
        // the fix; the choke point already knows how to say what is wrong.
        if let Some(item) = table.get("transform") {
            errors.extend(check(registry, "Transform", item, &node.path));
        }

        let Some(components) = table.get("components").and_then(Item::as_table_like) else {
            continue;
        };

        for (type_name, item) in components.iter() {
            // Written as a component, a transform validates cleanly and is then
            // invisible to the ECS, the renderer, physics and `measure` — the
            // node just stays at the origin. It is the spelling an agent
            // reaches for after seeing every other component written this way,
            // so it has to be named rather than accepted. Supporting both would
            // mean two sources of truth for one node's position.
            if type_name == "Transform" {
                let mut err = SceneError::new("unknown_component_type", &node.path);
                err.field = "Transform".to_owned();
                err.constraint = "a component type".to_owned();
                err.hint = Some(
                    "a transform is the node key, not a component: write \
                     `transform = { pos = [...] }` on the node itself."
                        .to_owned(),
                );
                errors.push(err);
                continue;
            }
            let schema_errors = check(registry, type_name, item, &node.path);
            // Cross-field rules run only on a component that already validates,
            // because they read typed values: a `WaterBody` with a string where
            // a number goes has nothing for the steepness limit to compute
            // against, and reporting both would be reporting one fault twice.
            if schema_errors.is_empty() && type_name == "WaterBody" {
                errors.extend(check_water(item, &node.path));
            }
            if schema_errors.is_empty() && type_name == "ParticleEmitter" {
                let has_body = components.get("RigidBody").is_some();
                errors.extend(check_emitter(item, &node.path, has_body, &mut gpu_emitters));
            }
            errors.extend(schema_errors);
        }
    }
    errors
}

/// The GPU-emitter rules, which are refusals rather than comments.
///
/// **The S4 lesson, applied before it can bite.** A key the loader does not
/// understand is a key it ignores, and a *constraint* nobody enforces is a
/// constraint the author discovers as a rendering artifact three commits later.
/// Every one of these has a symptom that looks like a bug in the engine:
///
/// - alpha blending with no sort reads as a scrambled plume,
/// - a pool smaller than the live population reads as blinking particles,
/// - a second GPU emitter reads as one of them silently not drawing.
///
/// `gpu` is the number of GPU emitters seen so far in this scene, carried
/// across nodes because "one per scene" is not a property of any single node.
fn check_emitter(
    item: &Item,
    node: &str,
    has_rigid_body: bool,
    gpu: &mut usize,
) -> Vec<SceneError> {
    // Through serde for the reason `check_water` gives at length: an omitted
    // field must be its documented default here exactly as it will be at load.
    let emitter = match item_to_json(item)
        .ok_or_else(|| "the component is not a table of values".to_owned())
        .and_then(|v| {
            serde_json::from_value::<components::ParticleEmitter>(v).map_err(|e| e.to_string())
        }) {
        Ok(emitter) => emitter,
        Err(why) => {
            let mut err = SceneError::new("component_unreadable", node);
            err.field = "ParticleEmitter".to_owned();
            err.constraint = "a readable ParticleEmitter".to_owned();
            err.hint = Some(format!(
                "{why}. The schema check passed, so this is a field the schema \
                 does not reach. None of the emitter rules could run until it \
                 is fixed."
            ));
            return vec![err];
        }
    };
    if !emitter.gpu {
        return Vec::new();
    }

    *gpu += 1;
    let mut errors = Vec::new();
    let mut refuse = |code: &str, field: &str, value: Value, constraint: String, hint: &str| {
        let mut err = SceneError::new(code, node);
        err.field = format!("ParticleEmitter.{field}");
        err.value = value;
        err.constraint = constraint;
        err.hint = Some(hint.to_owned());
        errors.push(err);
    };

    if !emitter.additive {
        refuse(
            "gpu_emitter_needs_additive",
            "additive",
            Value::from(false),
            "true when gpu = true".to_owned(),
            "there is no GPU sort and none is planned, so a GPU pool draws in \
             slot order — which is correct for additive blending and a visible \
             scramble for alpha. Set `additive = true`, or drop `gpu` and let \
             the CPU path sort it back to front.",
        );
    }

    // Two ordinals N apart share a slot, and they are born N/rate seconds
    // apart; a particle lives at most lifetime*(1 + jitter). The burst is
    // additive because its ordinals are all born at once. Duplicated from
    // `loom_particles::pool_size` on purpose: this crate depends on nothing in
    // the workspace, and the number has to be in the error.
    let live = emitter.rate.max(0.0) * emitter.lifetime.max(0.0)
        * (1.0 + emitter.lifetime_jitter.max(0.0));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let needed = u64::from(emitter.burst) + live.ceil().max(0.0) as u64;
    if needed > u64::from(components::GPU_POOL_MAX) {
        refuse(
            "gpu_emitter_pool_too_small",
            "rate",
            Value::from(emitter.rate),
            format!(
                "a pool of {needed} slots, over the {} the renderer holds",
                components::GPU_POOL_MAX
            ),
            "the pool needs burst + ceil(rate * lifetime * (1 + \
             lifetime_jitter)) slots, or a live particle is overwritten by its \
             successor and the plume blinks. Lower `rate`, `lifetime` or \
             `burst`.",
        );
    }

    if has_rigid_body {
        refuse(
            "gpu_emitter_on_a_moving_node",
            "gpu",
            Value::from(true),
            "a node with no RigidBody".to_owned(),
            "a headless `--sim N` catches the pool up in ONE dispatch, so every \
             particle born during those N ticks is born at the origin the node \
             ended at — the trail is laid along the wrong path. Put the emitter \
             on a static node, or drop `gpu`.",
        );
    }

    if *gpu > 1 {
        refuse(
            "second_gpu_emitter",
            "gpu",
            Value::from(true),
            "at most one GPU emitter per scene".to_owned(),
            "the renderer owns exactly one pool. A second would silently not \
             draw, which is the failure this refusal exists to replace. Move \
             the extra emitters to the CPU path by dropping `gpu`.",
        );
    }

    errors
}

/// The water rules a schema range cannot express, because each one relates
/// several fields to each other.
///
/// **The steepness limit is the one that matters.** A Gerstner wave whose `Q`
/// is too large for its amplitude and wavelength loops through itself: the
/// horizontal displacement stops being monotonic, the surface folds, and the
/// mesh is visibly broken. An agent asked to make the sea choppier will push
/// steepness until exactly that happens, and the symptom reads as a rendering
/// bug rather than as a parameter it chose — so the rejection carries the
/// computed limit and the reason.
fn check_water(item: &Item, node: &str) -> Vec<SceneError> {
    // Through serde rather than off the raw TOML, so an omitted field is its
    // documented default here exactly as it will be at load. Reading the tables
    // directly would compute the limit against zeros the runtime never sees.
    //
    // **A failure here is an error, never "nothing to check".** `.ok()` and an
    // empty vector meant one unreadable field switched the whole of the water
    // validation off — the steepness limit, the wave cap, all of it — while
    // the file still reported `ok: true`. `kind = "lava"` and a three-element
    // `direction` both got there, and both are spellings an agent will try.
    // Same silent no-op the `unknown_field` comment in `loom_reflect` exists
    // to prevent: a value this layer does not understand is a value it must
    // refuse, not one it may ignore.
    let body = match item_to_json(item)
        .ok_or_else(|| "the component is not a table of values".to_owned())
        .and_then(|v| {
            serde_json::from_value::<components::WaterBody>(v).map_err(|e| e.to_string())
        }) {
        Ok(body) => body,
        Err(why) => {
            let mut err = SceneError::new("component_unreadable", node);
            err.field = "WaterBody".to_owned();
            err.constraint = "a readable WaterBody".to_owned();
            err.hint = Some(format!(
                "{why}. The schema check passed, so this is a field the schema \
                 does not reach — a nested value or an enum name. None of the \
                 water rules could run until it is fixed."
            ));
            return vec![err];
        }
    };

    let mut errors = Vec::new();
    let count = body.waves.waves.len();
    if count > components::MAX_WAVES {
        let mut err = SceneError::new("too_many_waves", node);
        err.field = "WaterBody.waves.waves".to_owned();
        err.value = Value::from(count);
        err.constraint = format!("at most {} waves", components::MAX_WAVES);
        err.hint = Some(
            "per-vertex cost is linear in the wave count and the pattern stops \
             visibly repeating well before the cap. Merge or drop the smallest \
             waves."
                .to_owned(),
        );
        errors.push(err);
    }

    // The limit is shared out among the waves, so it depends on how many there
    // are: N waves each at the single-wave limit fold N times as hard.
    let n = count as f64;
    for (index, wave) in body.waves.waves.iter().enumerate() {
        let field = |name: &str| format!("WaterBody.waves.waves[{index}].{name}");
        let wavelength = f64::from(wave.wavelength);
        if wavelength <= 0.0 {
            let mut err = SceneError::new("wave_wavelength_not_positive", node);
            err.field = field("wavelength");
            err.value = Value::from(wave.wavelength);
            err.constraint = "greater than zero".to_owned();
            err.hint = Some(
                "wavelength sets the wave number k = 2π/λ, so zero or negative \
                 makes every derived quantity infinite or backwards."
                    .to_owned(),
            );
            errors.push(err);
            continue;
        }

        // **Not `.abs()`.** Amplitude and steepness are magnitudes, and
        // folding a negative one through `abs` made the steepness limit report
        // someone else's mistake: `amplitude = -5.0` blamed steepness and
        // printed "amplitude 5", and `steepness = -9.0` produced the
        // self-contradictory "value -9.0, constraint at most 3.183". A sign
        // typo is its own fault and reads as one.
        let before = errors.len();
        for (name, value) in [("amplitude", wave.amplitude), ("steepness", wave.steepness)]
            .into_iter()
            .filter(|&(_, value)| value < 0.0)
        {
            let mut err = SceneError::new(&format!("wave_{name}_negative"), node);
            err.field = field(name);
            err.value = Value::from(value);
            err.constraint = "at least zero".to_owned();
            err.hint = Some(
                "amplitude and steepness are magnitudes, not signed offsets — \
                 a negative one is a sign typo. Flip the wave with `direction` \
                 instead."
                    .to_owned(),
            );
            errors.push(err);
        }
        // A wave with a sign typo has nothing left for the steepness limit to
        // say — reporting both would be reporting one fault twice, which is
        // the same rule the caller applies to schema errors.
        if errors.len() != before {
            continue;
        }

        let k = std::f64::consts::TAU / wavelength;
        let amplitude = f64::from(wave.amplitude);
        let steepness = f64::from(wave.steepness);
        // Amplitude zero is a wave that does nothing, and its steepness is
        // unbounded rather than infinite — nothing to reject.
        if amplitude == 0.0 || steepness * n * k * amplitude <= 1.0 {
            continue;
        }

        let limit = 1.0 / (n * k * amplitude);
        let mut err = SceneError::new("wave_steepness_exceeds_limit", node);
        err.field = field("steepness");
        err.value = Value::from(wave.steepness);
        err.constraint = format!(
            "at most {limit:.3} for {count} wave(s) at wavelength {wavelength} \
             and amplitude {amplitude}"
        );
        err.hint = Some(
            "Q*k*A must stay under 1/N for N waves or the surface \
             self-intersects. Reduce steepness or amplitude."
                .to_owned(),
        );
        errors.push(err);
    }
    errors.extend(check_ripples(&body, node));
    errors
}

/// The Courant condition, refused at load with both numbers in the message.
///
/// **This is the only stability bound in the engine that an author can type
/// their way past.** Everything else about the water is a closed form that
/// cannot diverge; a ripple grid is an explicit finite-difference scheme, and
/// the explicit 2D wave equation is unconditionally unstable above
/// `c·dt/h ≤ 1/√2`. Past it the field does not look wrong — it doubles every
/// few ticks, reaches `inf` inside a second, and every body floating on it
/// leaves the scene. That failure is unattributable from the symptom, so it is
/// refused where the numbers are still in hand.
///
/// The second rule is a cost bound rather than a correctness one: the grid is
/// stepped on the CPU inside the fixed step, next to `rapier`, so its cell
/// count is a per-tick cost an agent has no intuition for.
fn check_ripples(body: &components::WaterBody, node: &str) -> Vec<SceneError> {
    let Some(ripples) = body.ripples else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    if ripples.cell <= 0.0 || ripples.extent <= 0.0 {
        let (name, value) = if ripples.cell <= 0.0 {
            ("cell", ripples.cell)
        } else {
            ("extent", ripples.extent)
        };
        let mut err = SceneError::new("ripple_size_not_positive", node);
        err.field = format!("WaterBody.ripples.{name}");
        err.value = Value::from(value);
        err.constraint = "greater than zero".to_owned();
        err.hint = Some(
            "the grid is `extent / cell` samples on a side; a non-positive \
             either makes it empty or divides by zero."
                .to_owned(),
        );
        errors.push(err);
        return errors;
    }

    // **The schema's ranges, enforced.** `#[schemars(range(...))]` documents
    // them and the loader does not read them, so `strength = 5.0` and
    // `cell = 20.0` both validated clean — and `strength` in particular is a
    // number an agent asked to make the ripples livelier will reach for. It is
    // measured *saturated* by the relative-velocity coupling: on `pool.loom`
    // 5.6x the authored value moves the ripple-vs-none picture by 1.6x while
    // leaving the sphere half a metre lower at tick 300 [measured]. That is the
    // worst combination there is — little visible gain, an invisible physics
    // change. Refused with the number in the message, the way the Courant bound
    // already is.
    for (name, value, low, high) in [
        ("extent", ripples.extent, 1.0, 512.0),
        ("cell", ripples.cell, 0.05, 8.0),
        ("speed", ripples.speed, 0.0, 100.0),
        ("damping", ripples.damping, 0.5, 1.0),
        ("strength", ripples.strength, 0.0, 2.0),
    ] {
        if value < low || value > high {
            let mut err = SceneError::new("ripple_value_out_of_range", node);
            err.field = format!("WaterBody.ripples.{name}");
            err.value = Value::from(value);
            err.constraint = format!("between {low} and {high}, and this is {value}");
            err.hint = Some(
                "these are the schema's own bounds — `loom describe WaterBody` \
                 prints them — and outside them the grid is either a no-op or \
                 an invisible change to the physics rather than to the picture."
                    .to_owned(),
            );
            errors.push(err);
        }
    }

    let side = components::ripple_side(ripples.extent, ripples.cell);
    // **A grid narrower than three edge bands is all edge.** The sponge is a
    // fixed number of cells, not a fraction, so on a small domain it reaches
    // the middle from both sides and damps the field to nothing everywhere —
    // an authored wake that never appears and never errors.
    if side < 3 * components::RIPPLE_EDGE_CELLS {
        let mut err = SceneError::new("ripple_grid_too_small", node);
        err.field = "WaterBody.ripples.extent".to_owned();
        err.value = Value::from(ripples.extent);
        err.constraint = format!(
            "at least {} samples a side; {} m / {} m is {side}",
            3 * components::RIPPLE_EDGE_CELLS,
            ripples.extent,
            ripples.cell
        );
        err.hint = Some(format!(
            "the absorbing edge is {} cells wide at every border, so under \
             three times that the taper covers the whole grid and no wake can \
             survive anywhere in it. Widen `extent` or narrow `cell`.",
            components::RIPPLE_EDGE_CELLS
        ));
        errors.push(err);
    }
    if side * side > components::MAX_RIPPLE_CELLS {
        let mut err = SceneError::new("ripple_grid_too_large", node);
        err.field = "WaterBody.ripples.cell".to_owned();
        err.value = Value::from(ripples.cell);
        err.constraint = format!(
            "at most {} cells; {} m / {} m is {side}x{side} = {}",
            components::MAX_RIPPLE_CELLS,
            ripples.extent,
            ripples.cell,
            side * side
        );
        err.hint = Some(
            "the grid is stepped on the CPU every fixed tick, beside rapier. \
             Widen `cell` or shrink `extent`."
                .to_owned(),
        );
        errors.push(err);
    }

    let courant = f64::from(ripples.speed) * f64::from(components::TICK_SECONDS)
        / f64::from(ripples.cell);
    if courant > components::COURANT_LIMIT {
        let limit = components::COURANT_LIMIT * f64::from(ripples.cell)
            / f64::from(components::TICK_SECONDS);
        let mut err = SceneError::new("ripple_speed_exceeds_courant", node);
        err.field = "WaterBody.ripples.speed".to_owned();
        err.value = Value::from(ripples.speed);
        err.constraint = format!(
            "at most {limit:.3} m/s: c*dt/h = {courant:.3} exceeds the 2D limit \
             1/sqrt(2) = {:.3} at dt = {} s and h = {} m",
            components::COURANT_LIMIT,
            components::TICK_SECONDS,
            ripples.cell
        );
        err.hint = Some(
            "the explicit wave stencil diverges above the Courant limit — the \
             field reaches infinity within a second and takes every floating \
             body with it. Lower `speed` or widen `cell`."
                .to_owned(),
        );
        errors.push(err);
    }
    errors
}

/// One component's worth of validation: finiteness, then the schema.
fn check(registry: &TypeRegistry, type_name: &str, item: &Item, node: &str) -> Vec<SceneError> {
    let Some(fields) = item.as_table_like() else {
        return Vec::new();
    };

    // Finiteness first, because it cannot be checked any later. `serde_json`
    // has no way to represent NaN or ±infinity, so `Value::from(f64)` maps them
    // to null — by the time the registry sees the value the evidence is gone,
    // and `mass = nan` was accepted where `mass = 0.0` is correctly rejected.
    // §1 is explicit that these poison the determinism hashes M3 depends on.
    let mut errors = Vec::new();
    for (field, value) in fields.iter() {
        non_finite(value, &format!("{type_name}.{field}"), &mut errors, node);
    }
    if !errors.is_empty() {
        return errors;
    }

    let mut object = serde_json::Map::new();
    for (field, value) in fields.iter() {
        if let Some(v) = item_to_json(value) {
            object.insert(field.to_owned(), v);
        }
    }

    match registry.validate(type_name, &Value::Object(object)) {
        Ok(()) => Vec::new(),
        Err(field_errors) => field_errors
            .into_iter()
            .map(|f| SceneError {
                error: f.error,
                node: node.to_owned(),
                field: f.field,
                value: f.value,
                constraint: f.constraint,
                hint: f.hint,
            })
            .collect(),
    }
}

/// `{ x = _, y = _, z = _ }` as `[x, y, z]`, per §7.
///
/// Exactly those three keys and nothing else: a table with a missing or extra
/// key is not a Vec3, and must stay whatever it was so the validator can
/// reject it rather than have it quietly become something valid.
fn vec3_from_table(table: &dyn toml_edit::TableLike) -> Option<Value> {
    if table.len() != 3 {
        return None;
    }
    let mut axes = Vec::with_capacity(3);
    for key in ["x", "y", "z"] {
        let item = table.get(key)?;
        // Integers coerce to floats where a float is expected (§7), so both
        // are a Vec3 component. Routing through `item_to_json` rather than
        // forcing f64 keeps that coercion in one place.
        if item.as_float().is_none() && item.as_integer().is_none() {
            return None;
        }
        axes.push(item_to_json(item)?);
    }
    Some(Value::Array(axes))
}

/// `"Vec3(0, 1, 0)"` as `[0.0, 1.0, 0.0]`, per §7.
///
/// Returns `None` for anything that is not exactly this shape, so an ordinary
/// string field is untouched and a malformed one stays a string for the
/// validator to reject.
fn vec3_from_str(text: &str) -> Option<Value> {
    let inner = text.trim().strip_prefix("Vec3(")?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').collect();
    if parts.len() != 3 {
        return None;
    }
    let mut out = Vec::with_capacity(3);
    for p in parts {
        // `"nan".parse::<f64>()` succeeds. Refusing here leaves the text as a
        // string, which the validator rejects — a non-finite component must
        // never reach a transform (§1).
        let n = p.trim().parse::<f64>().ok().filter(|n| n.is_finite())?;
        out.push(Value::from(n));
    }
    Some(Value::Array(out))
}

/// Record every non-finite float reachable from `item`, named by its path.
fn non_finite(item: &Item, field: &str, out: &mut Vec<SceneError>, node: &str) {
    if let Some(v) = item.as_float() {
        if !v.is_finite() {
            out.push(SceneError {
                error: "non_finite_float".to_owned(),
                node: node.to_owned(),
                field: field.to_owned(),
                value: Value::String(v.to_string()),
                constraint: "a finite number".to_owned(),
                hint: Some(
                    "NaN and infinity are rejected on every field: they poison the \
                     determinism hashes the simulation depends on."
                        .to_owned(),
                ),
            });
        }
        return;
    }
    if let Some(array) = item.as_array() {
        for (i, v) in array.iter().enumerate() {
            non_finite(&Item::Value(v.clone()), &format!("{field}[{i}]"), out, node);
        }
        return;
    }
    // `[[component.list]]` — an array of tables, which is NOT `as_array` and
    // NOT `as_table_like`. A voxel volume's op list is exactly this shape, and
    // missing the case here meant a `radius = nan` in the recipe validated
    // clean and was then dropped, silently changing the geometry the file
    // claims to describe (never-do #11).
    if let Some(tables) = item.as_array_of_tables() {
        for (i, table) in tables.iter().enumerate() {
            for (key, v) in table.iter() {
                non_finite(v, &format!("{field}[{i}].{key}"), out, node);
            }
        }
        return;
    }
    // The §7 object spelling of a Vec3 is a table, so without this a
    // `{ x = nan, ... }` is only caught as a type error, not as the
    // `non_finite_float` §6 names.
    if let Some(table) = item.as_table_like() {
        for (key, v) in table.iter() {
            non_finite(v, &format!("{field}.{key}"), out, node);
        }
    }
}

/// Bridge a TOML item into `serde_json`, which is what the registry validates
/// against. Only the shapes the format permits in a component field.
fn item_to_json(item: &Item) -> Option<Value> {
    if let Some(v) = item.as_str() {
        // §7: `"Vec3(0, 1, 0)"` is one of three interchangeable spellings.
        return Some(vec3_from_str(v).unwrap_or_else(|| Value::from(v)));
    }
    if let Some(v) = item.as_bool() {
        return Some(Value::from(v));
    }
    if let Some(v) = item.as_integer() {
        // Integers coerce to floats where a float is expected (§7).
        return Some(Value::from(v));
    }
    if let Some(v) = item.as_float() {
        return Some(Value::from(v));
    }
    if let Some(array) = item.as_array() {
        let elements: Vec<Value> = array
            .iter()
            .filter_map(|v| item_to_json(&Item::Value(v.clone())))
            .collect();
        return Some(Value::Array(elements));
    }
    // `[[component.list]]` — an array of tables, which is NOT `as_array` and
    // NOT `as_table_like`. Missing this case silently DROPS the whole list,
    // which is how a voxel volume's op list vanished and left a scene that
    // parsed cleanly and rendered nothing.
    if let Some(tables) = item.as_array_of_tables() {
        let elements: Vec<Value> = tables
            .iter()
            .filter_map(|t| item_to_json(&Item::Table(t.clone())))
            .collect();
        return Some(Value::Array(elements));
    }
    // §7: `{ x = 0.0, y = 1.0, z = 0.0 }` is one of three interchangeable
    // spellings. Normalising here rather than per-field means every Vec3
    // field gets it — position, rotation, scale, colour, half-extents — and
    // both the validator and serde see the one form they already understand.
    if let Some(v) = item.as_table_like().and_then(vec3_from_table) {
        return Some(v);
    }
    if let Some(table) = item.as_table_like() {
        let mut object: BTreeMap<String, Value> = BTreeMap::new();
        for (k, v) in table.iter() {
            if let Some(v) = item_to_json(v) {
                object.insert(k.to_owned(), v);
            }
        }
        return serde_json::to_value(object).ok();
    }
    None
}
