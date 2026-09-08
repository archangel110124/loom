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
//!
//! # Proposals
//!
//! An agent may answer with a **change instead of a sentence**: the same
//! transaction it would have applied, carried in the reply along with the diff
//! it produced under `--dry-run`. The editor shows the diff with Apply and
//! Discard, and Apply runs it through the ordinary op path — one History entry,
//! one Ctrl+Z, the same validation. Nothing new can reach the scene through
//! this file; it only decides *when* something reaches it.
//!
//! This exists because the loop is asynchronous by design. The human asks and
//! goes back to work, and an edit that lands unseen is one they discover by
//! noticing the scene changed. A proposal is the same edit with the human's
//! eyes in front of it, which is the difference between an assistant and a
//! process running in their file.
//!
//! The decision is recorded as a **human line in the inbox** — the same file
//! their questions go in, because accepting a change is a thing the human did.

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
    /// A change offered rather than made, if this turn carries one.
    pub proposal: Option<Proposal>,
    /// The human's answer to a proposal: `applied` or `discarded`. Only ever
    /// set on a human turn, and only on one that names an earlier id.
    pub decision: Option<String>,
}

/// A change an agent is offering, waiting on the human.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    /// The transaction, exactly as `loom scene --tx` would take it. Stored
    /// whole rather than as a summary, because the thing shown and the thing
    /// applied have to be the same thing.
    pub transaction: serde_json::Value,
    /// What it would do, from a `--dry-run`. Cached so the editor can show it
    /// without re-running anything, and so a proposal is readable in the file.
    pub diff: Vec<String>,
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
                proposal: value.get("tx").map(|transaction| Proposal {
                    transaction: transaction.clone(),
                    diff: value
                        .get("diff")
                        .and_then(serde_json::Value::as_array)
                        .map(|lines| {
                            lines
                                .iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                }),
                decision: value
                    .get("decision")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
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

/// Answer with a change instead of a sentence.
///
/// `transaction` is what `loom scene --tx` would take, and `diff` is what it
/// said under `--dry-run`. Both are stored: the editor shows the diff, and Apply
/// runs the transaction, so what was shown and what runs came from one place.
///
/// # Errors
/// If the line cannot be appended.
pub fn propose(
    scene: &Path,
    id: u64,
    text: &str,
    transaction: &serde_json::Value,
    diff: &[String],
) -> std::io::Result<()> {
    std::fs::create_dir_all(directory(scene))?;
    append(
        &outbox(scene),
        &serde_json::json!({ "id": id, "text": text, "tx": transaction, "diff": diff }),
    )
}

/// Record what the human did with a proposal.
///
/// **A human line, in the human's file.** Accepting a change is something the
/// person did, and putting it anywhere else would make the inbox a partial
/// record of their side.
///
/// # Errors
/// If the line cannot be appended.
pub fn decide(scene: &Path, id: u64, decision: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(directory(scene))?;
    append(
        &inbox(scene),
        &serde_json::json!({ "id": id, "decision": decision }),
    )
}

/// Proposals the human has neither applied nor discarded.
///
/// **What the editor puts buttons under.** A proposal whose decision line is
/// present is history; one without is a question still on the table.
#[must_use]
pub fn undecided(scene: &Path) -> Vec<Message> {
    let decided: std::collections::BTreeSet<u64> = read(&inbox(scene), Speaker::Human)
        .into_iter()
        .filter(|m| m.decision.is_some())
        .map(|m| m.id)
        .collect();
    read(&outbox(scene), Speaker::Agent)
        .into_iter()
        .filter(|m| m.proposal.is_some() && !decided.contains(&m.id))
        .collect()
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
        // A decision line is the human answering the agent, not asking. Left in
        // it would come back forever as a request with no text.
        .filter(|m| m.decision.is_none() && !answered.contains(&m.id))
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

    /// **A change offered, then taken.** What is shown and what is applied come
    /// out of one line, and once decided it stops asking.
    #[test]
    fn a_proposal_waits_until_it_is_decided() {
        let scene = scene("propose");
        let id = ask(&scene, "heavier fog", &[]).expect("ask");
        let tx = serde_json::json!({
            "label": "Fog to 0.012",
            "ops": [{ "op": "set_field", "node": "Rig", "field": "Environment.fog_density", "value": 0.012 }],
        });
        propose(&scene, id, "here is what I would change", &tx, &["-fog_density = 0.0028".to_owned()])
            .expect("propose");

        let waiting = undecided(&scene);
        assert_eq!(waiting.len(), 1, "{waiting:?}");
        let offered = waiting[0].proposal.as_ref().expect("carries a transaction");
        assert_eq!(offered.transaction, tx, "the stored transaction is the one to run");
        assert_eq!(offered.diff.len(), 1);

        decide(&scene, id, "applied").expect("decide");
        assert!(undecided(&scene).is_empty(), "a decided proposal stops asking");
    }

    /// **A decision is not a new request.** It goes in the inbox, which is also
    /// where questions live, so `pending` has to tell them apart or the agent
    /// picks up an empty request and answers it forever.
    #[test]
    fn a_decision_is_not_mistaken_for_a_question() {
        let scene = scene("decision");
        let id = ask(&scene, "heavier fog", &[]).expect("ask");
        propose(&scene, id, "proposed", &serde_json::json!({ "label": "x", "ops": [] }), &[])
            .expect("propose");
        decide(&scene, id, "discarded").expect("decide");

        assert!(pending(&scene).is_empty(), "{:?}", pending(&scene));
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
