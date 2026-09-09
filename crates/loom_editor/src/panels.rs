//! The editor's panels: toolbar, hierarchy, inspector, assets, console, and
//! the gizmo drawn over the viewport.
//!
//! **The inspector is generated from the type registry**, not hand-written per
//! component. That was always the point of the registry (design doc §2.1) and
//! it is why M5.5 called the viewer cheap: a new component type gets an
//! inspector for free, with its ranges enforced and its doc comment as the
//! tooltip, the same way it gets a schema and a CLI `describe` for free.
//!
//! A hand-written inspector is a second description of every type, and it
//! drifts out of sync by Thursday.
//!
//! Nothing here mutates anything. Panels return [`UiAction`]s and the caller
//! turns them into transactions, so the editor cannot grow a second write path
//! that skips the version check.

use loom_render::egui;

use crate::gizmo::{Handle, Mode};

/// What a panel interaction asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
    Select { path: String, extend: bool },
    /// `node`, `Type.field`, new value.
    SetField(String, String, serde_json::Value),
    /// `node`, `Type.field`, index, how many to remove, what to insert.
    ///
    /// The inspector's only way to change an array of objects. It is not
    /// [`UiAction::SetField`] with a rewritten array because that re-emits the
    /// whole list inline and takes the human's comments and formatting with
    /// it — see `SceneOp::SpliceArray`.
    Splice(String, String, usize, usize, Vec<serde_json::Value>),
    /// `node`, `Type.field` — put one overridden field back to the prefab.
    ///
    /// Empty `field` reverts the whole instance.
    RevertOverride(String, String),
    SetMode(Mode),
    /// Write the running game to a save file — ADR 0098. Play only.
    SaveGame,
    /// Read one back into the running game — ADR 0098. Play only.
    LoadGame,
    /// Give the selection a new name from the hierarchy, without the inspector.
    BeginRename(String),
    /// Fold a node's children away — ADR 0098.
    ToggleCollapsed(String),
    /// Narrow the hierarchy to nodes matching this text — ADR 0093.
    SetFilter(String),
    /// Choose what the viewport draws — ADR 0097.
    SetViewMode(loom_render::ablate::ViewMode),
    /// Turn increment snapping on or off — ADR 0093.
    SetSnap(bool),
    /// Change the increment for the mode currently selected.
    SetSnapStep(Mode, f32),
    /// Frame the selection.
    Focus,
    AddChild(String),
    /// Instance a prefab by its file-local alias, under the selection — ADR 0093.
    AddPrefabInstance(String),
    /// Open a different scene file — ADR 0093. Refused while there are
    /// unsaved edits.
    OpenScene(String),
    /// Ask the agent for something — ADR 0100.
    SendToAgent(String),
    /// Apply or throw away a change the agent offered rather than made —
    /// ADR 0103. Apply runs the stored transaction through the ordinary op
    /// path, so it is one History entry and one Ctrl+Z like any other edit.
    DecideProposal { id: u64, apply: bool },
    /// Drop an asset into the viewport at these window pixels — ADR 0099.
    ///
    /// The alias, and where the cursor let go. The caller turns the point into
    /// a place in the world; a panel has no camera.
    DropAsset { alias: String, at: (f32, f32) },
    /// Start a new scene beside this one, named by the editor — ADR 0099.
    NewScene,
    /// Write the scene to another name beside this one — ADR 0099.
    SaveAs(String),
    /// Create a node that draws something, in one step — ADR 0093.
    ///
    /// The mesh alias, which doubles as the name: `box`, `sphere`, `plane`,
    /// `cylinder`, `capsule`.
    CreatePrimitive(String),
    /// Put the selection on the clipboard, subtrees included — ADR 0093.
    Copy,
    /// Paste the clipboard under the selection — ADR 0093.
    Paste,
    Duplicate,
    Delete,
    /// Point the selection's `MeshRenderer` at an asset alias.
    AssignMesh(String),
    Undo,
    Redo,
    Save,
    /// Resolve a disk conflict, one way or the other.
    ReloadFromDisk,
    KeepMine,
    ClearLog,
    /// Give a node a new name.
    Rename(String, String),
    /// Add a component of this type to the selection, at its defaults.
    AddComponent(String),
    RemoveComponent(String, String),
    /// Remember a component's values, to write onto another node — ADR 0107.
    CopyComponent(String, String),
    /// Write the remembered component onto this node.
    PasteComponent(String),
    /// Move `node` under `parent`.
    Reparent { node: String, parent: String },
    Play,
    /// Toggle pause while playing.
    Pause,
    /// One tick, whether paused or not.
    StepOnce,
    Stop,
}

/// Whether a *value* can be edited — ADR 0099.
///
/// **True while playing.** Tuning a number and watching the result is the loop
/// the editor existed without: the edit goes to the scene, `Play` is rebuilt
/// from it, and the simulation is restored from the ADR 0088 snapshot, so the
/// boat keeps its position and velocity and obeys the new number next tick.
///
/// This was written as a comment next to a gate that said the opposite, and
/// shipped twice as done. Hence a named function and a test.
#[must_use]
pub fn fields_editable(state: &PanelState<'_>) -> bool {
    state.editable
}

/// Whether the *shape* of the scene can be changed — ADR 0099.
///
/// **False while playing**, and this is the half that was backwards. Adding,
/// deleting or reparenting a node during a run rebuilds the world underneath a
/// simulation that is holding paths into it; the snapshot restore then puts
/// back a world that no longer matches. Renaming is structural for the same
/// reason.
#[must_use]
pub fn structure_editable(state: &PanelState<'_>) -> bool {
    state.editable && state.playing.is_none()
}

/// One prefab a scene declares, and who instances it — ADR 0101.
///
/// **Read from the unresolved file by the caller.** `prefab_load::for_reading`
/// replaces an instance with the subtree it stood for, so the scene the
/// inspector reads has no prefabs in it at all — the panel asked it anyway and
/// said "this scene declares no prefabs" about a scene declaring five.
#[derive(Debug, Clone, PartialEq)]
pub struct PrefabRow {
    pub key: String,
    pub path: String,
    pub id: String,
    /// Paths of the nodes that instance it.
    pub instances: Vec<String>,
}

/// A line in window pixels: where it starts, where it ends.
pub type Segment = ((f32, f32), (f32, f32));

/// One thing wrong with the scene — ADR 0093.
///
/// **The editor reports what `loom validate` reports**, from the same sources:
/// a prefab override that points at nothing, an asset alias that resolves to
/// nothing, a voxel op that will not parse. Until now all three were reachable
/// only from a terminal, so a scene was validated by leaving the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// True for something that stops the scene working, false for a warning.
    pub blocking: bool,
    /// The node it is about, for the click-to-select.
    pub node: String,
    /// One line, in the language of the file.
    pub message: String,
}

/// One turn of the conversation with the agent — ADR 0100.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentTurn {
    /// False when the human said it.
    pub from_agent: bool,
    pub text: String,
    /// What was selected when it was asked.
    pub about: Vec<String>,
}

/// One frame's measured cost — ADR 0106.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameCost {
    pub cpu_ms: f32,
    pub sim_ms: f32,
    pub draw_ms: f32,
    pub total_ms: f32,
}

/// A change the agent is offering, waiting on a human — ADR 0103.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentProposal {
    /// The id of the request it answers, and the handle Apply sends back.
    pub id: u64,
    /// What the agent said about it.
    pub text: String,
    /// What it would do, from a dry-run. Shown verbatim: a summary of a diff is
    /// a second thing to get wrong, and the human is being asked to approve the
    /// diff, not a description of it.
    pub diff: Vec<String>,
}

/// One edit somebody else made to the scene — ADR 0093.
///
/// **The marks over the viewport fade after six seconds**, which is right for
/// an overlay and useless as a record. An agent that edited eleven nodes while
/// the human was reading the inspector left nothing behind to review. This is
/// that record.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentEdit {
    pub node: String,
    /// Added, Removed, Moved, Edited — the same coarse kinds the marks use.
    pub kind: String,
    pub seconds_ago: f32,
}

/// State the panels need that is not in the scene.
///
/// **The five scene fields are borrowed individually rather than as a
/// `SceneView`.** `SceneView` lives in `loom_cli` and is welded to the asset
/// pipeline — `MeshLibrary`, `VoxelCache`, `world_to_objects`, `scatter_objects`
/// — and `loom_cli` depends on *this* crate, so borrowing it here would invert
/// the edge. What the panels actually read is five plain values, every one of
/// them a `loom_scene` or `loom_render` type, so naming them costs four lines
/// and needs no trait (never-do #12) and no move of the model layer.
pub struct PanelState<'a> {
    /// The parsed scene, for the inspector's component tables.
    pub scene: &'a loom_scene::Scene,
    /// Every node path, in hierarchy order.
    pub paths: &'a [String],
    /// Which paths can be picked in the viewport — a node with no bounds
    /// draws nothing, and the hierarchy marks it.
    pub picks: &'a std::collections::BTreeMap<String, loom_scene::place::Bounds>,
    /// Asset aliases, for the asset panel and the mesh picker.
    pub assets: &'a [String],
    /// How many draw calls the scene resolved to — a status-bar count only.
    pub object_count: usize,
    pub selected: &'a [String],
    pub history: &'a [String],
    pub can_undo: bool,
    pub can_redo: bool,
    pub dirty: bool,
    /// The file moved under unsaved edits and the human has not chosen yet.
    pub conflict: bool,
    /// False in read-only mode: the panels show, but nothing offers to edit.
    pub editable: bool,
    pub registry: &'a loom_reflect::TypeRegistry,
    pub mode: Mode,
    /// Increment snapping, so the toolbar can show and change it — ADR 0093.
    pub snap: crate::gizmo::Snap,
    /// What the hierarchy is filtered to. Empty shows everything.
    pub filter: &'a str,
    /// Nodes whose children are folded away — ADR 0098.
    pub collapsed: &'a std::collections::BTreeSet<String>,
    /// The node being renamed in place, if any — ADR 0100.
    pub renaming: Option<&'a str>,
    /// What the viewport is drawing — ADR 0097.
    pub view_mode: loom_render::ablate::ViewMode,
    /// What is wrong with the scene, recomputed when it changes — ADR 0093.
    pub problems: &'a [Problem],
    /// The prefabs this scene declares, from the unresolved file — ADR 0101.
    pub prefabs: &'a [PrefabRow],
    /// Scene files found beside this one, for the Project panel — ADR 0093.
    pub scenes: &'a [String],
    /// Which of `scenes` is open.
    pub open_scene: &'a str,
    /// What somebody else changed, newest first, kept after the marks fade.
    pub agent_log: &'a [AgentEdit],
    /// The conversation with the agent, oldest first — ADR 0100.
    pub agent_chat: &'a [AgentTurn],
    /// True while a request of ours has no answer yet.
    pub agent_busy: bool,
    /// Changes offered and not yet decided, oldest first — ADR 0103.
    pub agent_proposals: &'a [AgentProposal],
    /// The last few seconds of frames, oldest first — ADR 0106.
    pub frames: &'a [FrameCost],
    /// The type name of the copied component, if there is one — ADR 0107.
    pub copied_component: Option<&'a str>,
    /// Labels of transactions that were undone and can be redone, newest last.
    pub redo_history: &'a [String],
    /// The selection's bounding box, as screen-space edges — ADR 0099.
    ///
    /// **Projected by the caller**, because the projection lives with the
    /// camera in `loom_cli` and this crate stays a pure function of what it is
    /// handed. Twelve segments per selected node, in window pixels.
    pub selection_edges: &'a [Segment],
    /// Rotation rings in window pixels, drawn in Rotate mode — ADR 0099.
    pub rings: &'a [(usize, Vec<(f32, f32)>)],
    /// Plane quads, drawn in Move mode — ADR 0099.
    pub planes: &'a [crate::gizmo::Plane],
    /// Gizmo handles in **window pixels**, as the viewport computed them.
    pub handles: &'a [Handle],
    /// The **axis** being dragged, so its handle can be drawn as grabbed.
    /// Axis, not index: a handle can drop out of the list when it goes
    /// edge-on, and an index would then highlight a different one.
    pub dragging: Option<usize>,
    pub fps: f32,
    /// Smoothed per-frame cost in milliseconds — ADR 0093. Which half of a
    /// frame to fix, without closing the editor to read a terminal.
    pub cpu_ms: f32,
    pub draw_ms: f32,
    /// What somebody else just changed: screen-space box, label, and how
    /// faded it is (1.0 fresh, 0.0 gone).
    pub agent_marks: &'a [AgentMark],
    /// Which `Type.field` keys each node overrides, by resolved node path.
    ///
    /// **Derived from the *unresolved* file, because resolution erases it.**
    /// `prefab_load::for_reading` folds an override into the component it
    /// targets and replaces the instance with the subtree it stood for, so the
    /// scene the inspector reads carries no trace of which values came from an
    /// override. Without this the marker and the revert button cannot exist.
    pub overrides: &'a std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    /// Console rows, oldest first — a snapshot the caller took this frame.
    pub console: &'a [crate::console::Entry],
    /// Seconds per simulation tick, so the transport can show elapsed time.
    /// Passed rather than assumed: the fixed timestep is the runtime's fact,
    /// and a second copy of it here is a number that can silently disagree.
    pub tick_seconds: f32,
    /// Ticks run, whether paused, and how many bodies — `None` in edit mode.
    pub playing: Option<(u32, bool, usize)>,
}

/// A node an agent just touched, ready to draw over the viewport.
pub struct AgentMark {
    /// Screen-space bounds in **window pixels**, as the viewport projected them.
    pub rect: (f32, f32, f32, f32),
    pub label: String,
    /// 1.0 when it just happened, fading to 0.0.
    pub freshness: f32,
}

/// The shapes `loom_asset::primitives` can make without an asset file.
///
/// Named here rather than imported because `loom_editor` depends on
/// `loom_scene`, `loom_reflect` and `loom_render` only — reaching into
/// `loom_asset` for five strings would buy a crate edge for nothing.
/// The shapes the Create menu offers.
///
/// **A copy of `loom_asset::primitives::NAMES`, and a test in `loom_cli` holds
/// the two together.** This crate does not depend on `loom_asset` and acquiring
/// an edge to share five strings would cost more than it saves — but a sixth
/// primitive added to the engine and not to this list is a shape the editor
/// cannot make, silently, so the drift is asserted rather than hoped for.
pub const PRIMITIVES: &[&str] = &["box", "plane", "sphere", "cylinder", "capsule"];

const AXIS_COLORS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(226, 84, 79),
    egui::Color32::from_rgb(124, 200, 96),
    egui::Color32::from_rgb(84, 148, 232),
];
const AXIS_NAMES: [&str; 3] = ["X", "Y", "Z"];

/// How much of a value to show before it stops being readable.
const VALUE_PREVIEW: usize = 160;

/// A read-only value, short enough to belong in a row.
///
/// An array gets its length first, because "4 ops" is the thing you actually
/// want from a voxel recipe and the JSON behind it is not reviewable in a
/// side panel at any width.
fn summarise(value: &serde_json::Value) -> String {
    let text = value.to_string();
    let prefix = match value {
        serde_json::Value::Array(items) => format!("{} items  ", items.len()),
        _ => String::new(),
    };
    if text.chars().count() <= VALUE_PREVIEW {
        return format!("{prefix}{text}");
    }
    let clipped: String = text.chars().take(VALUE_PREVIEW).collect();
    format!("{prefix}{clipped}…")
}

// **There is no `draw` here any more; [`crate::dock::Dock`] is the layout.**
// It used to fix the arrangement in code — a left panel, a right panel, two
// bottom panels — and keeping that alongside the dock would be two layouts of
// the same panels, drifting apart from the first time one of them gained a
// heading the other did not. The toolbar and the banner are still chrome on
// the root `Ui`; everything else is a tab body now and takes a plain `Ui`.

