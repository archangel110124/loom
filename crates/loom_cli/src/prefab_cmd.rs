//! `loom prefab` — the operations that span an instance and its prefab.
//!
//! `revert-overrides` and `unpack` are plain [`SceneOp`]s and could be issued
//! through `loom scene --tx`; they are here so the three §5 operations are one
//! command rather than two spellings.
//!
//! **`apply-overrides` is the one that could not be a single op.** Promoting a
//! deviation into the prefab writes *two* files — the prefab gains the value,
//! the instance loses the override — and a `Transaction` is scoped to one
//! scene. So it is two transactions, and it says so: the prefab's own undo
//! entry, and the instance's. Pretending otherwise would mean inventing a
//! cross-file transaction to make one command look tidy.
//!
//! Both halves still go through `SceneOp` and `apply_to_file`, so never-do #16
//! holds: there is no second code path an editor could take.

use std::path::{Path, PathBuf};

use loom_scene::{Scene, SceneOp, Transaction};

use crate::json_line;

/// Dispatch `loom prefab <verb> <scene> --node <path>`.
pub(crate) fn run(args: &[String]) -> (u8, String) {
    let (Some(verb), Some(path)) = (args.get(1), args.get(2)) else {
        return (2, usage());
    };
    let Some(node) = crate::flag(args, "--node") else {
        return (
            2,
            json_line(&serde_json::json!({
                "error": "missing_argument",
                "hint": "--node <path> names the prefab instance to operate on",
            })),
        );
    };
    let scene_path = PathBuf::from(path);
    let keys = crate::flags(args, "--key");

    match verb.as_str() {
        "unpack" => one(&scene_path, &node, SceneOp::UnpackPrefab { node: node.clone() }, "Unpack"),
        "revert-overrides" => one(
            &scene_path,
            &node,
            SceneOp::RevertOverrides { node: node.clone(), keys },
            "Revert overrides on",
        ),
        "apply-overrides" => apply_overrides(&scene_path, &node, &keys),
        _ => (2, usage()),
    }
}

fn usage() -> String {
    json_line(&serde_json::json!({
        "error": "usage",
        "hint": "loom prefab <unpack|revert-overrides|apply-overrides> <scene.loom> \
                 --node <path> [--key <Type.field> ...]",
    }))
}

/// One op, one file, one undo step.
fn one(path: &Path, node: &str, op: SceneOp, verb: &str) -> (u8, String) {
    let transaction = Transaction {
        // The label reaches the human's log panel and the git history, so it
        // names what happened rather than "update scene".
        label: format!("{verb} {node}"),
        ops: vec![op],
        dry_run: false,
        expect_version: None,
    };
    match loom_scene::apply_to_file(path, &transaction) {
        Ok(applied) => (
            0,
            json_line(&serde_json::json!({
                "ok": true,
                "label": applied.label,
                "version": applied.version,
                "diff": applied.diff,
            })),
        ),
        Err(e) => crate::file_apply_error(&path.to_string_lossy(), &e),
    }
}

