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
    // Whether this scene has state a nondeterministic surface would quietly
    // demote, which decides whether cinematic water is allowed to be silent
    // about what it costs (ADR 0053 §5). Scene-wide and read ahead of the loop,
    // because the rules may be authored on a node after the water — a check
    // that only looked backwards would pass or fail on node order.
    //
    // **`CharacterController` counts, and not by analogy.** A character
    // standing on a hull that a cinematic body is holding up has a position
    // that is this GPU's answer and no other machine's, and a script reads that
    // position every tick to decide where to walk next. That is the same
    // demotion `GameRules` gets refused for, arriving through the floor instead
    // of through the rules. It became reachable when a capsule could ride a
    // floating body; before that nothing did.
    let has_gameplay = entries.iter().any(|table| {
        table
            .get("components")
            .and_then(Item::as_table_like)
            .is_some_and(|c| {
                c.get("GameRules").is_some() || c.get("CharacterController").is_some()
            })
    });
    // Same shape, same reason: a cascade's fall ends at the scene's still-water
    // level, and the `WaterBody` that supplies it may be authored on a later
    // node than the cascade.
    let has_water = entries.iter().any(|table| {
        table
            .get("components")
            .and_then(Item::as_table_like)
            .is_some_and(|c| c.get("WaterBody").is_some())
    });
    // Cascades seen so far. Scene-wide, exactly as `gpu_emitters` is.
    let mut cascades = 0_usize;
    // Cinematic water bodies seen so far — ADR 0059's budget, and scene-wide
    // for the same reason the emitter count is: "one per scene" is a property
    // of the file that no single node can see.
    let mut cinematic_bodies = 0_usize;
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
                errors.extend(check_water(
                    item,
                    &node.path,
                    has_gameplay,
                    &mut cinematic_bodies,
                ));
            }
            if schema_errors.is_empty() && type_name == "Cascade" {
                errors.extend(check_cascade(item, &node.path, has_water, &mut cascades));
            }
            if schema_errors.is_empty() && type_name == "ParticleEmitter" {
                let has_body = components.get("RigidBody").is_some();
                errors.extend(check_emitter(item, &node.path, has_body, &mut gpu_emitters));
            }
            errors.extend(schema_errors);
        }
    }
    // Scene-wide and after the loop, because every Deform rule but the field
    // ranges is about a *subtree*: whether there is a mesh under it, whether
    // there is another Deform under it, whether a child has been moved out of
    // its frame. `nodes` already carries the parsed paths and transforms, so
    // this needs neither the TOML document nor a tree.
    errors.extend(check_deforms(nodes));
    errors.extend(check_moods(nodes));
    errors
}