pub(crate) fn toolbar(root: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    egui::Panel::top("toolbar").show(root, |ui| {
        // **Wrapped, not clipped.** A fixed row overdraws itself the moment the
        // tools outgrow the window: at 1440 px the Undo/Redo/Save group was
        // rendering on top of the stats as "R143 fpsSavepu 1.6". A professional
        // tool does not draw over its own chrome, so the row reflows and the
        // stats moved to a status bar of their own.
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("loom").strong());
            ui.separator();

            let editing = structure_editable(state);
            for mode in [Mode::Move, Mode::Rotate, Mode::Scale] {
                if ui
                    .selectable_label(state.mode == mode, mode.label())
                    .on_hover_text(match mode {
                        Mode::Move => "1 — drag a handle to move along that axis",
                        Mode::Rotate => "2 — drag to turn around that axis",
                        Mode::Scale => "3 — drag to stretch along that axis",
                    })
                    .clicked()
                {
                    actions.push(UiAction::SetMode(mode));
                }
            }

            ui.separator();

            // **The step shown is the one for the current mode**, because a
            // toolbar with three boxes for a thing that applies to one of them
            // at a time is three chances to type into the wrong one.
            let mut on = state.snap.enabled;
            if ui
                .checkbox(&mut on, "Snap")
                .on_hover_text("land on the grid — hold Ctrl to invert for one drag")
                .changed()
            {
                actions.push(UiAction::SetSnap(on));
            }
            let mut step = match state.mode {
                Mode::Move => state.snap.translate,
                Mode::Rotate => state.snap.rotate,
                Mode::Scale => state.snap.scale,
            };
            let suffix = match state.mode {
                Mode::Move => " m",
                Mode::Rotate => "°",
                Mode::Scale => "×",
            };
            if ui
                .add(
                    egui::DragValue::new(&mut step)
                        .speed(0.05)
                        .range(0.0..=90.0)
                        .suffix(suffix),
                )
                .on_hover_text("increment for the current tool; zero is no snapping")
                .changed()
            {
                actions.push(UiAction::SetSnapStep(state.mode, step));
            }

            ui.separator();

            // **A debug view is a question about the geometry**, so it sits
            // beside the tools that move it rather than in a menu.
            egui::ComboBox::from_id_salt("view_mode")
                .selected_text(state.view_mode.label())
                .show_ui(ui, |ui| {
                    for mode in loom_render::ablate::ViewMode::ALL {
                        if ui
                            .selectable_label(state.view_mode == mode, mode.label())
                            .clicked()
                        {
                            actions.push(UiAction::SetViewMode(mode));
                        }
                    }
                });

            ui.separator();
            if ui
                .button("Focus")
                .on_hover_text("F — frame the selection")
                .clicked()
            {
                actions.push(UiAction::Focus);
            }
            ui.add_enabled_ui(editing, |ui| {
                ui.menu_button("Create", |ui| {
                    for shape in PRIMITIVES {
                        if ui.button(*shape).clicked() {
                            actions.push(UiAction::CreatePrimitive((*shape).to_owned()));
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("a node that draws something, under the selection");
            });
            if ui
                .add_enabled(editing, egui::Button::new("Duplicate"))
                .on_hover_text("Ctrl+D — children go with it")
                .clicked()
            {
                actions.push(UiAction::Duplicate);
            }
            if ui
                .add_enabled(editing, egui::Button::new("Copy"))
                .on_hover_text("Ctrl+C — the selection and everything under it")
                .clicked()
            {
                actions.push(UiAction::Copy);
            }
            if ui
                .add_enabled(editing, egui::Button::new("Paste"))
                .on_hover_text("Ctrl+V — into the selection, or beside the original")
                .clicked()
            {
                actions.push(UiAction::Paste);
            }
            if ui
                .add_enabled(editing, egui::Button::new("Delete"))
                .on_hover_text("Del — children go with it, in one transaction")
                .clicked()
            {
                actions.push(UiAction::Delete);
            }

            ui.separator();
            transport(ui, state, actions);

            ui.separator();
            if ui
                .add_enabled(state.can_undo && state.playing.is_none(), egui::Button::new("Undo"))
                .on_hover_text("One transaction, however many ops it held")
                .clicked()
            {
                actions.push(UiAction::Undo);
            }
            if ui
                .add_enabled(state.can_redo && state.playing.is_none(), egui::Button::new("Redo"))
                .clicked()
            {
                actions.push(UiAction::Redo);
            }
            if ui
                .add_enabled(editing, egui::Button::new("Save"))
                .clicked()
            {
                actions.push(UiAction::Save);
            }

            if let Some((ticks, paused, bodies)) = state.playing {
                let seconds = f64::from(ticks) * f64::from(state.tick_seconds);
                ui.label(
                    egui::RichText::new(format!(
                        "{} tick {ticks} · {seconds:.2}s · {bodies} bodies",
                        if paused { "⏸" } else { "▶" }
                    ))
                    .color(egui::Color32::from_rgb(120, 200, 140))
                    .monospace(),
                );
            } else if !state.editable {
                ui.label(
                    egui::RichText::new("read-only — pass --edit to change anything")
                        .color(egui::Color32::from_rgb(150, 150, 160)),
                );
            } else if state.dirty {
                ui.label(
                    egui::RichText::new("● unsaved").color(egui::Color32::from_rgb(230, 170, 80)),
                );
            }


        });
    });
}

/// Unity's Add Component, straight off the type registry.
///
/// The list is every registered type the node does not already carry — there
/// is no second list to keep in step, and a new component type appears here
/// the moment it is registered, exactly as it appears in `loom describe`.
fn add_component_menu(
    ui: &mut egui::Ui,
    node: &loom_scene::Node,
    state: &PanelState<'_>,
    editing: bool,
    actions: &mut Vec<UiAction>,
) {
    ui.add_enabled_ui(editing, |ui| {
        ui.menu_button("Add Component", |ui| {
            // **Forty types is past the point of scanning a list.** egui keeps
            // this across frames while the menu is open, which is exactly its
            // lifetime — the box is not state anyone else needs.
            let id = ui.id().with("add_filter");
            let mut needle: String = ui.data(|d| d.get_temp(id).unwrap_or_default());
            if ui
                .add(egui::TextEdit::singleline(&mut needle).hint_text("search…"))
                .changed()
            {
                ui.data_mut(|d| d.insert_temp(id, needle.clone()));
            }
            let needle = needle.to_lowercase();

            let mut offered = 0;
            for type_name in state.registry.type_names() {
                if !needle.is_empty() && !type_name.to_lowercase().contains(&needle) {
                    continue;
                }
                // `Name` and `Transform` are node-key sugar (format §3), not
                // components anyone adds by hand.
                if matches!(type_name, "Name" | "Transform")
                    || node.components.contains_key(type_name)
                {
                    continue;
                }
                offered += 1;
                let description = state
                    .registry
                    .describe(type_name)
                    .and_then(|s| s.get("description").cloned())
                    .and_then(|d| d.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let button = ui.button(type_name);
                let button = if description.is_empty() {
                    button
                } else {
                    // The doc comment again: one act writes the schema, the
                    // agent's hint, and this tooltip.
                    button.on_hover_text(description.lines().next().unwrap_or_default())
                };
                if button.clicked() {
                    actions.push(UiAction::AddComponent(type_name.to_owned()));
                    ui.close();
                }
            }
            if offered == 0 {
                ui.weak("this node has every component");
            }
        });
    });
}

/// Play / Pause / Step / Stop.
///
/// The tick counter beside them is the point: this is the same fixed-tick
/// simulation `loom sim` runs headless, so what the human watches here and
/// what the agent asserts on there are the same run.
fn transport(ui: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    match state.playing {
        None => {
            if ui
                .button("▶ Play")
                .on_hover_text("Simulate the scene. Nothing is written; Stop restores it.")
                .clicked()
            {
                actions.push(UiAction::Play);
            }
        }
        Some((_, paused, _)) => {
            if ui
                .button(if paused { "▶ Resume" } else { "⏸ Pause" })
                .clicked()
            {
                actions.push(UiAction::Pause);
            }
            if ui
                .button("⏭ Step")
                .on_hover_text("Exactly one tick — the unit the simulation is defined in")
                .clicked()
            {
                actions.push(UiAction::StepOnce);
            }
            if ui
                .button("⏹ Stop")
                .on_hover_text("Back to the scene as authored")
                .clicked()
            {
                actions.push(UiAction::Stop);
            }
        }
    }
}

/// The one thing in this editor that must not be resolved for the human.
///
/// never-do #15: two divergent versions are never merged. Both are intact —
/// one in memory, one on disk — and the choice is stated in terms of what is
/// lost, because that is the only thing worth knowing here.
pub(crate) fn conflict_banner(root: &mut egui::Ui, actions: &mut Vec<UiAction>) {
    egui::Panel::top("conflict").show(root, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("⚠ the scene changed on disk while you have unsaved edits")
                    .color(egui::Color32::from_rgb(240, 180, 90))
                    .strong(),
            );
            if ui
                .button("Reload from disk")
                .on_hover_text("Take the other version. Your unsaved edits are discarded.")
                .clicked()
            {
                actions.push(UiAction::ReloadFromDisk);
            }
            if ui
                .button("Keep mine")
                .on_hover_text("Keep yours. Saving will overwrite the file.")
                .clicked()
            {
                actions.push(UiAction::KeepMine);
            }
        });
    });
}

/// Whether a node belongs in a filtered hierarchy.
///
/// **Case-insensitive, over the whole path**, so `boat/helm` finds the helm and
/// `helm` finds it too. Matching only the leaf name would make a filter useless
/// on a scene whose interesting nodes are called `Body` eleven times.
/// Whether an ancestor of `path` is folded shut — ADR 0098.
///
/// **Checked by prefix, not by walking a tree**, because the hierarchy is a
/// flat list of slash-separated paths and building a tree to answer one
/// question per row would be the only tree in the editor.
#[must_use]
pub fn hidden_by_collapse(path: &str, collapsed: &std::collections::BTreeSet<String>) -> bool {
    let mut at = 0;
    while let Some(slash) = path[at..].find('/') {
        at += slash;
        if collapsed.contains(&path[..at]) {
            return true;
        }
        at += 1;
    }
    false
}

#[must_use]
pub fn matches_filter(path: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    path.to_lowercase().contains(&filter.to_lowercase())
}

pub(crate) fn hierarchy(ui: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    ui.heading("Hierarchy");

    // **`deeper_demo` has 260 nodes.** Without this, finding `Rig/Boat/Helm`
    // is scrolling, and scrolling is what a human does instead of working.
    let mut filter = state.filter.to_owned();
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut filter)
                .hint_text("filter…")
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            actions.push(UiAction::SetFilter(filter.clone()));
        }
    });
    if !state.filter.is_empty() {
        let shown = state.paths.iter().filter(|p| matches_filter(p, state.filter)).count();
        ui.horizontal(|ui| {
            ui.weak(format!("{shown} of {}", state.paths.len()));
            if ui.small_button("clear").clicked() {
                actions.push(UiAction::SetFilter(String::new()));
            }
        });
    }
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| {
        // **A filter overrides the folds.** Searching for a node buried under a
        // shut parent must find it, or the filter is only a filter of what you
        // had already opened.
        let searching = !state.filter.is_empty();
        for path in state.paths.iter().filter(|p| {
            matches_filter(p, state.filter)
                && (searching || !hidden_by_collapse(p, state.collapsed))
        }) {
            // Indent by depth, so the hierarchy reads as a tree rather
            // than a flat list of slash-separated strings.
            let depth = path.matches('/').count();
            let name = path.rsplit('/').next().unwrap_or(path);
            let selected = state.selected.iter().any(|p| p == path);
            ui.horizontal(|ui| {
                #[allow(clippy::cast_precision_loss)]
                ui.add_space(depth as f32 * 14.0);
                // The fold triangle, or a gap where one would be, so names
                // stay in one column whether or not a node has children.
                let prefix = format!("{path}/");
                let has_children = state.paths.iter().any(|p| p.starts_with(&prefix));
                if has_children && !searching {
                    let shut = state.collapsed.contains(path);
                    // **ASCII, because the shipped font has no triangles.**
                    // U+25B8/U+25BE rendered as missing-glyph boxes, which is a
                    // worse control than no control.
                    if ui
                        .add(egui::Button::new(if shut { "+" } else { "-" }).frame(false))
                        .clicked()
                    {
                        actions.push(UiAction::ToggleCollapsed(path.clone()));
                    }
                } else {
                    ui.add_space(14.0);
                }
                // Nodes that draw nothing are still real; showing which
                // do saves opening the inspector to find out.
                let marker = if state.picks.contains_key(path) {
                    "▪"
                } else {
                    "·"
                };
                // A row is both a drag source and a drop target, which
                // is what makes reparenting a drag rather than a menu.
                // The op layer refuses cycles and name collisions, so
                // an impossible drop is rejected with a reason rather
                // than prevented by duplicated rules here.
                // **Renaming happens where the name is** — ADR 0100. F2 turns
                // the row into a field; the inspector's name box still works
                // and is the same op, but nobody looks there to rename a node
                // they are looking at.
                if state.renaming == Some(path.as_str()) {
                    let buffer_id = egui::Id::new(("hierarchy_rename", path));
                    let mut text: String = ui
                        .data_mut(|d| d.get_temp::<String>(buffer_id))
                        .unwrap_or_else(|| name.to_owned());
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut text).desired_width(160.0),
                    );
                    field.request_focus();
                    if field.changed() {
                        ui.data_mut(|d| d.insert_temp(buffer_id, text.clone()));
                    }
                    let done = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    if done && !text.trim().is_empty() && text.trim() != name {
                        actions.push(UiAction::Rename(path.clone(), text.trim().to_owned()));
                    }
                    if done || cancelled {
                        ui.data_mut(|d| d.remove::<String>(buffer_id));
                        actions.push(UiAction::BeginRename(String::new()));
                    }
                    return;
                }

                let id = egui::Id::new(("hierarchy", path));
                let response = ui
                    .dnd_drag_source(id, path.clone(), |ui| {
                        ui.selectable_label(selected, format!("{marker} {name}"))
                    })
                    .response
                    .on_hover_text(path);
                // **Gated like the menu beside it.** Reparenting rewrites the
                // paths a running simulation is keyed by, and this drop sat
                // outside the guard that covers the identical verb in the
                // context menu two lines below — the same verb behind two
                // different gates, which is the defect this taxonomy exists to
                // stop.
                if structure_editable(state)
                    && let Some(dragged) = response.dnd_release_payload::<String>()
                    && *dragged != *path
                {
                    actions.push(UiAction::Reparent {
                        node: (*dragged).clone(),
                        parent: path.clone(),
                    });
                }
                if response.clicked() {
                    actions.push(UiAction::Select {
                        path: path.clone(),
                        // Ctrl-click extends, as everywhere else.
                        extend: ui.input(|i| i.modifiers.ctrl),
                    });
                }
                // **Structural, so not while playing.** This was gated on
                // `editable` alone, which left Delete, Duplicate, Reparent and
                // Create reachable mid-run — the dangerous verbs open while the
                // safe ones were locked.
                if structure_editable(state) {
                    response.context_menu(|ui| {
                        // **The most common operation in a blockout editor,
                        // in one step.** It used to be three: add an empty
                        // child, add a `MeshRenderer`, then point it at an
                        // alias — for a cube.
                        ui.menu_button("Create", |ui| {
                            for shape in PRIMITIVES {
                                if ui.button(*shape).clicked() {
                                    actions.push(UiAction::Select {
                                        path: path.clone(),
                                        extend: false,
                                    });
                                    actions.push(UiAction::CreatePrimitive((*shape).to_owned()));
                                    ui.close();
                                }
                            }
                        });
                        if ui.button("Add child").clicked() {
                            actions.push(UiAction::AddChild(path.clone()));
                            ui.close();
                        }
                        if ui.button("Duplicate").clicked() {
                            actions.push(UiAction::Select {
                                path: path.clone(),
                                extend: false,
                            });
                            actions.push(UiAction::Duplicate);
                            ui.close();
                        }
                        if ui.button("Rename").clicked() {
                            actions.push(UiAction::BeginRename(path.clone()));
                            ui.close();
                        }
                        if ui.button("Copy").clicked() {
                            actions.push(UiAction::Select {
                                path: path.clone(),
                                extend: false,
                            });
                            actions.push(UiAction::Copy);
                            ui.close();
                        }
                        if ui.button("Paste into").clicked() {
                            actions.push(UiAction::Select {
                                path: path.clone(),
                                extend: false,
                            });
                            actions.push(UiAction::Paste);
                            ui.close();
                        }
                        if ui.button("Delete").clicked() {
                            actions.push(UiAction::Select {
                                path: path.clone(),
                                extend: false,
                            });
                            actions.push(UiAction::Delete);
                            ui.close();
                        }
                    });
                }
            });
        }
    });
}

