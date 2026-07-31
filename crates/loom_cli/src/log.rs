//! The console: what the engine has to say, where a human can see it.
//!
//! `stderr` is the wrong channel for an editor. The messages that matter most
//! while the agent is working — a rejected transaction, a scene that changed on
//! disk, a voxel volume that produced no surface — were being written to a
//! terminal nobody is looking at.
//!
//! `ponytail:` a global `Mutex<Vec<_>>` rather than threading a handle through
//! every call site. There is one console per process and the alternative is a
//! parameter on forty functions. Messages still mirror to stderr so the
//! headless CLI paths are unchanged.

use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub level: Level,
    pub text: String,
    /// How many times in a row this same message arrived. A per-frame
    /// rejection would otherwise scroll everything else off the panel.
    pub repeats: u32,
}

/// Beyond this the oldest entries are dropped — a window's worth of scrollback
/// is what a human reads; a session's worth is what a log file is for.
const CAPACITY: usize = 500;

fn store() -> &'static Mutex<Vec<Entry>> {
    static LOG: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    LOG.get_or_init(|| Mutex::new(Vec::new()))
}

fn push(level: Level, text: String) {
    let Ok(mut entries) = store().lock() else {
        eprintln!("loom: {text}");
        return;
    };
    if let Some(last) = entries.last_mut()
        && last.level == level
        && last.text == text
    {
        // Repeats are counted, not reprinted. The view is re-derived on every
        // frame of a drag, so a scene with one missing asset was writing the
        // same line hundreds of times a second into the log file.
        last.repeats += 1;
        return;
    }
    eprintln!("loom: {text}");
    if entries.len() >= CAPACITY {
        entries.remove(0);
    }
    entries.push(Entry {
        level,
        text,
        repeats: 1,
    });
}

pub fn info(text: impl Into<String>) {
    push(Level::Info, text.into());
}

pub fn warn(text: impl Into<String>) {
    push(Level::Warn, text.into());
}

pub fn error(text: impl Into<String>) {
    push(Level::Error, text.into());
}

/// Everything said so far, oldest first.
#[must_use]
pub fn entries() -> Vec<Entry> {
    store().lock().map(|e| e.clone()).unwrap_or_default()
}

pub fn clear() {
    if let Ok(mut entries) = store().lock() {
        entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rejection that repeats every frame must not push the message that
    /// explains it off the panel.
    ///
    /// One test, not two: the store is global and `cargo test` is threaded, so
    /// a second test calling `clear()` would race this one.
    #[test]
    fn repeats_collapse_and_distinct_messages_do_not() {
        clear();
        error("out of range");
        error("out of range");
        error("out of range");
        info("something else");

        let entries = entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].repeats, 3, "three identical errors, one row");
        assert_eq!(entries[1].repeats, 1);
    }
}
