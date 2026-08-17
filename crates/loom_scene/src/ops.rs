//! Scene mutation: transactions, version tokens, semantic placement.
//!
//! This lives in `loom_scene`, not `loom_agent`, and the dependency checker is
//! what made that obvious. `loom_agent` must be depended on by nothing, so the
//! agent layer stays removable and a bug is attributable to the engine or to
//! the agent but never ambiguously both (design doc §2.13). Mutation is not
//! the agent layer: never-do #16 says the **editor** issues these same ops
//! through this same code path, so a twelve-op agent transaction and a
//! twelve-op human edit undo identically.
//!
//! **CLI first, MCP second** (brief §7.10). Every mutation is a `SceneOp` that
//! `loom scene` can perform from a shell and `cargo test` can exercise. The
//! MCP server is a thin adapter over commands that already work — building the
//! agent interface *with* the agent is circular otherwise, and the MCP layer is
//! awkward to test without an agent driving it.
//!
//! Two rules this crate exists to enforce:
//!
//! - **A transaction is one undo step.** A twelve-op blockout undoes in one
//!   Ctrl+Z, because the editor will issue these same ops through this same
//!   code path (never-do #16).
//! - **A stale write is rejected, never merged** (§7.17). Silently destroying
//!   the human's edits is the worst bug class in this project.

use crate::Scene;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use toml_edit::{DocumentMut, Item, Table, value};

/// A content hash identifying the version of a scene file.
///
/// BLAKE3 of the file's bytes, matching `docs/format/README.md` §8. Every read
/// returns one; every write presents the one it read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionToken(pub String);

impl VersionToken {
    #[must_use]
    pub fn of(source: &str) -> Self {
        Self(blake3::hash(source.as_bytes()).to_hex().to_string())
    }
}

/// One scene mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SceneOp {
    /// Add a node under `parent`.
    SpawnNode {
        parent: String,
        name: String,
        /// Asset alias for a `MeshRenderer`, if it should draw something.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mesh: Option<String>,
        /// Prefab alias to instance, if this node stands for a prefab.
        ///
        /// **Mutually exclusive with `mesh`.** An instance owns no components
        /// — that is what makes overrides well-defined — so a node that is
        /// both would have two sources for the same data with no rule about
        /// which wins, which is precisely what the parser rejects.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefab: Option<String>,
    },
    /// Splice a node's array-valued field: remove `remove` entries at `index`,
    /// then insert `insert` in their place.
    ///
    /// **One op rather than three named ones** (append / remove / replace),
    /// because all three are this with different arguments, and because the
    /// callers that need it — the sculpt brush, `WaterBody.waves`,
    /// `Buoyancy.pontoons`, `Scatter.excludes`, the paint stroke lists —
    /// otherwise each pick a different one and the inspector has to know which.
    ///
    /// **The spelling on disk is preserved.** `[[node.components.X.ops]]` stays
    /// an array of tables and `ops = [...]` stays inline, because the two parse
    /// to different `toml_edit` items and this branches on which it found
    /// rather than on a policy. `crates/loom_scene/tests/toml_edit_contract.rs`
    /// pins that.
    ///
    /// A dotted path into the array (`ops.3.radius`) is deliberately **not**
    /// supported: [`SceneOp::SetField`] splits its field name once and uses the
    /// remainder as a literal TOML key, so it would write a key spelled
    /// `ops.3.radius` rather than reaching the third op.
    ///
    /// **`remove > insert.len()` is a net deletion**, which the destructive
    /// classifier treats as such — recorded here so that rule is discoverable
    /// from the op that triggers it rather than only from the classifier.
    SpliceArray {
        node: String,
        /// `ComponentType.field`, e.g. `VoxelVolume.ops`.
        field: String,
        /// Where to splice. Clamped to the array's length, so an append is
        /// `index: usize::MAX` or simply the current length.
        index: usize,
        /// How many existing entries to drop. Clamped to what is there.
        #[serde(default)]
        remove: usize,
        /// What to put in their place. May be empty, which makes this a
        /// deletion.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        insert: Vec<Value>,
    },
    /// Write an `[[asset]]` or `[[prefab]]` declaration.
    ///
    /// Authoring these was CLI-only, which meant the editor could reference an
    /// asset but never introduce one — so importing a mesh or creating a prefab
    /// could not be an editor action at all.
    Declare {
        /// `asset` or `prefab`.
        kind: String,
        /// The alias other nodes will use. File-local.
        key: String,
        /// The stable identity, for a prefab. A library is keyed by `id` and
        /// never by the alias, because two files may use one word for
        /// different prefabs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Path to the file, relative to the scene.
        path: String,
    },
    /// Replace a node's transform fields. Omitted fields are left alone.
    SetTransform {
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pos: Option<[f32; 3]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rot_euler: Option<[f32; 3]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<[f32; 3]>,
    },
    /// Set one component field, e.g. `Light.intensity`.
    SetField {
        node: String,
        field: String,
        value: Value,
    },
    /// Delete a node. Requires the `destructive` scope (§7.17).
    RemoveNode { node: String },
    /// Give a node a new name, keeping it where it is.
    RenameNode { node: String, name: String },
    /// Move a node — and everything under it — to a new parent.
    ReparentNode { node: String, parent: String },
    /// Take a component off a node. Adding one is [`SceneOp::SetField`],
    /// which creates the component table it writes into.
    RemoveComponent { node: String, component: String },
    /// Drop a prefab instance's deviations, putting it back to the prefab.
    ///
    /// `keys` empty means all of them. Named keys revert one deviation and
    /// leave the rest, which is what "revert this field" in an inspector does.
    RevertOverrides {
        node: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        keys: Vec<String>,
    },
    /// Replace a prefab instance with the concrete nodes it stood for.
    ///
    /// The instance stops tracking the prefab: later edits to the prefab no
    /// longer reach it. That is the point — it is the escape hatch for "this
    /// one needs to be different in a way overrides cannot express" — and it
    /// is why this is a deliberate operation rather than something that
    /// happens when you edit an instance.
    UnpackPrefab { node: String },
}

/// A batch of ops applied together, undone together.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Shown in the human's log panel and in git history. "Block out office:
    /// 14 nodes" beats "update scene".
    pub label: String,
    pub ops: Vec<SceneOp>,
    /// Validate and return the diff without writing.
    #[serde(default)]
    pub dry_run: bool,
    /// The version the caller read. `None` skips the check, which is only
    /// correct for a scene being created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_version: Option<VersionToken>,
}

/// What applying a transaction produced.
#[derive(Debug, Clone, Serialize)]
pub struct Applied {
    pub label: String,
    /// The scene text after applying.
    pub scene: String,
    /// The version after applying.
    pub version: VersionToken,
    /// Unified-ish diff, for review and for `--dry-run`.
    pub diff: Vec<String>,
    /// How to put the scene back. One entry, because a transaction is one
    /// undo step.
    pub undo: String,
}

/// Why a transaction did not apply.
#[derive(Debug, Clone, Serialize)]
pub struct TransactionError {
    pub error: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    pub constraint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// On a stale write, the current content so the caller can re-apply
    /// against it. **Never merged for them** — that would produce something
    /// neither party intended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// **Which op in the transaction failed**, zero-based, when one did.
    ///
    /// A transaction is one undo step and can be a dozen ops long; without this
    /// a caller is told *what* went wrong and left to guess *where*. The editor
    /// needs it to point at a row, and the agent needs it to retry the tail of
    /// a transaction rather than the whole thing.
    ///
    /// `None` for failures that belong to no single op — a stale version token,
    /// a parse error, a result that no longer loads.
    ///
    /// Additive to a *result* payload rather than to the scene format, so there
    /// is no `format` bump and nothing on disk changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op_index: Option<usize>,
}

impl std::fmt::Display for TransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error, self.constraint)
    }
}

impl std::error::Error for TransactionError {}

/// Apply a transaction to a scene's text.
///
/// The document is edited as a format-preserving DOM, so comments, spacing,
/// and key order survive — an agent write cannot delete a human's annotations
/// (never-do #15).
///
/// # Errors
/// [`TransactionError`] if the version is stale, the scene is invalid, an op
/// targets a missing node, or the result would not parse.
pub fn apply(source: &str, transaction: &Transaction) -> Result<Applied, Box<TransactionError>> {
    apply_with(source, transaction, &crate::prefab::Library::new())
}

/// As [`apply`], with prefabs available.
///
/// Only [`SceneOp::UnpackPrefab`] needs them — it writes out what an instance
/// stood for, and cannot know that without the prefab. Every other op is
/// unaffected, which is why `apply` stays the plain entry point rather than
/// forcing a library on callers that have none.
///
/// # Errors
/// As [`apply`], plus a missing prefab when unpacking.
pub fn apply_with(
    source: &str,
    transaction: &Transaction,
    library: &crate::prefab::Library,
) -> Result<Applied, Box<TransactionError>> {
    // Boxed: on a stale write the error carries the entire current scene, so
    // the unboxed variant would make every Ok return pay for the rare failure.
    let fail = |error: &str, constraint: String, node: Option<String>, hint: Option<String>| {
        Box::new(TransactionError {
            error: error.to_owned(),
            label: transaction.label.clone(),
            node,
            constraint,
            hint,
            current: None,
            op_index: None,
        })
    };
    // As `fail`, for the failures that belong to one op.
    let fail_at =
        |index: usize, error: &str, constraint: String, node: Option<String>, hint: Option<String>| {
            let mut e = TransactionError {
                error: error.to_owned(),
                label: transaction.label.clone(),
                node,
                constraint,
                hint,
                current: None,
                op_index: None,
            };
            e.op_index = Some(index);
            Box::new(e)
        };

    // Version first: re-applying against a scene that moved under you is the
    // whole point of the check, and doing any work before it is wasted.
    let current = VersionToken::of(source);
    if let Some(expected) = &transaction.expect_version
        && *expected != current
    {
        return Err(Box::new(TransactionError {
            error: "stale_version".to_owned(),
            label: transaction.label.clone(),
            node: None,
            constraint: format!("expected {}, found {}", expected.0, current.0),
            hint: Some(
                "The scene changed since you read it. Re-read and re-apply your \
                 ops against the current content — never force the write, and \
                 never merge the two versions."
                    .to_owned(),
            ),
            current: Some(source.to_owned()),
            op_index: None,
        }));
    }

    let mut doc: DocumentMut = source
        .parse()
        .map_err(|e: toml_edit::TomlError| fail("parse_error", e.to_string(), None, None))?;

    for (index, op) in transaction.ops.iter().enumerate() {
        // Unpack is the one op that needs to know what a prefab contains, so
        // it is applied here where the library is in scope rather than in
        // `apply_one`, which deliberately sees only the document.
        if let SceneOp::UnpackPrefab { node } = op {
            unpack(&mut doc, node, library)
                .map_err(|e| fail_at(index, &e.0, e.1, Some(e.2), e.3))?;
            continue;
        }
        apply_one(&mut doc, op).map_err(|e| fail_at(index, &e.0, e.1, Some(e.2), e.3))?;
    }

    let after = doc.to_string();

    // The result must still be a valid scene. Applying ops that produce an
    // unloadable file and writing it anyway would hand the human a broken
    // scene and call it success.
    Scene::parse(&after).map_err(|errors| {
        let first = errors.first();
        fail(
            "would_produce_invalid_scene",
            first.map_or_else(|| "unknown".to_owned(), |e| e.constraint.clone()),
            first.map(|e| e.node.clone()),
            Some("The transaction was rejected whole; the scene is unchanged.".to_owned()),
        )
    })?;

    Ok(Applied {
        label: transaction.label.clone(),
        version: VersionToken::of(&after),
        diff: diff_lines(source, &after),
        // The undo payload is the whole previous text. Simple, exact, and a
        // transaction is one undo step by construction.
        //
        // `ponytail:` whole-file snapshots. A scene is kilobytes; storing a
        // structural inverse per op would be more code and more ways to be
        // subtly wrong. Revisit if scenes reach megabytes.
        undo: source.to_owned(),
        scene: after,
    })
}