pub(crate) fn inspector(ui: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    ui.heading("Inspector");
    ui.separator();

    if state.selected.len() > 1 {
        multi_inspector(ui, state, actions);
        return;
    }
    let Some(path) = state.selected.first() else {
        ui.weak("nothing selected");
        return;
    };
    let Some(node) = state.scene.nodes().iter().find(|n| &n.path == path) else {
        return;
    };

    // **Fields stay live while playing — ADR 0099.** Tuning a buoyancy force
    // while watching the boat is the loop this editor existed without. The name
    // is the exception: renaming a node mid-run would rebuild the world around
    // a path the simulation is holding.
    let editable_now = state.editable;
    let short = path.rsplit('/').next().unwrap_or(path);
    // **The buffer has to outlive the frame.** It used to be rebuilt
    // from the node's name every frame, so each typed character was
    // overwritten before the next repaint and Rename could never fire —
    // the field looked editable and was inert. egui's own per-id store
    // keeps it across frames; keying it on the path means selecting a
    // different node starts a fresh buffer rather than carrying the
    // previous node's half-typed name over.
    let buffer_id = egui::Id::new(("rename", path));
    let mut renamed = ui
        .data_mut(|d| d.get_temp::<String>(buffer_id))
        .unwrap_or_else(|| short.to_owned());
    ui.horizontal(|ui| {
        ui.label("name");
        let response = ui.add_enabled(
            editable_now && state.playing.is_none(),
            egui::TextEdit::singleline(&mut renamed).desired_width(180.0),
        );
        if response.changed() {
            ui.data_mut(|d| d.insert_temp(buffer_id, renamed.clone()));
        }
        // On commit, not per keystroke: a transaction per character
        // would bury the log and make undo useless.
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            actions.push(UiAction::Rename(path.clone(), renamed.clone()));
            ui.data_mut(|d| d.remove::<String>(buffer_id));
        }
    });
    ui.label(egui::RichText::new(path).monospace().weak());
    ui.add_space(6.0);
    // Values, not shape — see `fields_editable`.
    let editing = fields_editable(state);

    let empty = std::collections::BTreeSet::new();
    let ctx = FieldContext {
        assets: state.assets,
        overridden: state.overrides.get(path).unwrap_or(&empty),
    };

    egui::ScrollArea::vertical().show(ui, |ui| {
        // Transform first — it is what a human reaches for, and it is
        // the node-key sugar rather than a component table.
        inspect_transform(ui, path, &node.transform, editing, actions);

        for (type_name, value) in &node.components {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(type_name).strong());
                if editing && ui.small_button("✖").on_hover_text("remove").clicked() {
                    actions.push(UiAction::RemoveComponent(
                        path.clone(),
                        type_name.clone(),
                    ));
                }
                // **Copy a whole component, not a field at a time** — ADR 0107.
                // The demo boat carries fifteen material nodes; matching one to
                // another meant reading five numbers off one and typing them
                // into the next, which is how two things that should be the
                // same drift apart.
                // The word, not a glyph: `⧉` is not in the bundled font and
                // rendered as a tofu box — a button whose icon is a missing
                // character is a button nobody presses.
                if ui.small_button("copy").on_hover_text("copy these values").clicked() {
                    actions.push(UiAction::CopyComponent(path.clone(), type_name.clone()));
                }
                if state.copied_component == Some(type_name.as_str())
                    && editing
                    && ui
                        .small_button("paste")
                        .on_hover_text("write the copied values over these")
                        .clicked()
                {
                    actions.push(UiAction::PasteComponent(path.clone()));
                }
            });
            inspect_component(
                ui,
                path,
                type_name,
                value,
                state.registry,
                editing,
                &ctx,
                actions,
            );
        }

        ui.add_space(12.0);
        add_component_menu(ui, node, state, editing, actions);
    });
}

/// Unity's Project panel, cut to what this engine has: the assets the scene
/// actually resolved. Clicking one points the selection at it.
pub(crate) fn assets(ui: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    // **Scenes first, because switching between them used to mean restarting.**
    // A project is more than one file and an editor that can only ever show the
    // one named on the command line is a viewer.
    //
    // **Grouped by folder and filterable, because the list is now the project
    // rather than one directory** — ADR 0104. This repository has 130-odd
    // scenes; a flat wrapped strip of them is a wall, and the folder a scene
    // lives in is most of what tells you what it is.
    if !state.scenes.is_empty() {
        ui.horizontal(|ui| {
            ui.heading("Scenes");
            ui.weak(format!("{}", state.scenes.len()));
            if state.dirty {
                ui.weak("— save first, opening another would lose unsaved edits");
            }
        });
        let filter_id = ui.id().with("scene_filter");
        let mut needle: String = ui.data(|d| d.get_temp(filter_id).unwrap_or_default());
        if ui
            .add(
                egui::TextEdit::singleline(&mut needle)
                    .hint_text("filter scenes…")
                    .desired_width(220.0),
            )
            .changed()
        {
            ui.data_mut(|d| d.insert_temp(filter_id, needle.clone()));
        }
        let needle = needle.to_lowercase();

        // Group by the directory each scene sits in. The caller sorts by
        // (folder, name), so equal folders are contiguous and a run is a group.
        let mut groups: Vec<(&str, Vec<&String>)> = Vec::new();
        for scene in state.scenes {
            if !needle.is_empty() && !scene.to_lowercase().contains(&needle) {
                continue;
            }
            let folder = scene.rsplit_once('/').map_or(".", |(dir, _)| dir);
            match groups.last_mut() {
                Some((last, list)) if *last == folder => list.push(scene),
                _ => groups.push((folder, vec![scene])),
            }
        }
        if groups.is_empty() {
            ui.weak("nothing matches that");
        }

        egui::ScrollArea::vertical()
            .id_salt("scene_list")
            .max_height(220.0)
            .show(ui, |ui| {
                // Folders are absolute and mostly the same prefix; showing
                // `/home/somebody/loom/assets/games` on every row is noise
                // around the one word that distinguishes it.
                let shared = groups
                    .iter()
                    .map(|(folder, _)| *folder)
                    .reduce(|a, b| {
                        let take = a
                            .char_indices()
                            .zip(b.chars())
                            .take_while(|((_, x), y)| x == y)
                            .map(|((i, c), _)| i + c.len_utf8())
                            .last()
                            .unwrap_or(0);
                        // Cut at a separator, so half a directory name never
                        // becomes the prefix.
                        &a[..a[..take].rfind('/').map_or(0, |i| i + 1)]
                    })
                    .unwrap_or("");
                for (folder, scenes) in &groups {
                    let shown = folder.strip_prefix(shared).unwrap_or(folder);
                    // Open by default when filtering: a search that hides its
                    // own results behind a twisty is a search that looks broken.
                    let holds_open = scenes.iter().any(|s| *s == state.open_scene);
                    egui::CollapsingHeader::new(format!("{shown}  ({})", scenes.len()))
                        .id_salt(("scene_group", folder))
                        .default_open(holds_open || !needle.is_empty())
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                for scene in scenes {
                                    let open = *scene == state.open_scene;
                                    let name = scene.rsplit('/').next().unwrap_or(scene);
                                    let button = egui::Button::new(if open {
                                        format!("▶ {name}")
                                    } else {
                                        format!("  {name}")
                                    });
                                    if ui
                                        .add_enabled(!open && !state.dirty, button)
                                        .on_hover_text(*scene)
                                        .clicked()
                                    {
                                        actions.push(UiAction::OpenScene((*scene).clone()));
                                    }
                                }
                            });
                        });
                }
            });
        ui.separator();
    }

    ui.horizontal(|ui| {
        ui.heading("Assets");
        // Discoverable, because a gesture nobody is told about is a gesture
        // nobody uses — ADR 0105.
        ui.weak("drop a .obj, .glb, .gltf or .png on the window to import one");
    });
    ui.separator();
    egui::ScrollArea::horizontal().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for asset in state.assets {
                // Voxel meshes are baked per node from the scene's op
                // list, not assignable to anything else.
                if asset.starts_with("voxel:") {
                    ui.add_enabled(false, egui::Button::new(format!("⛰ {asset}")))
                        .on_disabled_hover_text(
                            "baked from this node's op list — never a raw voxel array",
                        );
                    continue;
                }
                // **Draggable into the viewport, clickable onto the
                // selection.** Placing a mesh was select-a-node-then-assign,
                // which is a tools programmer's flow; dragging it into the
                // world is what an artist reaches for.
                // Not while playing: the drop is refused there, and a drag
                // that silently does nothing is worse than one you cannot start.
                let enabled = structure_editable(state) && !state.selected.is_empty();
                let droppable = structure_editable(state);
                let id = egui::Id::new(("asset", asset));
                let response = if droppable {
                    ui.dnd_drag_source(id, asset.clone(), |ui| {
                        ui.add_enabled(enabled, egui::Button::new(format!("◻ {asset}")));
                    })
                    .response
                    .on_hover_text("drag into the viewport, or click to assign to the selection")
                } else {
                    ui.add_enabled(false, egui::Button::new(format!("◻ {asset}")))
                        .on_disabled_hover_text("stop playing to place assets")
                };
                if response.clicked() && enabled {
                    actions.push(UiAction::AssignMesh(asset.clone()));
                }
            }
        });
    });
}

/// Where the frame went — ADR 0106.
///
/// **The status bar's numbers are smoothed and this one's are not.** Smoothing
/// is right for a number read at a glance and hides exactly what a profiler is
/// for: the editor reports `cpu 1.6 ms/frame mean` and `27.4 ms worst` in the
/// same breath, and no smoothed number can tell you which frame that was or
/// what it was doing.
pub(crate) fn profiler(ui: &mut egui::Ui, state: &PanelState<'_>) {
    ui.heading("Profiler");
    ui.separator();
    if state.frames.is_empty() {
        ui.weak("no frames measured yet");
        return;
    }

    let tokens = crate::theme::tokens(false);
    let count = state.frames.len();
    #[allow(clippy::cast_precision_loss)]
    let n = count as f32;
    let mean = |pick: fn(&FrameCost) -> f32| -> f32 {
        state.frames.iter().map(pick).sum::<f32>() / n
    };
    let worst = |pick: fn(&FrameCost) -> f32| -> f32 {
        state.frames.iter().map(pick).fold(0.0, f32::max)
    };

    // **The 99th percentile, not just the worst.** One 40 ms frame in four
    // seconds is a window manager doing something; one in twenty is the engine,
    // and the two read identically as a maximum.
    let mut totals: Vec<f32> = state.frames.iter().map(|f| f.total_ms).collect();
    totals.sort_by(f32::total_cmp);
    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let p99 = totals[((n * 0.99) as usize).min(count - 1)];

    ui.add_space(4.0);

    // **The history, drawn.** A hitch is a shape, not a number: a sawtooth is a
    // periodic rebuild, a single spike is a stall, a step is something that
    // started and did not stop.
    let height = 84.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(120.0), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, tokens.sunken);

    // **Scaled to the p99, not to the worst.** Scaled to the worst, one 83 ms
    // stall squashed every normal frame into a sliver a pixel high — a graph
    // that showed the outlier and hid the thing it was an outlier *from*. The
    // floor keeps an idle scene from magnifying half a millisecond of jitter
    // into a mountain range.
    let ceiling = p99.max(8.0);
    #[allow(clippy::cast_precision_loss)]
    let step = rect.width() / n.max(1.0);
    for (i, frame) in state.frames.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let x = rect.left() + i as f32 * step;
        let bar = |ms: f32, colour: egui::Color32| {
            let h = (ms / ceiling).clamp(0.0, 1.0) * rect.height();
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(x, rect.bottom() - h),
                    egui::vec2(step.max(1.0), h),
                ),
                0.0,
                colour,
            );
        };
        // Total behind, cpu in front: the gap between them is what the frame
        // spent waiting rather than working.
        bar(frame.total_ms, tokens.text_weak);
        bar(frame.cpu_ms, tokens.accent);
        // A frame taller than the scale is capped rather than clipped away —
        // the whole point of a p99 scale is that the outliers are still there,
        // and a bar that quietly stopped at the ceiling would deny it.
        if frame.total_ms > ceiling {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(x, rect.top()),
                    egui::vec2(step.max(1.0), 3.0),
                ),
                0.0,
                tokens.error,
            );
        }
    }

    // The 16.7 ms line, because "is this frame late" is the only question the
    // graph is really being asked.
    let sixty = rect.bottom() - (16.7 / ceiling).clamp(0.0, 1.0) * rect.height();
    painter.line_segment(
        [egui::pos2(rect.left(), sixty), egui::pos2(rect.right(), sixty)],
        egui::Stroke::new(1.0, tokens.warn),
    );
    ui.horizontal(|ui| {
        ui.colored_label(tokens.accent, "cpu");
        ui.colored_label(tokens.text_weak, "frame");
        ui.colored_label(tokens.warn, "60 fps");
        ui.colored_label(tokens.error, "over scale");
        ui.weak(format!("· scale {ceiling:.0} ms (p99)"));
    });

    // Scrolled, so a panel dragged short hides nothing permanently.
    egui::ScrollArea::vertical()
        .id_salt("profiler_numbers")
        .show(ui, |ui| {
    egui::Grid::new("profiler_numbers_grid")
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            ui.label("");
            ui.label("mean");
            ui.label("worst");
            ui.label("");
            ui.end_row();

            for (name, mean_ms, worst_ms, note) in [
                ("frame", mean(|f| f.total_ms), worst(|f| f.total_ms),
                 format!("p99 {p99:.1} ms")),
                ("cpu", mean(|f| f.cpu_ms), worst(|f| f.cpu_ms),
                 "input, step, draw calls, weather, panels".to_owned()),
                ("— simulation", mean(|f| f.sim_ms), worst(|f| f.sim_ms),
                 "the game's own share of cpu".to_owned()),
                ("gpu submit", mean(|f| f.draw_ms), worst(|f| f.draw_ms),
                 "submitting and presenting".to_owned()),
            ] {
                ui.label(name);
                ui.label(egui::RichText::new(format!("{mean_ms:.2} ms")).monospace());
                // A worst frame far above the mean is the finding, so it is
                // coloured rather than left to be noticed.
                let colour = if worst_ms > mean_ms * 4.0 && worst_ms > 4.0 {
                    tokens.warn
                } else {
                    tokens.text_weak
                };
                ui.colored_label(colour, egui::RichText::new(format!("{worst_ms:.2} ms")).monospace());
                ui.weak(note);
                ui.end_row();
            }
        });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.weak(format!("{count} frames"));
        ui.weak("·");
        ui.weak(format!("{} nodes", state.paths.len()));
        ui.weak("·");
        ui.weak(format!("{} draws", state.object_count));
    });
        });
}

/// Unity's Console. The reason it exists is that the messages worth reading —
/// a rejected transaction, a scene that moved on disk — were going to a
/// terminal nobody has in front of them.
///
/// **It used to be two columns of one panel and is now two tabs.** They were
/// side by side because what the engine said and what the scene did are read
/// together; docked, the human can put them side by side or stack them, which
/// is strictly more than the hard-coded pair offered.
pub(crate) fn console_column(
    ui: &mut egui::Ui,
    entries: &[crate::console::Entry],
    actions: &mut Vec<UiAction>,
) {
    ui.horizontal(|ui| {
        ui.heading("Console");
        if ui.small_button("Clear").clicked() {
            actions.push(UiAction::ClearLog);
        }
    });
    ui.separator();
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .id_salt("console_scroll")
        .show(ui, |ui| {
                    if entries.is_empty() {
                        ui.weak("nothing yet");
                    }
                    for entry in entries {
                        let (color, tag) = match entry.level {
                            crate::console::Level::Info => (egui::Color32::GRAY, " "),
                            crate::console::Level::Warn => {
                                (egui::Color32::from_rgb(230, 180, 90), "!")
                            }
                            crate::console::Level::Error => {
                                (egui::Color32::from_rgb(230, 110, 100), "✖")
                            }
                        };
                        let repeats = if entry.repeats > 1 {
                            format!("  ×{}", entry.repeats)
                        } else {
                            String::new()
                        };
                        ui.label(
                            egui::RichText::new(format!("{tag} {}{repeats}", entry.text))
                                .color(color)
                                .monospace(),
                        );
            }
        });
}

/// The transaction log: every change to the scene, by its label.
/// The prefabs this scene declares, and who instances them — ADR 0093.
///
/// **Read from the *unresolved* file.** `prefab_load::for_reading` replaces an
/// instance with the subtree it stood for, so the resolved scene the inspector
/// reads has no prefabs left in it at all — the same reason the override
/// markers need their own map.
pub(crate) fn prefabs(ui: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    ui.heading("Prefabs");
    ui.separator();

    if state.prefabs.is_empty() {
        ui.weak("this scene declares no prefabs");
        return;
    }

    egui::ScrollArea::vertical().id_salt("prefab_scroll").show(ui, |ui| {
        for decl in state.prefabs {
            let instances = &decl.instances;

            ui.horizontal(|ui| {
                ui.strong(&decl.key);
                ui.weak(format!("{} instance(s)", instances.len()));
            });
            ui.weak(&decl.path).on_hover_text(format!("id {}", decl.id));

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(structure_editable(state), egui::Button::new("Add instance"))
                    .on_hover_text("spawn one under the selection")
                    .clicked()
                {
                    actions.push(UiAction::AddPrefabInstance(decl.key.clone()));
                }
                // **A prefab is a scene**, so editing one is opening it — which
                // is prefab edit mode in the only form this format needs. The
                // path is a hint for finding the file (§3), and finding the
                // file is exactly what this does with it.
                if ui
                    .add_enabled(!state.dirty, egui::Button::new("Open"))
                    .on_hover_text(if state.dirty {
                        "save first"
                    } else {
                        "edit the prefab itself, not this instance"
                    })
                    .clicked()
                {
                    actions.push(UiAction::OpenScene(decl.path.clone()));
                }
                for path in instances.iter().take(6) {
                    let name = path.rsplit('/').next().unwrap_or(path);
                    if ui.small_button(name).on_hover_text(path).clicked() {
                        actions.push(UiAction::Select {
                            path: path.clone(),
                            extend: false,
                        });
                    }
                }
            });
            ui.separator();
        }
    });
}