/// The `Environment.stages` rules — ADR 0069, and refusals rather than
/// comments.
///
/// **The deserialise is the point, not the sorting.** `EnvironmentPatch` and
/// `Grade` carry `deny_unknown_fields`, and `loom_reflect::validate` walks a
/// component's *top-level* keys only — so `sun_strenth = 0.4` inside a stage
/// table is a key the schema check never reaches. Dropped silently, the stage
/// renders the near look at every `dread` and reports `{"ok": true}`, which is
/// the S4 prefab bug in a new place. Reading the value through serde here is
/// what turns that into an error with a line the author can find.
fn check_moods(nodes: &[Node]) -> Vec<SceneError> {
    let mut errors = Vec::new();
    for node in nodes {
        let Some(value) = node.components.get("Environment") else {
            continue;
        };
        let environment = match serde_json::from_value::<components::Environment>(value.clone()) {
            Ok(environment) => environment,
            Err(why) => {
                let mut err = SceneError::new("component_unreadable", &node.path);
                err.field = "Environment".to_owned();
                err.constraint = "a readable Environment".to_owned();
                err.hint = Some(format!(
                    "{why}. The schema check passed, so this is a field the \
                     schema does not reach — most likely a misspelled key \
                     inside a `stages` table, which would otherwise be dropped \
                     at load and render the near look at every `dread`."
                ));
                errors.push(err);
                continue;
            }
        };

        // One stage is a constant, and a constant that looks like a ramp is
        // worse than no ramp: the author sees a `stages` block, the scene
        // never changes, and nothing says why.
        if environment.stages.len() == 1 {
            let mut err = SceneError::new("field_out_of_range", &node.path);
            err.field = "Environment.stages".to_owned();
            err.constraint = "at least two stages, or none".to_owned();
            err.hint = Some(
                "One stage cannot be blended between, so `dread` would move \
                 nothing. Author a second stage, or drop the block."
                    .to_owned(),
            );
            errors.push(err);
        }

        // **The declared ranges, enforced where they can be.**
        //
        // `loom_reflect::validate` walks a component's *top-level* keys, so
        // `dread = 9.0` is refused and every `#[schemars(range(...))]` inside a
        // stage is decoration — measured: `gain = [-9.0, ..]`, `saturation =
        // 50.0` and `at = 5.0` all validated clean and exited 0. The struct is
        // already deserialised here, which is the cheapest place in the project
        // that can see them.
        //
        // **`at` is the one that fails silently**, and that is why this is a
        // refusal and not a doc note. `dread` is a 0..1 scalar and `mood_of`
        // clamps to the authored span, so a ladder written `at = 0..5` uses
        // only its first fifth: the far stages are unreachable, every rung
        // renders, and nothing anywhere says why the sea never gets worse.
        for stage in &environment.stages {
            let out_of_range = [
                ("at", stage.at, 0.0, 1.0),
                ("grade.gain.r", stage.grade.gain[0], 0.0, 4.0),
                ("grade.gain.g", stage.grade.gain[1], 0.0, 4.0),
                ("grade.gain.b", stage.grade.gain[2], 0.0, 4.0),
                ("grade.contrast", stage.grade.contrast, 0.2, 3.0),
                ("grade.saturation", stage.grade.saturation, 0.0, 2.0),
            ];
            // The two optional ones, checked only when named — `None` means
            // "leave the scene's own alone" and has no range to be outside of.
            let optional = [
                ("wind_speed", stage.wind_speed, 0.0, 40.0),
                ("rain_intensity", stage.rain_intensity, 0.0, 100.0),
            ];
            let named = optional
                .into_iter()
                .filter_map(|(f, v, lo, hi)| v.map(|v| (f, v, lo, hi)));
            for (field, value, low, high) in out_of_range.into_iter().chain(named) {
                if value >= low && value <= high {
                    continue;
                }
                let mut err = SceneError::new("field_out_of_range", &node.path);
                err.field = format!("Environment.stages.{field}");
                err.value = serde_json::json!(value);
                err.constraint = format!("{low}..={high}");
                err.hint = Some(format!(
                    "Stage `{}` sets {field} to {value}. The range is the one \
                     the field's schema declares; `loom_reflect::validate` \
                     reaches a component's top-level keys only, so this is \
                     where a stage's numbers are checked.",
                    stage.name
                ));
                errors.push(err);
            }
        }

        // Sorted and distinct: the blend divides by the span between a
        // bracketing pair, and two stages at one `at` is a zero-span divide.
        if let Some(w) = environment.stages.windows(2).find(|w| w[1].at <= w[0].at) {
            let mut err = SceneError::new("field_out_of_range", &node.path);
            err.field = "Environment.stages".to_owned();
            err.constraint = "sorted by `at`, strictly increasing".to_owned();
            err.hint = Some(format!(
                "`{}` is at {} and `{}` follows it at {}. Stages are read in \
                 file order and blended between neighbours.",
                w[0].name, w[0].at, w[1].name, w[1].at
            ));
            errors.push(err);
        }
    }
    errors
}