/// Replace a prefab instance with the concrete nodes it stood for.
///
/// Surgery on one node: everything else in the file — its other instances, its
/// comments, its formatting — is untouched, because the document is edited
/// rather than re-emitted.
fn unpack(
    doc: &mut DocumentMut,
    node: &str,
    library: &crate::prefab::Library,
) -> Result<(), OpFailure> {
    // **Distinguish "wrong entry point" from "prefab missing".** Both end up
    // unable to find the prefab, and only one is fixed by loading a file — so
    // an empty library is reported as what it is.
    if library.is_empty() {
        return Err((
            "prefab_library_required".to_owned(),
            format!("unpacking `{node}` needs the prefab it instances"),
            node.to_owned(),
            Some(
                "`unpack` is applied through `apply_with`, which takes the \
                 loaded prefabs. The CLI does this for you."
                    .to_owned(),
            ),
        ));
    }

    // Parsing the document back gives node paths and the prefab declarations,
    // which the raw DOM does not carry. It is the same text `apply` will
    // re-validate at the end, so this cannot see a scene that is not real.
    let scene = Scene::parse(&doc.to_string()).map_err(|errors| {
        (
            "would_produce_invalid_scene".to_owned(),
            errors.first().map_or_else(String::new, |e| e.constraint.clone()),
            node.to_owned(),
            None,
        )
    })?;
    let unpacked = crate::prefab::expand_instance(&scene, node, library).map_err(|errors| {
        let first = errors.first();
        (
            first.map_or_else(|| "unpack_failed".to_owned(), |e| e.error.clone()),
            first.map_or_else(String::new, |e| e.constraint.clone()),
            node.to_owned(),
            first.and_then(|e| e.hint.clone()),
        )
    })?;

    let index = require_node(doc, node)?;

    // The declarations first: a node referencing an alias nothing declares
    // would fail the re-validation at the end of `apply` with a confusing
    // message about the alias rather than about the unpack.
    if !unpacked.assets.is_empty() {
        if doc.get("asset").and_then(Item::as_array_of_tables).is_none() {
            doc["asset"] = Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
        }
        let assets = doc["asset"].as_array_of_tables_mut().expect("just ensured");
        for decl in &unpacked.assets {
            let mut table = Table::new();
            table["key"] = value(decl.key.as_str());
            table["id"] = value(decl.id.as_str());
            table["path"] = value(decl.path.as_str());
            assets.push(table);
        }
    }

    let array = nodes_mut(doc, node)?;
    let mut nodes = unpacked.nodes.into_iter();
    let Some(root) = nodes.next() else {
        return Err((
            "unpack_failed".to_owned(),
            format!("`{node}`'s prefab has no nodes"),
            node.to_owned(),
            None,
        ));
    };

    // The instance becomes its own contents: same name, same parent, same
    // placement, with the prefab reference and its deltas gone.
    let existing = array.get_mut(index).expect("index came from this array");
    existing.remove("prefab");
    existing.remove("overrides");
    write_components(existing, &root)?;

    // Children go straight after, keeping parents-before-children and leaving
    // the rest of the file where the author put it.
    for (offset, child) in nodes.enumerate() {
        let mut table = Table::new();
        table["name"] = value(child.name.as_str());
        if let Some(parent) = &child.parent {
            table["parent"] = value(parent.as_str());
        }
        write_components(&mut table, &child)?;
        array.insert(index + 1 + offset, table);
    }
    Ok(())
}

/// Write a node's transform and components into a table.
fn write_components(table: &mut Table, node: &crate::Node) -> Result<(), OpFailure> {
    if let Some(transform) = crate::prefab::transform_toml(&node.transform) {
        table["transform"] = Item::Value(transform.into());
    }

    if node.components.is_empty() {
        return Ok(());
    }
    let mut components = Table::new();
    components.set_implicit(true);
    for (type_name, data) in &node.components {
        let toml_edit::Value::InlineTable(fields) = json_to_toml(data, type_name)? else {
            continue;
        };
        let mut component = Table::new();
        for (key, field) in fields.iter() {
            component[key] = Item::Value(field.clone());
        }
        components[type_name.as_str()] = Item::Table(component);
    }
    table["components"] = Item::Table(components);
    Ok(())
}

/// Whether the node at `index` instances a prefab.
fn is_prefab_instance(doc: &DocumentMut, index: usize) -> bool {
    doc.get("node")
        .and_then(Item::as_array_of_tables)
        .and_then(|array| array.get(index))
        .is_some_and(|table| table.get("prefab").is_some())
}

/// Write one deviation into a prefab instance's `[node.overrides]`.
///
/// The key is used verbatim, so the `Child/Path::Type.field` spelling reaches
/// into the instanced sub-tree the same way it does in a hand-written file.
/// `Scene::parse` validates the grammar on the way back in, and `apply`
/// re-parses before returning — so a malformed key is rejected with the
/// scene unchanged rather than written and discovered later.
fn set_override(
    doc: &mut DocumentMut,
    index: usize,
    node: &str,
    key: &str,
    new: &Value,
) -> Result<(), OpFailure> {
    let value = json_to_toml(new, key)?;
    let array = nodes_mut(doc, node)?;
    let table = array.get_mut(index).expect("index came from this array");

    if table.get("overrides").is_none() {
        table["overrides"] = Item::Table(Table::new());
    }
    let overrides = table["overrides"].as_table_like_mut().ok_or_else(|| {
        (
            "malformed_node".to_owned(),
            format!("`{node}` has an `overrides` that is not a table"),
            node.to_owned(),
            Some("Overrides must be a table of dotted keys.".to_owned()),
        )
    })?;

    // Assign through the existing entry when there is one, so a human's
    // comment on the line survives — the same reason `SetField` does.
    if let Some(slot) = overrides.get_mut(key) {
        *slot = Item::Value(value);
    } else {
        overrides.insert(key, Item::Value(value));
    }
    Ok(())
}

/// Apply a splice to a plain JSON array, with both bounds clamped.
///
/// Clamped rather than validated: an editor issuing a splice against an array
/// that another write shortened should land at the end, not reject. The op is
/// still rejected when the *field* is missing, which is the error that means
/// something is actually wrong.
fn splice_values(mut current: Vec<Value>, index: usize, remove: usize, insert: &[Value]) -> Vec<Value> {
    let at = index.min(current.len());
    let drop = remove.min(current.len() - at);
    current.splice(at..at + drop, insert.iter().cloned());
    current
}

/// What a prefab instance's array field currently resolves to.
///
/// Reads the instance's own override when it has one. **When it does not, this
/// returns empty rather than reaching into the prefab**, because `apply_one`
/// deliberately sees only the document it is editing — the prefab library is
/// not in scope here, the same way it is not for any other op except
/// `UnpackPrefab`. The consequence is honest and worth stating: splicing a
/// field an instance has never overridden starts from nothing rather than from
/// the prefab's list. The editor always sends the resolved array it displayed,
/// so the case it hits is the one that works; a hand-written transaction
/// against an un-overridden field should set the whole field first.
fn resolved_array(doc: &DocumentMut, index: usize, key: &str) -> Result<Vec<Value>, OpFailure> {
    let existing = doc["node"]
        .as_array_of_tables()
        .and_then(|a| a.get(index))
        .and_then(|t| t.get("overrides"))
        .and_then(Item::as_table_like)
        .and_then(|o| o.get(key))
        .and_then(Item::as_value)
        .and_then(toml_edit::Value::as_array);
    let Some(array) = existing else {
        return Ok(Vec::new());
    };
    array
        .iter()
        .map(|v| {
            toml_to_json(v).ok_or_else(|| {
                (
                    "unsupported_value".to_owned(),
                    format!("`{key}` holds a value this op cannot read back"),
                    String::new(),
                    None,
                )
            })
        })
        .collect()
}

/// A `toml_edit` value as JSON, for the splice round-trip.
fn toml_to_json(v: &toml_edit::Value) -> Option<Value> {
    Some(match v {
        toml_edit::Value::String(s) => Value::String(s.value().clone()),
        toml_edit::Value::Integer(i) => Value::from(*i.value()),
        toml_edit::Value::Float(f) => Value::from(*f.value()),
        toml_edit::Value::Boolean(b) => Value::Bool(*b.value()),
        toml_edit::Value::Array(a) => {
            Value::Array(a.iter().map(toml_to_json).collect::<Option<Vec<_>>>()?)
        }
        toml_edit::Value::InlineTable(t) => Value::Object(
            t.iter()
                .map(|(k, v)| toml_to_json(v).map(|v| (k.to_owned(), v)))
                .collect::<Option<serde_json::Map<_, _>>>()?,
        ),
        toml_edit::Value::Datetime(_) => return None,
    })
}

/// One spliced entry as a `[[header]]` table.
fn json_to_table(entry: &Value, field: &str) -> Result<Table, OpFailure> {
    let Value::Object(map) = entry else {
        return Err((
            "not_a_table".to_owned(),
            format!("`{field}` is an array of tables, so every entry must be an object"),
            String::new(),
            Some("For example `{ \"kind\": \"box\", \"mode\": \"union\" }`.".to_owned()),
        ));
    };
    let mut table = Table::new();
    for (key, v) in map {
        table[key.as_str()] = Item::Value(json_to_toml(v, field)?);
    }
    Ok(table)
}

/// A node name must be usable as one path segment.
fn check_name(name: &str) -> Result<(), OpFailure> {
    if name.is_empty() || name.contains('/') {
        return Err((
            "invalid_name".to_owned(),
            "a node name must be non-empty and contain no `/`".to_owned(),
            name.to_owned(),
            None,
        ));
    }
    Ok(())
}

fn nodes_mut<'a>(
    doc: &'a mut DocumentMut,
    context: &str,
) -> Result<&'a mut toml_edit::ArrayOfTables, OpFailure> {
    doc.get_mut("node")
        .and_then(Item::as_array_of_tables_mut)
        .ok_or_else(|| {
            (
                "parse_error".to_owned(),
                "`node` is not an array of tables".to_owned(),
                context.to_owned(),
                None,
            )
        })
}

fn set_name(doc: &mut DocumentMut, index: usize, name: &str) -> Result<(), OpFailure> {
    let array = nodes_mut(doc, name)?;
    if let Some(table) = array.get_mut(index) {
        table["name"] = value(name);
    }
    Ok(())
}

/// Repoint every node beneath `from` at `to`.
///
/// Paths are how this format expresses parentage, so moving or renaming a node
/// is not one edit — it is one edit plus every descendant's. Prefix-matched on
/// `from/` specifically, so a sibling called `RoomService` is not caught by a
/// rename of `Room`.
fn rewrite_descendants(doc: &mut DocumentMut, from: &str, to: &str) {
    if from == to {
        return;
    }
    let prefix = format!("{from}/");
    let Some(array) = doc.get_mut("node").and_then(Item::as_array_of_tables_mut) else {
        return;
    };
    for table in array.iter_mut() {
        let Some(parent) = table.get("parent").and_then(Item::as_str) else {
            continue;
        };
        let moved = if parent == from {
            to.to_owned()
        } else if let Some(rest) = parent.strip_prefix(&prefix) {
            format!("{to}/{rest}")
        } else {
            continue;
        };
        table["parent"] = value(moved);
    }
}

/// Indices of a node and everything under it, in declaration order.
fn subtree_indices(doc: &DocumentMut, root: &str) -> Vec<usize> {
    let prefix = format!("{root}/");
    let Some(array) = doc.get("node").and_then(Item::as_array_of_tables) else {
        return Vec::new();
    };
    array
        .iter()
        .enumerate()
        .filter(|(_, table)| {
            let name = table.get("name").and_then(Item::as_str).unwrap_or_default();
            let path = match table.get("parent").and_then(Item::as_str) {
                Some(parent) => format!("{parent}/{name}"),
                None => name.to_owned(),
            };
            path == root || path.starts_with(&prefix)
        })
        .map(|(index, _)| index)
        .collect()
}