/// The undo stack, with the cursor in it — ADR 0093.
///
/// **`Transactions` is a log; this is a position.** The log answers "what has
/// happened", which is the wrong question when the thing you want is "how far
/// back can I go, and what is waiting to come forward". Clicking a row walks
/// the cursor to it, which is undo and redo without counting steps.
pub(crate) fn history(ui: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    ui.heading("History");
    ui.separator();

    if state.history.is_empty() && state.redo_history.is_empty() {
        ui.weak("no edits yet");
        return;
    }

    egui::ScrollArea::vertical().id_salt("history_scroll").show(ui, |ui| {
        // Done, oldest first. Clicking one walks back to just after it.
        for (index, label) in state.history.iter().enumerate() {
            let steps = state.history.len() - index - 1;
            let row = ui.selectable_label(false, format!("{:>3}  {label}", index + 1));
            if steps > 0 {
                row.on_hover_text(format!("undo {steps} step(s) to here"))
                    .clicked()
                    .then(|| {
                        for _ in 0..steps {
                            actions.push(UiAction::Undo);
                        }
                    });
            }
        }

        // The cursor: everything above has happened, everything below has not.
        ui.horizontal(|ui| {
            ui.add(egui::Separator::default().horizontal());
        });
        ui.weak("— now —");

        // Undone, nearest first. `redo_labels` is newest-last, so the next one
        // to come back is the end of it.
        for (offset, label) in state.redo_history.iter().rev().enumerate() {
            let steps = offset + 1;
            let row = ui.selectable_label(
                false,
                egui::RichText::new(format!("     {label}")).weak(),
            );
            row.on_hover_text(format!("redo {steps} step(s) to here"))
                .clicked()
                .then(|| {
                    for _ in 0..steps {
                        actions.push(UiAction::Redo);
                    }
                });
        }
    });
}

/// The menu bar — ADR 0098.
///
/// **Every editor has one and this had none.** Without it there is nowhere to
/// put a verb that is not worth a toolbar button, so verbs either crowd the
/// toolbar until it overdraws itself — which is exactly what happened — or do
/// not exist. It is also the first place a human looks to find out what a tool
/// can do.
pub(crate) fn menu_bar(root: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    egui::Panel::top("menubar").show(root, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            let editing = structure_editable(state);

            ui.menu_button("File", |ui| {
                if ui
                    .add_enabled(state.dirty, egui::Button::new("Save scene"))
                    .clicked()
                {
                    actions.push(UiAction::Save);
                    ui.close();
                }
                // **A name typed in the menu, not a file dialog.** Nothing in
                // this project draws one, and adding a dependency to pick a
                // filename would be the largest thing in the editor. Both of
                // these write beside the scene that is open, which is where a
                // human looking for them would go.
                if ui
                    .add_enabled(!state.dirty, egui::Button::new("New scene"))
                    .on_hover_text("an empty scene beside this one")
                    .clicked()
                {
                    actions.push(UiAction::NewScene);
                    ui.close();
                }
                ui.menu_button("Save as", |ui| {
                    let id = ui.id().with("save_as");
                    let mut name: String = ui.data(|d| d.get_temp(id).unwrap_or_default());
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut name).hint_text("name.loom"),
                    );
                    if response.changed() {
                        ui.data_mut(|d| d.insert_temp(id, name.clone()));
                    }
                    if ui
                        .add_enabled(!name.trim().is_empty(), egui::Button::new("Save"))
                        .clicked()
                    {
                        actions.push(UiAction::SaveAs(name.trim().to_owned()));
                        ui.close();
                    }
                });
                ui.separator();
                ui.menu_button("Open scene", |ui| {
                    if state.scenes.is_empty() {
                        ui.weak("no other scenes beside this one");
                    }
                    for scene in state.scenes {
                        let open = scene == state.open_scene;
                        let name = scene.rsplit('/').next().unwrap_or(scene);
                        if ui
                            .add_enabled(!open && !state.dirty, egui::Button::new(name))
                            .on_hover_text(scene)
                            .clicked()
                        {
                            actions.push(UiAction::OpenScene(scene.clone()));
                            ui.close();
                        }
                    }
                });
            });

            ui.menu_button("Edit", |ui| {
                if ui.add_enabled(state.can_undo, egui::Button::new("Undo")).clicked() {
                    actions.push(UiAction::Undo);
                    ui.close();
                }
                if ui.add_enabled(state.can_redo, egui::Button::new("Redo")).clicked() {
                    actions.push(UiAction::Redo);
                    ui.close();
                }
                ui.separator();
                for (label, action) in [
                    ("Copy", UiAction::Copy),
                    ("Paste", UiAction::Paste),
                    ("Duplicate", UiAction::Duplicate),
                    ("Delete", UiAction::Delete),
                ] {
                    if ui.add_enabled(editing, egui::Button::new(label)).clicked() {
                        actions.push(action);
                        ui.close();
                    }
                }
            });

            ui.menu_button("Create", |ui| {
                for shape in PRIMITIVES {
                    if ui.add_enabled(editing, egui::Button::new(*shape)).clicked() {
                        actions.push(UiAction::CreatePrimitive((*shape).to_owned()));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.add_enabled(editing, egui::Button::new("Empty child")).clicked()
                    && let Some(path) = state.selected.first()
                {
                    actions.push(UiAction::AddChild(path.clone()));
                    ui.close();
                }
            });

            ui.menu_button("View", |ui| {
                for mode in loom_render::ablate::ViewMode::ALL {
                    if ui
                        .selectable_label(state.view_mode == mode, mode.label())
                        .clicked()
                    {
                        actions.push(UiAction::SetViewMode(mode));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Frame selection").clicked() {
                    actions.push(UiAction::Focus);
                    ui.close();
                }
            });

            // **The engine's own verbs, where a human can reach them.** Save and
            // load are the ADR 0088 format — the same snapshot `loom sim --save`
            // writes — so a game saved here opens in the CLI.
            ui.menu_button("Play", |ui| {
                let playing = state.playing.is_some();
                if ui
                    .add_enabled(!playing, egui::Button::new("Play"))
                    .clicked()
                {
                    actions.push(UiAction::Play);
                    ui.close();
                }
                for (label, action, enabled) in [
                    ("Pause", UiAction::Pause, playing),
                    ("Step one tick", UiAction::StepOnce, playing),
                    ("Stop", UiAction::Stop, playing),
                ] {
                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                        actions.push(action);
                        ui.close();
                    }
                }
                ui.separator();
                if ui
                    .add_enabled(playing, egui::Button::new("Save game"))
                    .on_hover_text("the running world, in the format loom sim --save writes")
                    .clicked()
                {
                    actions.push(UiAction::SaveGame);
                    ui.close();
                }
                if ui
                    .add_enabled(playing, egui::Button::new("Load game"))
                    .clicked()
                {
                    actions.push(UiAction::LoadGame);
                    ui.close();
                }
            });
        });
    });
}

/// One line along the bottom: what the scene costs and what is selected.
///
/// **Its own strip, not the toolbar's tail.** Sharing a row with the tools
/// meant the two fought for width and the loser was drawn over.
pub(crate) fn status_bar(root: &mut egui::Ui, state: &PanelState<'_>) {
    egui::Panel::bottom("status").show(root, |ui| {
        ui.horizontal(|ui| {
            match state.selected {
                [] => ui.weak("nothing selected"),
                [one] => ui.weak(one.as_str()),
                many => ui.weak(format!("{} selected", many.len())),
            };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{:.0} fps · cpu {:.1} · draw {:.1} ms · {} nodes · {} draws",
                        state.fps,
                        state.cpu_ms,
                        state.draw_ms,
                        state.paths.len(),
                        state.object_count
                    ))
                    .weak()
                    .monospace(),
                );
            });
        });
    });
}

/// What the agent has been doing — ADR 0093.
///
/// **This is the human's half of the conversation.** The agent already drives
/// the same transactions the UI does, through `loom_agent`'s MCP tools over the
/// same CLI commands. What was missing was anywhere for a human to *see* it:
/// the viewport marks fade in six seconds, and the console interleaves agent
/// edits with everything else.
///
/// Every row selects its node, so reviewing what changed is one click per row
/// rather than a search through the hierarchy.
pub(crate) fn agent(ui: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    ui.heading("Agent");
    ui.separator();

    // **The half that was missing** — ADR 0100. The agent could already change
    // the scene and have it appear here within 250 ms; there was no way to ask
    // it for anything. This is that way.
    let id = ui.id().with("agent_prompt");
    let mut prompt: String = ui.data(|d| d.get_temp(id).unwrap_or_default());
    let send = |actions: &mut Vec<UiAction>, ui: &egui::Ui, text: &str| {
        if !text.trim().is_empty() {
            actions.push(UiAction::SendToAgent(text.trim().to_owned()));
            ui.data_mut(|d| d.remove::<String>(id));
        }
    };

    ui.horizontal(|ui| {
        // What the request will be about, said out loud — "make it sit lower"
        // means nothing without this, and a human should see what the agent
        // will see.
        match state.selected {
            [] => ui.weak("about: the whole scene"),
            [one] => ui.weak(format!("about: {one}")),
            many => ui.weak(format!("about: {} nodes", many.len())),
        };
    });
    let field = ui.add(
        egui::TextEdit::multiline(&mut prompt)
            .hint_text("ask for a change — the agent edits the scene and you watch it happen")
            .desired_rows(2)
            .desired_width(f32::INFINITY),
    );
    if field.changed() {
        ui.data_mut(|d| d.insert_temp(id, prompt.clone()));
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!prompt.trim().is_empty(), egui::Button::new("Send"))
            .clicked()
        {
            send(actions, ui, &prompt);
        }
        // Ctrl+Enter sends, because the field takes plain Enter for newlines.
        if field.has_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.ctrl)
        {
            send(actions, ui, &prompt);
        }
        ui.weak("Ctrl+Enter");
        if state.agent_busy {
            ui.spinner();
            ui.weak("waiting for a reply");
        }
    });
    ui.separator();

    // **Offered, not made** — ADR 0103. An edit that lands while the human is
    // elsewhere is one they discover by noticing the scene changed. These are
    // waiting on them, so they come first, above the conversation.
    //
    // **The buttons sit above the diff, not below it.** With them last, a short
    // panel pushed Apply off the bottom edge — the one control the block exists
    // for, invisible, in the first screenshot of it. One scroll area for the
    // whole block and none inside it: a scroll area nested in a scroll area
    // eats the wheel at whichever one the pointer happens to be over.
    if !state.agent_proposals.is_empty() {
        egui::ScrollArea::vertical()
            .id_salt("agent_proposals")
            .max_height(240.0)
            .show(ui, |ui| {
                for offer in state.agent_proposals {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(crate::theme::tokens(false).agent, "proposed");
                            ui.label(&offer.text);
                        });
                        ui.horizontal(|ui| {
                            // Apply is gated by the same rule as every other
                            // structural edit: a transaction against the
                            // authored scene means nothing while a simulation
                            // is running on top of it.
                            let allowed = structure_editable(state);
                            if ui
                                .add_enabled(allowed, egui::Button::new("Apply"))
                                .on_disabled_hover_text(
                                    "stop the game first — this edits the authored scene",
                                )
                                .clicked()
                            {
                                actions.push(UiAction::DecideProposal {
                                    id: offer.id,
                                    apply: true,
                                });
                            }
                            if ui.button("Discard").clicked() {
                                actions.push(UiAction::DecideProposal {
                                    id: offer.id,
                                    apply: false,
                                });
                            }
                            ui.weak("Ctrl+Z undoes it like any other edit");
                        });
                        if offer.diff.is_empty() {
                            ui.weak("no change — the scene already reads that way");
                        } else {
                            // Shown verbatim: a summary of a diff is a second
                            // thing to get wrong, and the human is approving
                            // the diff rather than a description of it. Long
                            // ones are cut with the count said out loud, never
                            // silently.
                            const SHOWN: usize = 24;
                            for line in offer.diff.iter().take(SHOWN) {
                                let colour = match line.as_bytes().first() {
                                    Some(b'+') => crate::theme::tokens(false).ok,
                                    Some(b'-') => crate::theme::tokens(false).error,
                                    _ => crate::theme::tokens(false).text_weak,
                                };
                                ui.colored_label(
                                    colour,
                                    egui::RichText::new(line).monospace(),
                                );
                            }
                            if offer.diff.len() > SHOWN {
                                ui.weak(format!(
                                    "… and {} more line(s) — `loom scene --tx --dry-run` \
                                     prints the whole diff",
                                    offer.diff.len() - SHOWN
                                ));
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
            });
    }
    if !state.agent_proposals.is_empty() {
        ui.separator();
    }

    if !state.agent_chat.is_empty() {
        egui::ScrollArea::vertical()
            .id_salt("agent_chat")
            .max_height(220.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for turn in state.agent_chat {
                    let (who, colour) = if turn.from_agent {
                        ("agent", crate::theme::tokens(false).agent)
                    } else {
                        ("you", crate::theme::tokens(false).text_strong)
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(colour, who);
                        ui.label(&turn.text);
                    });
                    if !turn.about.is_empty() && !turn.from_agent {
                        ui.weak(format!("   about {}", turn.about.join(", ")));
                    }
                }
            });
        ui.separator();
    }

    if state.agent_log.is_empty() {
        ui.weak("no edits from outside this window yet");
        ui.add_space(6.0);
        ui.weak(
            "An agent edits through the same transactions this editor does — \
             `loom scene --tx`, `loom place --op` — so anything it changes \
             lands here, is undoable, and shows in History.",
        );
        return;
    }

    ui.horizontal(|ui| {
        ui.label(format!("{} edit(s)", state.agent_log.len()));
        if let Some(last) = state.agent_log.first() {
            ui.weak(format!("last {:.0}s ago", last.seconds_ago));
        }
    });
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        for edit in state.agent_log {
            let selected = state.selected.contains(&edit.node);
            let name = edit.node.rsplit('/').next().unwrap_or(&edit.node);
            // Fresh edits read brighter, so "what just happened" is legible
            // without reading timestamps.
            let fresh = edit.seconds_ago < 10.0;
            let row = ui.selectable_label(
                selected,
                egui::RichText::new(format!(
                    "{name}  ·  {}  ·  {:.0}s",
                    edit.kind, edit.seconds_ago
                ))
                .color(if fresh {
                    egui::Color32::from_rgb(150, 200, 255)
                } else {
                    egui::Color32::GRAY
                }),
            );
            if row.on_hover_text(&edit.node).clicked() {
                actions.push(UiAction::Select {
                    path: edit.node.clone(),
                    extend: false,
                });
            }
        }
    });
}

