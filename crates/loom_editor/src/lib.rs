//! The editor's UI, and nothing else.
//!
//! **Everything here is a pure function of borrowed state.** A panel reads
//! what it is given and returns [`UiAction`]s; the caller in `loom_cli` turns
//! those into [`loom_scene::ops::Transaction`]s and applies them through the
//! same code path the agent uses. That is never-do #16 — the editor gets no
//! undo stack of its own — and keeping the UI in a crate that *cannot* reach
//! the apply path is what makes the rule structural rather than a convention
//! somebody remembers.
//!
//! The crate exists so that the editor is removable: `loom-play` will link
//! the runtime without it (Stage 5). It therefore depends on `loom_scene`,
//! `loom_reflect` and `loom_render` only — never on `loom_cli`, which depends
//! on *it*.

pub mod console;
pub mod gizmo;
pub mod panels;
pub mod theme;

pub use console::{Entry, Level};
pub use theme::{Tokens, apply as apply_theme, tokens};
pub use gizmo::{Handle, Mode};
pub use panels::{AgentMark, PanelState, UiAction, draw};
