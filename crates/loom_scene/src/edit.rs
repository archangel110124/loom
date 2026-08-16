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

use crate::{Applied, Transaction, TransactionError, VersionToken, apply, ops::apply_with};

/// Write a scene file so nobody can ever read a half-written one.
///
/// `std::fs::write` truncates and then writes, which leaves two windows where
/// the file on disk is neither the old scene nor the new one: a reader in
/// between sees a truncated file, and a crash in between leaves it that way
/// permanently. This project's editor re-reads the scene four times a second
/// and the agent writes it from another process, so that window is not
/// theoretical — and the thing at stake is the human's authored work, which
/// never-do #15 makes the highest-severity loss in the codebase.
///
/// Write beside it, flush, then rename. `rename` within a directory is atomic
/// on POSIX: a reader sees the old file or the new one, never a partial. The
/// temporary name carries the process id so two `loom` processes writing the
/// same scene cannot clobber each other's intermediate file.
///
/// # Errors
/// [`std::io::Error`] if the temporary cannot be written or the rename fails.
/// The original is untouched in both cases.
pub fn write_atomically(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    let directory = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let name = path
        .file_name()
        .map_or_else(|| "scene.loom".into(), std::ffi::OsStr::to_string_lossy);
    // Same directory, because `rename` is only atomic within one filesystem.
    //
    // The pid separates processes; the counter separates threads within one.
    // Without the counter two threads pick the same temporary name, and the
    // first rename moves the file out from under the second — which fails with
    // ENOENT after having already reported nothing wrong.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = directory.join(format!(".{name}.{}.{unique}.tmp", std::process::id()));

    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        // Durability before visibility: without this the rename can land while
        // the contents are still only in the page cache, so a crash leaves an
        // empty file where a complete scene used to be.
        file.sync_all()
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&temporary);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(e);
    }
    Ok(())
}

/// Apply a transaction to a scene file: read, apply, write, as one step.
///
/// **The version-token check is only as good as the window it covers.** Every
/// writer used to read the whole scene, compare the token against that
/// in-memory snapshot, and then write unconditionally — with nothing held
/// across the three steps. Two processes interleaving their reads both wrote,
/// the second silently erased the first, and both reported `ok: true`. The
/// editor polls four times a second while the agent writes from another
/// process, so that interleaving is the ordinary case.
///
/// Holding an exclusive lock across the whole cycle is what makes
/// `expect_version` mean anything: the token is now compared against a scene
/// that cannot change before the write lands.
///
/// # Errors
/// [`FileApplyError::Io`] if the file cannot be read, locked or written;
/// [`FileApplyError::Rejected`] if the transaction is invalid or stale.
pub fn apply_to_file(
    path: &std::path::Path,
    transaction: &Transaction,
) -> Result<Applied, FileApplyError> {
    let _guard = lock_scene(path).map_err(FileApplyError::Io)?;

    let source = std::fs::read_to_string(path).map_err(FileApplyError::Io)?;

    // **Prefabs are loaded inside the lock, from the scene as it is now.**
    // `UnpackPrefab` needs to know what the instance stood for, and reading
    // the declarations from a copy taken before the lock would let the file
    // move underneath. Every other op ignores the library.
    let library = crate::Scene::parse(&source)
        .ok()
        .and_then(|scene| crate::prefab::library_for(&scene, path).ok())
        .unwrap_or_default();
    let applied = apply_with(&source, transaction, &library).map_err(FileApplyError::Rejected)?;
    if !transaction.dry_run {
        write_atomically(path, &applied.scene).map_err(FileApplyError::Io)?;
    }
    Ok(applied)
}

/// Take the exclusive lock guarding one scene file. Released on drop.
///
/// The lock lives on a sidecar rather than the scene itself, because
/// [`write_atomically`] renames a new file over the target: a lock held on the
/// scene's inode protects an inode that is about to stop being the scene, and
/// the next process to open the path would get a different inode and lock
/// that instead. The sidecar is never renamed, so everyone contends for the
/// same object.
///
/// # Errors
/// [`std::io::Error`] if the lock file cannot be created or locked.
fn lock_scene(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let directory = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let name = path
        .file_name()
        .map_or_else(|| "scene.loom".into(), std::ffi::OsStr::to_string_lossy);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(directory.join(format!(".{name}.lock")))?;
    file.lock()?;
    Ok(file)
}