/// The `Deform` rules — ADR 0062, and refusals rather than comments.
///
/// **The S4 lesson again.** Each of these has a symptom that reads as a bug in
/// the engine rather than as a mistake in the file, and four of them produce a
/// scene that renders and validates cleanly while doing nothing:
///
/// - `amplitude = 0` is a knob wired to nothing: the node draws unchanged and
///   `loom validate` says `ok: true`;
/// - a beat along its own travel axis makes the displacement a function of the
///   coordinate it moves, so the Jacobian is no longer rank-1 and the analytic
///   normal is silently wrong — invisible in a diff, obvious in motion;
/// - `wavelength = 0` divides by zero and NaNs every vertex of the mesh, in a
///   shader nobody can put a breakpoint in;
///
/// `span_start` outside [0, 0.99] is refused too, by `#[schemars(range)]` on
/// the field rather than here — the schema names the bound and prints the
/// value, and duplicating it below would report one fault twice;
/// - a `Deform` with no mesh under it, or nested inside another, or over a
///   child that has been moved away from its parent, each render as one of
///   "nothing happened" or "the animal came apart" with no message anywhere.
///
/// **All of it here, and that is why `amplitude` is a body fraction rather than
/// metres.** The peak-slope ceiling is the one rule that would otherwise need
/// the mesh, and the only function in this project holding a mesh library is
/// `loom_cli::world_to_objects` — called every frame, with no way to refuse
/// anything and no error to return. Authoring the amplitude as a fraction makes
/// that rule arithmetic, so it runs at load with everything else.
fn check_deforms(nodes: &[Node]) -> Vec<SceneError> {
    let mut errors = Vec::new();
    // A node is "under" another when its path is that path plus a `/`. Paths
    // are already slash-separated and unique, so this is the whole of the
    // subtree question and needs no tree to be built.
    let under = |ancestor: &str, path: &str| path.len() > ancestor.len()
        && path.starts_with(ancestor)
        && path.as_bytes()[ancestor.len()] == b'/';

    for node in nodes {
        let Some(value) = node.components.get("Deform") else {
            continue;
        };
        // Through serde for the reason `check_water` gives at length: an
        // omitted field must be its documented default here exactly as it will
        // be at load.
        let deform = match serde_json::from_value::<components::Deform>(value.clone()) {
            Ok(deform) => deform,
            Err(why) => {
                let mut err = SceneError::new("component_unreadable", &node.path);
                err.field = "Deform".to_owned();
                err.constraint = "a readable Deform".to_owned();
                err.hint = Some(format!(
                    "{why}. The schema check passed, so this is a field the \
                     schema does not reach. None of the deform rules could run \
                     until it is fixed."
                ));
                errors.push(err);
                continue;
            }
        };

        let mut refuse = |code: &str, field: &str, value: Value, constraint: String, hint: String| {
            let mut err = SceneError::new(code, &node.path);
            err.field = format!("Deform.{field}");
            err.value = value;
            err.constraint = constraint;
            err.hint = Some(hint);
            errors.push(err);
        };

        if deform.amplitude <= 0.0 {
            refuse(
                "deform_has_no_amplitude",
                "amplitude",
                Value::from(f64::from(deform.amplitude)),
                "greater than 0 body lengths".to_owned(),
                "the shader early-outs at `amplitude <= 0`, so this node draws \
                 exactly as it would with no Deform at all and every other \
                 field here is dead text. There is no sane default, and there \
                 is no unit to guess one in: this is a fraction of the body, \
                 so 0.1 is a tenth of the animal whatever size the animal is."
                    .to_owned(),
            );
        }
        if deform.beat.index() == deform.nose.axis() {
            refuse(
                "deform_beats_along_its_own_axis",
                "beat",
                serde_json::to_value(deform.beat).unwrap_or(Value::Null),
                format!(
                    "an axis other than nose's ({})",
                    serde_json::to_value(deform.nose)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default()
                ),
                "the displacement is a function of the body coordinate and \
                 points along a different axis, which is what makes its \
                 Jacobian rank-1 and the corrected normal three operations. \
                 Beating along the travel axis breaks that, and the wrong \
                 normal is invisible in a diff and obvious in motion."
                    .to_owned(),
            );
        }
        // **A slope ceiling rather than an amplitude ceiling**, because
        // shortening `wavelength` steepens the shear exactly as much as raising
        // `amplitude` does, and a refusal that only looked at amplitude could
        // not see it. Closed form, and it over-reads the true worst gradient by
        // roughly 30%, which is the safe direction; the measured calibration
        // table is on `DEFORM_MAX_SLOPE`.
        //
        // This is arithmetic rather than geometry only because `amplitude` is a
        // body fraction. In metres it would need the mesh, and the only place
        // in this project holding a mesh library is a function called every
        // frame with no way to refuse anything.
        //
        // Only on a component whose other fields are already in range: a
        // `span_start` of 1.0 makes this arbitrarily large, and reporting that
        // as "too steep" on top of the schema's own range error would be
        // reporting one fault twice with the second message misleading.
        let sane = deform.wavelength > 0.0 && (0.0..1.0).contains(&deform.span_start);
        let slope = if sane {
            deform.amplitude
                * (std::f32::consts::TAU / deform.wavelength + 2.0 / (1.0 - deform.span_start))
        } else {
            0.0
        };
        if sane && slope > components::DEFORM_MAX_SLOPE {
            let ceiling = deform.amplitude * components::DEFORM_MAX_SLOPE / slope;
            refuse(
                "deform_surface_is_too_steep",
                "amplitude",
                Value::from(f64::from(deform.amplitude)),
                format!(
                    "a peak surface slope of at most {}, which these other \
                     settings put at amplitude {ceiling:.3}",
                    components::DEFORM_MAX_SLOPE
                ),
                format!(
                    "this authors {slope:.2}. Past about 2.6 the shaded normal \
                     has turned more than 90 degrees from its rest direction \
                     and the surface has folded as far as lighting is \
                     concerned — the body reads as torn rather than as \
                     swimming. Lower `amplitude`, lengthen `wavelength`, or \
                     lower `span_start` to spread the same displacement over \
                     more of the body."
                ),
            );
        }
        if deform.wavelength <= 0.0 {
            refuse(
                "deform_wavelength_is_not_positive",
                "wavelength",
                Value::from(f64::from(deform.wavelength)),
                "greater than 0 body lengths".to_owned(),
                "the wavenumber is 1/wavelength, so zero divides by zero and \
                 NaNs every vertex of every mesh under this node — in a vertex \
                 shader, where there is nothing to breakpoint and the mesh \
                 simply vanishes."
                    .to_owned(),
            );
        }

        let mut has_mesh = node.components.contains_key("MeshRenderer");
        for other in nodes {
            if !under(&node.path, &other.path) {
                continue;
            }
            if other.components.contains_key("MeshRenderer") {
                has_mesh = true;
                // Every mesh under the deform shares one body coordinate, and
                // that is only correct while it shares the deform node's frame.
                //
                // **This refusal has a known expiry date** — the next creature
                // with an offset fin hits it on day one. The arithmetic that
                // lifts it, written down so it is not rediscovered: for a child
                // at translation `t` with uniform scale `k` along the nose
                // axis, `lead' = (lead - t) / k`, `inv_len' = inv_len * k` and
                // `amplitude' = amplitude / k`. A *rotated* child additionally
                // needs both axis codes rotated, which the two-code encoding
                // cannot express at all. Do not build any of it speculatively.
                if !is_identity(&other.transform) {
                    let mut err = SceneError::new("deform_child_is_not_at_identity", &other.path);
                    err.field = "transform".to_owned();
                    err.value = Value::from(format!(
                        "pos {:?} rot {:?} scale {:?}",
                        other.transform.pos, other.transform.rot_euler, other.transform.scale
                    ));
                    err.constraint = format!(
                        "identity, because {} carries a Deform above it",
                        node.path
                    );
                    err.hint = Some(
                        "the wave is one body coordinate shared by every mesh \
                         in the subtree — that is what keeps the shell and the \
                         trim agreeing about where the tail is. A child moved \
                         away from that frame would be deformed against the \
                         parent's body and slide off it. Put the offset on the \
                         deform node itself, or give the child its own Deform."
                            .to_owned(),
                    );
                    errors.push(err);
                }
            }
            if other.components.contains_key("Deform") {
                let mut err = SceneError::new("deform_inside_a_deform", &other.path);
                err.field = "Deform".to_owned();
                err.constraint = format!("no Deform under {}, which already has one", node.path);
                err.hint = Some(
                    "the inner one would have to compose two body coordinates \
                     and does not: the outer wave moves the vertices and the \
                     inner one reads their rest positions, so the two disagree \
                     about where the body is. One wave per body."
                        .to_owned(),
                );
                errors.push(err);
            }
        }
        if !has_mesh {
            let mut err = SceneError::new("deform_has_no_mesh", &node.path);
            err.field = "Deform".to_owned();
            err.constraint = "a MeshRenderer on this node or under it".to_owned();
            err.hint = Some(
                "a Deform displaces vertices and nothing else — no transform, \
                 no collider, no rigid body. With no mesh in the subtree it is \
                 a component that validates, loads, and cannot possibly have \
                 an effect."
                    .to_owned(),
            );
            errors.push(err);
        }
    }
    errors
}

