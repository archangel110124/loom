//! The agent-facing entrypoint. CLI first, MCP second.
//!
//! Subcommands land with the milestone that makes them real:
//!   new | validate  M1 · render  M2 · run [--watch]  M5.5 · scene  M9 · voxel  M10
//! No argument parser until there is more than one thing to parse.

fn main() {
    println!("loom {} — M0 skeleton, no subcommands yet", env!("CARGO_PKG_VERSION"));
}