/// Why applying a transaction to a file did not happen.
#[derive(Debug)]
pub enum FileApplyError {
    Io(std::io::Error),
    Rejected(Box<TransactionError>),
}

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
        // Held across the re-read and the write, for the same reason
        // `apply_to_file` holds it: checking a token against a snapshot and
        // then writing is only a check if nothing can land in between. An
        // agent write arriving in that window was overwritten and `save`
        // returned Ok — the staleness check below is precisely what is meant
        // to stop that.
        let _guard = lock_scene(&self.path).map_err(SaveRejected::Io)?;

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
        write_atomically(&self.path, &self.text).map_err(SaveRejected::Io)?;
        self.disk = self.version.clone();
        Ok(())
    }

    /// Accept the file's current state as this session's baseline **without**
    /// taking its contents.
    ///
    /// The one legitimate way past a stale-save refusal, and it exists because
    /// making `save` conditional otherwise turned the editor's "Keep mine"
    /// button into a dead end: every subsequent save was refused, and the only
    /// escape offered was the one that discards the human's work.
    ///
    /// This is not a violation of never-do #15. That rule forbids *silent*
    /// force-writes against a stale token and automatic merges. Here the human
    /// has been shown both versions and has explicitly chosen one; nothing is
    /// merged and nothing is silent.
    ///
    /// # Errors
    /// [`std::io::Error`] if the file cannot be read.
    pub fn accept_disk_version(&mut self) -> Result<(), std::io::Error> {
        let on_disk = std::fs::read_to_string(&self.path)?;
        self.disk = VersionToken::of(&on_disk);
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

    /// Refusing a stale save must not trap the human. Making `save` conditional
    /// fixed a data-loss bug and created a dead end: "Keep mine" left the
    /// session's baseline stale, so every later save was refused and the only
    /// way out discarded the very edits the human had chosen to keep.
    #[test]
    fn keeping_your_version_lets_you_save_it() {
        let dir = std::env::temp_dir().join("loom_keep_mine");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("scene.loom");
        std::fs::write(&path, SCENE).expect("write");

        let mut session = Session::open(&path).expect("open");
        session.apply(spawn("Lamp")).expect("edit applies");
        std::fs::write(&path, format!("{SCENE}\n# the agent was here\n")).expect("write");

        assert!(session.save().is_err(), "still refused before the human chooses");

        // The human looks at both versions and keeps theirs.
        session.accept_disk_version().expect("acknowledge");
        session.save().expect("their choice must be writable");

        assert!(
            std::fs::read_to_string(&path).expect("read").contains("\"Lamp\""),
            "the kept version reached disk"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Two writers must not lose each other's work.** Every writer read the
    /// whole scene, checked the version token against that in-memory snapshot,
    /// and then wrote unconditionally. Nothing held the file across those three
    /// steps, so two processes interleaving their reads both wrote, the second
    /// silently erased the first, and both reported `ok: true`.
    ///
    /// The editor polls four times a second while the agent writes from another
    /// process, so this is the ordinary case, not a rare one. never-do #15
    /// makes losing the human's authored work the worst bug class here.
    #[test]
    fn concurrent_writers_do_not_lose_each_others_nodes() {
        let dir = std::env::temp_dir().join("loom_concurrent_apply");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("scene.loom");
        super::write_atomically(&path, SCENE).expect("seed");

        const WRITERS: usize = 8;
        let threads: Vec<_> = (0..WRITERS)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let transaction = Transaction {
                        label: format!("writer {i}"),
                        ops: vec![crate::SceneOp::SpawnNode {
                            parent: "Room".to_owned(),
                            name: format!("N{i}"),
                            mesh: None,
                        }],
                        dry_run: false,
                        expect_version: None,
                    };
                    super::apply_to_file(&path, &transaction)
                })
            })
            .collect();
        for t in threads {
            t.join().expect("writer thread").expect("write should succeed");
        }

        let final_text = std::fs::read_to_string(&path).expect("read back");
        let _ = std::fs::remove_dir_all(&dir);

        let missing: Vec<usize> = (0..WRITERS)
            .filter(|i| !final_text.contains(&format!("\"N{i}\"")))
            .collect();
        assert!(missing.is_empty(), "these writers were silently erased: {missing:?}");
    }

    /// **The editor's save had the same TOCTOU the CLI's writes did.** `save`
    /// re-reads, compares the token, then writes — with nothing held across
    /// the three steps. So an agent write landing between the check and the
    /// write is overwritten, and `save` returns `Ok`: the staleness check it
    /// performs is exactly the check that is supposed to prevent that.
    ///
    /// The invariant: if a save and an agent transaction both succeed, the
    /// only legal ordering is save-then-apply, because after the apply the
    /// session's baseline is stale and the save must be refused. So the
    /// spawned node must be present at the end. If it is missing, a successful
    /// save silently erased a successful transaction — never-do #15.
    #[test]
    fn a_save_racing_an_agent_write_cannot_erase_it() {
        let dir = std::env::temp_dir().join("loom_save_race");
        std::fs::create_dir_all(&dir).expect("temp dir");

        for round in 0..64 {
            let path = dir.join(format!("scene{round}.loom"));
            super::write_atomically(&path, SCENE).expect("seed");

            let mut session = Session::open(&path).expect("open");
            session
                .apply(Transaction {
                    label: "the human's edit".to_owned(),
                    ops: vec![SceneOp::SetTransform {
                        node: "Room".to_owned(),
                        pos: Some([1.0, 0.0, 0.0]),
                        rot_euler: None,
                        scale: None,
                    }],
                    dry_run: false,
                    expect_version: None,
                })
                .expect("edit");

            let agent = {
                let path = path.clone();
                std::thread::spawn(move || {
                    super::apply_to_file(
                        &path,
                        &Transaction {
                            label: "the agent's spawn".to_owned(),
                            ops: vec![SceneOp::SpawnNode {
                                parent: "Room".to_owned(),
                                name: "AgentNode".to_owned(),
                                mesh: None,
                            }],
                            dry_run: false,
                            expect_version: None,
                        },
                    )
                    .is_ok()
                })
            };
            let saved = session.save().is_ok();
            let applied = agent.join().expect("agent thread");

            if saved && applied {
                let text = std::fs::read_to_string(&path).expect("read back");
                assert!(
                    text.contains("AgentNode"),
                    "round {round}: a successful save erased a successful transaction"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A reader must never see a half-written scene.** `fs::write` truncates
    /// and then writes, so a reader in between sees a prefix of the new text
    /// and a crash in between leaves the file that way. The editor re-reads the
    /// scene four times a second while the agent writes it from another
    /// process, so that window is real — and what is at stake is the human's
    /// authored work.
    ///
    /// The assertion is byte equality with one of the two known texts, not
    /// "it parses". A truncated scene very often still parses — my first
    /// version of this test padded with comment lines, so every torn read was
    /// a valid scene and the test passed against `fs::write` too.
    #[test]
    fn a_concurrent_reader_never_sees_a_partial_scene() {
        let dir = std::env::temp_dir().join("loom_atomic_write");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("scene.loom");

        // Big enough that a truncate-then-write is not instantaneous.
        let long = format!("{SCENE}{}", "\n# padding so the write takes a while".repeat(20_000));
        std::fs::write(&path, &long).expect("seed");

        let reading = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let torn = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reader = {
            let (path, reading, torn, long) =
                (path.clone(), reading.clone(), torn.clone(), long.clone());
            std::thread::spawn(move || {
                while reading.load(std::sync::atomic::Ordering::Relaxed) {
                    if let Ok(seen) = std::fs::read_to_string(&path)
                        && seen != SCENE
                        && seen != long
                    {
                        torn.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            })
        };

        for _ in 0..20 {
            super::write_atomically(&path, SCENE).expect("write");
            super::write_atomically(&path, &long).expect("write");
        }
        reading.store(false, std::sync::atomic::Ordering::Relaxed);
        reader.join().expect("reader");

        let torn = torn.load(std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(torn, 0, "a reader saw a scene that was neither version");
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