/// What is wrong with the scene — ADR 0093.
///
/// Clicking a row selects the node it is about, because a problem you cannot
/// navigate to is a problem you read and then go looking for by hand.
pub(crate) fn problems(ui: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    ui.heading("Problems");
    ui.separator();
    if state.problems.is_empty() {
        ui.weak("nothing to report");
        return;
    }
    let blocking = state.problems.iter().filter(|p| p.blocking).count();
    ui.horizontal(|ui| {
        if blocking > 0 {
            ui.colored_label(egui::Color32::from_rgb(230, 110, 100), format!("{blocking} error(s)"));
        }
        let warnings = state.problems.len() - blocking;
        if warnings > 0 {
            ui.weak(format!("{warnings} warning(s)"));
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for problem in state.problems {
            let marker = if problem.blocking { "✖" } else { "▲" };
            let label = if problem.node.is_empty() {
                format!("{marker} {}", problem.message)
            } else {
                format!("{marker} {}  —  {}", problem.node, problem.message)
            };
            let row = ui.selectable_label(false, label);
            if row.clicked() && !problem.node.is_empty() {
                actions.push(UiAction::Select {
                    path: problem.node.clone(),
                    extend: false,
                });
            }
        }
    });
}

pub(crate) fn transactions(ui: &mut egui::Ui, history: &[String]) {
    ui.heading("Transactions");
    ui.separator();
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .id_salt("transaction_scroll")
        .show(ui, |ui| {
            if history.is_empty() {
                ui.weak("no edits yet");
            }
            // Newest last, matching a log. Labels are why transactions carry
            // one: this is the human's window onto what changed.
            for (index, label) in history.iter().enumerate() {
                ui.label(format!("{:>3}  {label}", index + 1));
            }
        });
}

/// Outline what somebody else just changed.
///
/// **This is the point of the whole editor.** An agent is authoring the file
/// while a human watches, and until now the only signal was a console line
/// saying the scene had changed — true, and useless for finding *what*. A box
/// around the nodes that moved, fading over a few seconds, answers "what did
/// it just do" at a glance and then gets out of the way.
///
/// Deliberately not a modal, not a list to acknowledge, not a notification to
/// dismiss: the human is looking at the viewport already.
pub(crate) fn agent_overlay(root: &mut egui::Ui, state: &PanelState<'_>) {
    if state.agent_marks.is_empty() {
        return;
    }
    let ctx = root.ctx().clone();
    let scale = ctx.pixels_per_point();
    let painter = ctx.layer_painter(egui::LayerId::background());

    for mark in state.agent_marks {
        let (x0, y0, x1, y1) = mark.rect;
        let rect = egui::Rect::from_min_max(
            egui::pos2(x0 / scale, y0 / scale),
            egui::pos2(x1 / scale, y1 / scale),
        );
        // One hue for "somebody else did this", distinct from the axis colours
        // so it cannot be mistaken for a gizmo.
        let alpha = (mark.freshness.clamp(0.0, 1.0) * 220.0) as u8;
        let colour = egui::Color32::from_rgba_unmultiplied(120, 200, 255, alpha);
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.5, colour),
            egui::StrokeKind::Outside,
        );
        painter.text(
            rect.left_top() + egui::vec2(2.0, -14.0),
            egui::Align2::LEFT_TOP,
            &mark.label,
            egui::FontId::monospace(11.0),
            colour,
        );
    }
}

/// Draw the transform handles over the viewport.
///
/// In the background layer: over the 3D image, under every panel, so a handle
/// never draws on top of the inspector it is behind.
/// Draw a box round whatever is selected — ADR 0099.
///
/// **Clicking a name in the hierarchy has to show you the thing.** Selecting
/// put a gizmo at the node and marked nothing else, so on a boat of 260 parts
/// the only feedback that you had picked the right one was three small axis
/// lines somewhere in the picture.
///
/// A projected bounding box rather than a rendered outline: an outline needs a
/// stencil pass or a jump-flood, and this is twelve line segments over a
/// picture that is already drawn. It reads through geometry, which is what you
/// want when the thing you selected is inside something else.
pub(crate) fn selection_overlay(
    root: &mut egui::Ui,
    state: &PanelState<'_>,
    clip: Option<egui::Rect>,
) {
    if state.selection_edges.is_empty() {
        return;
    }
    let ctx = root.ctx().clone();
    let scale = ctx.pixels_per_point();
    let mut painter = ctx.layer_painter(egui::LayerId::background());
    if let Some(rect) = clip {
        painter.set_clip_rect(rect);
    }
    let point = |(x, y): (f32, f32)| egui::pos2(x / scale, y / scale);
    let colour = crate::theme::tokens(false).accent;
    for (from, to) in state.selection_edges {
        painter.line_segment([point(*from), point(*to)], egui::Stroke::new(1.5, colour));
    }
}

pub(crate) fn gizmo_overlay(
    root: &mut egui::Ui,
    state: &PanelState<'_>,
    clip: Option<egui::Rect>,
) {
    if state.handles.is_empty() {
        return;
    }
    let ctx = root.ctx().clone();
    // The viewport is the whole window; panels are drawn over it. So window
    // pixels map to egui points by the one scale factor, with no offset.
    let scale = ctx.pixels_per_point();
    let mut painter = ctx.layer_painter(egui::LayerId::background());
    if let Some(rect) = clip {
        painter.set_clip_rect(rect);
    }
    let point = |(x, y): (f32, f32)| egui::pos2(x / scale, y / scale);

    // **The planes under the axes**, filled faintly so they read as a surface
    // rather than another line, and tinted by the two axes they span.
    for plane in state.planes {
        let (a, b) = plane.axes;
        let blend = |x: u8, y: u8| u8::try_from((u16::from(x) + u16::from(y)) / 2).unwrap_or(x);
        let (ca, cb) = (AXIS_COLORS[a], AXIS_COLORS[b]);
        let tint = egui::Color32::from_rgba_unmultiplied(
            blend(ca.r(), cb.r()),
            blend(ca.g(), cb.g()),
            blend(ca.b(), cb.b()),
            // Faint enough to read as a surface, solid enough to aim at. At
            // 56 it was invisible against a lit scene.
            110,
        );
        let corners: Vec<egui::Pos2> = plane.corners.iter().map(|c| point(*c)).collect();
        painter.add(egui::Shape::convex_polygon(
            corners,
            tint,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(
                blend(ca.r(), cb.r()),
                blend(ca.g(), cb.g()),
                blend(ca.b(), cb.b()),
            )),
        ));
    }

    // **The rings first, under the handles.** In Rotate mode they are the
    // thing you aim at; the axis caps still sit on top so the colours agree.
    for (axis, points) in state.rings {
        let colour = AXIS_COLORS[*axis];
        for pair in points.windows(2) {
            painter.line_segment(
                [point(pair[0]), point(pair[1])],
                egui::Stroke::new(2.0, colour),
            );
        }
    }

    for handle in state.handles {
        let grabbed = state.dragging == Some(handle.axis);
        let color = AXIS_COLORS[handle.axis];
        let width = if grabbed { 4.0 } else { 2.5 };
        painter.line_segment(
            [point(handle.origin), point(handle.tip)],
            egui::Stroke::new(width, color),
        );
        // A cap the human can aim at, plus the axis letter — which mode is
        // active is in the toolbar, but which axis is which should be on the
        // handle itself.
        painter.circle_filled(point(handle.tip), if grabbed { 7.0 } else { 5.0 }, color);
        painter.text(
            point((handle.tip.0, handle.tip.1 - 16.0 * scale)),
            egui::Align2::CENTER_CENTER,
            AXIS_NAMES[handle.axis],
            egui::FontId::monospace(11.0),
            color,
        );
    }
    if let Some(first) = state.handles.first() {
        painter.circle_stroke(
            point(first.origin),
            4.0,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(220, 220, 230)),
        );
    }
}

fn inspect_transform(
    ui: &mut egui::Ui,
    path: &str,
    transform: &loom_scene::components::Transform,
    editable: bool,
    actions: &mut Vec<UiAction>,
) {
    ui.label(egui::RichText::new("Transform").strong());
    for (field, values) in [
        ("pos", transform.pos),
        ("rot_euler", transform.rot_euler),
        ("scale", transform.scale),
    ] {
        let mut edited = values;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{field:<10}")).monospace());
            let mut changed = false;
            for (axis, value) in edited.iter_mut().enumerate() {
                changed |= ui
                    .add_enabled(editable, egui::DragValue::new(value).speed(0.05))
                    .on_hover_text(AXIS_NAMES[axis])
                    .changed();
            }
            if changed {
                actions.push(UiAction::SetField(
                    path.to_owned(),
                    format!("Transform.{field}"),
                    serde_json::json!(edited),
                ));
            }
        });
    }
}

/// Fields the whole selection shares, and whether they agree on a value.
///
/// **Only components *every* selected node carries.** Offering a field that
/// half the selection lacks would either write it onto nodes that should not
/// have it or silently skip them, and both are worse than not offering it.
pub(crate) fn common_fields(
    scene: &loom_scene::Scene,
    selected: &[String],
) -> Vec<(String, String, serde_json::Value, bool)> {
    let nodes: Vec<&loom_scene::Node> = selected
        .iter()
        .filter_map(|p| scene.nodes().iter().find(|n| &n.path == p))
        .collect();
    let Some((first, rest)) = nodes.split_first() else {
        return Vec::new();
    };
    if rest.len() + 1 != selected.len() {
        // A path that resolved to nothing: the selection is mid-reload and
        // half of it does not exist yet. Offering an edit against that would
        // write to whichever nodes happened to be present.
        return Vec::new();
    }

    let mut out = Vec::new();
    for (type_name, value) in &first.components {
        let Some(fields) = value.as_object() else { continue };
        for (field, current) in fields {
            let others: Vec<Option<&serde_json::Value>> = rest
                .iter()
                .map(|n| n.components.get(type_name).and_then(|c| c.get(field)))
                .collect();
            if others.iter().any(Option::is_none) {
                continue;
            }
            let agreed = others.iter().all(|v| *v == Some(current));
            out.push((type_name.clone(), field.clone(), current.clone(), agreed));
        }
    }
    out
}

/// The inspector for more than one node.
///
/// Edits fan out: one `SetField` per selected node, all inside the same
/// transaction the caller builds, so a multi-edit is **one** undo step. That is
/// never-do #16 — the editor issues the same ops the agent does — and it is why
/// this returns actions rather than writing anything.
fn multi_inspector(ui: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    ui.label(format!("{} nodes selected", state.selected.len()));
    ui.weak("Move, duplicate and delete apply to all of them.");
    ui.add_space(8.0);

    let shared = common_fields(state.scene, state.selected);
    if shared.is_empty() {
        ui.weak("no components in common");
        return;
    }
    // Values across a selection — live while playing, like the single
    // inspector. See `fields_editable`.
    let editing = fields_editable(state);
    let empty = std::collections::BTreeSet::new();
    let ctx = FieldContext { assets: state.assets, overridden: &empty };

    egui::ScrollArea::vertical().show(ui, |ui| {
        let mut last_type = "";
        for (type_name, field, current, agreed) in &shared {
            if type_name != last_type {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(type_name).strong());
                last_type = type_name;
            }
            let key = format!("{type_name}.{field}");
            // **A disagreeing field is shown, not hidden.** Hiding it would
            // make the panel's contents depend on values rather than on types,
            // so a field would appear and disappear as the human scrubbed it.
            // The marker says the values differ; editing sets them all.
            let mut fanned = Vec::new();
            ui.horizontal(|ui| {
                field_label(ui, field, "", None);
                if !agreed {
                    ui.label(egui::RichText::new("≠").weak())
                        .on_hover_text("these nodes do not agree — editing sets all of them");
                }
                // The first node's value stands in for the group, which is why
                // the marker matters: without it a mixed field would look like
                // a settled one.
                draw_field(
                    ui, &state.selected[0], &key, field, current, None, editing, &ctx, &mut fanned,
                );
            });
            // One action per node, from whatever the widget produced for the
            // first. `Splice` and `RevertOverride` are deliberately not fanned
            // out: an index into one node's array means nothing in another's.
            for action in fanned {
                if let UiAction::SetField(_, key, value) = action {
                    for path in state.selected {
                        actions.push(UiAction::SetField(path.clone(), key.clone(), value.clone()));
                    }
                }
            }
        }
    });
}

/// How wide the label column is, so every field's control starts at the same x.
///
/// Ragged left edges are what make a generated inspector look generated: the
/// widget for `roughness` began 40 px right of the one for `uv` purely because
/// the word is longer, and the eye reads that as disorder rather than as
/// hierarchy.
const LABEL_WIDTH: f32 = 96.0;

/// Does this field hold a colour rather than three unrelated numbers?
///
/// **A heuristic, and deliberately a narrow one.** The schema cannot answer it:
/// `albedo` and a position are both `[f32; 3]` with identical JSON Schema, so
/// there is nothing to read. Rather than guess from the value — a position that
/// happens to sit in 0..1 would sprout a colour picker and then stop having one
/// when the node moved — this matches the name, which is stable.
///
/// The drag row is drawn either way. The swatch is *added*, never substituted,
/// so a wrong guess costs a redundant button rather than an uneditable field.
fn looks_like_a_colour(field: &str) -> bool {
    const NAMES: [&str; 6] = ["color", "colour", "albedo", "tint", "emissive", "sky"];
    let lower = field.to_ascii_lowercase();
    NAMES.iter().any(|n| lower.contains(n))
}

/// The constraint line that goes last in a tooltip.
///
/// Last rather than first: the doc comment says what the field *means*, which
/// is what someone hovering wants; the range is what they want a second later,
/// when the slider will not go where they are pushing it.
fn range_note(schema: Option<&serde_json::Value>) -> Option<String> {
    let s = schema?;
    let lo = s.get("minimum").and_then(serde_json::Value::as_f64);
    let hi = s.get("maximum").and_then(serde_json::Value::as_f64);
    match (lo, hi) {
        (Some(lo), Some(hi)) => Some(format!("range: {lo} to {hi}")),
        (Some(lo), None) => Some(format!("minimum: {lo}")),
        (None, Some(hi)) => Some(format!("maximum: {hi}")),
        (None, None) => None,
    }
}

/// The label column of one field row, with its tooltip.
fn field_label(ui: &mut egui::Ui, field: &str, doc: &str, constraint: Option<String>) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(LABEL_WIDTH, ui.spacing().interact_size.y),
        egui::Sense::hover(),
    );
    let mut label = ui.put(
        rect,
        egui::Label::new(egui::RichText::new(field).monospace())
            .halign(egui::Align::LEFT)
            .truncate(),
    );
    // **The name leads the tooltip, because the label truncates.** `LABEL_WIDTH`
    // is fixed, so `surface_height` reads `surface_heig…` at the default panel
    // width — and a field with no doc comment had no hover at all, leaving no
    // way to find out what it was called without widening the panel.
    let tip = match (doc.is_empty(), constraint) {
        (true, None) => field.to_owned(),
        (true, Some(c)) => format!("{field}\n\n{c}"),
        (false, None) => format!("{field}\n\n{doc}"),
        (false, Some(c)) => format!("{field}\n\n{doc}\n\n{c}"),
    };
    label = label.on_hover_text(tip);
    let _ = label;
}