/// One past the last table belonging to `root`'s subtree.
fn subtree_end(doc: &DocumentMut, root: &str) -> usize {
    subtree_indices(doc, root)
        .last()
        .map_or(0, |last| last + 1)
}

/// Move these tables so they sit directly after `after_subtree_of`, keeping
/// their relative order.
///
/// Two rules force this. The format requires a parent to be declared before
/// its children (§3), and canonical form requires node order to be depth-first
/// (§2 rule 3) — so a reparented subtree does not just need to be *later* than
/// its new parent, it needs to be immediately after that parent's own subtree.
/// Appending to the end would satisfy the loader and quietly leave the file
/// non-canonical, which shows up later as a diff nobody asked for.
fn move_after(
    doc: &mut DocumentMut,
    indices: &[usize],
    after_subtree_of: &str,
    context: &str,
) -> Result<(), OpFailure> {
    let array = nodes_mut(doc, context)?;
    // `toml_edit` writes tables in the order it recorded when parsing, not in
    // array order, so reordering the array alone changes nothing on disk. The
    // slots are collected here and handed back out in the new order below.
    let mut slots: Vec<isize> = array.iter().filter_map(toml_edit::Table::position).collect();
    slots.sort_unstable();
    // A table pushed earlier in this same transaction has no recorded position
    // yet. Bailing out when any was missing meant a spawn-then-reparent pair
    // never reordered at all — the child stayed declared before its new parent
    // and the whole transaction was rejected as an invalid scene.
    //
    // Node tables are the last thing in the document, so extending past the
    // highest existing slot cannot collide with `[scene]` or `[[asset]]`.
    let mut next = slots.last().copied().unwrap_or(0);
    while slots.len() < array.len() {
        next += 1;
        slots.push(next);
    }

    let mut moved = Vec::with_capacity(indices.len());
    // Descending, so each removal cannot shift an index still to be removed.
    for index in indices.iter().rev() {
        if *index < array.len() {
            moved.push(array.remove(*index));
        }
    }
    // Recomputed after the removals, because every index above a removed one
    // has shifted down.
    let destination = subtree_end(doc, after_subtree_of);
    let array = nodes_mut(doc, context)?;
    let at = destination.min(array.len());
    for (offset, table) in moved.into_iter().rev().enumerate() {
        array.insert(at + offset, table);
    }

    if slots.len() == array.len() {
        for (table, position) in array.iter_mut().zip(slots) {
            table.set_position(Some(position));
        }
    }
    Ok(())
}

type OpFailure = (String, String, String, Option<String>);

