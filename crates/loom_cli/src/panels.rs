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
use crate::scene_view::SceneView;

/// What a panel interaction asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
    Select { path: String, extend: bool },
    /// `node`, `Type.field`, new value.
    SetField(String, String, serde_json::Value),
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
    Play,
    /// Toggle pause while playing.
    Pause,
    /// One tick, whether paused or not.
    StepOnce,
    Stop,
}

/// State the panels need that is not in the scene.
pub struct PanelState<'a> {
    pub view: &'a SceneView,
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
    /// Ticks run, whether paused, and how many bodies — `None` in edit mode.
    pub playing: Option<(u32, bool, usize)>,
}

const AXIS_COLORS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(226, 84, 79),
    egui::Color32::from_rgb(124, 200, 96),
    egui::Color32::from_rgb(84, 148, 232),
];
const AXIS_NAMES: [&str; 3] = ["X", "Y", "Z"];

/// Draw every panel, returning whatever the human asked for.
///
/// Panels attach to a root `Ui`, and **order matters**: the first added is
/// outermost, and anything filling the centre must come last or it eats the
/// space the side panels needed.
pub fn draw(root: &mut egui::Ui, state: &PanelState<'_>) -> Vec<UiAction> {
    let mut actions = Vec::new();

    toolbar(root, state, &mut actions);
    if state.conflict {
        conflict_banner(root, &mut actions);
    }
    hierarchy(root, state, &mut actions);
    inspector(root, state, &mut actions);
    console(root, state, &mut actions);
    assets(root, state, &mut actions);
    gizmo_overlay(root, state);

    actions
}