/// Push an instance's deviations into the prefab, then drop them.
///
/// Every other instance of that prefab picks the values up, which is the
/// point: "this is how the lamp should have been all along."
fn apply_overrides(path: &Path, node: &str, keys: &[String]) -> (u8, String) {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return (2, json_line(&serde_json::json!({
                "error": "io_error", "path": path.display().to_string(),
                "constraint": e.to_string(),
            })));
        }
    };
    let scene = match Scene::parse(&src) {
        Ok(s) => s,
        Err(errors) => return (1, json_line(&serde_json::json!({ "errors": errors }))),
    };

    let Some(instance) = scene.nodes().iter().find(|n| n.path == node) else {
        return (1, json_line(&serde_json::json!({
            "error": "unknown_node", "node": node,
        })));
    };
    let Some(alias) = instance.prefab.as_deref() else {
        return (1, json_line(&serde_json::json!({
            "error": "not_a_prefab_instance", "node": node,
            "hint": "only an instance has overrides to apply",
        })));
    };
    let Some(decl) = scene.prefabs().into_iter().find(|p| p.key == alias) else {
        return (1, json_line(&serde_json::json!({
            "error": "unresolved_prefab", "node": node, "value": alias,
        })));
    };

    // Which deviations to promote. Naming none promotes all of them.
    let chosen: Vec<(&String, &serde_json::Value)> = instance
        .overrides
        .iter()
        .filter(|(key, _)| keys.is_empty() || keys.iter().any(|k| k == *key))
        .collect();
    if chosen.is_empty() {
        return (1, json_line(&serde_json::json!({
            "error": "unknown_override", "node": node,
            "hint": "the instance carries no overrides matching that selection",
        })));
    }

    // Each override key names a node inside the prefab and a field on it.
    // `Child/Path::Type.field` targets a descendant; a bare `Type.field`
    // targets the prefab's own root, whose name is not the instance's.
    let prefab_path = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&decl.path);
    let prefab_src = match std::fs::read_to_string(&prefab_path) {
        Ok(s) => s,
        Err(e) => {
            return (2, json_line(&serde_json::json!({
                "error": "io_error", "path": prefab_path.display().to_string(),
                "constraint": e.to_string(),
            })));
        }
    };
    let prefab_scene = match Scene::parse(&prefab_src) {
        Ok(s) => s,
        Err(errors) => return (1, json_line(&serde_json::json!({ "errors": errors }))),
    };
    let Some(prefab_root) = prefab_scene.nodes().first().map(|n| n.path.clone()) else {
        return (1, json_line(&serde_json::json!({
            "error": "empty_prefab", "node": node,
        })));
    };

    let mut ops = Vec::new();
    let mut promoted = Vec::new();
    for (key, value) in chosen {
        let (child, field) = match key.split_once("::") {
            Some((child, field)) => (Some(child), field),
            None => (None, key.as_str()),
        };
        let target = match child {
            None => prefab_root.clone(),
            Some(child) => format!("{prefab_root}/{child}"),
        };
        if !prefab_scene.nodes().iter().any(|n| n.path == target) {
            return (1, json_line(&serde_json::json!({
                "error": "orphaned_override", "node": node, "field": key,
                "constraint": format!("the prefab has no node at `{target}`"),
                "hint": "retarget or delete the override; it cannot be applied",
            })));
        }
        ops.push(SceneOp::SetField {
            node: target,
            field: field.to_owned(),
            value: value.clone(),
        });
        promoted.push(key.clone());
    }

    // The prefab first. If this fails nothing has changed anywhere; doing the
    // instance first could strip the overrides and then fail to record them,
    // which loses the author's work — the never-do #15 shape of mistake.
    let to_prefab = Transaction {
        label: format!("Apply overrides from {node}"),
        ops,
        dry_run: false,
        expect_version: None,
    };
    if let Err(e) = loom_scene::apply_to_file(&prefab_path, &to_prefab) {
        return crate::file_apply_error(&prefab_path.to_string_lossy(), &e);
    }

    let to_instance = Transaction {
        label: format!("Applied overrides on {node}"),
        ops: vec![SceneOp::RevertOverrides {
            node: node.to_owned(),
            keys: promoted.clone(),
        }],
        dry_run: false,
        expect_version: None,
    };
    match loom_scene::apply_to_file(path, &to_instance) {
        Ok(applied) => (
            0,
            json_line(&serde_json::json!({
                "ok": true,
                "applied": promoted,
                "prefab": prefab_path.display().to_string(),
                "version": applied.version,
                // Two files changed, so there are two undo entries. Said out
                // loud rather than implied — a caller that expects one would
                // leave the prefab edited and think it had rolled back.
                "undo_steps": 2,
            })),
        ),
        Err(e) => crate::file_apply_error(&path.to_string_lossy(), &e),
    }
}
