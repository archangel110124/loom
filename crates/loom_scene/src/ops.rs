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
        })
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
        }));
    }

    let mut doc: DocumentMut = source
        .parse()
        .map_err(|e: toml_edit::TomlError| fail("parse_error", e.to_string(), None, None))?;

    for op in &transaction.ops {
        apply_one(&mut doc, op).map_err(|e| fail(&e.0, e.1, Some(e.2), e.3))?;
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
        SceneOp::SpawnNode { parent, name, mesh } => {
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
                        array.push(f64::from(*component));
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
            let value = toml_edit::Item::Value(json_to_toml(new, field)?);
            match component.get_mut(field_name) {
                Some(existing) => *existing = value,
                None => {
                    component.insert(field_name, value);
                }
            }
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
                        mesh: Some("box".into()),
                    },
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

    #[test]
    fn spawning_a_node_preserves_the_humans_comment() {
        let applied = apply(
            SCENE,
            &tx(
                "Add a lamp",
                vec![SceneOp::SpawnNode {
                    parent: "Room".into(),
                    name: "Lamp".into(),
                    mesh: Some("sphere".into()),
                }],
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
                        mesh: None,
                    }],
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
                        mesh: None,
                    }],
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
                        mesh: None,
                    },
                    SceneOp::SpawnNode {
                        parent: "Nowhere".into(),
                        name: "Broken".into(),
                        mesh: None,
                    },
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