fn apply_one(doc: &mut DocumentMut, op: &SceneOp) -> Result<(), OpFailure> {
    match op {
        SceneOp::SpawnNode { parent, name, mesh, prefab } => {
            if mesh.is_some() && prefab.is_some() {
                return Err((
                    "mesh_and_prefab".to_owned(),
                    "a node instances a prefab or draws a mesh, never both".to_owned(),
                    name.clone(),
                    Some(
                        "A prefab instance owns no components of its own; give the \
                         instance an override instead, or unpack it."
                            .to_owned(),
                    ),
                ));
            }
            if name.is_empty() || name.contains('/') {
                return Err((
                    "invalid_name".to_owned(),
                    "a node name must be non-empty and contain no `/`".to_owned(),
                    name.clone(),
                    None,
                ));
            }
            if find_node(doc, &format!("{parent}/{name}")).is_some() {
                return Err((
                    "duplicate_sibling_name".to_owned(),
                    format!("`{parent}` already has a child named `{name}`"),
                    format!("{parent}/{name}"),
                    Some("Sibling names must be unique.".to_owned()),
                ));
            }
            if find_node(doc, parent).is_none() {
                return Err((
                    "unknown_parent".to_owned(),
                    format!("no node at `{parent}`"),
                    parent.clone(),
                    Some("Parents must exist and be declared before children.".to_owned()),
                ));
            }

            let mut table = Table::new();
            table["name"] = value(name.as_str());
            table["parent"] = value(parent.as_str());
            if let Some(prefab) = prefab {
                table["prefab"] = value(prefab.as_str());
            }
            if let Some(mesh) = mesh {
                let mut renderer = Table::new();
                let mut asset = toml_edit::InlineTable::new();
                asset.insert("asset", mesh.as_str().into());
                renderer["mesh"] = value(asset);
                let mut components = Table::new();
                components.set_implicit(true);
                components["MeshRenderer"] = Item::Table(renderer);
                table["components"] = Item::Table(components);
            }
            let array = doc
                .entry("node")
                .or_insert(Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
                .as_array_of_tables_mut()
                .ok_or_else(|| {
                    (
                        "parse_error".to_owned(),
                        "`node` is not an array of tables".to_owned(),
                        name.clone(),
                        None,
                    )
                })?;
            array.push(table);
            Ok(())
        }

        SceneOp::SetTransform {
            node,
            pos,
            rot_euler,
            scale,
        } => {
            let index = require_node(doc, node)?;
            let array = doc["node"].as_array_of_tables_mut().unwrap();
            let table = array.get_mut(index).unwrap();

            // Read whichever spelling is on disk. `transform` may be an
            // inline table or a `[node.transform]` sub-table, and the parser
            // accepts both — but only the inline form was read here, so for a
            // sub-table this started from `Transform::default()` and wiped
            // every axis the op was not setting. Setting a rotation deleted
            // the position.
            let existing: Vec<(String, Item)> = table
                .get("transform")
                .and_then(Item::as_table_like)
                .map(|t| {
                    t.iter()
                        .map(|(k, v)| (k.to_owned(), v.clone()))
                        .collect()
                })
                .unwrap_or_default();

            let mut inline = toml_edit::InlineTable::new();
            for (key, item) in existing {
                if let Some(v) = item.as_value() {
                    inline.insert(&key, v.clone());
                }
            }
            for (key, values) in [("pos", pos), ("rot_euler", rot_euler), ("scale", scale)] {
                if let Some(v) = values {
                    let mut array = toml_edit::Array::new();
                    for component in v {
                        // **The shortest decimal that identifies the `f32`**,
                        // not the exact `f64` widening of it. `f64::from(0.1f32)`
                        // is 0.10000000149011612, and writing that into the file
                        // made every gizmo drag and every scripted transform
                        // spray seventeen digits across a diff nobody can read.
                        // `prefab.rs:186` already does this; the two are now the
                        // same rule.
                        //
                        // Round-trips exactly: the decimal is chosen to identify
                        // the `f32` uniquely, so parsing it back gives the same
                        // bits. Grid snapping's defaults depend on that — a snap
                        // to 0.25 must read back as 0.25 and not as something
                        // 1e-8 away from it.
                        let shortest = component
                            .to_string()
                            .parse::<f64>()
                            .unwrap_or_else(|_| f64::from(*component));
                        array.push(shortest);
                    }
                    inline.insert(key, array.into());
                }
            }
            table["transform"] = value(inline);
            Ok(())
        }

        SceneOp::SetField { node, field, value: new } => {
            let (type_name, field_name) = field.split_once('.').ok_or_else(|| {
                (
                    "invalid_field".to_owned(),
                    format!("`{field}` must be `ComponentType.field`"),
                    node.clone(),
                    Some("For example `Light.intensity`.".to_owned()),
                )
            })?;
            let index = require_node(doc, node)?;

            // **On a prefab instance this becomes an override.** An instance
            // owns no components — writing one would give the node two sources
            // for the same data with no rule about which wins, and the parser
            // rejects exactly that. Routing here means the editor's inspector
            // and `loom scene --tx` need no idea whether a node is an
            // instance: setting a field does the right thing either way.
            if is_prefab_instance(doc, index) {
                return set_override(doc, index, node, field, new);
            }
            let array = doc["node"].as_array_of_tables_mut().unwrap();
            let table = array.get_mut(index).unwrap();

            if table.get("components").is_none() {
                let mut components = Table::new();
                components.set_implicit(true);
                table["components"] = Item::Table(components);
            }
            // `as_table_like_mut` covers both the sub-table and the inline
            // spelling. `as_table_mut().unwrap()` covered only one of them, so
            // editing a field on a node the parser accepts killed the process
            // and took any unsaved editor work with it.
            let malformed = |what: &str| {
                (
                    "malformed_node".to_owned(),
                    format!("`{node}` has a `{what}` that is not a table"),
                    node.clone(),
                    Some("Components must be tables.".to_owned()),
                )
            };
            let components = table["components"]
                .as_table_like_mut()
                .ok_or_else(|| malformed("components"))?;
            if components.get(type_name).is_none() {
                components.insert(type_name, Item::Table(Table::new()));
            }
            let component = components
                .get_mut(type_name)
                .and_then(Item::as_table_like_mut)
                .ok_or_else(|| malformed(type_name))?;
            // Assign through the existing entry when there is one. `insert`
            // replaces the *key* as well as the value, and a key carries its
            // decor — so editing a field the human had commented deleted the
            // comment and the indentation with it. Preserving human annotation
            // is the entire reason this layer edits a format-preserving DOM
            // instead of re-emitting the file.
            let value = json_to_toml(new, field)?;
            // Whether the thing being replaced was written in header form
            // (`[node.components.X.field]` or `[[...]]`). Such a key carries no
            // trailing space, because a header does not need one — so reusing
            // its slot for a key-value pair emitted `ops= [...]`.
            let replacing_header = component
                .get(field_name)
                .is_some_and(|item| !item.is_value());
            match component.get_mut(field_name) {
                Some(existing) => {
                    // A comment on the *same line* as a value lives in that
                    // value's own suffix decor, so replacing the Item drops it
                    // — which is what `docs/format/README.md` promises will
                    // not happen. Carried across explicitly. (A comment on the
                    // line above lives on the key, which `get_mut` already
                    // preserves by not touching it.)
                    let carried = existing.as_value().map(|v| v.decor().clone());
                    *existing = toml_edit::Item::Value(value);
                    if let (Some(decor), Some(v)) = (carried, existing.as_value_mut()) {
                        *v.decor_mut() = decor;
                    }
                }
                None => {
                    component.insert(field_name, toml_edit::Item::Value(value));
                }
            }
            if replacing_header
                && let Some(mut key) = component.key_mut(field_name)
            {
                // Canonical form on the first write rather than the second.
                key.leaf_decor_mut().set_suffix(" ");
            }
            Ok(())
        }

        SceneOp::SpliceArray { node, field, index, remove, insert } => {
            let (type_name, field_name) = field.split_once('.').ok_or_else(|| {
                (
                    "invalid_field".to_owned(),
                    format!("`{field}` must be `ComponentType.field`"),
                    node.clone(),
                    Some("For example `VoxelVolume.ops`.".to_owned()),
                )
            })?;
            let node_index = require_node(doc, node)?;

            // **On a prefab instance, materialise the resolved array as an
            // override and splice that.** Splicing "the array" on an instance
            // has no other well-defined reading: the instance owns no
            // components, so index 3 would mean the prefab's index 3, and the
            // result would silently change when the prefab did. Materialising
            // makes the edit mean what the human saw when they made it.
            if is_prefab_instance(doc, node_index) {
                let current = resolved_array(doc, node_index, field)?;
                let spliced = splice_values(current, *index, *remove, insert);
                return set_override(doc, node_index, node, field, &Value::Array(spliced));
            }

            let array = doc["node"].as_array_of_tables_mut().unwrap();
            let table = array.get_mut(node_index).unwrap();
            let components = table
                .get_mut("components")
                .and_then(Item::as_table_like_mut)
                .ok_or_else(|| {
                    (
                        "unknown_component".to_owned(),
                        format!("`{node}` has no `{type_name}`"),
                        node.clone(),
                        Some("Splicing needs the array to exist; set the field first.".to_owned()),
                    )
                })?;
            let component = components
                .get_mut(type_name)
                .and_then(Item::as_table_like_mut)
                .ok_or_else(|| {
                    (
                        "unknown_component".to_owned(),
                        format!("`{node}` has no `{type_name}`"),
                        node.clone(),
                        Some("Splicing needs the array to exist; set the field first.".to_owned()),
                    )
                })?;

            match component.get_mut(field_name) {
                // The header spelling. Kept as one, entry by entry, so the
                // human's comments and indentation on every op they did not
                // touch survive.
                Some(Item::ArrayOfTables(existing)) => {
                    let at = (*index).min(existing.len());
                    let drop = (*remove).min(existing.len().saturating_sub(at));
                    for _ in 0..drop {
                        existing.remove(at);
                    }
                    for (offset, entry) in insert.iter().enumerate() {
                        let table = json_to_table(entry, field)?;
                        existing.insert(at + offset, table);
                    }
                    Ok(())
                }
                // The inline spelling, and the fallback for a value that is
                // an ordinary array of scalars.
                Some(item) => {
                    let existing = item.as_value().and_then(toml_edit::Value::as_array).ok_or_else(
                        || {
                            (
                                "not_an_array".to_owned(),
                                format!("`{field}` on `{node}` is not an array"),
                                node.clone(),
                                Some("Only array-valued fields can be spliced.".to_owned()),
                            )
                        },
                    )?;
                    let mut next = existing.clone();
                    let at = (*index).min(next.len());
                    let drop = (*remove).min(next.len().saturating_sub(at));
                    for _ in 0..drop {
                        next.remove(at);
                    }
                    for (offset, entry) in insert.iter().enumerate() {
                        next.insert(at + offset, json_to_toml(entry, field)?);
                    }
                    *item = Item::Value(toml_edit::Value::Array(next));
                    Ok(())
                }
                None => Err((
                    "unknown_field".to_owned(),
                    format!("`{node}` has no `{field}`"),
                    node.clone(),
                    Some("Splicing needs the array to exist; set the field first.".to_owned()),
                )),
            }
        }

        SceneOp::Declare { kind, key, id, path } => {
            if kind != "asset" && kind != "prefab" {
                return Err((
                    "invalid_declaration".to_owned(),
                    format!("`{kind}` is not a declaration kind"),
                    key.clone(),
                    Some("Only `asset` and `prefab` can be declared.".to_owned()),
                ));
            }
            check_name(key)?;
            // A duplicate alias is a file where one word means two things, and
            // the loader would silently take one of them.
            if let Some(existing) = doc.get(kind).and_then(Item::as_array_of_tables)
                && existing
                    .iter()
                    .any(|t| t.get("key").and_then(Item::as_str) == Some(key.as_str()))
            {
                return Err((
                    "duplicate_alias".to_owned(),
                    format!("`{key}` is already declared as a {kind} in this file"),
                    key.clone(),
                    Some("Aliases are file-local and must be unique within it.".to_owned()),
                ));
            }

            let mut table = Table::new();
            table["key"] = value(key.as_str());
            if let Some(id) = id {
                table["id"] = value(id.as_str());
            }
            table["path"] = value(path.as_str());
            doc.entry(kind)
                .or_insert(Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
                .as_array_of_tables_mut()
                .ok_or_else(|| {
                    (
                        "parse_error".to_owned(),
                        format!("`{kind}` is not an array of tables"),
                        key.clone(),
                        None,
                    )
                })?
                .push(table);
            Ok(())
        }

        SceneOp::RemoveComponent { node, component } => {
            let index = require_node(doc, node)?;
            let array = nodes_mut(doc, node)?;
            // `as_table_like_mut`, matching `SetField`. Using `as_table_mut`
            // here meant the inline `components = { ... }` spelling reported
            // `unknown_component` for a component the node demonstrably has —
            // the same blind spot, one function over.
            let removed = array
                .get_mut(index)
                .and_then(|table| table.get_mut("components"))
                .and_then(Item::as_table_like_mut)
                .and_then(|components| components.remove(component));
            if removed.is_none() {
                return Err((
                    "unknown_component".to_owned(),
                    format!("`{node}` has no `{component}`"),
                    node.clone(),
                    Some("Only components the node actually carries can be removed.".to_owned()),
                ));
            }
            Ok(())
        }

        SceneOp::RevertOverrides { node, keys } => {
            let index = require_node(doc, node)?;
            if !is_prefab_instance(doc, index) {
                return Err((
                    "not_a_prefab_instance".to_owned(),
                    format!("`{node}` does not instance a prefab"),
                    node.clone(),
                    Some(
                        "Only a node with `prefab = \"...\"` has overrides to \
                         revert."
                            .to_owned(),
                    ),
                ));
            }

            let array = nodes_mut(doc, node)?;
            let table = array.get_mut(index).expect("index came from this array");
            if keys.is_empty() {
                table.remove("overrides");
                return Ok(());
            }

            let Some(overrides) =
                table.get_mut("overrides").and_then(Item::as_table_like_mut)
            else {
                return Err((
                    "unknown_override".to_owned(),
                    format!("`{node}` has no overrides"),
                    node.clone(),
                    None,
                ));
            };
            for key in keys {
                if overrides.remove(key).is_none() {
                    return Err((
                        "unknown_override".to_owned(),
                        format!("`{node}` has no override `{key}`"),
                        node.clone(),
                        Some(
                            "Reverting is per-key; name one the instance \
                             actually carries, or pass none to revert all."
                                .to_owned(),
                        ),
                    ));
                }
            }
            // An empty table left behind is noise in the file and reads as "an
            // instance with overrides" to anyone skimming it.
            if overrides.is_empty() {
                table.remove("overrides");
            }
            Ok(())
        }

        SceneOp::UnpackPrefab { node } => {
            // Needs the prefab's contents, which `apply` cannot read — see
            // `apply_with`. Reaching here means the caller used the plain
            // `apply`, and saying so beats a confusing "prefab not found".
            Err((
                "prefab_library_required".to_owned(),
                format!("unpacking `{node}` needs the prefab it instances"),
                node.clone(),
                Some(
                    "`unpack` is applied through `apply_with`, which takes the \
                     loaded prefabs. The CLI does this for you."
                        .to_owned(),
                ),
            ))
        }

        SceneOp::RenameNode { node, name } => {
            let index = require_node(doc, node)?;
            check_name(name)?;
            let parent = node.rsplit_once('/').map(|(p, _)| p.to_owned());
            let destination = match &parent {
                Some(parent) => format!("{parent}/{name}"),
                None => name.clone(),
            };
            if destination != *node && find_node(doc, &destination).is_some() {
                return Err((
                    "duplicate_sibling_name".to_owned(),
                    format!("`{destination}` already exists"),
                    destination,
                    Some("Sibling names must be unique.".to_owned()),
                ));
            }

            set_name(doc, index, name)?;
            // A parent is stored as a path, so renaming a node rewrites every
            // descendant's `parent`. Skip this and the children are orphaned —
            // the scene stops loading, and the op looked like it worked.
            rewrite_descendants(doc, node, &destination);
            Ok(())
        }

        SceneOp::ReparentNode { node, parent } => {
            let index = require_node(doc, node)?;
            if find_node(doc, parent).is_none() {
                return Err((
                    "unknown_parent".to_owned(),
                    format!("no node at `{parent}`"),
                    parent.clone(),
                    Some("Parents must exist.".to_owned()),
                ));
            }
            // Under itself or under its own descendant. Either leaves the node
            // with no path at all, which writes a file that cannot be loaded
            // and cannot be repaired by re-reading it.
            if parent == node || parent.starts_with(&format!("{node}/")) {
                return Err((
                    "would_create_a_cycle".to_owned(),
                    format!("`{parent}` is `{node}` or lives inside it"),
                    node.clone(),
                    Some("Move the subtree out first, or pick a parent outside it.".to_owned()),
                ));
            }

            let name = node.rsplit('/').next().unwrap_or(node).to_owned();
            let destination = format!("{parent}/{name}");
            if destination != *node && find_node(doc, &destination).is_some() {
                return Err((
                    "duplicate_sibling_name".to_owned(),
                    format!("`{parent}` already has a child named `{name}`"),
                    destination,
                    Some("Rename it first, or pick another parent.".to_owned()),
                ));
            }

            // Collected before anything moves, using the paths as they are now.
            let subtree = subtree_indices(doc, node);

            let array = nodes_mut(doc, node)?;
            if let Some(table) = array.get_mut(index) {
                table["parent"] = value(parent.as_str());
            }
            rewrite_descendants(doc, node, &destination);
            // The format requires a parent to be declared before its children
            // (`docs/format/README.md` §3), so moving a node under a parent
            // that comes later in the file has to move the node's tables too.
            // Sending the subtree to the end satisfies the rule for any legal
            // destination and leaves every other node's position alone.
            move_after(doc, &subtree, parent, node)?;
            Ok(())
        }

        SceneOp::RemoveNode { node } => {
            let index = require_node(doc, node)?;
            // Removing a parent orphans its children, which would produce an
            // unloadable scene. Caught here so the message names the cause
            // rather than surfacing as `unknown_parent` after the fact.
            let prefix = format!("{node}/");
            let has_children = doc["node"]
                .as_array_of_tables()
                .map(|a| {
                    a.iter().any(|t| {
                        t.get("parent")
                            .and_then(Item::as_str)
                            .is_some_and(|p| p == node || p.starts_with(&prefix))
                    })
                })
                .unwrap_or(false);
            if has_children {
                return Err((
                    "node_has_children".to_owned(),
                    format!("`{node}` still has children"),
                    node.clone(),
                    Some("Remove the children first, or reparent them.".to_owned()),
                ));
            }
            doc["node"].as_array_of_tables_mut().unwrap().remove(index);
            Ok(())
        }
    }
}

fn require_node(doc: &DocumentMut, path: &str) -> Result<usize, OpFailure> {
    find_node(doc, path).ok_or_else(|| {
        (
            "unknown_node".to_owned(),
            format!("no node at `{path}`"),
            path.to_owned(),
            Some("Paths include the root, e.g. `Office/Desk`.".to_owned()),
        )
    })
}

/// Index of the `[[node]]` table whose resolved path is `path`.
fn find_node(doc: &DocumentMut, path: &str) -> Option<usize> {
    let array = doc.get("node")?.as_array_of_tables()?;
    array.iter().position(|table| {
        let name = table.get("name").and_then(Item::as_str).unwrap_or_default();
        match table.get("parent").and_then(Item::as_str) {
            Some(parent) => format!("{parent}/{name}") == path,
            None => name == path,
        }
    })
}

/// A JSON value as TOML, or a refusal.
///
/// **Total, and fallible.** This used to fall through to `value("")` for
/// anything that was not a scalar, which meant an object became the empty
/// string and an array of objects became `[]` — silently, with the transaction
/// reporting success and the result still parsing. That is how clicking an
/// asset in the editor destroyed a node's mesh reference, and how duplicating
/// a terrain node erased the op list that never-do #11 makes the only
/// representation of its shape.
///
/// The read side already knew about this failure mode: `item_to_json` in
/// `scene.rs` carries a comment about a voxel op list vanishing. Only the
/// write side was left flattening.
fn json_to_toml(value: &Value, field: &str) -> Result<toml_edit::Value, OpFailure> {
    let unrepresentable = |what: &str| {
        (
            "unrepresentable_value".to_owned(),
            format!("`{field}` cannot hold {what}: TOML has no such value"),
            field.to_owned(),
            Some("Use a number, string, boolean, array, or table.".to_owned()),
        )
    };

    Ok(match value {
        Value::Bool(b) => (*b).into(),
        // Integer-ness is load-bearing: TOML distinguishes 4 from 4.0, and the
        // voxel reader takes `chunks` through `as_u64`. Writing every number as
        // a float made those fields stop parsing.
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into()
            } else if let Some(f) = n.as_f64() {
                f.into()
            } else {
                return Err(unrepresentable("a number this large"));
            }
        }
        Value::String(s) => s.as_str().into(),
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(json_to_toml(item, field)?);
            }
            array.into()
        }
        Value::Object(map) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, item) in map {
                table.insert(key, json_to_toml(item, field)?);
            }
            table.into()
        }
        // Nothing in TOML is null. Refusing is the only honest answer; writing
        // an empty string is how a field quietly becomes the wrong type.
        Value::Null => return Err(unrepresentable("null")),
    })
}