/// One component's fields, generated from its schema.
///
/// **Nothing here is hand-written per component type.** A new component gets an
/// inspector the same way it gets a JSON Schema and a CLI `describe`: for free,
/// with its ranges enforced and its doc comment as the tooltip. A hand-written
/// inspector is a second description of every type and it drifts by Thursday.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn inspect_component(
    ui: &mut egui::Ui,
    path: &str,
    type_name: &str,
    value: &serde_json::Value,
    registry: &loom_reflect::TypeRegistry,
    editable: bool,
    ctx: &FieldContext<'_>,
    actions: &mut Vec<UiAction>,
) {
    let root = registry.describe(type_name).map(loom_reflect::SchemaHandle::as_value);
    let properties = root
        .and_then(|s| s.get("properties"))
        .and_then(serde_json::Value::as_object);

    let Some(fields) = value.as_object() else {
        return;
    };

    // **Every field the component has, not only the ones on disk.**
    //
    // The loop used to walk the node's *stored* table, so a field the author
    // never wrote simply did not appear — and TOML cannot hold a null, so every
    // optional field of every component was invisible in the editor by
    // construction. `WaterBody` showed seven of its sixteen; `swell`, `fetch`,
    // `flow` and `extent` were not "read-only", they were absent, and there was
    // no way to reach them at all. Unity, Unreal and Godot all show a
    // component's whole surface, and so must this.
    //
    // Both orders are alphabetical — `serde_json::Map` here, schemars'
    // properties there — so the union interleaves rather than appending a
    // second block, and a field appears where it belongs. Unknown keys the
    // schema does not name still show: they are in the file, so the human has
    // to be able to see them.
    let unwritten: Vec<&String> = properties
        .map(|p| p.keys().filter(|k| !fields.contains_key(*k)).collect())
        .unwrap_or_default();
    let mut rows: Vec<(&String, Option<&serde_json::Value>)> = fields
        .iter()
        .map(|(field, current)| (field, Some(current)))
        .chain(unwritten.into_iter().map(|field| (field, None)))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));

    for (field, stored) in rows {
        // A field not in the file has whatever the schema declares — which for
        // an optional field is `null`, and that is what "not set" means.
        let default = properties
            .and_then(|p| p.get(field))
            .and_then(|spec| spec.get("default"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let current = stored.unwrap_or(&default);
        let key = format!("{type_name}.{field}");
        // One walker, shared with the validator: `$ref` followed through
        // `$defs` and both spellings of an enum flattened. See
        // `loom_reflect::field_schema`.
        let resolved = match (root, properties.and_then(|p| p.get(field))) {
            (Some(r), Some(f)) => Some(resolve_items(r, loom_reflect::field_schema(r, f))),
            _ => None,
        };
        let schema = resolved.as_deref();
        let doc = schema
            .and_then(|f| f.get("description"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");

        ui.horizontal(|ui| {
            field_label(ui, field, doc, range_note(schema));

            // **An overridden field is marked and revertable.** `RevertOverrides`
            // has existed since S4 and the editor had never issued it, so the
            // only way back to the prefab's value was to edit the file.
            if ctx.overridden.contains(&key)
                && ui
                    .small_button("●")
                    .on_hover_text("overridden — click to revert to the prefab")
                    .clicked()
            {
                actions.push(UiAction::RevertOverride(path.to_owned(), key.clone()));
            }

            draw_field(ui, path, &key, field, current, schema, editable, ctx, actions);
        });
    }
}

/// A field schema with its array `items` `$ref` followed.
///
/// **`field_schema` resolves the field's own `$ref` and stops.** An array of
/// structs — `Environment.stages`, `Buoyancy.pontoons`, `Scatter.exclude` —
/// therefore arrived with `items: { "$ref": "#/$defs/MoodStage" }`, and every
/// question anything downstream asks of an entry ("does it have properties?",
/// "does it carry an `at`?", "what is this number's range?") got no answer from
/// a reference it could not follow. Two of those questions decide whether a
/// button corrupts the scene, so this is not cosmetic.
fn resolve_items<'a>(
    root: &'a serde_json::Value,
    schema: std::borrow::Cow<'a, serde_json::Value>,
) -> std::borrow::Cow<'a, serde_json::Value> {
    let Some(items) = schema.get("items") else {
        return schema;
    };
    if items.get("$ref").is_none() {
        return schema;
    }
    let resolved = loom_reflect::field_schema(root, items).into_owned();
    let mut owned = schema.into_owned();
    if let Some(object) = owned.as_object_mut() {
        object.insert("items".to_owned(), resolved);
    }
    std::borrow::Cow::Owned(owned)
}

/// Does this array hold numbers — a vector, a colour, an attenuation triple?
///
/// **An empty array cannot answer for itself.** `Iterator::all` is vacuously
/// true on nothing, so `[]` matched "all numbers", drew zero drag boxes, and
/// left a blank row with no way to add anything to it. The schema knows: an
/// array of `MoodStage` or of voxel ops declares object items (or `items: true`,
/// the untyped union a voxel recipe is), and only a numeric one declares
/// `number`. So an empty array is routed by its declaration and a populated one
/// by what is in it.
#[must_use]
pub fn array_is_numeric(items: &[serde_json::Value], schema: Option<&serde_json::Value>) -> bool {
    if !items.is_empty() {
        return items.iter().all(serde_json::Value::is_number);
    }
    schema
        .and_then(|s| s.get("items"))
        .and_then(|i| i.get("type"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|t| matches!(t, "number" | "integer"))
}

/// The value a "set" button should write into an absent optional field.
///
/// **The schema's own `minimum`, never a number chosen here.** The smallest
/// value the field's author declared legal is a real answer; anything else would
/// be this layer inventing a quantity it knows nothing about. `Some` only for a
/// scalar — a nullable object's fields may be refused rather than defaulted, so
/// there is nothing safe to write and the caller offers no button at all.
#[must_use]
pub fn optional_start(schema: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    // `type: ["number", "null"]` is how schemars spells an `Option<f32>`. An
    // `Option<SomeStruct>` is an `anyOf` instead, and falls out here.
    let types = schema?.get("type")?.as_array()?;
    let nullable = types.iter().any(|t| t.as_str() == Some("null"));
    let scalar = types.iter().find_map(|t| match t.as_str()? {
        "null" => None,
        other => Some(other),
    })?;
    if !nullable {
        return None;
    }
    match scalar {
        "number" | "integer" => {
            let lo = schema?
                .get("minimum")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
            serde_json::Number::from_f64(lo).map(serde_json::Value::Number)
        }
        "boolean" => Some(serde_json::Value::Bool(false)),
        // A nullable string has no smallest legal value, and an empty one is a
        // guess about whether empty is allowed.
        _ => None,
    }
}

/// Everything the widgets need that is not the field itself.
pub(crate) struct FieldContext<'a> {
    /// Asset aliases, for the reference picker.
    pub assets: &'a [String],
    /// `Type.field` keys this node overrides, when it instances a prefab.
    pub overridden: &'a std::collections::BTreeSet<String>,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn draw_field(
    ui: &mut egui::Ui,
    path: &str,
    key: &str,
    field: &str,
    current: &serde_json::Value,
    schema: Option<&serde_json::Value>,
    editable: bool,
    ctx: &FieldContext<'_>,
    actions: &mut Vec<UiAction>,
) {
    let set = |v: serde_json::Value| UiAction::SetField(path.to_owned(), key.to_owned(), v);

    // An enum is a dropdown, whichever way schemars spelled it. Typing into a
    // text box and having the validator reject it is the failure this removes.
    if let Some(variants) = schema
        .and_then(|s| s.get("enum"))
        .and_then(serde_json::Value::as_array)
        && let Some(chosen) = current.as_str()
    {
        let mut picked = chosen.to_owned();
        egui::ComboBox::from_id_salt((path, key))
            .selected_text(&picked)
            .show_ui(ui, |ui| {
                for variant in variants {
                    if let Some(name) = variant.as_str() {
                        ui.selectable_value(&mut picked, name.to_owned(), name);
                    }
                }
            });
        if editable && picked != chosen {
            actions.push(set(serde_json::json!(picked)));
        }
        return;
    }

    match current {
        serde_json::Value::Number(n) => {
            let mut v = n.as_f64().unwrap_or(0.0);
            let bounds = (
                schema.and_then(|s| s.get("minimum")).and_then(serde_json::Value::as_f64),
                schema.and_then(|s| s.get("maximum")).and_then(serde_json::Value::as_f64),
            );
            let widget = match bounds {
                (Some(lo), Some(hi)) => {
                    // **The empty half of the track has to be visible.**
                    // `slider_trailing_fill` paints only the part left of the
                    // handle, and the rest was `sunken` — 0x0F1216 against a
                    // panel barely lighter, so a slider near the bottom of its
                    // range read as a stray mark and one at zero as nothing at
                    // all. `Environment.ambient` is 0.45 of 0..4: eleven per
                    // cent lit, and the other 89% invisible.
                    let track = crate::theme::tokens(false).line_strong;
                    ui.visuals_mut().widgets.inactive.bg_fill = track;
                    ui.visuals_mut().widgets.hovered.bg_fill = track;
                    ui.visuals_mut().widgets.active.bg_fill = track;
                    ui.add_enabled(editable, egui::Slider::new(&mut v, lo..=hi))
                }
                _ => ui.add_enabled(editable, egui::DragValue::new(&mut v).speed(0.1)),
            };
            if widget.changed() {
                actions.push(set(serde_json::json!(v)));
            }
        }

        serde_json::Value::Bool(b) => {
            let mut v = *b;
            if ui.add_enabled(editable, egui::Checkbox::new(&mut v, "")).changed() {
                actions.push(set(serde_json::json!(v)));
            }
        }

        // **Strings are editable now, and this was the single most limiting
        // gap.** `Script.path`, `GameRules.path`, `Name.value` and every enum
        // this project has not documented were read-only labels, so the
        // inspector could show you what your script was and never let you
        // change it.
        serde_json::Value::String(s) => {
            let buffer_id = egui::Id::new(("field", path, key));
            let mut text = ui
                .data_mut(|d| d.get_temp::<String>(buffer_id))
                .unwrap_or_else(|| s.clone());
            // The file is the truth: if it changed under us and the human is
            // not mid-edit, follow it.
            let focused = ui.memory(|m| m.has_focus(buffer_id));
            if !focused && text != *s {
                text = s.clone();
            }
            let response = ui.add_enabled(
                editable,
                egui::TextEdit::singleline(&mut text).id(buffer_id).desired_width(f32::INFINITY),
            );
            if response.changed() {
                ui.data_mut(|d| d.insert_temp(buffer_id, text.clone()));
            }
            // On commit, not per keystroke: a transaction per character would
            // bury the transaction log and make undo useless. Same rule the
            // rename field has followed since M12.
            let committed = response.lost_focus()
                && (ui.input(|i| i.key_pressed(egui::Key::Enter)) || !response.has_focus());
            if committed && text != *s {
                actions.push(set(serde_json::json!(text)));
                ui.data_mut(|d| d.remove::<String>(buffer_id));
            }
        }

        // An empty numeric array draws no boxes, so say so rather than leaving a
        // blank line. A fixed-size vector is never empty in practice, which is
        // why this is a label and not an editor.
        serde_json::Value::Array(items) if items.is_empty() && array_is_numeric(items, schema) => {
            ui.add(egui::Label::new(
                egui::RichText::new("empty").weak().italics(),
            ))
            .on_hover_text("a list of numbers with nothing in it — author it with `loom scene --tx`");
        }

        serde_json::Value::Array(items) if array_is_numeric(items, schema) => {
            let mut edited: Vec<f64> =
                items.iter().filter_map(serde_json::Value::as_f64).collect();
            let (lo, hi) = schema
                .and_then(|f| f.get("items"))
                .map(|i| {
                    (
                        i.get("minimum").and_then(serde_json::Value::as_f64),
                        i.get("maximum").and_then(serde_json::Value::as_f64),
                    )
                })
                .unwrap_or((None, None));

            // The swatch first, so it lands at the column edge where the eye
            // is already looking. Added to the drags, never replacing them —
            // see `looks_like_a_colour`.
            if looks_like_a_colour(field) && (edited.len() == 3 || edited.len() == 4) {
                #[allow(clippy::cast_possible_truncation)]
                let mut rgb = [edited[0] as f32, edited[1] as f32, edited[2] as f32];
                // **Authored linear, shown as sRGB.** `Material::albedo` is a
                // linear value; handing it to a picker raw makes a mid grey
                // look nearly black and every colour a human picks come out
                // wrong. egui's `rgb` picker is the sRGB one.
                let mut srgb = rgb.map(|c| c.clamp(0.0, 1.0).powf(1.0 / 2.2));
                if ui.add_enabled(editable, ColourButton(&mut srgb)).changed() {
                    rgb = srgb.map(|c| c.powf(2.2));
                    let mut next: Vec<f64> = rgb.iter().map(|c| f64::from(*c)).collect();
                    // A four-element colour keeps whatever the fourth
                    // component meant — on `albedo` it is porosity, not alpha,
                    // and a picker must not silently overwrite it.
                    if edited.len() == 4 {
                        next.push(edited[3]);
                    }
                    actions.push(set(serde_json::json!(next)));
                }
            }

            let mut changed = false;
            for v in &mut edited {
                let widget = match (lo, hi) {
                    (Some(lo), Some(hi)) => egui::DragValue::new(v).range(lo..=hi).speed(0.02),
                    _ => egui::DragValue::new(v).speed(0.05),
                };
                changed |= ui.add_enabled(editable, widget).changed();
            }
            if changed {
                actions.push(set(serde_json::json!(edited)));
            }
        }

        // An asset reference: `{ "asset": "alias" }`. A dropdown of what this
        // scene actually declared, rather than a name to be typed correctly.
        serde_json::Value::Object(map)
            if map.len() == 1 && map.contains_key("asset") =>
        {
            let chosen = map["asset"].as_str().unwrap_or("");
            let mut picked = chosen.to_owned();
            egui::ComboBox::from_id_salt((path, key))
                .selected_text(if picked.is_empty() { "<none>" } else { &picked })
                .show_ui(ui, |ui| {
                    for alias in ctx.assets {
                        ui.selectable_value(&mut picked, alias.clone(), alias);
                    }
                });
            if editable && picked != chosen {
                actions.push(set(serde_json::json!({ "asset": picked })));
            }
        }

        // **An array of objects gets rows and a splice, not a JSON blob.**
        // `Buoyancy.pontoons`, `Scatter.exclude`, `Environment.stages` and a
        // voxel recipe were all a single unreadable line that could only be
        // edited in a text editor.
        //
        // **Not `WaterBody.waves`**, which this comment used to claim. That
        // field is a `WaveSet` *object*, so it takes the nested-object arm
        // below; its `waves` array is one level further down, where
        // `object_fields` edits only scalars and small numeric arrays. So the
        // per-wave Gerstner list is still text here, and `SetField` cannot
        // address `WaterBody.waves.waves` either — it splits the field name once
        // and treats the rest as a literal key. Authoring individual Gerstner
        // waves is `loom scene --tx` work until that changes.
        // **A nested object is edited, not printed.** `WaterBody.swell` — the
        // three numbers that *are* the FFT sea — reached the inspector as the
        // literal text `{"direction":[0.25881904,0.9659258],"fetch":420000.0,
        // "u10":18.0}` and could not be changed. So could `optics`, `flow` and
        // `waves`. The one thing this editor exists to do is change a value,
        // and for a whole class of fields it could only describe one.
        //
        // The whole object is written back on any change rather than a dotted
        // sub-path: `SetField` splits its field name once and treats the
        // remainder as a literal TOML key, so `WaterBody.swell.u10` would write
        // a key spelled `swell.u10` instead of reaching into the table.
        serde_json::Value::Object(map) if !map.is_empty() => {
            if let Some(edited) = object_fields(ui, current, editable, schema, key) {
                actions.push(set(edited));
            }
        }

        // **Absent, said plainly — and settable where that is honest.** `null`
        // used to fall through to the summary, printing the word `null` in the
        // same weak grey a real value uses.
        //
        // A scalar optional gets a button, because the schema already names a
        // value nobody has to invent: its `minimum`, the smallest thing its
        // author declared legal. `Rain.duration` becomes a shower of zero
        // seconds you drag out; `WaterBody.fetch` becomes one metre you drag up.
        //
        // An optional *object* does not, and that is not laziness. `Swell`'s
        // fields are documented as "refused at load rather than defaulted into
        // silence" — its defaults are deliberately values no sea can have — so
        // any value this button could write would be rejected, and a button that
        // always fails is worse than none. Copying the component from a node
        // that has one is the path, and the hover says so.
        serde_json::Value::Null => {
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(egui::RichText::new("not set").weak().italics()));
                match optional_start(schema) {
                    Some(start) => {
                        if ui
                            .add_enabled(editable, egui::Button::new("set").small())
                            .on_hover_text(format!(
                                "give it a value — starts at {start}, then drag"
                            ))
                            .clicked()
                        {
                            actions.push(set(start));
                        }
                    }
                    None => {
                        // **Two different reasons, said apart.** A nullable
                        // *group* has no safe value because its own fields are
                        // refused rather than defaulted; a nullable *list* has
                        // no safe value because nobody declared what one entry
                        // should start as. Telling a human that `extent` has
                        // "fields refused at load" is a confident wrong answer
                        // about a two-component vector.
                        let list = schema
                            .and_then(|s| s.get("type"))
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|types| {
                                types.iter().any(|t| t.as_str() == Some("array"))
                            });
                        ui.weak("?").on_hover_text(if list {
                            "an optional list with no value, and no declared \
                             starting entry — author it with `loom scene --tx`, \
                             or copy the component from a node that has one."
                        } else {
                            "an optional group with no value. Its fields are refused \
                             rather than defaulted, so there is nothing safe to write \
                             here — copy the component from a node that has one, or \
                             author it with `loom scene --tx`."
                        });
                    }
                }
            });
        }

        // Everything else array-shaped, **including an empty one**. `all(..)` is
        // vacuously true on an empty iterator, so `[]` used to match the numeric
        // arm above, draw zero drag boxes, and render as a blank dead row with
        // no "+ add" anywhere — which is every `Environment.stages` before its
        // first stage, every `VoxelVolume.ops` before its first op, and every
        // object-array whose last entry you just deleted. `array_is_numeric`
        // sends an empty array here by asking the schema what its items are.
        serde_json::Value::Array(items) if items.iter().all(serde_json::Value::is_object) => {
            array_of_objects(ui, path, key, items, schema, editable, actions);
        }

        _ => {
            ui.add(egui::Label::new(egui::RichText::new(summarise(current)).weak()).wrap());
        }
    }
}

