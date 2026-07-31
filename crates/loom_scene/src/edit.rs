//! The editing session: selection, transactions, and one shared undo stack.
//!
//! **never-do #16 is the whole design.** The editor does not get its own undo
//! stack. It issues the same [`SceneOp`](crate::SceneOp) transactions the agent
//! does, through the same [`apply`](crate::apply), so a twelve-op agent
//! transaction undoes in one Ctrl+Z and a human edit is indistinguishable from
//! an agent edit in the file.
//!
//! Two undo stacks that disagree about history is a bug class with no clean
//! fix, which is why this type exists at all rather than the viewer keeping its
//! own list of changes.
//!
//! §7.17 rides along: every write carries the version it read. A stale write is
//! rejected and reloaded, never merged — from the editor exactly as from the
//! agent, because it is the same code path.

use crate::{Applied, Transaction, TransactionError, VersionToken, apply};

/// Why a save did not happen.
#[derive(Debug)]
pub enum SaveRejected {
    /// The file moved since this session last read it. Its current contents
    /// come back so the caller can offer both versions — never merge them.
    Stale { current: String },
    Io(std::io::Error),
}

impl std::fmt::Display for SaveRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale { .. } => write!(
                f,
                "the scene changed on disk since it was opened; saving would \
                 overwrite that. Reload, or keep yours and save again."
            ),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SaveRejected {}

/// An open scene being edited.
pub struct Session {
    path: std::path::PathBuf,
    text: String,
    version: VersionToken,
    /// Whole-scene snapshots, one per transaction.
    ///
    /// `ponytail:` snapshots, not structural inverses. A scene is kilobytes,
    /// and "one transaction is one undo step" falls out for free instead of
    /// being a property to maintain per op. Revisit at megabytes.
    undo: Vec<String>,
    redo: Vec<String>,
    /// Labels, newest last — this is what the human's log panel shows.
    history: Vec<String>,
    /// The gesture the last transaction belonged to, if it was part of one.
    gesture: Option<String>,
    /// The version this session last read from, or wrote to, the file.
    ///
    /// `save` compares the file against this before overwriting. Without it an
    /// unconditional `fs::write` destroys whatever landed on disk since — and
    /// the agent writes on a 250 ms watcher tick, so the window is real.
    disk: VersionToken,
}