/// Line-level diff, so `--dry-run` shows what would land.
///
/// `ponytail:` naive longest-common-prefix/suffix rather than a real diff
/// algorithm. Transactions touch a handful of lines in a file of tens, and the
/// output only has to be readable. Swap for a proper LCS if it ever misreads a
/// reordering.
fn diff_lines(before: &str, after: &str) -> Vec<String> {
    let before: Vec<&str> = before.lines().collect();
    let after: Vec<&str> = after.lines().collect();

    let head = before
        .iter()
        .zip(&after)
        .take_while(|(a, b)| a == b)
        .count();
    let tail = before[head..]
        .iter()
        .rev()
        .zip(after[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    let mut diff = Vec::new();
    for line in &before[head..before.len() - tail] {
        diff.push(format!("-{line}"));
    }
    for line in &after[head..after.len() - tail] {
        diff.push(format!("+{line}"));
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = "\
# A human wrote this comment and it must survive every agent write.
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Room\"

[[node]]
name = \"Desk\"
parent = \"Room\"
transform = { pos = [0.0, 0.0, 0.0] }
";

    fn tx(label: &str, ops: Vec<SceneOp>) -> Transaction {
        Transaction {
            label: label.to_owned(),
            ops,
            dry_run: false,
            expect_version: None,
        }
    }

    /// A scene whose voxel ops use the header spelling every `.loom` file on
    /// disk actually uses.
    const VOXELS: &str = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Ground\"

  [node.components.VoxelVolume]
  voxel_size = 0.25

  # The slab, and this comment must survive a splice.
  [[node.components.VoxelVolume.ops]]
  kind = \"box\"
  mode = \"union\"

  [[node.components.VoxelVolume.ops]]
  kind = \"sphere\"
  mode = \"subtract\"
";

    fn splice(index: usize, remove: usize, insert: Vec<Value>) -> SceneOp {
        SceneOp::SpliceArray {
            node: "Ground".into(),
            field: "VoxelVolume.ops".into(),
            index,
            remove,
            insert,
        }
    }

    /// **The spelling on disk survives.** A splice that re-emitted the array
    /// inline would rewrite every op in the recipe as one unreadable line and
    /// take the human's comments with it — which is what `SetField` on `ops`
    /// does, and the reason this op exists.
    #[test]
    fn splicing_keeps_the_array_of_tables_spelling_and_the_comments() {
        let applied = apply(
            VOXELS,
            &tx(
                "Carve a capsule",
                vec![splice(2, 0, vec![serde_json::json!({"kind": "capsule", "mode": "union"})])],
            ),
        )
        .expect("splice applies");

        assert_eq!(
            applied.scene.matches("[[node.components.VoxelVolume.ops]]").count(),
            3,
            "the appended op was written inline:\n{}",
            applied.scene
        );
        assert!(applied.scene.contains("# The slab, and this comment must survive a splice."));
        assert!(applied.scene.contains("capsule"));
    }

    /// A splice is not only an append — voxel ops apply in order, so where an
    /// op lands changes the surface.
    #[test]
    fn splicing_in_the_middle_keeps_order() {
        let applied = apply(
            VOXELS,
            &tx("Insert", vec![splice(1, 0, vec![serde_json::json!({"kind": "capsule"})])]),
        )
        .expect("splice applies");

        let order: Vec<&str> = applied
            .scene
            .lines()
            .filter_map(|l| l.trim().strip_prefix("kind = "))
            .collect();
        assert_eq!(order, ["\"box\"", "\"capsule\"", "\"sphere\""]);
    }

    /// `remove` with nothing to insert is a deletion, and the entries around
    /// it keep their decor.
    #[test]
    fn splicing_can_delete() {
        let applied = apply(VOXELS, &tx("Drop the sphere", vec![splice(1, 1, vec![])]))
            .expect("splice applies");

        assert_eq!(
            applied.scene.matches("[[node.components.VoxelVolume.ops]]").count(),
            1
        );
        assert!(!applied.scene.contains("sphere"));
        assert!(applied.scene.contains("# The slab"));
    }

    /// Both bounds clamp rather than reject: an index past the end appends.
    /// The op still fails when the *field* is absent, which is the case that
    /// actually means something is wrong.
    #[test]
    fn an_out_of_range_splice_appends_and_a_missing_field_does_not() {
        let applied = apply(
            VOXELS,
            &tx("Append", vec![splice(usize::MAX, 99, vec![serde_json::json!({"kind": "capsule"})])]),
        )
        .expect("clamped, not rejected");
        assert_eq!(
            applied.scene.matches("[[node.components.VoxelVolume.ops]]").count(),
            3
        );

        let err = apply(
            VOXELS,
            &tx(
                "Nonsense",
                vec![SceneOp::SpliceArray {
                    node: "Ground".into(),
                    field: "VoxelVolume.nope".into(),
                    index: 0,
                    remove: 0,
                    insert: vec![serde_json::json!({"kind": "box"})],
                }],
            ),
        )
        .expect_err("a field that does not exist is an error");
        assert_eq!(err.error, "unknown_field");
        assert_eq!(err.op_index, Some(0));
    }

    /// **R14: a splice against a prefab instance materialises an override.**
    ///
    /// Splicing "the array" on an instance has no other well-defined reading —
    /// the instance owns no components, so an index would refer to the
    /// prefab's list and the result would change silently when the prefab did.
    #[test]
    fn splicing_a_prefab_instance_writes_an_override() {
        const INSTANCED: &str = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[prefab]]
key = \"rock\"
id = \"11111111-2222-3333-4444-555555555555\"
path = \"rock.loom\"

[[node]]
name = \"Boulder\"
prefab = \"rock\"

  [node.overrides]
  \"VoxelVolume.ops\" = [ { kind = \"box\" } ]
";
        let applied = apply(
            INSTANCED,
            &tx(
                "Carve the boulder",
                vec![SceneOp::SpliceArray {
                    node: "Boulder".into(),
                    field: "VoxelVolume.ops".into(),
                    index: 1,
                    remove: 0,
                    insert: vec![serde_json::json!({"kind": "sphere"})],
                }],
            ),
        )
        .expect("splice applies to an instance");

        // An override, not a component: an instance that grew a `[components]`
        // table would have two sources for one field and the parser rejects it.
        assert!(
            !applied.scene.contains("[node.components"),
            "the instance grew a component:\n{}",
            applied.scene
        );
        assert!(applied.scene.contains("VoxelVolume.ops"));
        assert!(applied.scene.contains("sphere"));
        // The existing entry survived and the new one landed after it.
        let ops_line = applied
            .scene
            .lines()
            .find(|l| l.contains("VoxelVolume.ops"))
            .expect("the override line");
        let box_at = ops_line.find("box").expect("box kept");
        let sphere_at = ops_line.find("sphere").expect("sphere added");
        assert!(box_at < sphere_at, "order lost: {ops_line}");
        // And it still parses, which is the check that the override is legal
        // rather than merely present.
        crate::Scene::parse(&applied.scene).expect("the spliced instance must still load");
    }

    /// `Declare` is how the editor introduces an asset at all — referencing
    /// one was possible, authoring one was CLI-only.
    #[test]
    fn declaring_an_asset_and_rejecting_a_duplicate_alias() {
        let applied = apply(
            SCENE,
            &tx(
                "Import lamp.glb",
                vec![SceneOp::Declare {
                    kind: "asset".into(),
                    key: "lamp".into(),
                    id: None,
                    path: "meshes/lamp.glb".into(),
                }],
            ),
        )
        .expect("declaration applies");
        assert!(applied.scene.contains("[[asset]]"));
        assert!(applied.scene.contains("meshes/lamp.glb"));
        crate::Scene::parse(&applied.scene).expect("still loads");

        let err = apply(
            &applied.scene,
            &tx(
                "Import it twice",
                vec![SceneOp::Declare {
                    kind: "asset".into(),
                    key: "lamp".into(),
                    id: None,
                    path: "meshes/other.glb".into(),
                }],
            ),
        )
        .expect_err("one alias cannot mean two things in one file");
        assert_eq!(err.error, "duplicate_alias");
    }

    /// A node instances a prefab or draws a mesh. Both is the state the parser
    /// rejects, so the op must refuse to author it.
    #[test]
    fn spawning_a_prefab_instance_and_refusing_mesh_plus_prefab() {
        let applied = apply(
            SCENE,
            &tx(
                "Place a rock",
                // Declared and instanced in one transaction, which is what
                // dropping a prefab into the viewport actually is — and the
                // validator refuses the instance on its own, correctly.
                vec![
                    SceneOp::Declare {
                        kind: "prefab".into(),
                        key: "rock".into(),
                        id: Some("11111111-2222-3333-4444-555555555555".into()),
                        path: "rock.loom".into(),
                    },
                    SceneOp::SpawnNode {
                        parent: "Room".into(),
                        name: "Boulder".into(),
                        mesh: None,
                        prefab: Some("rock".into()),
                    },
                ],
            ),
        )
        .expect("spawn applies");
        assert!(applied.scene.contains("prefab = \"rock\""));

        let err = apply(
            SCENE,
            &tx(
                "Both",
                vec![SceneOp::SpawnNode {
                    parent: "Room".into(),
                    name: "Confused".into(),
                    mesh: Some("box".into()),
                    prefab: Some("rock".into()),
                }],
            ),
        )
        .expect_err("a node is one or the other");
        assert_eq!(err.error, "mesh_and_prefab");
    }

    /// Renaming has to rewrite every descendant's `parent`, because a parent
    /// is stored as a path. Miss that and the children are orphaned — the
    /// scene stops loading and the op looked like it worked.
    #[test]
    fn renaming_a_node_carries_its_children() {
        let applied = apply(
            SCENE,
            &tx(
                "Rename Room",
                vec![SceneOp::RenameNode {
                    node: "Room".into(),
                    name: "Studio".into(),
                }],
            ),
        )
        .expect("should apply");

        let scene = crate::Scene::parse(&applied.scene).expect("still valid");
        let paths: Vec<&str> = scene.nodes().iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, ["Studio", "Studio/Desk"]);
    }

    #[test]
    fn renaming_onto_an_existing_sibling_is_refused() {
        let scene = format!("{SCENE}\n[[node]]\nname = \"Chair\"\nparent = \"Room\"\n");
        let err = apply(
            &scene,
            &tx(
                "Collide",
                vec![SceneOp::RenameNode {
                    node: "Room/Desk".into(),
                    name: "Chair".into(),
                }],
            ),
        )
        .expect_err("must be refused");

        assert_eq!(err.error, "duplicate_sibling_name");
    }

    #[test]
    fn reparenting_moves_a_node_and_its_children() {
        let scene = format!("{SCENE}\n[[node]]\nname = \"Alcove\"\nparent = \"Room\"\n");
        let applied = apply(
            &scene,
            &tx(
                "Move the desk into the alcove",
                vec![SceneOp::ReparentNode {
                    node: "Room/Desk".into(),
                    parent: "Room/Alcove".into(),
                }],
            ),
        )
        .expect("should apply");

        let parsed = crate::Scene::parse(&applied.scene).expect("still valid");
        let paths: Vec<&str> = parsed.nodes().iter().map(|n| n.path.as_str()).collect();
        assert!(paths.contains(&"Room/Alcove/Desk"), "{paths:?}");
        assert!(!paths.contains(&"Room/Desk"));
    }

    /// A node reparented under its own child has no path at all. Left
    /// unchecked this writes a scene that cannot be loaded and cannot be
    /// undone by re-reading, because the file itself is now nonsense.
    /// Canonical form is depth-first (§2 rule 3), so a moved subtree belongs
    /// directly after its new parent — not appended to the end, which would
    /// load fine and leave the file quietly non-canonical.
    #[test]
    fn a_reparented_subtree_lands_next_to_its_new_parent() {
        let scene = format!(
            "{SCENE}\n[[node]]\nname = \"Alcove\"\nparent = \"Room\"\n\n\
             [[node]]\nname = \"Drawer\"\nparent = \"Room/Desk\"\n\n\
             [[node]]\nname = \"Window\"\nparent = \"Room\"\n"
        );
        let applied = apply(
            &scene,
            &tx(
                "Into the alcove",
                vec![SceneOp::ReparentNode {
                    node: "Room/Desk".into(),
                    parent: "Room/Alcove".into(),
                }],
            ),
        )
        .expect("should apply");

        let parsed = crate::Scene::parse(&applied.scene).expect("still valid");
        let paths: Vec<&str> = parsed.nodes().iter().map(|n| n.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Room",
                "Room/Alcove",
                "Room/Alcove/Desk",
                "Room/Alcove/Desk/Drawer",
                "Room/Window",
            ],
            "the subtree travels together and stays depth-first"
        );
    }

    #[test]
    fn reparenting_a_node_under_its_own_descendant_is_refused() {
        let err = apply(
            SCENE,
            &tx(
                "Cycle",
                vec![SceneOp::ReparentNode {
                    node: "Room".into(),
                    parent: "Room/Desk".into(),
                }],
            ),
        )
        .expect_err("must be refused");

        assert_eq!(err.error, "would_create_a_cycle");
    }

    #[test]
    fn reparenting_a_node_under_itself_is_refused() {
        let err = apply(
            SCENE,
            &tx(
                "Self",
                vec![SceneOp::ReparentNode {
                    node: "Room/Desk".into(),
                    parent: "Room/Desk".into(),
                }],
            ),
        )
        .expect_err("must be refused");

        assert_eq!(err.error, "would_create_a_cycle");
    }

    #[test]
    fn removing_a_component_leaves_the_node() {
        let scene = apply(
            SCENE,
            &tx(
                "Light it",
                vec![SceneOp::SetField {
                    node: "Room/Desk".into(),
                    field: "Light.intensity".into(),
                    value: serde_json::json!(300.0),
                }],
            ),
        )
        .expect("added")
        .scene;

        let applied = apply(
            &scene,
            &tx(
                "Unlight it",
                vec![SceneOp::RemoveComponent {
                    node: "Room/Desk".into(),
                    component: "Light".into(),
                }],
            ),
        )
        .expect("removed");

        let parsed = crate::Scene::parse(&applied.scene).expect("still valid");
        let desk = parsed
            .nodes()
            .iter()
            .find(|n| n.path == "Room/Desk")
            .expect("node survives");
        assert!(!desk.components.contains_key("Light"));
    }

    #[test]
    fn removing_a_component_a_node_does_not_have_is_refused() {
        let err = apply(
            SCENE,
            &tx(
                "Nope",
                vec![SceneOp::RemoveComponent {
                    node: "Room/Desk".into(),
                    component: "Light".into(),
                }],
            ),
        )
        .expect_err("must be refused");

        assert_eq!(err.error, "unknown_component");
    }

    #[test]
    fn renaming_preserves_the_humans_comment() {
        let applied = apply(
            SCENE,
            &tx(
                "Rename",
                vec![SceneOp::RenameNode {
                    node: "Room/Desk".into(),
                    name: "Workbench".into(),
                }],
            ),
        )
        .expect("should apply");

        assert!(applied.scene.contains("A human wrote this comment"));
    }

    /// **The worst bug class in this project, reached from a toolbar button.**
    /// `json_to_item` flattened anything that was not a scalar: an object
    /// became `""`, an array of objects became `[]`. The editor's Assets panel
    /// sends `MeshRenderer.mesh = {"asset": "box"}`, so clicking it destroyed
    /// the asset reference and reported success; Duplicate re-emits every
    /// component field, so duplicating a terrain node wrote `ops = []` and
    /// erased the CSG recipe that never-do #11 makes its only representation.
    #[test]
    fn setting_an_object_field_keeps_the_object() {
        let scene = format!("{SCENE}\n  [node.components.MeshRenderer]\n  mesh = {{ asset = \"desk\" }}\n");
        let applied = apply(
            &scene,
            &tx(
                "Assign a mesh",
                vec![SceneOp::SetField {
                    node: "Room/Desk".into(),
                    field: "MeshRenderer.mesh".into(),
                    value: serde_json::json!({ "asset": "box" }),
                }],
            ),
        )
        .expect("should apply");

        let parsed = crate::Scene::parse(&applied.scene).expect("still valid");
        let node = parsed.nodes().iter().find(|n| n.path == "Room/Desk").expect("node");
        assert_eq!(
            node.components["MeshRenderer"]["mesh"]["asset"],
            serde_json::json!("box"),
            "the asset reference must survive: {}",
            applied.scene
        );
    }

    /// The voxel case, which loses the most: an op list is a whole terrain.
    #[test]
    fn setting_an_array_of_objects_keeps_every_entry() {
        let ops = serde_json::json!([
            { "kind": "sphere", "center": [1.0, 2.0, 3.0], "radius": 4.0, "mode": "union" },
            { "kind": "capsule", "a": [0.0, 0.0, 0.0], "b": [1.0, 0.0, 0.0], "radius": 2.0, "mode": "subtract" },
        ]);
        let applied = apply(
            SCENE,
            &tx(
                "Carve",
                vec![SceneOp::SetField {
                    node: "Room/Desk".into(),
                    field: "VoxelVolume.ops".into(),
                    value: ops.clone(),
                }],
            ),
        )
        .expect("should apply");

        let parsed = crate::Scene::parse(&applied.scene).expect("still valid");
        let node = parsed.nodes().iter().find(|n| n.path == "Room/Desk").expect("node");
        assert_eq!(node.components["VoxelVolume"]["ops"], ops, "{}", applied.scene);
    }

    /// TOML distinguishes 4 from 4.0 and the voxel reader uses `as_u64`, so an
    /// integer that comes back as a float is a field that silently stops
    /// parsing.
    #[test]
    fn integers_do_not_become_floats() {
        let applied = apply(
            SCENE,
            &tx(
                "Size it",
                vec![SceneOp::SetField {
                    node: "Room/Desk".into(),
                    field: "VoxelVolume.chunks".into(),
                    value: serde_json::json!([4, 3, 4]),
                }],
            ),
        )
        .expect("should apply");

        assert!(
            applied.scene.contains("chunks = [4, 3, 4]"),
            "integers must stay integers: {}",
            applied.scene
        );
    }

    /// Nothing in TOML represents null. Refusing is the only honest answer;
    /// writing `""` is how a field quietly becomes the wrong type.
    #[test]
    fn a_null_field_is_refused_rather_than_flattened() {
        let err = apply(
            SCENE,
            &tx(
                "Null it",
                vec![SceneOp::SetField {
                    node: "Room/Desk".into(),
                    field: "Light.color".into(),
                    value: serde_json::Value::Null,
                }],
            ),
        )
        .expect_err("must be refused");

        assert_eq!(err.error, "unrepresentable_value");
    }

    /// The format accepts a component written as an inline table. `SetField`
    /// assumed the sub-table spelling and `unwrap()`ed, so editing a field on a
    /// node the parser was perfectly happy with killed the process — taking any
    /// unsaved editor work with it.
    #[test]
    fn setting_a_field_on_an_inline_component_does_not_panic() {
        let scene = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Room\"

[[node]]
name = \"Lamp\"
parent = \"Room\"
components = { Light = { intensity = 400.0 } }
";
        assert!(crate::Scene::parse(scene).is_ok(), "the parser accepts this spelling");

        let result = apply(
            scene,
            &tx(
                "Dim it",
                vec![SceneOp::SetField {
                    node: "Room/Lamp".into(),
                    field: "Light.intensity".into(),
                    value: serde_json::json!(120.0),
                }],
            ),
        );

        let applied = result.expect("must not panic, and should apply");
        let parsed = crate::Scene::parse(&applied.scene).expect("still valid");
        let node = parsed.nodes().iter().find(|n| n.path == "Room/Lamp").expect("node");
        assert_eq!(node.components["Light"]["intensity"], serde_json::json!(120.0));
    }

    /// **A failure names which op failed.** A transaction is one undo step and
    /// can be a dozen ops long; "node not found" without an index tells the
    /// caller what went wrong and leaves them to guess where.
    #[test]
    fn a_failure_says_which_op_failed() {
        let scene = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d15\"

[[node]]
name = \"Desk\"
";
        let err = apply(
            scene,
            &tx(
                "Two moves, the second impossible",
                vec![
                    SceneOp::SetTransform {
                        node: "Desk".into(),
                        pos: Some([1.0, 0.0, 0.0]),
                        rot_euler: None,
                        scale: None,
                    },
                    SceneOp::SetTransform {
                        node: "NoSuchNode".into(),
                        pos: Some([2.0, 0.0, 0.0]),
                        rot_euler: None,
                        scale: None,
                    },
                ],
            ),
        )
        .expect_err("the second op cannot apply");

        assert_eq!(err.op_index, Some(1), "the SECOND op failed, not the first");
    }

    /// **A transform writes the shortest decimal that identifies the `f32`.**
    ///
    /// `f64::from(0.1f32)` is `0.10000000149011612`, and that is what this used
    /// to write — so a gizmo drag or a scripted move sprayed seventeen digits
    /// across a diff a human is supposed to review. The value must still
    /// round-trip exactly, because grid snapping's whole premise is that a snap
    /// to 0.25 reads back as 0.25.
    #[test]
    fn a_transform_writes_a_readable_float_that_round_trips() {
        let scene = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d14\"

[[node]]
name = \"Desk\"
";
        let applied = apply(
            scene,
            &tx(
                "Move",
                vec![SceneOp::SetTransform {
                    node: "Desk".into(),
                    pos: Some([0.1, 0.25, -2.5]),
                    rot_euler: None,
                    scale: None,
                }],
            ),
        )
        .expect("applies");
        let text = applied.scene;

        assert!(
            text.contains("0.1") && !text.contains("0.10000000149011612"),
            "0.1 must be written as `0.1`, got:\n{text}"
        );
        // And it must still be the same number when read back.
        let reparsed = crate::Scene::parse(&text).expect("re-parses");
        let node = reparsed
            .nodes()
            .iter()
            .find(|n| n.path == "Desk")
            .expect("the node survives");
        let pos = node.transform.pos;
        assert!(
            (pos[0] - 0.1).abs() < f32::EPSILON
                && (pos[1] - 0.25).abs() < f32::EPSILON
                && (pos[2] + 2.5).abs() < f32::EPSILON,
            "round-trip must be exact, got {pos:?}"
        );
    }

    /// Same shape of bug on the transform: only the inline spelling was read,
    /// so for a `[node.transform]` sub-table the op started from the default
    /// and silently wiped the axes it was not setting.
    #[test]
    fn setting_one_transform_axis_keeps_the_others() {
        let scene = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Room\"

[[node]]
name = \"Desk\"
parent = \"Room\"

  [node.transform]
  pos = [1.0, 2.0, 3.0]
  scale = [2.0, 2.0, 2.0]
";
        assert!(crate::Scene::parse(scene).is_ok(), "the parser accepts this spelling");

        let applied = apply(
            scene,
            &tx(
                "Rotate only",
                vec![SceneOp::SetTransform {
                    node: "Room/Desk".into(),
                    pos: None,
                    rot_euler: Some([0.0, 90.0, 0.0]),
                    scale: None,
                }],
            ),
        )
        .expect("should apply");

        let parsed = crate::Scene::parse(&applied.scene).expect("still valid");
        let desk = parsed.nodes().iter().find(|n| n.path == "Room/Desk").expect("node");
        assert_eq!(desk.transform.pos, [1.0, 2.0, 3.0], "position must survive");
        assert_eq!(desk.transform.scale, [2.0, 2.0, 2.0], "scale must survive");
        assert_eq!(desk.transform.rot_euler, [0.0, 90.0, 0.0]);
    }

    /// Spawn and reparent in one transaction. A freshly pushed table has no
    /// recorded document position, and the reorder bailed out entirely when any
    /// table lacked one — so the subtree kept its old place and the file came
    /// out non-canonical, or with a child declared before its parent.
    #[test]
    fn a_node_spawned_and_reparented_in_one_transaction_lands_in_order() {
        let scene = format!("{SCENE}\n[[node]]\nname = \"Alcove\"\nparent = \"Room\"\n");
        let applied = apply(
            &scene,
            &tx(
                "Add a lamp, then hang it in the alcove",
                vec![
                    // The new parent is appended at the end of the file...
                    SceneOp::SpawnNode {
                        parent: "Room".into(),
                        name: "Shelf".into(),
                        mesh: Some("box".into()), prefab: None, },
                    // ...and an existing node declared *before* it moves in,
                    // so the subtree genuinely has to be reordered.
                    SceneOp::ReparentNode {
                        node: "Room/Desk".into(),
                        parent: "Room/Shelf".into(),
                    },
                ],
            ),
        )
        .expect("should apply");

        let parsed = crate::Scene::parse(&applied.scene).expect("still valid");
        let paths: Vec<&str> = parsed.nodes().iter().map(|n| n.path.as_str()).collect();
        assert_eq!(
            paths,
            ["Room", "Room/Alcove", "Room/Shelf", "Room/Shelf/Desk"],
            "depth-first, parent before child: {}",
            applied.scene
        );
    }

    /// Editing a field must not delete the note the human left on it. The
    /// switch to `TableLike::insert` replaced the key as well as the value,
    /// and a key carries its decor — so the comment went with it. That is the
    /// same class of loss this whole layer exists to prevent.
    #[test]
    fn editing_a_field_keeps_the_comment_above_it() {
        let scene = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Room\"

[[node]]
name = \"Lamp\"
parent = \"Room\"

  [node.components.Light]
  # Dimmed on purpose: this room is meant to feel like evening.
  intensity = 400.0
";
        let applied = apply(
            scene,
            &tx(
                "Dim it further",
                vec![SceneOp::SetField {
                    node: "Room/Lamp".into(),
                    field: "Light.intensity".into(),
                    value: serde_json::json!(120.0),
                }],
            ),
        )
        .expect("should apply");

        assert!(
            applied.scene.contains("meant to feel like evening"),
            "the human's comment must survive an edit to the field it annotates:\n{}",
            applied.scene
        );
    }

    /// The inline spelling must be removable too — `SetField` learned to read
    /// it and `RemoveComponent` did not.
    #[test]
    fn removing_a_component_written_inline_works() {
        let scene = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Room\"

[[node]]
name = \"Lamp\"
parent = \"Room\"
components = { Light = { intensity = 400.0 } }
";
        let applied = apply(
            scene,
            &tx(
                "Unlight it",
                vec![SceneOp::RemoveComponent {
                    node: "Room/Lamp".into(),
                    component: "Light".into(),
                }],
            ),
        )
        .expect("must find the component it can plainly see");

        let parsed = crate::Scene::parse(&applied.scene).expect("still valid");
        let lamp = parsed.nodes().iter().find(|n| n.path == "Room/Lamp").expect("node");
        assert!(!lamp.components.contains_key("Light"));
    }

    /// A comment on the *same line* as the value is still the human's note.
    /// The previous fix only rescued comments on the line above, and the format
    /// spec promises both survive.
    #[test]
    fn editing_a_field_keeps_a_trailing_comment() {
        let scene = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Room\"

[[node]]
name = \"Lamp\"
parent = \"Room\"

  [node.components.Light]
  intensity = 400.0 # dimmed on purpose: evening
";
        let applied = apply(
            scene,
            &tx(
                "Dim it",
                vec![SceneOp::SetField {
                    node: "Room/Lamp".into(),
                    field: "Light.intensity".into(),
                    value: serde_json::json!(120.0),
                }],
            ),
        )
        .expect("should apply");

        assert!(
            applied.scene.contains("dimmed on purpose: evening"),
            "a trailing comment is annotation too:\n{}",
            applied.scene
        );
    }

    /// Replacing a field that was written as a sub-table must still emit a
    /// canonical `key = value`. The header form's key carries no trailing
    /// space, so the first write came out as `ops= [...]`.
    #[test]
    fn replacing_a_sub_table_field_stays_canonical() {
        let scene = "\
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Room\"

[[node]]
name = \"Hill\"
parent = \"Room\"

  [node.components.VoxelVolume]
  voxel_size = 0.25
  chunks = [1, 1, 1]

    [[node.components.VoxelVolume.ops]]
    kind = \"sphere\"
    center = [4.0, 4.0, 4.0]
    radius = 2.0
    mode = \"union\"
";
        let applied = apply(
            scene,
            &tx(
                "Recarve",
                vec![SceneOp::SetField {
                    node: "Room/Hill".into(),
                    field: "VoxelVolume.ops".into(),
                    value: serde_json::json!([
                        { "kind": "box", "center": [4.0, 4.0, 4.0],
                          "half_extents": [2.0, 2.0, 2.0], "mode": "union" }
                    ]),
                }],
            ),
        )
        .expect("should apply");

        assert!(
            !applied.scene.contains("ops="),
            "canonical form is `ops = [...]`, with the space:\n{}",
            applied.scene
        );
        assert!(applied.scene.contains("ops = ["), "{}", applied.scene);
        crate::Scene::parse(&applied.scene).expect("still valid");
    }

    #[test]
    fn spawning_a_node_preserves_the_humans_comment() {
        let applied = apply(
            SCENE,
            &tx(
                "Add a lamp",
                vec![SceneOp::SpawnNode {
                    parent: "Room".into(),
                    name: "Lamp".into(),
                    mesh: Some("sphere".into()), prefab: None, }],
            ),
        )
        .expect("should apply");

        assert!(
            applied.scene.contains("A human wrote this comment"),
            "an agent write must never delete a comment"
        );
        assert!(applied.scene.contains("name = \"Lamp\""));
        Scene::parse(&applied.scene).expect("result must still load");
    }

    /// **§7.17.** A write against a version that moved is rejected, and the
    /// caller gets the current content back so it can re-apply.
    #[test]
    fn a_stale_write_is_rejected_and_returns_the_current_content() {
        let stale = VersionToken("0000000000000000".into());
        let err = apply(
            SCENE,
            &Transaction {
                expect_version: Some(stale),
                ..tx(
                    "Late write",
                    vec![SceneOp::SpawnNode {
                        parent: "Room".into(),
                        name: "Late".into(),
                        mesh: None, prefab: None, }],
                )
            },
        )
        .expect_err("stale version must be refused");

        assert_eq!(err.error, "stale_version");
        assert_eq!(err.current.as_deref(), Some(SCENE), "so it can re-apply");
        assert!(err.hint.unwrap().contains("never merge"));
    }

    #[test]
    fn a_matching_version_is_accepted() {
        let token = VersionToken::of(SCENE);
        let applied = apply(
            SCENE,
            &Transaction {
                expect_version: Some(token),
                ..tx(
                    "Fresh write",
                    vec![SceneOp::SpawnNode {
                        parent: "Room".into(),
                        name: "Fresh".into(),
                        mesh: None, prefab: None, }],
                )
            },
        )
        .expect("current version must be accepted");

        assert_ne!(
            applied.version,
            VersionToken::of(SCENE),
            "the version must move"
        );
    }

    /// One transaction is one undo step, no matter how many ops it holds.
    #[test]
    fn a_twelve_op_transaction_undoes_in_one_step() {
        let ops: Vec<SceneOp> = (0..12)
            .map(|i| SceneOp::SpawnNode {
                parent: "Room".into(),
                name: format!("Box{i}"),
                mesh: Some("box".into()),
                prefab: None,
            })
            .collect();

        let applied = apply(SCENE, &tx("Block out: 12 nodes", ops)).expect("should apply");
        assert_eq!(applied.scene.matches("[[node]]").count(), 14);

        assert_eq!(applied.undo, SCENE, "one undo restores everything");
    }

    /// The whole transaction is rejected, not partly applied — otherwise a
    /// failure leaves the scene in a state nobody asked for.
    #[test]
    fn a_failing_op_rejects_the_whole_transaction() {
        let err = apply(
            SCENE,
            &tx(
                "Half-valid",
                vec![
                    SceneOp::SpawnNode {
                        parent: "Room".into(),
                        name: "Fine".into(),
                        mesh: None, prefab: None, },
                    SceneOp::SpawnNode {
                        parent: "Nowhere".into(),
                        name: "Broken".into(),
                        mesh: None, prefab: None, },
                ],
            ),
        )
        .expect_err("second op is invalid");

        assert_eq!(err.error, "unknown_parent");
    }

    #[test]
    fn setting_a_field_creates_the_component() {
        let applied = apply(
            SCENE,
            &tx(
                "Light the desk",
                vec![SceneOp::SetField {
                    node: "Room/Desk".into(),
                    field: "Light.intensity".into(),
                    value: serde_json::json!(420.0),
                }],
            ),
        )
        .expect("should apply");

        assert!(applied.scene.contains("intensity = 420.0"));
        Scene::parse(&applied.scene).expect("result must load");
    }

    /// A field that violates its schema must not land, even though the TOML
    /// edit itself succeeds.
    #[test]
    fn an_out_of_range_field_is_refused() {
        let err = apply(
            SCENE,
            &tx(
                "Too bright",
                vec![SceneOp::SetField {
                    node: "Room/Desk".into(),
                    field: "Light.intensity".into(),
                    value: serde_json::json!(40000.0),
                }],
            ),
        )
        .expect_err("40000 exceeds the maximum");

        assert_eq!(err.error, "would_produce_invalid_scene");
        assert!(err.hint.unwrap().contains("scene is unchanged"));
    }

    #[test]
    fn removing_a_node_with_children_is_refused() {
        let err = apply(
            SCENE,
            &tx(
                "Delete the room",
                vec![SceneOp::RemoveNode {
                    node: "Room".into(),
                }],
            ),
        )
        .expect_err("Room has a child");

        assert_eq!(err.error, "node_has_children");
    }

    #[test]
    fn a_dry_run_diff_shows_only_the_changed_lines() {
        let applied = apply(
            SCENE,
            &tx(
                "Move the desk",
                vec![SceneOp::SetTransform {
                    node: "Room/Desk".into(),
                    pos: Some([1.0, 0.0, 2.0]),
                    rot_euler: None,
                    scale: None,
                }],
            ),
        )
        .expect("should apply");

        assert_eq!(applied.diff.len(), 2, "one line out, one in: {:?}", applied.diff);
        assert!(applied.diff[1].contains("1.0"));
    }
}

