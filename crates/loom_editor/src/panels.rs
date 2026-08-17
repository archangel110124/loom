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
    /// Frame the selection.
    Focus,
    AddChild(String),
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
    /// Move `node` under `parent`.
    Reparent { node: String, parent: String },
    Play,
    /// Toggle pause while playing.
    Pause,
    /// One tick, whether paused or not.
    StepOnce,
    Stop,
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
    /// Gizmo handles in **window pixels**, as the viewport computed them.
    pub handles: &'a [Handle],
    /// The **axis** being dragged, so its handle can be drawn as grabbed.
    /// Axis, not index: a handle can drop out of the list when it goes
    /// edge-on, and an index would then highlight a different one.
    pub dragging: Option<usize>,
    pub fps: f32,
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
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("loom").strong());
            ui.separator();

            let editing = state.editable && state.playing.is_none();
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
            if ui
                .button("Focus")
                .on_hover_text("F — frame the selection")
                .clicked()
            {
                actions.push(UiAction::Focus);
            }
            if ui
                .add_enabled(editing, egui::Button::new("Duplicate"))
                .on_hover_text("Ctrl+D")
                .clicked()
            {
                actions.push(UiAction::Duplicate);
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

            // Right-aligned stats, the way Unity's are.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{:.0} fps · {} nodes · {} draws",
                        state.fps,
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
            let mut offered = 0;
            for type_name in state.registry.type_names() {
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

pub(crate) fn hierarchy(ui: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    ui.heading("Hierarchy");
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for path in state.paths {
            // Indent by depth, so the hierarchy reads as a tree rather
            // than a flat list of slash-separated strings.
            let depth = path.matches('/').count();
            let name = path.rsplit('/').next().unwrap_or(path);
            let selected = state.selected.iter().any(|p| p == path);
            ui.horizontal(|ui| {
                #[allow(clippy::cast_precision_loss)]
                ui.add_space(depth as f32 * 14.0);
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
                let id = egui::Id::new(("hierarchy", path));
                let response = ui
                    .dnd_drag_source(id, path.clone(), |ui| {
                        ui.selectable_label(selected, format!("{marker} {name}"))
                    })
                    .response
                    .on_hover_text(path);
                if let Some(dragged) = response.dnd_release_payload::<String>()
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
                if state.editable {
                    response.context_menu(|ui| {
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

    let editable_now = state.editable && state.playing.is_none();
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
            editable_now,
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
    let editing = state.editable && state.playing.is_none();

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
    ui.heading("Assets");
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
                let enabled = state.editable && !state.selected.is_empty();
                if ui
                    .add_enabled(enabled, egui::Button::new(format!("◻ {asset}")))
                    .on_hover_text("assign to the selection")
                    .clicked()
                {
                    actions.push(UiAction::AssignMesh(asset.clone()));
                }
            }
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
pub(crate) fn gizmo_overlay(root: &mut egui::Ui, state: &PanelState<'_>) {
    if state.handles.is_empty() {
        return;
    }
    let ctx = root.ctx().clone();
    // The viewport is the whole window; panels are drawn over it. So window
    // pixels map to egui points by the one scale factor, with no offset.
    let scale = ctx.pixels_per_point();
    let painter = ctx.layer_painter(egui::LayerId::background());
    let point = |(x, y): (f32, f32)| egui::pos2(x / scale, y / scale);

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
    let editing = state.editable && state.playing.is_none();
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
    let tip = match (doc.is_empty(), constraint) {
        (true, None) => String::new(),
        (true, Some(c)) => c,
        (false, None) => doc.to_owned(),
        (false, Some(c)) => format!("{doc}\n\n{c}"),
    };
    if !tip.is_empty() {
        label = label.on_hover_text(tip);
    }
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

    for (field, current) in fields {
        let key = format!("{type_name}.{field}");
        // One walker, shared with the validator: `$ref` followed through
        // `$defs` and both spellings of an enum flattened. See
        // `loom_reflect::field_schema`.
        let resolved = match (root, properties.and_then(|p| p.get(field))) {
            (Some(r), Some(f)) => Some(loom_reflect::field_schema(r, f)),
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

        serde_json::Value::Array(items) if items.iter().all(serde_json::Value::is_number) => {
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
        // `WaterBody.waves`, `Buoyancy.pontoons`, `Scatter.excludes` and a
        // voxel recipe were all a single unreadable line that could only be
        // edited in a text editor.
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
            ui.add(
                egui::Label::new(egui::RichText::new(summarise(item)).weak().small()).wrap(),
            );
        }
        if editable && ui.small_button("+ add").clicked() {
            // A new entry at the schema's defaults, which for an untyped
            // recipe is an empty table the human then fills in. Appending
            // something invalid would be worse: the transaction would be
            // rejected and the button would look broken.
            let blank = default_entry(schema);
            actions.push(UiAction::Splice(
                path.to_owned(),
                key.to_owned(),
                items.len(),
                0,
                vec![blank],
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