/// Whether a local transform is the one the deform arithmetic assumes.
fn is_identity(t: &components::Transform) -> bool {
    t.pos.iter().all(|v| v.abs() < 1.0e-6)
        && t.rot_euler.iter().all(|v| v.abs() < 1.0e-6)
        && t.scale.iter().all(|v| (v - 1.0).abs() < 1.0e-6)
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
/// The cascade rules — ADR 0054, and refusals rather than comments.
///
/// Each of these is a bug that would otherwise be diagnosed as "the waterfall
/// does not draw", which is the least informative symptom a scene can produce:
///
/// - a cascade in a scene with no `WaterBody` has nothing to fall *to*, and the
///   water pass it is drawn inside is skipped entirely, so it is invisible;
/// - a second cascade is silently not drawn, because the environment buffer
///   carries one;
/// - a lip whose two ends coincide has no direction for the water to leave
///   along, and the strip collapses to a line.
fn check_cascade(item: &Item, node: &str, has_water: bool, seen: &mut usize) -> Vec<SceneError> {
    // Through serde for the reason `check_water` gives at length: an omitted
    // field must be its documented default here exactly as it will be at load.
    let cascade = match item_to_json(item)
        .ok_or_else(|| "the component is not a table of values".to_owned())
        .and_then(|v| {
            serde_json::from_value::<components::Cascade>(v).map_err(|e| e.to_string())
        }) {
        Ok(cascade) => cascade,
        Err(why) => {
            let mut err = SceneError::new("component_unreadable", node);
            err.field = "Cascade".to_owned();
            err.constraint = "a readable Cascade".to_owned();
            err.hint = Some(format!(
                "{why}. The schema check passed, so this is a field the schema \
                 does not reach. None of the cascade rules could run until it \
                 is fixed."
            ));
            return vec![err];
        }
    };

    *seen += 1;
    let mut errors = Vec::new();
    let mut refuse = |code: &str, field: &str, value: Value, constraint: &str, hint: &str| {
        let mut err = SceneError::new(code, node);
        err.field = format!("Cascade.{field}");
        err.value = value;
        err.constraint = constraint.to_owned();
        err.hint = Some(hint.to_owned());
        errors.push(err);
    };

    if !has_water {
        refuse(
            "cascade_needs_water_to_fall_into",
            "discharge",
            Value::from(f64::from(cascade.discharge)),
            "a WaterBody somewhere in the scene",
            "the sheet is drawn inside the water block and ends at the scene's \
             still-water level, so a scene with no WaterBody draws no cascade \
             at all. Add a WaterBody node with the pool's surface_height.",
        );
    }
    if *seen > 1 {
        refuse(
            "one_cascade_per_scene",
            "discharge",
            Value::from(f64::from(cascade.discharge)),
            "at most one Cascade per scene",
            "the environment buffer carries one lip, so a second cascade would \
             validate and then not draw. Two falls need two scenes today.",
        );
    }
    let span = [
        cascade.lip_b[0] - cascade.lip_a[0],
        cascade.lip_b[1] - cascade.lip_a[1],
        cascade.lip_b[2] - cascade.lip_a[2],
    ];
    let length = span.iter().map(|c| c * c).sum::<f32>().sqrt();
    if length < 1.0e-3 {
        refuse(
            "cascade_lip_has_no_length",
            "lip_b",
            Value::from(f64::from(length)),
            "lip_a and lip_b at least a millimetre apart",
            "the water leaves along `cross(lip_b - lip_a, +Y)`, so a lip with \
             coincident ends has no direction and the sheet collapses to a \
             line. Swap the two ends to send the fall the other way.",
        );
    }
    errors
}

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

/// The cells a cinematic domain of this extent gets.
///
/// Mirrors `loom_render::fluid_grid`, which this crate cannot call: `loom_scene`
/// depends on nothing else in the workspace, which is a CI-enforced rule. The
/// arithmetic is two lines and the test below pins the two together by the
/// numbers a real scene produces.
fn fluid_cells(extent: [f32; 3]) -> usize {
    let longest = extent.iter().copied().fold(0.0_f32, f32::max);
    if longest <= 0.0 {
        return 0;
    }
    let cell = longest / 64.0;
    extent
        .iter()
        .map(|e| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let n = (e / cell).round().max(1.0) as usize;
            n
        })
        .product()
}