#[cfg(test)]
mod prefab_ops {
    use super::*;
    use crate::prefab::Library;

    const LAMP_ID: &str = "7d3e1b90-4c25-4a68-9f01-2b6ce8a4d517";

    const ROOM: &str = "\
[scene]
format = 1
id = \"5a2f6c81-9e34-4d07-b1a8-3f7d02c9e461\"

[[prefab]]
key = \"lamp\"
id = \"7d3e1b90-4c25-4a68-9f01-2b6ce8a4d517\"
path = \"lamp.loom\"

[[node]]
name = \"Room\"

[[node]]
name = \"Lamp\"
parent = \"Room\"
prefab = \"lamp\"
transform = { pos = [2.0, 0.0, 0.0] }

  [node.overrides]
  \"Light.intensity\" = 30.0
";

    fn library() -> Library {
        let mut library = Library::new();
        library.insert(
            LAMP_ID,
            Scene::parse(
                "[scene]\nformat = 1\n\n[[node]]\nname = \"Lamp\"\n\n  \
                 [node.components.Light]\n  intensity = 120.0\n\n\
                 [[node]]\nname = \"Shade\"\nparent = \"Lamp\"\n\
                 transform = { pos = [0.0, 0.42, 0.0] }\n\n  \
                 [node.components.Material]\n  roughness = 0.7\n",
            )
            .expect("prefab is valid"),
        );
        library
    }

