//! Editor panels: scene tree, inspector, transaction log.
//!
//! **The inspector is generated from the type registry**, not hand-written per
//! component. That was always the point of the registry (design doc §2.1) and
//! it is why M5.5 called the viewer cheap: a new component type gets an
//! inspector for free, with its ranges enforced and its doc comment as the
//! tooltip, the same way it gets a schema and a CLI `describe` for free.
//!
//! A hand-written inspector is a second description of every type, and it
//! drifts out of sync by Thursday.

use loom_render::egui;

/// What a panel interaction asked for. Returned rather than applied, so every
/// change still goes through one transaction path.
#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
    Select(usize),
    /// `node`, `Type.field`, new value.
    SetField(String, String, serde_json::Value),
    Undo,
    Redo,
    Save,
}

/// State the panels need that is not in the scene.
pub struct PanelState<'a> {
    pub paths: &'a [String],
    pub selected: usize,
    pub history: &'a [String],
    pub can_undo: bool,
    pub can_redo: bool,
    pub dirty: bool,
    pub scene: Option<&'a loom_scene::Scene>,
    pub registry: &'a loom_reflect::TypeRegistry,
}

/// Draw every panel, returning whatever the human asked for.
/// Panels attach to a root `Ui`, and **order matters**: the first added is
/// outermost, and `CentralPanel` must come last or it eats the space the side
/// panels needed.
pub fn draw(root: &mut egui::Ui, state: &PanelState<'_>) -> Vec<UiAction> {
    let mut actions = Vec::new();

    egui::Panel::top("toolbar").show(root, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("loom").strong());
            ui.separator();
            if ui
                .add_enabled(state.can_undo, egui::Button::new("Undo"))
                .on_hover_text("One transaction, however many ops it held")
                .clicked()
            {
                actions.push(UiAction::Undo);
            }
            if ui
                .add_enabled(state.can_redo, egui::Button::new("Redo"))
                .clicked()
            {
                actions.push(UiAction::Redo);
            }
            if ui.button("Save").clicked() {
                actions.push(UiAction::Save);
            }
            if state.dirty {
                ui.label(egui::RichText::new("● unsaved").color(egui::Color32::from_rgb(230, 170, 80)));
            }
        });
    });

    egui::Panel::left("tree")
        .default_size(210.0)
        .show(root, |ui| {
            ui.heading("Scene");
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (index, path) in state.paths.iter().enumerate() {
                    // Indent by depth, so the hierarchy reads as a tree rather
                    // than a flat list of slash-separated strings.
                    let depth = path.matches('/').count();
                    let name = path.rsplit('/').next().unwrap_or(path);
                    ui.horizontal(|ui| {
                        ui.add_space(depth as f32 * 14.0);
                        if ui
                            .selectable_label(index == state.selected, name)
                            .on_hover_text(path)
                            .clicked()
                        {
                            actions.push(UiAction::Select(index));
                        }
                    });
                }
            });
        });

    egui::Panel::right("inspector")
        .default_size(300.0)
        .show(root, |ui| {
            ui.heading("Inspector");
            ui.separator();
            let Some(scene) = state.scene else {
                ui.label("no scene");
                return;
            };
            let Some(path) = state.paths.get(state.selected) else {
                ui.label("nothing selected");
                return;
            };
            let Some(node) = scene.nodes().iter().find(|n| &n.path == path) else {
                return;
            };

            ui.label(egui::RichText::new(path).monospace());
            ui.add_space(6.0);

            egui::ScrollArea::vertical().show(ui, |ui| {
                // Transform first — it is what a human reaches for, and it is
                // the node-key sugar rather than a component table.
                inspect_transform(ui, path, &node.transform, &mut actions);

                for (type_name, value) in &node.components {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(type_name).strong());
                    inspect_component(ui, path, type_name, value, state.registry, &mut actions);
                }
            });
        });

    egui::Panel::bottom("log")
        .default_size(120.0)
        .show(root, |ui| {
            ui.heading("Transactions");
            ui.separator();
            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                if state.history.is_empty() {
                    ui.weak("no edits yet");
                }
                // Newest last, matching a log. Labels are why transactions
                // carry one: this is the human's window onto what changed.
                for (index, label) in state.history.iter().enumerate() {
                    ui.label(format!("{:>3}  {label}", index + 1));
                }
            });
        });

    actions
}

fn inspect_transform(
    ui: &mut egui::Ui,
    path: &str,
    transform: &loom_scene::components::Transform,
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
            for value in &mut edited {
                changed |= ui.add(egui::DragValue::new(value).speed(0.05)).changed();
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
                            ui.add(egui::Slider::new(&mut v, lo..=hi))
                        }
                        _ => ui.add(egui::DragValue::new(&mut v).speed(0.1)),
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
                    if ui.checkbox(&mut v, "").changed() {
                        actions.push(UiAction::SetField(
                            path.to_owned(),
                            format!("{type_name}.{field}"),
                            serde_json::json!(v),
                        ));
                    }
                }
                serde_json::Value::Array(items) if items.iter().all(serde_json::Value::is_number) => {
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
                        changed |= match (lo, hi) {
                            (Some(lo), Some(hi)) => {
                                ui.add(egui::DragValue::new(v).range(lo..=hi).speed(0.02))
                            }
                            _ => ui.add(egui::DragValue::new(v).speed(0.05)),
                        }
                        .changed();
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
                // than half-edited. Editing an asset reference belongs to the
                // asset browser, which is not built.
                _ => {
                    ui.weak(current.to_string());
                }
            }
        });
    }
}
