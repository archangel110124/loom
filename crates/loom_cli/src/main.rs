//! The agent-facing entrypoint. CLI first, MCP second (brief §7.10).
//!
//! Subcommands land with the milestone that makes them real:
//!   validate | describe  M1 · render  M2 · run [--watch]  M5.5 · scene  M9 · voxel  M10
//!
//! Every subcommand emits structured JSON on stdout and uses the exit code as
//! the coarse signal, so both a shell and an agent can consume it.
//!
//! `ponytail:` hand-rolled argument matching. Two subcommands do not justify a
//! parser dependency. Switch to `clap` when M9 lands `scene place/measure/...`
//! and the count goes past four — `run` is the seam, so it is a local change.

use std::process::ExitCode;

use loom_scene::{Scene, components};

const USAGE: &str = "\
loom — AI-native engine CLI

USAGE:
    loom validate <scene.loom>    Validate a scene; exit 1 with JSON errors if invalid
    loom describe <TypeName>      Print a component's JSON Schema
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (code, output) = run(&args);
    if !output.is_empty() {
        println!("{output}");
    }
    ExitCode::from(code)
}

/// Exit code plus whatever should go to stdout.
///
/// Split out from `main` so it is testable without spawning a process.
/// Codes: 0 ok · 1 the thing was invalid · 2 the invocation was wrong.
fn run(args: &[String]) -> (u8, String) {
    match args.first().map(String::as_str) {
        Some("validate") => match args.get(1) {
            Some(path) => validate(path),
            None => (2, USAGE.to_owned()),
        },
        Some("describe") => match args.get(1) {
            Some(name) => describe(name),
            None => (2, USAGE.to_owned()),
        },
        _ => (2, USAGE.to_owned()),
    }
}

fn validate(path: &str) -> (u8, String) {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return (
                2,
                json_line(&serde_json::json!({
                    "error": "io_error",
                    "path": path,
                    "constraint": e.to_string(),
                })),
            );
        }
    };

    match Scene::parse(&src) {
        Ok(scene) => (
            0,
            json_line(&serde_json::json!({
                "ok": true,
                "path": path,
                "nodes": scene.nodes().len(),
            })),
        ),
        // Every violation, not just the first — one round-trip per fix is the
        // retry loop `docs/format/README.md` §6 exists to avoid.
        Err(errors) => (1, json_line(&serde_json::json!({ "errors": errors }))),
    }
}

fn describe(name: &str) -> (u8, String) {
    let registry = components::registry();
    match registry.describe(name) {
        Some(schema) => (0, json_line(schema)),
        None => {
            let known: Vec<&str> = registry.type_names().collect();
            (
                2,
                json_line(&serde_json::json!({
                    "error": "unknown_component_type",
                    "value": name,
                    // Listing the alternatives turns a failed lookup into one
                    // correction instead of a guessing loop (§6).
                    "hint": format!("known types: {}", known.join(", ")),
                })),
            )
        }
    }
}

fn json_line<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn describe_emits_the_schema_including_constraints() {
        let (code, out) = run(&args(&["describe", "Light"]));

        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(v["properties"]["intensity"]["maximum"], 10000.0);
        assert!(
            v["properties"]["intensity"]["description"]
                .as_str()
                .is_some_and(|d| d.contains("Interior lights")),
            "doc comment should reach the agent"
        );
    }

    /// A failed lookup lists the alternatives, so it costs one correction
    /// rather than a guessing loop.
    #[test]
    fn describe_of_an_unknown_type_lists_the_known_ones() {
        let (code, out) = run(&args(&["describe", "Ligth"]));

        assert_eq!(code, 2);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "unknown_component_type");
        let hint = v["hint"].as_str().unwrap();
        assert!(hint.contains("Light"), "should suggest Light, got: {hint}");
    }

    #[test]
    fn validate_accepts_the_canonical_fixture() {
        let (code, out) = run(&args(&["validate", "../../assets/test/office.loom"]));

        assert_eq!(code, 0, "office.loom should be valid: {out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["nodes"], 4);
    }

    #[test]
    fn validate_reports_the_out_of_range_field_with_its_node() {
        let (code, out) = run(&args(&["validate", "../../assets/test/bad_intensity.loom"]));

        assert_eq!(code, 1);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let e = &v["errors"][0];
        assert_eq!(e["error"], "field_out_of_range");
        assert_eq!(e["node"], "Office/CeilingLight");
        assert_eq!(e["field"], "Light.intensity");
        assert_eq!(e["constraint"], "0.0..=10000.0");
    }

    #[test]
    fn a_missing_file_is_an_invocation_error_not_an_invalid_scene() {
        let (code, out) = run(&args(&["validate", "nope.loom"]));

        assert_eq!(code, 2, "2 means 'you called it wrong', 1 means 'invalid'");
        assert!(out.contains("io_error"));
    }

    #[test]
    fn no_arguments_prints_usage() {
        let (code, out) = run(&[]);

        assert_eq!(code, 2);
        assert!(out.contains("loom validate"));
    }
}