    fn tx(label: &str, ops: Vec<SceneOp>) -> Transaction {
        Transaction { label: label.into(), ops, dry_run: false, expect_version: None }
    }

    /// **Setting a field on an instance writes an override, not a component.**
    /// An instance owns no components — the parser rejects a node with both —
    /// so the inspector and the agent need no idea which kind of node it is.
    #[test]
    fn setting_a_field_on_an_instance_becomes_an_override() {
        let applied = apply(
            ROOM,
            &tx(
                "Dim it further",
                vec![SceneOp::SetField {
                    node: "Room/Lamp".into(),
                    field: "Light.intensity".into(),
                    value: serde_json::json!(12.0),
                }],
            ),
        )
        .expect("applies");

        assert!(applied.scene.contains("\"Light.intensity\" = 12.0"));
        assert!(
            !applied.scene.contains("[node.components.Light]"),
            "must not write a component onto an instance:\n{}",
            applied.scene
        );
        let parsed = Scene::parse(&applied.scene).expect("still valid");
        let lamp = parsed.nodes().iter().find(|n| n.path == "Room/Lamp").expect("node");
        assert_eq!(lamp.overrides["Light.intensity"], 12.0);
    }

    /// The child-path spelling survives the round trip through an op.
    #[test]
    fn an_override_can_be_set_on_a_child_of_the_instance() {
        let applied = apply(
            ROOM,
            &tx(
                "Roughen the shade",
                vec![SceneOp::SetField {
                    node: "Room/Lamp".into(),
                    field: "Shade::Material.roughness".into(),
                    value: serde_json::json!(0.2),
                }],
            ),
        )
        .expect("applies");

        let parsed = Scene::parse(&applied.scene).expect("still valid");
        let lamp = parsed.nodes().iter().find(|n| n.path == "Room/Lamp").expect("node");
        assert_eq!(lamp.overrides["Shade::Material.roughness"], 0.2);
    }