fn toolbar(root: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
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
                let seconds = f64::from(ticks) * f64::from(crate::play::TICK_SECONDS);
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
                        state.view.paths.len(),
                        state.view.objects.len()
                    ))
                    .weak()
                    .monospace(),
                );
            });
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
fn conflict_banner(root: &mut egui::Ui, actions: &mut Vec<UiAction>) {
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

fn hierarchy(root: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    egui::Panel::left("hierarchy")
        .default_size(230.0)
        .resizable(true)
        .show(root, |ui| {
            ui.heading("Hierarchy");
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for path in &state.view.paths {
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
                        let marker = if state.view.picks.contains_key(path) {
                            "▪"
                        } else {
                            "·"
                        };
                        let response = ui
                            .selectable_label(selected, format!("{marker} {name}"))
                            .on_hover_text(path);
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
        });
}

fn inspector(root: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    egui::Panel::right("inspector")
        .default_size(320.0)
        .resizable(true)
        .show(root, |ui| {
            ui.heading("Inspector");
            ui.separator();

            if state.selected.len() > 1 {
                ui.label(format!("{} nodes selected", state.selected.len()));
                ui.weak("Move, duplicate and delete apply to all of them.");
                return;
            }
            let Some(path) = state.selected.first() else {
                ui.weak("nothing selected");
                return;
            };
            let Some(node) = state.view.scene.nodes().iter().find(|n| &n.path == path) else {
                return;
            };

            ui.label(egui::RichText::new(path).monospace());
            ui.add_space(6.0);

            egui::ScrollArea::vertical().show(ui, |ui| {
                // Transform first — it is what a human reaches for, and it is
                // the node-key sugar rather than a component table.
                inspect_transform(ui, path, &node.transform, state.editable, actions);

                for (type_name, value) in &node.components {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(type_name).strong());
                    inspect_component(
                        ui,
                        path,
                        type_name,
                        value,
                        state.registry,
                        state.editable,
                        actions,
                    );
                }
            });
        });
}

/// Unity's Project panel, cut to what this engine has: the assets the scene
/// actually resolved. Clicking one points the selection at it.
fn assets(root: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    egui::Panel::bottom("assets")
        .default_size(120.0)
        .resizable(true)
        .show(root, |ui| {
            ui.heading("Assets");
            ui.separator();
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for asset in &state.view.assets {
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
        });
}

/// Unity's Console. The reason it exists is that the messages worth reading —
/// a rejected transaction, a scene that moved on disk — were going to a
/// terminal nobody has in front of them.
fn console(root: &mut egui::Ui, state: &PanelState<'_>, actions: &mut Vec<UiAction>) {
    egui::Panel::bottom("console")
        .default_size(170.0)
        .resizable(true)
        .show(root, |ui| {
            // Two columns rather than two strips of screen: what the engine
            // said and what the scene did are read together, and the second
            // explains the first.
            ui.columns(2, |columns| {
                console_column(&mut columns[0], actions);
                transactions(&mut columns[1], state.history);
            });
        });
}

fn console_column(ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
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
                    let entries = crate::log::entries();
                    if entries.is_empty() {
                        ui.weak("nothing yet");
                    }
                    for entry in entries {
                        let (color, tag) = match entry.level {
                            crate::log::Level::Info => (egui::Color32::GRAY, " "),
                            crate::log::Level::Warn => {
                                (egui::Color32::from_rgb(230, 180, 90), "!")
                            }
                            crate::log::Level::Error => {
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
fn transactions(ui: &mut egui::Ui, history: &[String]) {
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

/// Draw the transform handles over the viewport.
///
/// In the background layer: over the 3D image, under every panel, so a handle
/// never draws on top of the inspector it is behind.
fn gizmo_overlay(root: &mut egui::Ui, state: &PanelState<'_>) {
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

/// Build widgets for a component from its **schema**, not from a hand-written
/// match on the type name.
fn inspect_component(
    ui: &mut egui::Ui,
    path: &str,
    type_name: &str,
    value: &serde_json::Value,
    registry: &loom_reflect::TypeRegistry,
    editable: bool,
    actions: &mut Vec<UiAction>,
) {
    let schema = registry.describe(type_name).and_then(|s| s.as_object());
    let properties = schema
        .and_then(|s| s.get("properties"))
        .and_then(serde_json::Value::as_object);

    let Some(fields) = value.as_object() else {
        return;
    };

    for (field, current) in fields {
        let field_schema = properties.and_then(|p| p.get(field));
        // The doc comment became the schema `description` at M1, and it becomes
        // the tooltip here — writing a good doc comment, teaching the agent,
        // and labelling the editor are all one act.
        let tooltip = field_schema
            .and_then(|f| f.get("description"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        // Ranges come from `#[schemars(range(...))]`, so a slider cannot be
        // dragged out of what the validator would accept.
        let bounds = field_schema.map(|f| {
            (
                f.get("minimum").and_then(serde_json::Value::as_f64),
                f.get("maximum").and_then(serde_json::Value::as_f64),
            )
        });

        ui.horizontal(|ui| {
            let label = ui.label(egui::RichText::new(format!("  {field:<12}")).monospace());
            if !tooltip.is_empty() {
                label.on_hover_text(tooltip);
            }

            match current {
                serde_json::Value::Number(n) => {
                    let mut v = n.as_f64().unwrap_or(0.0);
                    let widget = match bounds {
                        Some((Some(lo), Some(hi))) => {
                            ui.add_enabled(editable, egui::Slider::new(&mut v, lo..=hi))
                        }
                        _ => ui.add_enabled(editable, egui::DragValue::new(&mut v).speed(0.1)),
                    };
                    if widget.changed() {
                        actions.push(UiAction::SetField(
                            path.to_owned(),
                            format!("{type_name}.{field}"),
                            serde_json::json!(v),
                        ));
                    }
                }
                serde_json::Value::Bool(b) => {
                    let mut v = *b;
                    if ui.add_enabled(editable, egui::Checkbox::new(&mut v, "")).changed() {
                        actions.push(UiAction::SetField(
                            path.to_owned(),
                            format!("{type_name}.{field}"),
                            serde_json::json!(v),
                        ));
                    }
                }
                serde_json::Value::Array(items)
                    if items.iter().all(serde_json::Value::is_number) =>
                {
                    let mut edited: Vec<f64> =
                        items.iter().filter_map(serde_json::Value::as_f64).collect();
                    let mut changed = false;
                    let (lo, hi) = field_schema
                        .and_then(|f| f.get("items"))
                        .map(|i| {
                            (
                                i.get("minimum").and_then(serde_json::Value::as_f64),
                                i.get("maximum").and_then(serde_json::Value::as_f64),
                            )
                        })
                        .unwrap_or((None, None));
                    for v in &mut edited {
                        let widget = match (lo, hi) {
                            (Some(lo), Some(hi)) => {
                                egui::DragValue::new(v).range(lo..=hi).speed(0.02)
                            }
                            _ => egui::DragValue::new(v).speed(0.05),
                        };
                        changed |= ui.add_enabled(editable, widget).changed();
                    }
                    if changed {
                        actions.push(UiAction::SetField(
                            path.to_owned(),
                            format!("{type_name}.{field}"),
                            serde_json::json!(edited),
                        ));
                    }
                }
                serde_json::Value::String(s) => {
                    ui.label(egui::RichText::new(s).monospace().weak());
                }
                // Nested objects (an AssetRef, say) are shown read-only rather
                // than half-edited. Editing an asset reference is what the
                // Assets panel is for.
                _ => {
                    ui.weak(current.to_string());
                }
            }
        });
    }
}