/// Rows for an array of objects, each removable, with an add button.
///
/// Every mutation is a `SpliceArray`, so the entries the human did not touch
/// keep their comments and their `[[header]]` spelling on disk.
/// Edit the fields of one object inside an array — ADR 0098.
///
/// **Returns the changed object rather than pushing an action**, because the
/// caller has to put it back with a `Splice`: the op that edits an array
/// element replaces the whole element, and that is deliberate — it keeps the
/// human's formatting and comments in the file around it (see `SpliceArray`).
///
/// Typed off the JSON, not the schema. A number gets a drag box, a bool a
/// checkbox, a short array of numbers a row of boxes — which covers waves,
/// pontoons, voxel ops and mood stages. A nested object stays a summary; one
/// level of recursion is where this stops being an inspector and starts being
/// a JSON editor.
fn object_fields(
    ui: &mut egui::Ui,
    item: &serde_json::Value,
    editable: bool,
    schema: Option<&serde_json::Value>,
    salt: &str,
) -> Option<serde_json::Value> {
    let object = item.as_object()?;
    let mut edited = object.clone();
    let mut changed = false;

    // **Salted, because a component may carry two of these.** A `WaterBody` has
    // `swell` and `optics` side by side, and two grids under one id is the
    // duplicate-id error egui draws as red text across the panel.
    egui::Grid::new(("entry", salt)).num_columns(2).spacing([8.0, 3.0]).show(ui, |ui| {
        // The sub-schema, so a nested number gets the same range and the same
        // tooltip it would get at the top level. Without it every nested field
        // is an unbounded drag and the human is guessing at units.
        let properties = schema
            .and_then(|s| s.get("properties"))
            .and_then(serde_json::Value::as_object);
        // **The same union the top level does** — a nested optional field is
        // invisible for the same reason its parent's were: TOML cannot write a
        // null, so a `Swell` authored without `fetch` (legal — absent means
        // unlimited fetch) showed two of its three fields and no way to reach
        // the third.
        let unwritten: Vec<&String> = properties
            .map(|p| p.keys().filter(|k| !object.contains_key(*k)).collect())
            .unwrap_or_default();
        let mut rows: Vec<(&String, Option<&serde_json::Value>)> = object
            .iter()
            .map(|(key, value)| (key, Some(value)))
            .chain(unwritten.into_iter().map(|key| (key, None)))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(b.0));

        for (key, stored) in rows {
            let spec = properties.and_then(|p| p.get(key));
            let absent = spec
                .and_then(|f| f.get("default"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let value = stored.unwrap_or(&absent);
            let doc = spec
                .and_then(|f| f.get("description"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let bounds = (
                spec.and_then(|f| f.get("minimum")).and_then(serde_json::Value::as_f64),
                spec.and_then(|f| f.get("maximum")).and_then(serde_json::Value::as_f64),
            );
            let label = ui.label(egui::RichText::new(key).monospace().small());
            if !doc.is_empty() {
                label.on_hover_text(doc);
            }
            match value {
                serde_json::Value::Bool(b) => {
                    let mut on = *b;
                    if ui.add_enabled(editable, egui::Checkbox::new(&mut on, "")).changed() {
                        edited.insert(key.clone(), serde_json::Value::Bool(on));
                        changed = true;
                    }
                }
                serde_json::Value::Number(n) => {
                    let mut v = n.as_f64().unwrap_or_default();
                    let widget = match bounds {
                        (Some(lo), Some(hi)) => {
                            egui::DragValue::new(&mut v).range(lo..=hi).speed(0.01)
                        }
                        _ => egui::DragValue::new(&mut v).speed(0.01),
                    };
                    if ui.add_enabled(editable, widget).changed()
                        && let Some(number) = serde_json::Number::from_f64(v)
                    {
                        edited.insert(key.clone(), serde_json::Value::Number(number));
                        changed = true;
                    }
                }
                serde_json::Value::String(text) => {
                    let mut buffer = text.clone();
                    if ui
                        .add_enabled(editable, egui::TextEdit::singleline(&mut buffer))
                        .changed()
                    {
                        edited.insert(key.clone(), serde_json::Value::String(buffer));
                        changed = true;
                    }
                }
                serde_json::Value::Array(items)
                    if items.len() <= 4 && items.iter().all(serde_json::Value::is_number) =>
                {
                    let mut numbers: Vec<f64> =
                        items.iter().filter_map(serde_json::Value::as_f64).collect();
                    let mut row = false;
                    ui.horizontal(|ui| {
                        for slot in &mut numbers {
                            row |= ui
                                .add_enabled(
                                    editable,
                                    egui::DragValue::new(slot).speed(0.01).max_decimals(4),
                                )
                                .changed();
                        }
                    });
                    if row {
                        edited.insert(
                            key.clone(),
                            serde_json::Value::Array(
                                numbers
                                    .into_iter()
                                    .filter_map(serde_json::Number::from_f64)
                                    .map(serde_json::Value::Number)
                                    .collect(),
                            ),
                        );
                        changed = true;
                    }
                }
                // **The same affordance the top level has.** A nested optional
                // is `null` here for exactly the reason its parent's was:
                // nothing wrote it, and TOML has no way to.
                serde_json::Value::Null => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(
                            egui::RichText::new("not set").weak().italics().small(),
                        ));
                        if let Some(start) = optional_start(spec)
                            && ui
                                .add_enabled(editable, egui::Button::new("set").small())
                                .on_hover_text(format!("starts at {start}, then drag"))
                                .clicked()
                        {
                            edited.insert(key.clone(), start);
                            changed = true;
                        }
                    });
                }
                other => {
                    ui.add(egui::Label::new(
                        egui::RichText::new(summarise(other)).weak().small(),
                    ));
                }
            }
            ui.end_row();
        }
    });

    changed.then_some(serde_json::Value::Object(edited))
}

/// Where a new entry goes in an array the format keeps sorted — ADR 0101.
///
/// **Appending to `Environment.stages` always failed.** Its entries carry an
/// `at` on 0..=1 and must be strictly increasing, so "+ add" put a default
/// entry with `at = 0` after one with `at = 1` and the op layer refused it,
/// every time, on every scene. The button was not broken so much as
/// permanently wrong, and the console said `sorted by at, strictly increasing`
/// while the human looked at a list that would not grow.
///
/// So: find the widest gap between neighbouring values — counting the run from
/// 0 to the first and from the last to 1 — and put the new entry in the middle
/// of it. That is both always valid and what somebody adding a stage means: a
/// new one between the ones already there.
///
/// `None` when the entries carry no `at`, which is every other array and where
/// appending is right.
#[must_use]
pub fn sorted_insertion(
    items: &[serde_json::Value],
    entry: Option<&serde_json::Value>,
) -> Option<(usize, f64)> {
    // **An empty array cannot say what it holds, so the schema must.** This
    // returned `Some((0, 0.0))` for *any* empty array, so "+ add" injected an
    // `at` — a `MoodStage` field — into the blank entry of every empty array in
    // the engine. On `VoxelVolume.ops` that produced `ops = [{ at = 0.0 }]`,
    // which the op layer accepted and `loom validate` then refused: a button
    // that wrote a scene the engine will not load.
    let declares_at = entry
        .and_then(|e| e.get("properties"))
        .and_then(serde_json::Value::as_object)
        .is_some_and(|p| p.contains_key("at"));
    if items.is_empty() {
        return declares_at.then_some((0, 0.0));
    }
    let ats: Vec<f64> = items
        .iter()
        .map(|item| item.get("at").and_then(serde_json::Value::as_f64))
        .collect::<Option<Vec<_>>>()?;
    if ats.is_empty() {
        return Some((0, 0.0));
    }

    // Every gap, including the ends, as (index to insert at, low, high).
    let mut gaps = vec![(0usize, 0.0, ats[0])];
    for (i, pair) in ats.windows(2).enumerate() {
        gaps.push((i + 1, pair[0], pair[1]));
    }
    gaps.push((ats.len(), ats[ats.len() - 1], 1.0));

    let (index, low, high) = gaps
        .into_iter()
        .max_by(|a, b| (a.2 - a.1).total_cmp(&(b.2 - b.1)))?;
    // A zero-width gap has no room; the caller gets the midpoint anyway and the
    // op layer refuses it, which is the honest outcome for a full array.
    Some((index, f64::midpoint(low, high)))
}

fn array_of_objects(
    ui: &mut egui::Ui,
    path: &str,
    key: &str,
    items: &[serde_json::Value],
    schema: Option<&serde_json::Value>,
    editable: bool,
    actions: &mut Vec<UiAction>,
) {
    ui.vertical(|ui| {
        for (index, item) in items.iter().enumerate() {
            ui.horizontal(|ui| {
                // The most identifying field first: a voxel op's `kind`, a
                // wave's `kind`. "3 items" tells you nothing about which one
                // to delete.
                let title = item
                    .get("kind")
                    .or_else(|| item.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map_or_else(|| format!("[{index}]"), |k| format!("[{index}] {k}"));
                ui.label(egui::RichText::new(title).monospace());
                // **Move controls on the row they move** — ADR 0099. A separate
                // row of "0 up / 1 up / 2 up" buttons was functional and unlike
                // any editor anyone has used.
                if editable && items.len() > 1 {
                    let mut swap = None;
                    if ui
                        .add_enabled(index > 0, egui::Button::new("^").small())
                        .on_hover_text("move up")
                        .clicked()
                    {
                        swap = Some(index - 1);
                    }
                    if ui
                        .add_enabled(index + 1 < items.len(), egui::Button::new("v").small())
                        .on_hover_text("move down")
                        .clicked()
                    {
                        swap = Some(index + 1);
                    }
                    if let Some(with) = swap {
                        // The whole array in one op: remove-then-insert is two
                        // splices whose indices shift under each other, and two
                        // undo steps for one click.
                        let mut reordered = items.to_vec();
                        reordered.swap(index, with);
                        actions.push(UiAction::Splice(
                            path.to_owned(),
                            key.to_owned(),
                            0,
                            items.len(),
                            reordered,
                        ));
                    }
                }
                if editable
                    && ui.small_button("✖").on_hover_text("remove this entry").clicked()
                {
                    actions.push(UiAction::Splice(
                        path.to_owned(),
                        key.to_owned(),
                        index,
                        1,
                        Vec::new(),
                    ));
                }
            });
            // **Collapsed by default.** `Environment.stages` has three entries
            // of a dozen fields; opening all of them at once buries the rest of
            // the inspector.
            egui::CollapsingHeader::new(egui::RichText::new("fields").weak().small())
                .id_salt((path, key, index))
                .show(ui, |ui| {
                    // The entry schema, so a stage's fields carry their
                    // ranges too. `items` is where an array's element schema
                    // lives; `field_schema` has already followed any `$ref`.
                    let entry = schema.and_then(|s| s.get("items"));
                    let salt = format!("{key}[{index}]");
                    if let Some(updated) = object_fields(ui, item, editable, entry, &salt) {
                        // One element, replaced in place — the array around it
                        // keeps its comments and its order.
                        actions.push(UiAction::Splice(
                            path.to_owned(),
                            key.to_owned(),
                            index,
                            1,
                            vec![updated],
                        ));
                    }
                });
        }
        // **Only where the editor can build an entry the engine will load.**
        // `default_entry` writes the item schema's required fields; an item
        // schema with no `properties` at all gives it nothing, and `{}` is not a
        // legal entry everywhere. Measured on every object-array in the engine:
        // `Buoyancy.pontoons` and `Scatter.exclude` accept `{}`,
        // `Environment.stages` takes its `at`, and `VoxelVolume.chunks`
        // (`[i32; 3]`, not a struct) and `VoxelVolume.ops` (`Vec<Value>`, whose
        // vocabulary lives in `loom_voxel` where `loom_scene` cannot reach it)
        // do not — `ops = [{}]` applies and then fails `loom validate` with
        // `missing field 'kind'`.
        //
        // The op layer cannot catch this: it re-parses with `Scene::parse`,
        // while the voxel-op check lives in the CLI on the far side of a
        // dependency boundary. So the button is the place to be honest.
        let buildable = schema
            .and_then(|s| s.get("items"))
            .and_then(|i| i.get("properties"))
            .is_some();
        if !buildable {
            ui.add_enabled(false, egui::Button::new("+ add").small())
                .on_disabled_hover_text(
                    "this list's entries are not described by the schema, so the \
                     editor cannot write one that will load — author it with \
                     `loom scene --tx`",
                );
        }
        if buildable && editable && ui.small_button("+ add").clicked() {
            // A new entry at the schema's defaults, which for an untyped
            // recipe is an empty table the human then fills in. Appending
            // something invalid would be worse: the transaction would be
            // rejected and the button would look broken.
            let mut blank = default_entry(schema);
            // A sorted array gets the entry placed, not appended — ADR 0101.
            let at = sorted_insertion(items, schema.and_then(|s| s.get("items")));
            if let Some((_, value)) = at
                && let Some(object) = blank.as_object_mut()
                && let Some(number) = serde_json::Number::from_f64(value)
            {
                object.insert("at".to_owned(), serde_json::Value::Number(number));
            }
            // **A mood ladder starts with two rungs, because one is refused.**
            // `check_moods` says "at least two stages, or none" — a ladder with
            // a single rung is a constant, and the format would rather say that
            // by having no stages at all. So the first click could never
            // succeed: it inserted exactly one and the transaction was rejected
            // every time, on every scene. The pair is the whole axis, 0 and 1,
            // which is also what somebody adding their first stages means.
            let mut entries = vec![blank.clone()];
            if items.is_empty()
                && at.is_some()
                && let Some(object) = blank.as_object_mut()
            {
                object.insert("at".to_owned(), serde_json::json!(1.0));
                entries.push(blank);
            }
            actions.push(UiAction::Splice(
                path.to_owned(),
                key.to_owned(),
                at.map_or(items.len(), |(index, _)| index),
                0,
                entries,
            ));
        }
    });
}

/// What "+ add" appends: the required fields of the array's item schema, at
/// their defaults, so the result validates.
fn default_entry(schema: Option<&serde_json::Value>) -> serde_json::Value {
    let Some(items) = schema.and_then(|s| s.get("items")) else {
        return serde_json::json!({});
    };
    let mut entry = serde_json::Map::new();
    if let Some(properties) = items.get("properties").and_then(serde_json::Value::as_object) {
        let required: Vec<&str> = items
            .get("required")
            .and_then(serde_json::Value::as_array)
            .map(|r| r.iter().filter_map(serde_json::Value::as_str).collect())
            .unwrap_or_default();
        for name in required {
            if let Some(field) = properties.get(name) {
                entry.insert(name.to_owned(), default_for(field));
            }
        }
    }
    serde_json::Value::Object(entry)
}

/// A schema's `default`, or the emptiest legal value of its declared type.
fn default_for(schema: &serde_json::Value) -> serde_json::Value {
    if let Some(default) = schema.get("default") {
        return default.clone();
    }
    // An enum's first variant is a real choice rather than an empty string,
    // which would fail validation the moment it was written.
    if let Some(first) = schema
        .get("enum")
        .and_then(serde_json::Value::as_array)
        .and_then(|v| v.first())
    {
        return first.clone();
    }
    match schema.get("type").and_then(serde_json::Value::as_str) {
        Some("number" | "integer") => serde_json::json!(0),
        Some("boolean") => serde_json::json!(false),
        Some("array") => serde_json::json!([]),
        Some("object") => serde_json::json!({}),
        _ => serde_json::json!(""),
    }
}

/// egui's colour button, as a `Widget` so it can go through `add_enabled`.
struct ColourButton<'a>(&'a mut [f32; 3]);