/// The cells one cinematic domain may hold — ADR 0059.
///
/// **A budget stated in cells because cost is measured flat in domain size.**
/// The solver's per-tick cost is dominated by dispatch count, which is a
/// function of the multigrid level count and not of the cells in them, so a
/// small domain does not buy back much; what the budget is really bounding is
/// the particle count, the P2G gather and the CPU marching cubes, all of which
/// are linear in cells.
///
/// 65,536 is twice `plough_cinematic`'s 32,768, which measures 3.2 ms a tick on
/// an RTX 4090 — 19% of a 60 Hz frame before anything is drawn. Anything that
/// wants more is asking for the whole frame.
pub const MAX_CINEMATIC_CELLS: usize = 65_536;

/// The tier rules — ADR 0053 §5 and ADR 0059, as refusals rather than comments.
///
/// **Same S4 lesson the emitter rules are written from.** A constraint nobody
/// enforces is a constraint the author meets as a symptom later, and each of
/// these has a symptom that reads as an engine fault:
///
/// - a cinematic body with no domain has nowhere to solve, so it draws the
///   analytic surface and the author concludes the tier does not work;
/// - `extent` on a deterministic body reads as a bounded pool and silently
///   is not one;
/// - a game whose rules read cinematic water still says `ok: true`, still
///   replays on this box, and disagrees with itself on any other.
fn check_tier(
    body: &components::WaterBody,
    node: &str,
    has_gameplay: bool,
    cinematic_bodies: &mut usize,
) -> Vec<SceneError> {
    let cinematic = body.simulation == components::WaterSimTier::Cinematic;
    if cinematic {
        *cinematic_bodies += 1;
    }
    let mut errors = Vec::new();
    let mut refuse = |code: &str, field: &str, value: Value, constraint: &str, hint: &str| {
        let mut err = SceneError::new(code, node);
        err.field = format!("WaterBody.{field}");
        err.value = value;
        err.constraint = constraint.to_owned();
        err.hint = Some(hint.to_owned());
        errors.push(err);
    };

    if cinematic && body.extent.is_none() {
        refuse(
            "cinematic_water_needs_an_extent",
            "extent",
            Value::Null,
            "extent = [x, y, z] in metres when simulation = \"cinematic\"",
            "the solver is a bounded volume and has to be told how big it is; \
             there is no default, because a guessed domain is either empty at \
             the surface or a grid nobody can afford. It is centred on this \
             node and anchored to it, never to the camera (ADR 0053 §5).",
        );
    }

    if !cinematic && body.extent.is_some() {
        refuse(
            "extent_on_deterministic_water",
            "extent",
            Value::from(body.extent.map(Vec::from).unwrap_or_default()),
            "extent only on simulation = \"cinematic\"",
            "the analytic surface is an unbounded plane and bounding it is a \
             separate, deferred decision — the trigger is two visible water \
             levels on camera. Accepting the field here would implement it by \
             accident and half-way. Set `simulation = \"cinematic\"`, or drop \
             `extent`.",
        );
    }

    // The grid is 64 cells along the longest axis, always, so the cell size is
    // not authored — it is `max_extent / 64`, and the extent is the only knob.
    // Outside [0.05, 0.5] m the answer is useless in one of two ways and both
    // read as an engine fault: below it the 64³ grid covers less than three
    // metres and the water is a puddle in the corner of the shot; above it a
    // cell is half a metre and a pontoon smaller than one cell can be entirely
    // inside a cell the solver calls air.
    if let Some(extent) = body.extent.filter(|_| cinematic) {
        let longest = extent.iter().copied().fold(0.0_f32, f32::max);
        let cell = longest / 64.0;
        if !(0.05..=0.5).contains(&cell) {
            refuse(
                "cinematic_cell_out_of_range",
                "extent",
                Value::from(Vec::from(extent)),
                "the longest extent between 3.2 m and 32.0 m",
                &format!(
                    "the grid is 64 cells along the longest axis, so the cell \
                     is {cell:.4} m and must be in [0.05, 0.5]. This extent's \
                     longest axis is {longest:.2} m; make it at least 3.2 and \
                     at most 32.0 (ADR 0057)."
                ),
            );
        }
    }

    // **The tier is a hero volume with a budget, not the water system** —
    // ADR 0059. The same authored event measures 141 fps on `plough.loom` and
    // 37 on `plough_cinematic.loom`, and the deterministic height-field tier is
    // what carries a scene. Both refusals below are the ADR 0047 pattern: the
    // number the author needs is in the message, because a budget nobody can
    // read is a budget nobody meets.
    if *cinematic_bodies > 1 {
        refuse(
            "second_cinematic_water_body",
            "simulation",
            Value::from("cinematic"),
            "at most one cinematic WaterBody per scene",
            "the engine builds one solver, with its own Vulkan device, its own \
             particle buffer and its own synchronous readback inside the fixed \
             step. A second would silently not solve — the shape of failure \
             this refusal exists to replace. Everything outside the hero volume \
             belongs on `simulation = \"deterministic\"`, which is the water \
             system and runs at 140 fps in the same scene.",
        );
    }

    if let Some(extent) = body.extent.filter(|_| cinematic) {
        let cells = fluid_cells(extent);
        if cells > MAX_CINEMATIC_CELLS {
            refuse(
                "cinematic_domain_over_budget",
                "extent",
                Value::from(Vec::from(extent)),
                "at most 65,536 cells, so the two shorter axes must be small",
                &format!(
                    "the grid is 64 cells along the longest axis, so this \
                     extent is {cells} cells and the budget is \
                     {MAX_CINEMATIC_CELLS}. Measured on this machine, an RTX \
                     4090: 32,768 cells is 3.2 ms a tick, which at 60 Hz is \
                     19% of a 60 fps frame's whole budget before anything is \
                     drawn. Reshape the volume — a long shallow tank is the \
                     shape a hero effect reads in, and a cube of the same \
                     length is eight times the cost (ADR 0059)."
                ),
            );
        }
    }

    if cinematic && has_gameplay && !body.acknowledge_nondeterminism {
        refuse(
            "cinematic_water_demotes_a_game",
            "simulation",
            Value::from("cinematic"),
            "acknowledge_nondeterminism = true, or deterministic water",
            "ADR 0053 §5: a deterministic body may never be forced by a \
             cinematic one, and this scene has GameRules or a \
             CharacterController. Rules read `submersion`, which comes off this \
             water; a character rides a hull this water holds up, and its \
             script reads that position every tick. A replay, a save file or a \
             networked session involving either is not guaranteed to agree \
             between two computers. Silently demoting a game's determinism is \
             the failure this clause exists to prevent, so say it out loud: \
             `acknowledge_nondeterminism = true`.",
        );
    }

    errors
}