    /// Reverting everything puts the instance back to the prefab.
    #[test]
    fn reverting_all_overrides_removes_the_table() {
        let applied = apply(
            ROOM,
            &tx("Back to stock", vec![SceneOp::RevertOverrides {
                node: "Room/Lamp".into(),
                keys: Vec::new(),
            }]),
        )
        .expect("applies");

        assert!(!applied.scene.contains("node.overrides"), "{}", applied.scene);
        let parsed = Scene::parse(&applied.scene).expect("still valid");
        let lamp = parsed.nodes().iter().find(|n| n.path == "Room/Lamp").expect("node");
        assert!(lamp.overrides.is_empty());
        assert_eq!(lamp.prefab.as_deref(), Some("lamp"), "still an instance");
    }

    /// Reverting one key leaves the others — "revert this field" in an
    /// inspector, rather than "revert everything".
    #[test]
    fn reverting_one_key_leaves_the_rest() {
        let two = apply(
            ROOM,
            &tx("Add a second", vec![SceneOp::SetField {
                node: "Room/Lamp".into(),
                field: "Shade::Material.roughness".into(),
                value: serde_json::json!(0.2),
            }]),
        )
        .expect("applies")
        .scene;

        let applied = apply(
            &two,
            &tx("Revert the shade", vec![SceneOp::RevertOverrides {
                node: "Room/Lamp".into(),
                keys: vec!["Shade::Material.roughness".into()],
            }]),
        )
        .expect("applies");

        let parsed = Scene::parse(&applied.scene).expect("still valid");
        let lamp = parsed.nodes().iter().find(|n| n.path == "Room/Lamp").expect("node");
        assert_eq!(lamp.overrides.len(), 1);
        assert_eq!(lamp.overrides["Light.intensity"], 30.0);
    }

    /// Naming a key the instance does not carry is an error, not a no-op — a
    /// silently ignored revert reads as "this field is back to the prefab"
    /// when it is not.
    #[test]
    fn reverting_an_override_that_is_not_there_is_refused() {
        let err = apply(
            ROOM,
            &tx("Revert nothing", vec![SceneOp::RevertOverrides {
                node: "Room/Lamp".into(),
                keys: vec!["Light.color".into()],
            }]),
        )
        .expect_err("no such override");

        assert_eq!(err.error, "unknown_override");
    }

    /// Overrides only exist on instances.
    #[test]
    fn reverting_on_a_plain_node_is_refused() {
        let err = apply(
            ROOM,
            &tx("Nonsense", vec![SceneOp::RevertOverrides {
                node: "Room".into(),
                keys: Vec::new(),
            }]),
        )
        .expect_err("not an instance");

        assert_eq!(err.error, "not_a_prefab_instance");
    }

    /// **Unpack makes an instance concrete**: the prefab's nodes are written
    /// out, the reference and its deltas go, and the override is baked in.
    #[test]
    fn unpacking_writes_the_prefabs_nodes_into_the_file() {
        let applied = apply_with(
            ROOM,
            &tx("Unpack the lamp", vec![SceneOp::UnpackPrefab { node: "Room/Lamp".into() }]),
            &library(),
        )
        .expect("applies");

        let parsed = Scene::parse(&applied.scene).expect("still valid");
        let paths: Vec<&str> = parsed.nodes().iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, ["Room", "Room/Lamp", "Room/Lamp/Shade"]);

        let lamp = parsed.nodes().iter().find(|n| n.path == "Room/Lamp").expect("node");
        assert!(lamp.prefab.is_none(), "no longer an instance");
        assert!(lamp.overrides.is_empty(), "the deltas are baked in, not kept");
        assert_eq!(
            lamp.components["Light"]["intensity"], 30.0,
            "the override became the value"
        );
        assert_eq!(lamp.transform.pos, [2.0, 0.0, 0.0], "placement survives");

        let shade = parsed.nodes().iter().find(|n| n.path == "Room/Lamp/Shade").expect("child");
        assert_eq!(shade.transform.pos, [0.0, 0.42, 0.0]);
    }

    /// After unpacking, an edit to the prefab no longer reaches the node.
    /// That is the whole meaning of the operation.
    #[test]
    fn an_unpacked_instance_stops_tracking_the_prefab() {
        let applied = apply_with(
            ROOM,
            &tx("Unpack", vec![SceneOp::UnpackPrefab { node: "Room/Lamp".into() }]),
            &library(),
        )
        .expect("applies");

        let scene = Scene::parse(&applied.scene).expect("valid");
        let resolved = crate::prefab::resolve(&scene, &library()).expect("nothing to resolve");

        let lamp = resolved.scene.nodes().iter().find(|n| n.path == "Room/Lamp").expect("node");
        assert_eq!(lamp.components["Light"]["intensity"], 30.0, "its own value now");
    }

    /// Unpacking through the plain `apply` says what is missing rather than
    /// claiming the prefab does not exist.
    #[test]
    fn unpacking_without_a_library_says_so() {
        let err = apply(
            ROOM,
            &tx("Unpack", vec![SceneOp::UnpackPrefab { node: "Room/Lamp".into() }]),
        )
        .expect_err("no prefabs supplied");

        assert_eq!(err.error, "prefab_library_required");
    }

    /// **The S4 exit criterion's last clause.** Overrides set across many ops
    /// and an unpack undo together, in one step, because the undo payload is
    /// the whole previous text.
    #[test]
    fn a_prefab_transaction_undoes_in_one_step() {
        let ops = vec![
            SceneOp::SetField {
                node: "Room/Lamp".into(),
                field: "Light.intensity".into(),
                value: serde_json::json!(7.0),
            },
            SceneOp::SetField {
                node: "Room/Lamp".into(),
                field: "Shade::Material.roughness".into(),
                value: serde_json::json!(0.1),
            },
            SceneOp::UnpackPrefab { node: "Room/Lamp".into() },
        ];

        let applied =
            apply_with(ROOM, &tx("Override twice and unpack", ops), &library()).expect("applies");

        assert!(applied.scene.contains("[node.components.Light]"), "it really unpacked");
        assert_eq!(applied.undo, ROOM, "one undo restores everything");
    }

    /// Unpacking brings the prefab's asset declarations with it — otherwise
    /// the written nodes reference aliases the file does not declare, and the
    /// scene stops loading.
    #[test]
    fn unpacking_brings_the_prefabs_asset_declarations() {
        let mut library = Library::new();
        library.insert(
            LAMP_ID,
            Scene::parse(
                "[scene]\nformat = 1\n\n[[asset]]\nkey = \"brass\"\n\
                 id = \"cccccccc-0000-4000-8000-000000000003\"\npath = \"brass.png\"\n\n\
                 [[node]]\nname = \"Lamp\"\n\n  [node.components.Material]\n  \
                 albedo_map = { asset = \"brass\" }\n",
            )
            .expect("prefab is valid"),
        );

        let applied = apply_with(
            ROOM,
            &tx("Unpack", vec![SceneOp::UnpackPrefab { node: "Room/Lamp".into() }]),
            &library,
        )
        .expect("applies");

        let parsed = Scene::parse(&applied.scene).expect("still valid");
        assert_eq!(parsed.asset_path("brass"), Some("brass.png"));
    }

    /// **The author's number survives, and defaults stay omitted.**
    ///
    /// A `Transform` holds f32 and JSON holds f64, so the obvious
    /// serialize-then-convert wrote `1.4` back as `1.399999976158142` — noise
    /// in place of what the author typed, in a format whose whole premise is
    /// that the text is the source of truth. It also emitted
    /// `rot_euler = [0.0, 0.0, 0.0]` and `scale = [1.0, 1.0, 1.0]` onto nodes
    /// that never had them, against §4.
    #[test]
    fn unpacking_writes_the_authored_numbers_not_widened_ones() {
        let applied = apply_with(
            ROOM,
            &tx("Unpack", vec![SceneOp::UnpackPrefab { node: "Room/Lamp".into() }]),
            &library(),
        )
        .expect("applies");

        assert!(
            applied.scene.contains("pos = [2.0, 0.0, 0.0]"),
            "the placement was rewritten:\n{}",
            applied.scene
        );
        assert!(
            applied.scene.contains("pos = [0.0, 0.42, 0.0]"),
            "the child's transform was rewritten:\n{}",
            applied.scene
        );
        assert!(
            !applied.scene.contains("999999"),
            "an f32 was widened into noise:\n{}",
            applied.scene
        );
        assert!(
            !applied.scene.contains("rot_euler"),
            "a default was written out, against §4:\n{}",
            applied.scene
        );
        assert!(!applied.scene.contains("scale ="), "same for scale:\n{}", applied.scene);
    }

    /// Everything else in the file is left exactly as the author wrote it,
    /// comments included — the reason this layer edits a DOM.
    #[test]
    fn unpacking_leaves_the_rest_of_the_file_alone() {
        let commented = ROOM.replace("[[node]]\nname = \"Room\"", "# The room itself.\n[[node]]\nname = \"Room\"");

        let applied = apply_with(
            &commented,
            &tx("Unpack", vec![SceneOp::UnpackPrefab { node: "Room/Lamp".into() }]),
            &library(),
        )
        .expect("applies");

        assert!(applied.scene.contains("# The room itself."), "{}", applied.scene);
    }
}
