//! Talking to the agent, from inside the editor — ADR 0100.
//!
//! **The editor already watches the agent; this is the other direction.** An
//! agent editing the scene shows up in the viewport within 250 ms, because the
//! file is polled and reloaded (`App::poll_file`). What there was no way to do
//! was *ask* for something.
//!
//! The channel is two append-only files beside the scene:
//!
//! ```text
//! <scene-dir>/.loom-agent/inbox.jsonl    what the human asked for
//! <scene-dir>/.loom-agent/outbox.jsonl   what the agent said back
//! ```
//!
//! **Files, not a socket or an API key.** Three reasons, in order. It needs no
//! dependency, and this project has spent a lot of care not acquiring any. It
//! works with the agent the human actually has — a Claude Code session in this
//! repository, which can read a file and run `loom scene --tx` — rather than
//! requiring the editor to hold a credential and speak somebody's HTTP. And it
//! is inspectable: when the loop misbehaves, the conversation is two text files
//! you can read, which is not true of a socket.
//!
//! One line of JSON per message, appended and never rewritten, so a reader and
//! a writer cannot corrupt each other without a lock.

use std::path::{Path, PathBuf};

/// Who said it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    /// The person at the editor.
    Human,
    /// Whatever is answering.
    Agent,
}

/// One turn of the conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// Monotonic within a scene, so a reply can name what it answers.
    pub id: u64,
    pub speaker: Speaker,
    pub text: String,
    /// What was selected when the human sent it — the difference between
    /// "make it sit lower" meaning something and meaning nothing.
    pub about: Vec<String>,
}

/// Where the conversation for a scene lives.
#[must_use]
pub fn directory(scene: &Path) -> PathBuf {
    scene
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".loom-agent")
}

fn inbox(scene: &Path) -> PathBuf {
    directory(scene).join("inbox.jsonl")
}

fn outbox(scene: &Path) -> PathBuf {
    directory(scene).join("outbox.jsonl")
}

/// Read one file of the conversation.
fn read(path: &Path, speaker: Speaker) -> Vec<Message> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(Message {
                id: value.get("id").and_then(serde_json::Value::as_u64)?,
                speaker,
                text: value
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                about: value
                    .get("about")
                    .and_then(serde_json::Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// The whole conversation for a scene, oldest first.
///
/// **Interleaved by id**, so a reply sits under the thing it answers even
/// though the two halves live in different files.
#[must_use]
pub fn transcript(scene: &Path) -> Vec<Message> {
    let mut all = read(&inbox(scene), Speaker::Human);
    all.extend(read(&outbox(scene), Speaker::Agent));
    // Human before agent at the same id: the question precedes its answer.
    all.sort_by_key(|m| (m.id, m.speaker == Speaker::Agent));
    all
}

/// Ask for something. Returns the id it was filed under.
///
/// # Errors
/// If the directory cannot be made or the line cannot be appended.
pub fn ask(scene: &Path, text: &str, about: &[String]) -> std::io::Result<u64> {
    let directory = directory(scene);
    std::fs::create_dir_all(&directory)?;
    // One past whatever is there, counting both halves so an id is unique
    // across the conversation rather than within a file.
    let id = transcript(scene).iter().map(|m| m.id).max().unwrap_or(0) + 1;
    let line = serde_json::json!({
        "id": id,
        "at": scene.to_string_lossy(),
        "about": about,
        "text": text,
    });
    append(&inbox(scene), &line)?;
    Ok(id)
}

/// Answer something. Called by the agent, not the editor.
///
/// # Errors
/// If the line cannot be appended.
pub fn reply(scene: &Path, id: u64, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(directory(scene))?;
    append(&outbox(scene), &serde_json::json!({ "id": id, "text": text }))
}

/// Requests with no reply yet — what an agent should work on.
#[must_use]
pub fn pending(scene: &Path) -> Vec<Message> {
    let answered: std::collections::BTreeSet<u64> = read(&outbox(scene), Speaker::Agent)
        .into_iter()
        .map(|m| m.id)
        .collect();
    read(&inbox(scene), Speaker::Human)
        .into_iter()
        .filter(|m| !answered.contains(&m.id))
        .collect()
}

fn append(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{value}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **One directory per test.** They share a process and run in parallel,
    /// and a shared temp directory that each one deletes on entry is three
    /// tests clobbering each other — which is what this was, and it passed
    /// individually while failing together.
    fn scene(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("loom-agent-link-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join("scene.loom")
    }

    /// **A question and its answer, in order, out of two files.**
    #[test]
    fn a_conversation_reads_back_in_order() {
        let scene = scene("order");
        let first = ask(&scene, "lower the boat", &["Rig/Boat".to_owned()]).expect("ask");
        reply(&scene, first, "dropped the buoyancy coefficient").expect("reply");
        let second = ask(&scene, "and darken the sky", &[]).expect("ask");

        let turns = transcript(&scene);
        assert_eq!(turns.len(), 3, "{turns:?}");
        assert_eq!(turns[0].speaker, Speaker::Human);
        assert_eq!(turns[0].text, "lower the boat");
        assert_eq!(turns[0].about, vec!["Rig/Boat".to_owned()]);
        assert_eq!(turns[1].speaker, Speaker::Agent, "the answer follows the question");
        assert_eq!(turns[2].id, second);
        assert!(second > first, "ids move forward");
    }

    /// **What the agent should pick up**: asked and not yet answered.
    #[test]
    fn pending_is_what_has_not_been_answered() {
        let scene = scene("pending");
        let first = ask(&scene, "one", &[]).expect("ask");
        let second = ask(&scene, "two", &[]).expect("ask");
        assert_eq!(pending(&scene).len(), 2);

        reply(&scene, first, "done").expect("reply");
        let waiting = pending(&scene);
        assert_eq!(waiting.len(), 1, "{waiting:?}");
        assert_eq!(waiting[0].id, second);
    }

    /// A scene nobody has spoken to is quiet, not an error.
    #[test]
    fn an_untouched_scene_has_no_conversation() {
        let scene = std::env::temp_dir().join("loom-agent-nothing-here/scene.loom");
        assert!(transcript(&scene).is_empty());
        assert!(pending(&scene).is_empty());
    }

    /// **A half-written line must not lose the rest.** Both files are appended
    /// to by two processes; a torn write is a line that does not parse, and
    /// that is one lost message rather than a lost conversation.
    #[test]
    fn a_corrupt_line_is_skipped_not_fatal() {
        let scene = scene("corrupt");
        ask(&scene, "good", &[]).expect("ask");
        let path = super::inbox(&scene);
        let mut text = std::fs::read_to_string(&path).expect("read");
        text.push_str("{\"id\": 2, \"text\": \"tor\n");
        std::fs::write(&path, text).expect("write");

        let turns = transcript(&scene);
        assert_eq!(turns.len(), 1, "the good line survives: {turns:?}");
        assert_eq!(turns[0].text, "good");
    }
}
