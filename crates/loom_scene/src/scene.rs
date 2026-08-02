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
    fn new(error: &str, node: &str) -> Self {
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
    /// Advisory is the operative word (`docs/format/README.md` §3): identity is
    /// the UUID. This is what an *importer* uses to find the source file, and
    /// nothing resolves a reference through it at runtime.
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

        // §5 of the format spec — prefab instances, `[node.overrides]`,
        // `extends` — is documented but not built. The parser did not know
        // these words, so it ignored them: a prefab instance node ended up
        // with no components at all, drew nothing, lit nothing, and validated
        // clean. Silence is the one answer that is definitely wrong; refusing
        // says so and costs nothing when prefabs do get built.
        for key in ["prefab", "extends", "overrides"] {
            if table.get(key).is_some() {
                let mut err = SceneError::new("not_implemented", name);
                err.field = key.to_owned();
                err.constraint = "a key this build understands".to_owned();
                err.hint = Some(format!(
                    "`{key}` is described in docs/format/README.md §5 (prefab \
                     instances and overrides), which is not implemented yet. \
                     Write the components on the node directly."
                ));
                errors.push(err);
            }
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

        known.insert(path.clone());
        nodes.push(Node {
            name: name.to_owned(),
            parent: parent.map(str::to_owned),
            path,
            transform,
            components: component_map,
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

/// Check every component on every node against its registered schema.
fn validate_components(doc: &DocumentMut, nodes: &[Node], registry: &TypeRegistry) -> Vec<SceneError> {
    let Some(entries) = doc.get("node").and_then(Item::as_array_of_tables) else {
        return Vec::new();
    };

    let mut errors = Vec::new();
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
            errors.extend(check(registry, type_name, item, &node.path));
        }
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