/// The swell rules, as refusals rather than comments — the second sea a
/// `spectrum` body may author.
///
/// **A nested table gets no schema check at all**, and that is why the ranges
/// below are spelled out here instead of being left to `schemars`. The
/// registry validates one level: it resolves a field's `$ref`, checks its
/// declared type, its enum and its range, and for a field whose value is an
/// *object* there is no number to range-check and it descends no further. So
/// `swell.fetch = 5.0e9` passes the schema exactly as `flow.speed = 900.0`
/// does. The attributes on [`components::Swell`] are still worth carrying —
/// they are what the inspector and `loom describe` draw — but they are
/// documentation here, not enforcement.
///
/// Each refusal replaces a symptom that reads as an engine fault:
///
/// - a swell on a `gerstner` body is a field the loader reads past entirely,
///   so the author sees the sea they did not ask for and nothing says why;
/// - a swell with no heading normalises to the *wind's* heading, which is a
///   crossing sea that is silently not crossing — the one failure this whole
///   feature exists to make visible;
/// - a swell with no wind carries no energy, so the second sea is authored,
///   validated, summed, and worth exactly zero.
fn check_swell(body: &components::WaterBody, node: &str) -> Vec<SceneError> {
    let Some(swell) = body.swell else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    let mut refuse = |code: &str, field: &str, value: Value, constraint: &str, hint: String| {
        let mut err = SceneError::new(code, node);
        err.field = format!("WaterBody.swell.{field}");
        err.value = value;
        err.constraint = constraint.to_owned();
        err.hint = Some(hint);
        errors.push(err);
    };

    if body.wave_model != components::WaveModel::Spectrum {
        refuse(
            "swell_on_gerstner_water",
            "u10",
            Value::from(swell.u10),
            "a swell only on wave_model = \"spectrum\"",
            "a swell is a second spectrum summed into the amplitude grid, and \
             the gerstner model has no grid: it is a sum of sixteen waves \
             derived from one wind, so this swell would be authored, \
             validated and read past — the same silent drop \
             `spectrum_water_authors_waves` refuses in the other direction. \
             Set `wave_model = \"spectrum\"`, or drop `swell`."
                .to_owned(),
        );
    }

    // Length rather than each component: `[0, 0]` is the spelling that arrives
    // by omission, and `normalise` answers `None` for it — after which
    // `running_direction` falls back to the *wind's* heading and the crossing
    // sea the author wrote is a following one. Nothing downstream can tell
    // that from a swell genuinely authored along the wind.
    let [dx, dz] = swell.direction;
    if dx.hypot(dz) <= 0.0 {
        refuse(
            "swell_without_direction",
            "direction",
            Value::from(vec![dx, dz]),
            "direction = [x, z], any non-zero length",
            "a swell is weather that has left, so its heading is the whole \
             point of authoring one — and a zero vector does not normalise, so \
             this sea would run along the scene's `Wind` instead and read as a \
             swell that simply agrees with it. Travel direction on XZ, \
             clockwise from +X as `Wind.direction_degrees` is: [1, 0] runs \
             toward +X, [0, 1] toward +Z."
                .to_owned(),
        );
    }

    // The heading is taken modulo π — one complex amplitude per cell is one
    // direction of travel — so past 90° the sea lands on the authored axis
    // running the wind's way along it. Not refused, because the *axis* is
    // still what was asked for and a swell with no wind sea beside it (a dead
    // calm after a gale) is realised exactly; it is on `Swell`'s docs instead.

    for (field, value, min, max) in [
        ("u10", swell.u10, 0.1_f32, 40.0_f32),
        ("fetch", swell.fetch.unwrap_or(1.0), 1.0, 1_000_000.0),
    ] {
        if !(min..=max).contains(&value) {
            refuse(
                &format!("swell_{field}_out_of_range"),
                field,
                Value::from(value),
                &format!("between {min} and {max}"),
                format!(
                    "the schema says {min}–{max} and nothing enforces it on a \
                     nested table, so it is enforced here. `{field}` is \
                     {value}. A swell's `u10` is the old wind at 10 m and its \
                     `fetch` is how far that wind blew, in metres; leave \
                     `fetch` out entirely for unlimited fetch — the \
                     fully-developed sea — rather than writing a large number \
                     for it."
                ),
            );
        }
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
fn check_water(
    item: &Item,
    node: &str,
    has_gameplay: bool,
    cinematic_bodies: &mut usize,
) -> Vec<SceneError> {
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
    errors.extend(check_tier(&body, node, has_gameplay, cinematic_bodies));
    errors.extend(check_swell(&body, node));
    let count = body.waves.waves.len();

    // **Two seas authored in one component** — ADR 0076. The spectrum builds
    // the entire surface from a wavenumber grid, so a hand-written wave list
    // beside it is not summed in, not blended, and not warned about: it is
    // dropped. That is the same shape of failure the emitter refusals exist to
    // replace — a field the author typed, that the engine reads past, whose
    // symptom arrives later as "the waves I authored do nothing".
    //
    // The message says which half to remove because either is a coherent
    // scene and only the author knows which one was meant.
    if body.wave_model == components::WaveModel::Spectrum && count > 0 {
        let mut err = SceneError::new("spectrum_water_authors_waves", node);
        err.field = "WaterBody.wave_model".to_owned();
        err.value = Value::from("spectrum");
        err.constraint =
            "wave_model = \"spectrum\" with no waves.waves entries".to_owned();
        err.hint = Some(format!(
            "this body asks for the spectrum model and also authors {count} \
             Gerstner wave(s); the spectrum replaces the wave sum outright, so \
             those waves would silently draw and push nothing. Remove the \
             {count} `[[node.components.WaterBody.waves.waves]]` table(s) to \
             keep `wave_model = \"spectrum\"` — a spectrum sea is shaped by \
             the scene's `Wind` and by `fetch`, not by an amplitude list — or \
             remove `wave_model` to keep the waves."
        ));
        errors.push(err);
    }
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
    errors
}

// **`check_ripples` is gone — ADR 0056.** It refused a grid past the Courant
// limit and a grid too large to step, both of which were properties of an
// explicit finite-difference scheme that no longer exists. A wavelet event is a
// closed form: there is no stencil to destabilise, no cell count to bound, and
// nothing about it in the scene text to refuse.

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
