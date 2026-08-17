//! What a console row *is*, so the panel can draw one.
//!
//! **The store stays in `loom_cli::log` and is unchanged** — the global
//! `Mutex<Vec<Entry>>`, the repeat collapsing and the 500-entry cap all live
//! there, and `loom_cli::log` re-exports these two types so its call sites and
//! its own test are untouched.
//!
//! Only the *vocabulary* is here, because the panels crate cannot see
//! `loom_cli` (it is the other way round) and because the console is not
//! editor-only: `sound.rs`, `telemetry.rs` and `play.rs` all write to it, and
//! the runtime binary that Stage 5 splits out still wants a log that mirrors
//! to stderr. Moving the store here would have made logging an editor feature.

/// How loud a console row is. Nothing else about it is styled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// One line of console output.
#[derive(Debug, Clone)]
pub struct Entry {
    pub level: Level,
    pub text: String,
    /// How many times in a row this same message arrived. A per-frame
    /// rejection would otherwise scroll everything else off the panel.
    pub repeats: u32,
}
