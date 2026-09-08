# ADR 0101 — A control-by-control audit

- **Date:** 2026-09-08
- **Status:** **accepted**. Seven defects found and fixed.
- **Human decision this records:** *"every function in the editor works. Look at
  every button, tab, selector and agent integration."*

## 1. Method

104 interactive controls across 19 panel functions, 40 `UiAction` verbs, 11 tabs.
Four passes, cheapest first:

1. **Structural.** Diff what the UI can *emit* against what the app *handles*.
   40 declared, 40 handled, 39 emitted — one dead.
2. **Enablement.** Every `add_enabled` condition, looking for controls that can
   never be reached. Two permanent disables, both intentional and explained.
3. **Staleness.** Every `PanelState` field, asking whether it is read live or
   cached — and if cached, whether anything fills it on open.
4. **Behavioural.** Apply every op the editor can emit to a real scene and
   validate the result; screenshot every tab against a scene that should give
   it something to show.

The fourth pass found what the first three could not, and the third pass found
what no screenshot would have shown on a scene the human had already edited.

## 2. The seven

| # | Control | What it did |
|---|---|---|
| 1 | Rename | `BeginRename` declared, handler `=> {}`, emitted by nothing |
| 2 | F / Focus | Framed the whole scene, not the selection |
| 3 | Agent marks | Drew nothing for the nodes an agent most often edits |
| 4 | `+ add` on `stages` | Refused every time, on every scene |
| 5 | Problems tab | "nothing to report" about a scene `loom validate` rejects |
| 6 | Project tab | Listed no scenes |
| 7 | Prefabs tab | "this scene declares no prefabs" about a scene declaring five |

## 3. Three patterns, not seven accidents

**`node_bounds` was the wrong question, asked three times** (2, 3, and the gizmo
last round). It answers "what does this node draw". The gizmo, the focus key and
the agent marks all wanted "what does this node *and everything under it* draw".
A rig node — `Rig/Boat` is the whole boat — draws nothing, so all three silently
received `None` and did nothing visible.

After the last caller moved to `subtree_bounds`, the compiler reported the
method dead. That is how I know every site is converted, and it is why the
method is now deleted rather than left with a warning comment: a name that
answers a subtly wrong question will be reached for again.

**Derived state computed only on change is empty on open** (5, 6). `problems`
and the scene list were filled in `show`, which runs when the scene *changes*.
A freshly opened editor therefore knew neither. The same shape had already
appeared once, in the `--select` flag, and was fixed there and nowhere else.

**A comment can say the opposite of the code beside it** (7, and the play-mode
gate the round before). ADR 0093 states plainly that the Prefabs panel must read
the *unresolved* file, because `prefab_load::for_reading` replaces an instance
with the subtree it stood for. The panel then asked `state.scene` — the resolved
one — and got nothing, on every scene, for as long as it had existed.

Both times the prose was right and unread. A comment is not a test.

## 4. What was checked and found correct

Recorded because "I looked and it was fine" is worth as much as a fix, and
because each of these looks like a bug from one angle:

- `SpliceArray` refusing an out-of-order stage — the validator working.
- `assign_mesh` doing nothing with an empty selection — its button is disabled
  then.
- `rename` returning early when the name is unchanged.
- Two permanently disabled controls: voxel meshes, which are baked per node and
  not assignable, and asset drag sources during Play, which would silently
  no-op. Both carry a disabled-hover explaining themselves.
- The transport swapping Play for Pause/Step/Stop, with the structural verbs
  greying out and Focus staying live — confirmed in a screenshot with `--play`.

## 5. What is verified by chain rather than by eye

The History and Transactions panels are empty in a freshly opened editor, which
is correct — there are no edits yet — and there is no headless way to make an
in-editor edit, so no screenshot can show them populated.

They are covered instead by: the panels render given content
(`every_tab_draws_with_content` passes a history and a redo stack); the data is
read live from `Session::history()` and `redo_labels()` rather than cached; and
`Session` growing its history on apply is tested in `loom_scene`. That chain is
sound, and it is weaker than looking, which is why it is written down.

The conflict banner is the same: it appears only when the file changes on disk
under unsaved edits, which two racing processes can produce and one screenshot
cannot. It now has a draw test, because its two buttons discard one version or
the other and that is the worst thing in this editor to find broken.