impl egui::Widget for ColourButton<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        ui.color_edit_button_rgb(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::sorted_insertion;

    /// **The add button that always failed.** `Environment.stages` runs
    /// 0.0, 0.25, 0.5, 0.8, 1.0 and must be strictly increasing; appending a
    /// default entry put `at = 0` after `at = 1` and the op layer refused it
    /// on every scene, every time.
    #[test]
    fn a_new_stage_lands_in_the_widest_gap() {
        let stages: Vec<serde_json::Value> = [0.0, 0.25, 0.5, 0.8, 1.0]
            .iter()
            .map(|at| serde_json::json!({ "at": at }))
            .collect();
        let (index, at) = sorted_insertion(&stages, None).expect("these carry an at");
        // The widest run is 0.5..0.8; 0.25..0.5 and 0.0..0.25 are narrower.
        assert_eq!(index, 3, "between 0.5 and 0.8");
        assert!((at - 0.65).abs() < 1e-9, "{at}");

        // And the result is still strictly increasing.
        let mut values: Vec<f64> = stages
            .iter()
            .filter_map(|s| s.get("at").and_then(serde_json::Value::as_f64))
            .collect();
        values.insert(index, at);
        assert!(
            values.windows(2).all(|w| w[0] < w[1]),
            "still sorted: {values:?}"
        );
    }

    /// The ends count as gaps, or a run that stops early can never be extended.
    #[test]
    fn the_room_after_the_last_stage_is_a_gap() {
        let stages = vec![serde_json::json!({ "at": 0.0 }), serde_json::json!({ "at": 0.1 })];
        let (index, at) = sorted_insertion(&stages, None).expect("carries an at");
        assert_eq!(index, 2, "after the last, where the room is");
        assert!(at > 0.1 && at < 1.0, "{at}");
    }

    /// An empty *stage* array starts at the beginning — and an empty anything
    /// else does not get an `at` at all.
    ///
    /// **This test used to assert `sorted_insertion(&[]) == Some((0, 0.0))`
    /// unconditionally**, which is where the defect lived: "+ add" then wrote an
    /// `at` into the blank entry of every empty array in the engine, and
    /// `ops = [{ at = 0.0 }]` applied cleanly and then failed `loom validate`
    /// with `missing field 'kind'`.
    #[test]
    fn the_first_stage_starts_at_zero_and_nothing_else_does() {
        let stage = serde_json::json!({
            "properties": { "at": { "type": "number" }, "name": { "type": "string" } },
        });
        assert_eq!(sorted_insertion(&[], Some(&stage)), Some((0, 0.0)));

        // A pontoon, a voxel op, an untyped entry: no `at`, no placement.
        let pontoon = serde_json::json!({
            "properties": { "offset": { "type": "array" }, "radius": { "type": "number" } },
        });
        assert_eq!(sorted_insertion(&[], Some(&pontoon)), None);
        assert_eq!(sorted_insertion(&[], Some(&serde_json::json!(true))), None);
        assert_eq!(sorted_insertion(&[], None), None);
    }

    /// **Every other array still appends.** Pontoons and voxel ops carry no
    /// `at` and have no order to keep; inventing one would move entries a human
    /// deliberately arranged.
    #[test]
    fn an_unsorted_array_is_left_alone() {
        let pontoons = vec![serde_json::json!({ "radius": 1.0 })];
        assert_eq!(sorted_insertion(&pontoons, None), None);
    }


    use super::{PanelState, fields_editable, structure_editable};

    /// **The gate that shipped broken twice.** A comment saying fields stay
    /// live during Play sat directly above an expression disabling them, and
    /// nothing tested either, so "done" was claimed on a feature that was inert.
    ///
    /// Values live while playing; shape does not. That distinction is the whole
    /// point, and it is now a thing with a name rather than an expression
    /// repeated at four call sites.
    #[test]
    fn values_stay_editable_while_playing_and_structure_does_not() {
        let scene = loom_scene::Scene::parse("[scene]\nformat = 1\n\n[[node]]\nname = \"A\"\n")
            .expect("a one-node scene");
        let registry = loom_reflect::TypeRegistry::new();
        // Bound outside the closure: the temporaries have to outlive the
        // borrows in the struct.
        let picks = std::collections::BTreeMap::new();
        let collapsed = std::collections::BTreeSet::new();
        let overrides = std::collections::BTreeMap::new();
        let make = |playing: Option<(u32, bool, usize)>| PanelState {
            playing,
            editable: true,
            scene: &scene,
            paths: &[],
            picks: &picks,
            assets: &[],
            object_count: 0,
            selected: &[],
            history: &[],
            can_undo: false,
            can_redo: false,
            dirty: false,
            conflict: false,
            registry: &registry,
            mode: crate::gizmo::Mode::Move,
            snap: crate::gizmo::Snap::default(),
            view_mode: loom_render::ablate::ViewMode::default(),
            filter: "",
            collapsed: &collapsed,
            problems: &[],
            prefabs: &[],
            scenes: &[],
            open_scene: "",
            agent_log: &[],
            agent_chat: &[],
            agent_proposals: &[],
            frames: &[],
            copied_component: None,
            agent_busy: false,
            renaming: None,
            redo_history: &[],
            selection_edges: &[],
            rings: &[],
            planes: &[],
            handles: &[],
            dragging: None,
            fps: 60.0,
            cpu_ms: 1.0,
            draw_ms: 2.0,
            agent_marks: &[],
            overrides: &overrides,
            console: &[],
            tick_seconds: 1.0 / 60.0,
        };

        let stopped = make(None);
        assert!(fields_editable(&stopped));
        assert!(structure_editable(&stopped));

        let playing = make(Some((0, false, 0)));
        assert!(
            fields_editable(&playing),
            "tuning a value is the reason play mode is worth having"
        );
        assert!(
            !structure_editable(&playing),
            "deleting a node the simulation is holding is not a tuning knob"
        );
    }

    /// Read-only means read-only, playing or not.
    #[test]
    fn nothing_is_editable_in_a_read_only_session() {
        let scene = loom_scene::Scene::parse("[scene]\nformat = 1\n\n[[node]]\nname = \"A\"\n")
            .expect("a one-node scene");
        let registry = loom_reflect::TypeRegistry::new();
        let picks = std::collections::BTreeMap::new();
        let collapsed = std::collections::BTreeSet::new();
        let overrides = std::collections::BTreeMap::new();
        let state = PanelState {
            editable: false,
            playing: None,
            scene: &scene,
            paths: &[],
            picks: &picks,
            assets: &[],
            object_count: 0,
            selected: &[],
            history: &[],
            can_undo: false,
            can_redo: false,
            dirty: false,
            conflict: false,
            registry: &registry,
            mode: crate::gizmo::Mode::Move,
            snap: crate::gizmo::Snap::default(),
            view_mode: loom_render::ablate::ViewMode::default(),
            filter: "",
            collapsed: &collapsed,
            problems: &[],
            prefabs: &[],
            scenes: &[],
            open_scene: "",
            agent_log: &[],
            agent_chat: &[],
            agent_proposals: &[],
            frames: &[],
            copied_component: None,
            agent_busy: false,
            renaming: None,
            redo_history: &[],
            selection_edges: &[],
            rings: &[],
            planes: &[],
            handles: &[],
            dragging: None,
            fps: 60.0,
            cpu_ms: 1.0,
            draw_ms: 2.0,
            agent_marks: &[],
            overrides: &overrides,
            console: &[],
            tick_seconds: 1.0 / 60.0,
        };
        assert!(!fields_editable(&state));
        assert!(!structure_editable(&state));
    }

    use super::hidden_by_collapse;

    /// **A fold hides descendants, not the node itself**, and not a sibling
    /// whose name merely starts the same way — `Rig/Boat` must not fold
    /// `Rig/BoatHouse`.
    #[test]
    fn a_collapsed_parent_hides_only_its_own_subtree() {
        let shut = std::collections::BTreeSet::from(["Rig/Boat".to_owned()]);
        assert!(!hidden_by_collapse("Rig", &shut));
        assert!(!hidden_by_collapse("Rig/Boat", &shut), "the folded node stays visible");
        assert!(hidden_by_collapse("Rig/Boat/Helm", &shut));
        assert!(hidden_by_collapse("Rig/Boat/Deck/Rail", &shut), "at any depth");
        assert!(!hidden_by_collapse("Rig/BoatHouse", &shut), "not a name prefix");
        assert!(!hidden_by_collapse("Rig/Player", &shut));
    }

    #[test]
    fn nothing_is_hidden_when_nothing_is_folded() {
        let none = std::collections::BTreeSet::new();
        assert!(!hidden_by_collapse("A/B/C", &none));
    }

    use super::matches_filter;

    /// The filter reads the whole path, not the leaf. A scene with eleven
    /// nodes called `Body` is exactly when somebody reaches for a filter.
    #[test]
    fn a_filter_matches_anywhere_in_the_path_and_ignores_case() {
        assert!(matches_filter("Rig/Boat/Helm", "helm"));
        assert!(matches_filter("Rig/Boat/Helm", "BOAT/HELM"));
        assert!(matches_filter("Rig/Boat/Helm", "rig"));
        assert!(!matches_filter("Rig/Boat/Helm", "creel"));
    }

    /// An empty filter is not a filter that matches nothing.
    #[test]
    fn an_empty_filter_shows_everything() {
        assert!(matches_filter("anything/at/all", ""));
    }

    use super::{default_entry, default_for, looks_like_a_colour, range_note};


    /// **Only fields every selected node has.** Offering one that half the
    /// selection lacks would either write it onto nodes that should not carry
    /// it or silently skip them, and a multi-edit that quietly applies to a
    /// subset is worse than one that refuses.
    #[test]
    fn multi_edit_offers_the_intersection_and_flags_disagreement() {
        let scene = loom_scene::Scene::parse(
            "\
[scene]
format = 1
id = \"22222222-2222-4222-8222-222222222222\"

[[node]]
name = \"Room\"

[[node]]
name = \"A\"
parent = \"Room\"

  [node.components.Light]
  intensity = 100.0
  color = [1.0, 1.0, 1.0]

[[node]]
name = \"B\"
parent = \"Room\"

  [node.components.Light]
  intensity = 100.0

[[node]]
name = \"C\"
parent = \"Room\"

  [node.components.Light]
  intensity = 250.0
  color = [1.0, 1.0, 1.0]
",
        )
        .expect("valid scene");

        let both = ["Room/A".to_owned(), "Room/C".to_owned()];
        let shared = super::common_fields(&scene, &both);
        let named: Vec<(&str, bool)> = shared
            .iter()
            .map(|(_, field, _, agreed)| (field.as_str(), *agreed))
            .collect();
        assert!(
            named.contains(&("intensity", false)),
            "100 and 250 disagree and must be marked: {named:?}"
        );
        assert!(
            named.contains(&("color", true)),
            "both are white and must read as settled: {named:?}"
        );

        // `B` has no `range`, so it drops out of the shared set entirely.
        let with_b = ["Room/A".to_owned(), "Room/B".to_owned()];
        let shared = super::common_fields(&scene, &with_b);
        let fields: Vec<&str> = shared.iter().map(|(_, f, _, _)| f.as_str()).collect();
        assert_eq!(fields, ["intensity"], "color is not shared with B");
    }

    /// A selection naming a node that does not exist offers nothing rather
    /// than editing whichever members happened to resolve — the state during a
    /// reload, and the one where a partial write would be invisible.
    #[test]
    fn multi_edit_refuses_a_selection_it_cannot_fully_resolve() {
        let scene = loom_scene::Scene::parse(
            "\
[scene]
format = 1
id = \"22222222-2222-4222-8222-222222222222\"

[[node]]
name = \"A\"

  [node.components.Light]
  intensity = 100.0
",
        )
        .expect("valid scene");

        let stale = ["A".to_owned(), "Deleted".to_owned()];
        assert!(super::common_fields(&scene, &stale).is_empty());
    }

    /// The swatch is offered for the fields that hold colours and not for the
    /// ones that hold three unrelated numbers.
    ///
    /// A heuristic on the name, so it is worth pinning what it answers — and
    /// worth stating what it costs when wrong, which is a redundant button
    /// beside a drag row that still works.
    #[test]
    fn colour_fields_are_recognised_and_positions_are_not() {
        for yes in ["albedo", "color", "sky_colour", "tint", "emissive"] {
            assert!(looks_like_a_colour(yes), "{yes} should offer a swatch");
        }
        for no in ["pos", "rot_euler", "scale", "half_extents", "center", "uv_scale"] {
            assert!(!looks_like_a_colour(no), "{no} is not a colour");
        }
    }

    /// The range goes in the tooltip, in whichever of the four shapes the
    /// schema declared.
    #[test]
    fn the_constraint_line_reads_from_the_schema() {
        let both = serde_json::json!({"minimum": 0.0, "maximum": 1.0});
        assert_eq!(range_note(Some(&both)).as_deref(), Some("range: 0 to 1"));
        let lo = serde_json::json!({"minimum": 0.0});
        assert_eq!(range_note(Some(&lo)).as_deref(), Some("minimum: 0"));
        let hi = serde_json::json!({"maximum": 8.0});
        assert_eq!(range_note(Some(&hi)).as_deref(), Some("maximum: 8"));
        assert_eq!(range_note(Some(&serde_json::json!({}))), None);
        assert_eq!(range_note(None), None);
    }

    /// **"+ add" must append something that validates**, or the button looks
    /// broken: the transaction is rejected, the row never appears, and the
    /// human has no way to tell that the entry they asked for was illegal
    /// rather than that the editor is buggy.
    #[test]
    fn a_new_array_entry_carries_its_required_fields() {
        let schema = serde_json::json!({
            "type": "array",
            "items": {
                "type": "object",
                "required": ["kind", "amplitude"],
                "properties": {
                    "kind": { "enum": ["sine", "gerstner"] },
                    "amplitude": { "type": "number" },
                    "phase": { "type": "number" }
                }
            }
        });
        let entry = default_entry(Some(&schema));
        assert_eq!(entry["kind"], "sine", "an enum takes its first variant");
        assert_eq!(entry["amplitude"], 0);
        assert!(
            entry.get("phase").is_none(),
            "only required fields — an optional one left out keeps the file minimal"
        );
    }

    /// **An empty array is not an array of numbers.**
    ///
    /// `Iterator::all` is vacuously true on nothing, so `[]` matched the numeric
    /// arm, drew zero drag boxes and left a blank row — no "+ add", no way in.
    /// That is every `Environment.stages` before its first stage, every
    /// `VoxelVolume.ops` before its first op, and any object-array whose last
    /// entry was just deleted. The schema is what can answer for an empty array.
    #[test]
    fn an_empty_array_is_routed_by_its_schema_not_by_being_empty() {
        let stages = serde_json::json!({
            "type": "array", "default": [], "items": { "$ref": "#/$defs/MoodStage" },
        });
        assert!(
            !super::array_is_numeric(&[], Some(&stages)),
            "an empty stage list must reach the object arm, where `+ add` lives"
        );

        // A voxel recipe is a union of five shapes, so schemars writes
        // `items: true` — untyped, and still not numeric.
        let ops = serde_json::json!({ "type": "array", "default": [], "items": true });
        assert!(!super::array_is_numeric(&[], Some(&ops)));

        // An empty numeric array stays numeric: nothing should offer to append
        // an object into a colour.
        let colour = serde_json::json!({
            "type": "array", "default": [], "items": { "type": "number" },
        });
        assert!(super::array_is_numeric(&[], Some(&colour)));

        // A populated array answers for itself, schema or no schema.
        let numbers = [serde_json::json!(1.0), serde_json::json!(2.0)];
        assert!(super::array_is_numeric(&numbers, None));
        let objects = [serde_json::json!({ "at": 0.0 })];
        assert!(!super::array_is_numeric(&objects, None));
        // And with no schema at all, an empty array is not claimed as numeric.
        assert!(!super::array_is_numeric(&[], None));
    }

    /// **"set" writes the schema's own minimum, or offers nothing.**
    ///
    /// A nullable object — `WaterBody.swell` — must get no button: `Swell`'s
    /// fields are refused at load rather than defaulted, so every value this
    /// could write would be rejected, and a button that always fails is worse
    /// than no button. That mistake has been made in this editor before, in the
    /// "+ add" that could never append a mood stage (ADR 0101).
    #[test]
    fn an_optional_scalar_can_be_set_and_an_optional_group_cannot() {
        // `Rain.duration`, as schemars actually spells it.
        let duration = serde_json::json!({
            "type": ["number", "null"], "default": null,
            "minimum": 0.0, "maximum": 86400.0,
        });
        assert_eq!(super::optional_start(Some(&duration)), Some(serde_json::json!(0.0)));

        // `WaterBody.fetch` — the minimum is 1, not 0, and the button must not
        // substitute a friendlier-looking zero the schema calls illegal.
        let fetch = serde_json::json!({
            "type": ["number", "null"], "default": null, "minimum": 1.0,
        });
        assert_eq!(super::optional_start(Some(&fetch)), Some(serde_json::json!(1.0)));

        // `WaterBody.swell` — an `anyOf` of a `$ref` and null.
        let swell = serde_json::json!({
            "anyOf": [{ "$ref": "#/$defs/Swell" }, { "type": "null" }], "default": null,
        });
        assert_eq!(super::optional_start(Some(&swell)), None, "a group must offer no button");

        // A nullable array (`WaterBody.extent`) has no smallest legal value
        // either — three zeroes is a guess about what an extent of nothing means.
        let extent = serde_json::json!({
            "type": ["array", "null"], "default": null, "minItems": 3,
        });
        assert_eq!(super::optional_start(Some(&extent)), None);

        // A plain required number is not optional and gets no button; it always
        // has a value already.
        let plain = serde_json::json!({ "type": "number", "minimum": 2.0 });
        assert_eq!(super::optional_start(Some(&plain)), None);
        assert_eq!(super::optional_start(None), None);
    }

    /// A declared `default` beats the type's empty value, because the schema
    /// author knew better than this function does.
    #[test]
    fn a_declared_default_wins() {
        let with = serde_json::json!({"type": "number", "default": 0.5});
        assert_eq!(default_for(&with), 0.5);
        let without = serde_json::json!({"type": "boolean"});
        assert_eq!(default_for(&without), false);
        let untyped = serde_json::json!({});
        assert_eq!(default_for(&untyped), "");
    }

    /// An array with no item schema — a voxel recipe, which is a union of five
    /// shapes and therefore untyped — appends an empty table rather than
    /// guessing at fields it cannot know.
    #[test]
    fn an_untyped_array_appends_an_empty_entry() {
        assert_eq!(default_entry(None), serde_json::json!({}));
        assert_eq!(
            default_entry(Some(&serde_json::json!({"type": "array"}))),
            serde_json::json!({})
        );
    }
}
