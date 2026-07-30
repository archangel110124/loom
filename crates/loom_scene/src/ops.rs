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

            let mut inline = table
                .get("transform")
                .and_then(Item::as_inline_table)
                .cloned()
                .unwrap_or_default();
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
            let components = table["components"].as_table_mut().unwrap();
            if components.get(type_name).is_none() {
                components[type_name] = Item::Table(Table::new());
            }
            let component = components[type_name].as_table_mut().unwrap();
            component[field_name] = json_to_item(new);
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

fn json_to_item(value: &Value) -> Item {
    match value {
        Value::Bool(b) => toml_edit::value(*b),
        Value::Number(n) => n.as_f64().map_or_else(
            || toml_edit::value(n.as_i64().unwrap_or(0)),
            toml_edit::value,
        ),
        Value::String(s) => toml_edit::value(s.as_str()),
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                match item {
                    Value::Number(n) => array.push(n.as_f64().unwrap_or(0.0)),
                    Value::Bool(b) => array.push(*b),
                    Value::String(s) => array.push(s.as_str()),
                    _ => {}
                }
            }
            toml_edit::value(array)
        }
        _ => toml_edit::value(""),
    }
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
