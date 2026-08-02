//! MCP server over the CLI.
//!
//! **Depended on by nothing** — `scripts/check-deps.sh` enforces it. The agent
//! layer has to be removable, or a bug is never cleanly attributable to the
//! engine rather than to the agent (design doc §2.13).
//!
//! **CLI first, MCP second** (brief §7.10). Every mutation this will expose
//! already exists as a `loom` subcommand that a shell and `cargo test` can
//! drive: `loom scene --tx`, `loom place --op`, `loom measure`, `loom render`,
//! `loom sim --assert`. This crate is a thin adapter over commands that
//! already work — building the agent interface *with* the agent is circular
//! otherwise, and an MCP layer is awkward to test without an agent driving it.
//!
//! The tool surface, per design doc §2.8, is catalog-mode: a handful of
//! always-present tools with the rest fetched on demand, because a server that
//! dumps hundreds of schemas costs tens of thousands of tokens before the
//! agent does anything.

/// The always-loaded tools, and the command each wraps.
///
/// Declared here so the shape is reviewable before the transport exists, and
/// so a missing implementation is visible rather than merely absent.
pub const TOOLS: &[(&str, &str)] = &[
    // Was "loom validate / loom measure". Only the first word after `loom` is
    // ever run, so measure-shaped arguments were quietly handed to `validate`
    // and the wrong result came back as a success. `scene_measure` below is
    // the tool that wraps measure.
    ("scene_query", "loom validate"),
    ("scene_edit", "loom scene --tx"),
    ("scene_place", "loom place --op"),
    ("scene_measure", "loom measure"),
    ("describe_type", "loom describe"),
    ("render_preview", "loom render"),
    ("run_scene", "loom sim --assert"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every declared tool must name a command that exists, or the catalog is
    /// a promise the CLI does not keep.
    #[test]
    fn every_tool_wraps_a_real_subcommand() {
        let usage = include_str!("../../loom_cli/src/main.rs");
        for (tool, command) in TOOLS {
            let subcommand = command
                .split_whitespace()
                .nth(1)
                .expect("command should name a subcommand");
            assert!(
                usage.contains(&format!("Some(\"{subcommand}\")")),
                "{tool} wraps `{command}`, but `{subcommand}` is not a CLI subcommand"
            );
        }
    }
}