impl Session {
    /// Open a scene for editing.
    ///
    /// # Errors
    /// [`std::io::Error`] if the file cannot be read.
    pub fn open(path: &std::path::Path) -> Result<Self, std::io::Error> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            version: VersionToken::of(&text),
            disk: VersionToken::of(&text),
            text,
            undo: Vec::new(),
            redo: Vec::new(),
            history: Vec::new(),
            gesture: None,
        })
    }

    /// Open from text, for tests and for callers holding a scene already.
    #[must_use]
    pub fn from_text(path: &std::path::Path, text: String) -> Self {
        Self {
            path: path.to_path_buf(),
            version: VersionToken::of(&text),
            disk: VersionToken::of(&text),
            text,
            undo: Vec::new(),
            redo: Vec::new(),
            history: Vec::new(),
            gesture: None,
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn version(&self) -> &VersionToken {
        &self.version
    }

    /// Transaction labels applied so far, oldest first.
    #[must_use]
    pub fn history(&self) -> &[String] {
        &self.history
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Apply a transaction — the **only** way this session changes.
    ///
    /// The version is filled in from what the session last read, so an editor
    /// cannot accidentally skip the staleness check that the agent path
    /// enforces.
    ///
    /// # Errors
    /// [`TransactionError`] as [`apply`], including `stale_version` when the
    /// file changed underneath.
    pub fn apply(&mut self, transaction: Transaction) -> Result<Applied, Box<TransactionError>> {
        self.gesture = None;
        self.commit(transaction)
    }

    /// Apply as one frame of a continuing gesture — a gizmo drag, a slider
    /// being scrubbed.
    ///
    /// Such a gesture produces a transaction per frame. Kept as written that
    /// is a thousand undo steps for one movement of the hand, and Ctrl+Z stops
    /// meaning anything. Frames sharing a `gesture` key collapse into a single
    /// entry that undoes the whole movement, which is what never-do #16 asks
    /// for at the scale of one gesture rather than one op.
    ///
    /// Any ordinary [`apply`](Self::apply) in between ends the run, so an
    /// agent write landing mid-drag cannot be swallowed into the human's undo
    /// entry and lost on one keystroke.
    ///
    /// # Errors
    /// As [`apply`](Self::apply).
    pub fn apply_coalescing(
        &mut self,
        transaction: Transaction,
        gesture: &str,
    ) -> Result<Applied, Box<TransactionError>> {
        let continuing = self.gesture.as_deref() == Some(gesture);
        let applied = self.commit(transaction)?;
        if continuing {
            // Drop what this frame added, keeping the snapshot from the frame
            // that began the gesture — so one undo rewinds the whole thing.
            self.undo.pop();
            self.history.pop();
        }
        self.gesture = Some(gesture.to_owned());
        Ok(applied)
    }

    fn commit(&mut self, mut transaction: Transaction) -> Result<Applied, Box<TransactionError>> {
        transaction.expect_version = Some(self.version.clone());
        let applied = apply(&self.text, &transaction)?;

        self.undo.push(applied.undo.clone());
        // A new edit invalidates the redo branch. Keeping it would let a user
        // redo their way into a history that never happened.
        self.redo.clear();
        self.history.push(applied.label.clone());
        self.text = applied.scene.clone();
        self.version = applied.version.clone();
        Ok(applied)
    }

    /// Step back one **transaction**, however many ops it held.
    pub fn undo(&mut self) -> bool {
        self.gesture = None;
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.text, previous));
        self.version = VersionToken::of(&self.text);
        self.history.pop();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.gesture = None;
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.text, next));
        self.version = VersionToken::of(&self.text);
        true
    }

    /// Write to disk.
    ///
    /// # Errors
    /// [`std::io::Error`] if the file cannot be written.
    pub fn save(&mut self) -> Result<(), SaveRejected> {
        // Re-read before writing. §7.17 is about the agent's writes being
        // rejected rather than merged; an unconditional `fs::write` from the
        // editor is the same collision with the roles reversed, and it was
        // reachable from Ctrl+S while the conflict banner was on screen.
        //
        // A missing file is not a conflict — it means the scene was moved or
        // deleted, and writing it back is the useful thing to do.
        if let Ok(on_disk) = std::fs::read_to_string(&self.path) {
            let current = VersionToken::of(&on_disk);
            if current != self.disk {
                return Err(SaveRejected::Stale { current: on_disk });
            }
        }
        std::fs::write(&self.path, &self.text).map_err(SaveRejected::Io)?;
        self.disk = self.version.clone();
        Ok(())
    }

    /// Re-read from disk after a rejected write.
    ///
    /// The **only** correct response to `stale_version` (§7.17). Never force
    /// the write, never merge the two versions — a silent merge produces
    /// something neither party intended and is worse than a rejection.
    ///
    /// # Errors
    /// [`std::io::Error`] if the file cannot be read.
    pub fn reload(&mut self) -> Result<(), std::io::Error> {
        self.text = std::fs::read_to_string(&self.path)?;
        self.version = VersionToken::of(&self.text);
        self.disk = self.version.clone();
        // History is about *this* session's transactions; the file moving under
        // us invalidates the undo chain, and offering it anyway would let a
        // user undo their way onto someone else's work.
        self.undo.clear();
        self.redo.clear();
        self.gesture = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SceneOp;

    const SCENE: &str = "\
# A human's comment, which no edit may destroy.
[scene]
format = 1
id = \"3c7e1f88-9a05-4b21-bd6e-51f0a2c48d13\"

[[node]]
name = \"Room\"
";

    fn session() -> Session {
        Session::from_text(std::path::Path::new("/tmp/loom_edit_test.loom"), SCENE.to_owned())
    }

    fn move_to(x: f32) -> Transaction {
        Transaction {
            label: "Move Room".into(),
            ops: vec![SceneOp::SetTransform {
                node: "Room".into(),
                pos: Some([x, 0.0, 0.0]),
                rot_euler: None,
                scale: None,
            }],
            dry_run: false,
            expect_version: None,
        }
    }

    fn spawn(name: &str) -> Transaction {
        Transaction {
            label: format!("Add {name}"),
            ops: vec![SceneOp::SpawnNode {
                parent: "Room".into(),
                name: name.into(),
                mesh: Some("box".into()),
            }],
            dry_run: false,
            expect_version: None,
        }
    }

    /// **never-do #16.** Twelve ops in one transaction undo in one step.
    #[test]
    fn a_twelve_op_transaction_undoes_in_one_step() {
        let mut session = session();
        let ops: Vec<SceneOp> = (0..12)
            .map(|i| SceneOp::SpawnNode {
                parent: "Room".into(),
                name: format!("Box{i}"),
                mesh: Some("box".into()),
            })
            .collect();
        session
            .apply(Transaction {
                label: "Block out: 12 nodes".into(),
                ops,
                dry_run: false,
                expect_version: None,
            })
            .expect("should apply");
        assert_eq!(session.text().matches("[[node]]").count(), 13);

        assert!(session.undo(), "one undo");
        assert_eq!(session.text(), SCENE, "all twelve gone together");
    }

    /// A human edit and an agent edit go through the same call, so the file
    /// cannot tell them apart — which is the property M12's exit criterion
    /// actually asks for.
    #[test]
    fn an_editor_edit_is_identical_to_an_agent_edit() {
        let mut editor = session();
        editor.apply(spawn("Lamp")).expect("editor applies");

        // The "agent" path: the free function, with no session at all.
        let agent = crate::apply(SCENE, &spawn("Lamp")).expect("agent applies");

        assert_eq!(editor.text(), agent.scene, "same bytes, same path");
    }

    /// A drag produces a transaction per frame. Left alone that is a thousand
    /// undo steps for one gesture, and Ctrl+Z stops meaning anything — so
    /// frames of the same gesture fold into one entry.
    #[test]
    fn a_dragged_gesture_is_one_undo_step() {
        let mut session = session();
        for i in 1..=50 {
            session
                .apply_coalescing(move_to(i as f32), "drag:Room")
                .expect("applies");
        }

        assert_eq!(session.history().len(), 1, "one gesture, one entry");
        assert!(session.undo(), "one undo");
        assert_eq!(session.text(), SCENE, "and the whole drag is gone");
        assert!(!session.can_undo());
    }

    /// A different gesture must not be folded into the previous one.
    #[test]
    fn two_gestures_stay_two_undo_steps() {
        let mut session = session();
        session.apply_coalescing(move_to(1.0), "drag:Room").unwrap();
        session.apply_coalescing(move_to(2.0), "drag:Room").unwrap();
        session.apply_coalescing(move_to(3.0), "drag:Other").unwrap();

        assert_eq!(session.history().len(), 2);
    }

    /// An ordinary transaction between two frames of the same key ends the
    /// gesture — otherwise an agent write landing mid-drag would be swallowed
    /// into the human's undo entry and lost on one Ctrl+Z.
    #[test]
    fn an_uncoalesced_transaction_breaks_the_run() {
        let mut session = session();
        session.apply_coalescing(move_to(1.0), "drag:Room").unwrap();
        session.apply(spawn("Lamp")).unwrap();
        session.apply_coalescing(move_to(2.0), "drag:Room").unwrap();

        assert_eq!(session.history().len(), 3);
    }

    /// **Ctrl+S must not be a data-loss button.** `save` was an unconditional
    /// `fs::write`, so anything the agent landed since the editor opened the
    /// file was gone — silently, with a "saved" message. §7.17 rejects the
    /// agent's stale writes; this is the same collision with the roles
    /// reversed, and it was reachable even while the conflict banner was up.
    #[test]
    fn saving_over_a_file_that_moved_is_refused() {
        let dir = std::env::temp_dir().join("loom_save_conflict");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("scene.loom");
        std::fs::write(&path, SCENE).expect("write");

        let mut session = Session::open(&path).expect("open");
        session.apply(spawn("Lamp")).expect("edit applies");

        // Somebody else — the agent — writes the file underneath.
        std::fs::write(&path, format!("{SCENE}\n# the agent was here\n")).expect("write");

        let err = session.save().expect_err("must be refused");
        assert!(matches!(err, crate::SaveRejected::Stale { .. }));
        assert!(
            std::fs::read_to_string(&path).expect("read").contains("the agent was here"),
            "the other version must still be on disk"
        );

        // After taking their version, saving is allowed again.
        session.reload().expect("reload");
        session.save().expect("saving is fine once in sync");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_and_redo_walk_the_history() {
        let mut session = session();
        session.apply(spawn("A")).unwrap();
        session.apply(spawn("B")).unwrap();
        assert_eq!(session.history(), ["Add A", "Add B"]);

        assert!(session.undo());
        assert!(session.text().contains("\"A\"") && !session.text().contains("\"B\""));
        assert!(session.redo());
        assert!(session.text().contains("\"B\""));
    }

    /// A new edit after undoing must discard the redo branch, or a user can
    /// redo into a history that never happened.
    #[test]
    fn editing_after_undo_discards_the_redo_branch() {
        let mut session = session();
        session.apply(spawn("A")).unwrap();
        session.undo();
        session.apply(spawn("B")).unwrap();

        assert!(!session.can_redo(), "the A branch is gone");
        assert!(session.text().contains("\"B\""));
    }

    /// §7.17 from the editor side. The session supplies its own version, so an
    /// editor cannot skip the check the agent path enforces.
    #[test]
    fn a_write_against_a_moved_file_is_rejected() {
        let mut session = session();
        // Something else changed the scene underneath.
        session.version = VersionToken("stale".into());

        let err = session.apply(spawn("Late")).expect_err("must be refused");

        assert_eq!(err.error, "stale_version");
        assert!(err.hint.as_ref().unwrap().contains("never merge"));
        assert_eq!(session.text(), SCENE, "and nothing changed");
    }

    #[test]
    fn edits_preserve_the_humans_comment() {
        let mut session = session();
        session.apply(spawn("Lamp")).unwrap();

        assert!(session.text().contains("A human's comment"));
    }

    #[test]
    fn undo_on_an_empty_history_is_a_no_op() {
        let mut session = session();

        assert!(!session.undo());
        assert_eq!(session.text(), SCENE);
    }
}
