//! The game's overlay: authored `Hud` elements drawn over the scene.
//!
//! **Unity's idea, without Unity's Canvas.** The part worth keeping is that a
//! HUD is content: it belongs in the scene file, so moving the score is an
//! edit rather than a rebuild, and the agent can author one through the same
//! transactions it uses for everything else.
//!
//! The part not worth keeping is the retained mesh. A Canvas holds a vertex
//! buffer per batch and invalidates it whenever any element inside changes,
//! which is a lot of bookkeeping to avoid rebuilding a few hundred vertices.
//! egui rebuilds the whole overlay every frame into one buffer and uploads it
//! — at HUD scale that is cheaper than tracking what changed, and there is no
//! invalidation logic that can be wrong. It also costs no new pass: the
//! editor's UI layer already draws into the swapchain image the scene wrote.

use loom_render::egui;
use loom_script::GameState;

/// One resolved line, ready to draw.
pub(crate) struct Element {
    anchor: egui::Align2,
    offset: egui::Vec2,
    text: String,
    size: f32,
    color: egui::Color32,
}

/// Read the scene's `Hud` components and fill in the game's numbers.
///
/// Resolved every frame rather than cached: the values change every tick, and
/// a cache keyed on "did the state change" is more code than the formatting it
/// would save.
pub(crate) fn elements(
    world: &loom_ecs::World,
    state: &GameState,
    playing: bool,
) -> Vec<Element> {
    world
        .hud_elements()
        .into_iter()
        .filter(|component| {
            playing
                || !component
                    .get("only_in_play")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
        })
        .map(|component| {
            let defaults = loom_scene::components::Hud::default();
            #[allow(clippy::cast_possible_truncation)]
            let scalar = |name: &str, fallback: f32| {
                component
                    .get(name)
                    .and_then(serde_json::Value::as_f64)
                    .map_or(fallback, |v| v as f32)
            };
            let pair = |name: &str, fallback: [f32; 2]| {
                let Some(values) = component.get(name).and_then(serde_json::Value::as_array) else {
                    return fallback;
                };
                let mut out = fallback;
                for (slot, value) in out.iter_mut().zip(values) {
                    if let Some(v) = value.as_f64() {
                        #[allow(clippy::cast_possible_truncation)]
                        {
                            *slot = v as f32;
                        }
                    }
                }
                out
            };

            let text = component
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let offset = pair("offset", defaults.offset);
            let rgb = {
                let mut out = defaults.color;
                if let Some(values) = component.get("color").and_then(serde_json::Value::as_array) {
                    for (slot, value) in out.iter_mut().zip(values) {
                        if let Some(v) = value.as_f64() {
                            #[allow(clippy::cast_possible_truncation)]
                            {
                                *slot = v as f32;
                            }
                        }
                    }
                }
                out
            };

            let anchor = component
                .get("anchor")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("top_left");

            Element {
                anchor: align(anchor),
                // Always *inward*: an offset of 16 means 16 pixels from the
                // edge you anchored to, whichever edge that is. Signed screen
                // coordinates would make the same number mean opposite things
                // at the top and the bottom.
                offset: egui::vec2(
                    offset[0] * horizontal_sign(anchor),
                    offset[1] * vertical_sign(anchor),
                ),
                text: interpolate(text, state),
                size: scalar("size", defaults.size),
                color: to_color(rgb),
            }
        })
        .collect()
}

/// Draw the resolved elements into the viewport.
///
/// **Anchored to the viewport, and claiming none of it.** Anchoring to the
/// whole window put the score on top of the hierarchy panel; the fix for that
/// was a transparent `CentralPanel`, which fixed the position and broke
/// clicking — a panel is an interactive region, so egui reported every click
/// in the viewport as consumed and the viewer never saw it. The crosshair was
/// visible and the trigger did nothing.
///
/// `available_rect_before_wrap` is the same region a `CentralPanel` would
/// take — whatever the side and bottom panels left over, at any window size
/// and whether or not the panels are there — but *reading* it adds nothing to
/// the layout and claims no input.
///
/// Painted rather than laid out, and clipped to that rect by hand. A HUD is a
/// fixed set of things pinned to known points with nothing to click, so one
/// `Painter::text` each is both less code and incapable of stealing a click.
///
/// Returns the viewport and where each element landed, which is what makes
/// the placement testable without a window.
pub(crate) fn draw(
    root: &mut egui::Ui,
    elements: &[Element],
) -> (egui::Rect, Vec<egui::Rect>) {
    if elements.is_empty() {
        return (egui::Rect::NOTHING, Vec::new());
    }
    let viewport = root.available_rect_before_wrap();
    let painter = root.painter().with_clip_rect(viewport);

    let painted = elements
        .iter()
        .map(|element| {
            let at = element.anchor.pos_in_rect(&viewport) + element.offset;
            painter.text(
                at,
                element.anchor,
                &element.text,
                egui::FontId::proportional(element.size),
                element.color,
            )
        })
        .collect();

    (viewport, painted)
}

/// Replace `{name}` with the game's own numbers.
///
/// An unknown name is left standing rather than blanked, so a typo appears on
/// screen instead of quietly rendering nothing — the same reasoning as
/// `unknown_field` being an error rather than a shrug.
fn interpolate(text: &str, state: &GameState) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find('{') {
        // The closing brace is found BEFORE anything is consumed. Pushing the
        // prefix first and then bailing left `rest` unadvanced, so the tail
        // was written twice — "100% {" came out as "100% 100% {".
        let Some(end) = rest[start..].find('}').map(|i| start + i) else {
            // An unclosed brace is just text. Better than swallowing the rest
            // of the line looking for a terminator that is not there.
            break;
        };
        out.push_str(&rest[..start]);
        let name = &rest[start + 1..end];
        match name {
            "status" => out.push_str(state.status().as_str()),
            "message" => out.push_str(state.message()),
            _ => match state.number(name) {
                // Trimmed to a whole number: a score of `200.0` is a score of
                // 200, and nothing keeping fractional state has surfaced.
                Some(value) => out.push_str(&format!("{value:.0}")),
                None => out.push_str(&rest[start..=end]),
            },
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn align(anchor: &str) -> egui::Align2 {
    match anchor {
        "top_center" => egui::Align2::CENTER_TOP,
        "top_right" => egui::Align2::RIGHT_TOP,
        "center_left" => egui::Align2::LEFT_CENTER,
        "center" => egui::Align2::CENTER_CENTER,
        "center_right" => egui::Align2::RIGHT_CENTER,
        "bottom_left" => egui::Align2::LEFT_BOTTOM,
        "bottom_center" => egui::Align2::CENTER_BOTTOM,
        "bottom_right" => egui::Align2::RIGHT_BOTTOM,
        _ => egui::Align2::LEFT_TOP,
    }
}

/// Which way "inward" is from each edge.
fn horizontal_sign(anchor: &str) -> f32 {
    if anchor.ends_with("_right") {
        -1.0
    } else {
        1.0
    }
}

fn vertical_sign(anchor: &str) -> f32 {
    if anchor.starts_with("bottom") {
        -1.0
    } else {
        1.0
    }
}

/// Linear RGB to what egui wants, which is sRGB bytes.
///
/// Scene colours are physical quantities everywhere else in this project, and
/// handing a linear value straight to egui would draw every HUD element darker
/// than it was authored.
fn to_color(rgb: [f32; 3]) -> egui::Color32 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let encode = |c: f32| {
        let c = c.clamp(0.0, 1.0);
        let srgb = if c <= 0.003_130_8 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (srgb * 255.0).round() as u8
    };
    egui::Color32::from_rgb(encode(rgb[0]), encode(rgb[1]), encode(rgb[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay out one element with a side panel taking the left of the window,
    /// and report where it landed. No window, no GPU — egui runs headless.
    fn placed(anchor: egui::Align2, offset: egui::Vec2, panel_width: f32) -> (egui::Rect, egui::Rect) {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            ..egui::RawInput::default()
        };

        let element = Element {
            anchor,
            offset,
            text: "SCORE 0".to_owned(),
            size: 20.0,
            color: egui::Color32::WHITE,
        };

        let mut result = (egui::Rect::NOTHING, egui::Rect::NOTHING);
        // Two passes: egui settles panel sizes on the first, and fonts are
        // laid out lazily. The second is the one that means anything.
        for _ in 0..2 {
            let _ = ctx.run_ui(input.clone(), |root| {
                if panel_width > 0.0 {
                    egui::Panel::left("panel")
                        .exact_size(panel_width)
                        .show(root, |ui| {
                            ui.label("hierarchy");
                        });
                }
                let (viewport, painted) = draw(root, std::slice::from_ref(&element));
                result = (viewport, painted[0]);
            });
        }
        result
    }

    /// Whether egui claims a pointer sitting at `x` in the viewport.
    ///
    /// If it does, the viewer's event loop treats the click as consumed and
    /// returns before the mouse button ever reaches the input map — which is
    /// how a visible crosshair ends up attached to a trigger that does
    /// nothing.
    fn pointer_claimed_at(x: f32, draw_hud: bool) -> bool {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            events: vec![egui::Event::PointerMoved(egui::pos2(x, 300.0))],
            ..egui::RawInput::default()
        };
        let element = Element {
            anchor: egui::Align2::CENTER_CENTER,
            offset: egui::Vec2::ZERO,
            text: "+".to_owned(),
            size: 26.0,
            color: egui::Color32::WHITE,
        };

        let mut claimed = false;
        for _ in 0..2 {
            let _ = ctx.run_ui(input.clone(), |root| {
                egui::Panel::left("panel").exact_size(240.0).show(root, |ui| {
                    ui.label("hierarchy");
                });
                if draw_hud {
                    let _ = draw(root, std::slice::from_ref(&element));
                }
            });
            claimed = ctx.is_pointer_over_egui();
        }
        claimed
    }

    /// **The overlay must not eat the trigger.** Fixing the anchoring with a
    /// transparent `CentralPanel` put the score in the right place and made
    /// the whole viewport an interactive egui region — so every click was
    /// reported as consumed, the viewer returned before recording the button,
    /// and shooting silently stopped working while the crosshair kept
    /// rendering.
    #[test]
    fn the_overlay_does_not_claim_clicks_in_the_viewport() {
        assert!(
            !pointer_claimed_at(600.0, true),
            "the HUD swallowed a click in the middle of the viewport"
        );
    }

    /// And the panels still do claim theirs, or clicking the hierarchy would
    /// also shoot.
    #[test]
    fn a_click_on_a_panel_is_still_the_panels() {
        assert!(
            pointer_claimed_at(60.0, true),
            "a click on the hierarchy should belong to the hierarchy"
        );
    }

    /// **The bug this exists for.** `Area::anchor` measures from the edges of
    /// the whole window, so in the editor the score was drawn on top of the
    /// hierarchy panel and the objective counter on top of the inspector. A
    /// HUD belongs to the game's view, and in an editor that is whatever the
    /// panels have left over.
    #[test]
    fn the_overlay_stays_inside_the_viewport_not_the_window() {
        let panel = 240.0;
        let (viewport, text) = placed(egui::Align2::LEFT_TOP, egui::vec2(16.0, 14.0), panel);

        assert!(
            viewport.left() >= panel,
            "viewport started at {} with a {panel}px panel",
            viewport.left()
        );
        assert!(
            text.left() >= panel,
            "the score was drawn over the panel: text at {}, panel is {panel} wide",
            text.left()
        );
    }

    /// With no panels the viewport is the window, so a bare viewer puts the
    /// HUD in the corner it was authored in rather than inset by nothing.
    #[test]
    fn without_panels_the_viewport_is_the_whole_window() {
        let (viewport, text) = placed(egui::Align2::LEFT_TOP, egui::vec2(16.0, 14.0), 0.0);

        assert!(viewport.left() < 1.0, "viewport was inset: {viewport:?}");
        assert!(text.left() < 20.0, "text was inset: {text:?}");
    }

    /// Offsets are inward from whichever edge was anchored to, so the same
    /// number does not mean opposite things at the top and the bottom.
    #[test]
    fn a_right_anchored_element_is_inset_from_the_right() {
        let (viewport, text) = placed(egui::Align2::RIGHT_TOP, egui::vec2(-18.0, 14.0), 240.0);

        assert!(
            (viewport.right() - text.right() - 18.0).abs() < 1.0,
            "expected 18px from the right edge: viewport {} text {}",
            viewport.right(),
            text.right()
        );
    }

    #[test]
    fn a_bottom_anchored_element_is_inset_from_the_bottom() {
        let (viewport, text) = placed(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0), 240.0);

        assert!(
            (viewport.bottom() - text.bottom() - 40.0).abs() < 1.0,
            "expected 40px from the bottom: viewport {} text {}",
            viewport.bottom(),
            text.bottom()
        );
    }

    /// The right and bottom tests above hand `draw` an already-signed offset,
    /// so they say nothing about where that sign comes from. This covers the
    /// authored side: a scene writes a positive number and means "inward",
    /// whichever edge it anchored to.
    #[test]
    fn an_authored_offset_is_inward_from_every_edge() {
        let world = loom_ecs::World::from_scene(
            &loom_scene::Scene::parse(
                "[scene]\nformat = 1\nid = \"7c1f0b52-9a34-4d68-b0e1-2f45a8c37d90\"\n\n\
                 [[node]]\nname = \"Root\"\n\n\
                   [node.components.Hud]\n  anchor = \"bottom_right\"\n\
                   offset = [20.0, 30.0]\n  text = \"x\"\n",
            )
            .expect("valid scene"),
        );

        let resolved = elements(&world, &GameState::default(), false);

        assert_eq!(resolved.len(), 1);
        // Anchored bottom-right, so both components must point back into the
        // screen. Written as they are, the element would sit off the corner.
        assert!(
            resolved[0].offset.x < 0.0 && resolved[0].offset.y < 0.0,
            "offset was {:?}, which is outward",
            resolved[0].offset
        );
    }

    #[test]
    fn a_state_number_lands_in_the_text() {
        let mut host = loom_script::ScriptHost::default();
        host.compile("r", "state.score = 250;").expect("valid");
        let mut state = GameState::default();
        let view = loom_script::WorldView { positions: &[], events: &[] };
        host.rules("r", 1, 1.0 / 60.0, &view, &mut state).expect("runs");

        assert_eq!(interpolate("Score {score}", &state), "Score 250");
    }

    #[test]
    fn status_and_message_interpolate_too() {
        let mut host = loom_script::ScriptHost::default();
        host.compile("r", r#"status = "won"; message = "nice";"#).expect("valid");
        let mut state = GameState::default();
        let view = loom_script::WorldView { positions: &[], events: &[] };
        host.rules("r", 1, 1.0 / 60.0, &view, &mut state).expect("runs");

        assert_eq!(interpolate("{status}: {message}", &state), "won: nice");
    }

    /// A typo has to be visible. Blanking it would render an empty HUD that
    /// looks like the state is missing rather than the name being wrong.
    #[test]
    fn an_unknown_name_stays_on_screen() {
        assert_eq!(
            interpolate("Ammo {amo}", &GameState::default()),
            "Ammo {amo}"
        );
    }

    #[test]
    fn text_without_any_braces_is_untouched() {
        assert_eq!(interpolate("READY", &GameState::default()), "READY");
    }

    /// An unclosed brace is text, not the start of a name that swallows the
    /// rest of the line.
    #[test]
    fn an_unclosed_brace_is_left_alone() {
        assert_eq!(interpolate("100% {", &GameState::default()), "100% {");
    }

    /// Authored colours are linear like every other colour in the project;
    /// egui wants sRGB. Passing linear straight through draws a mid grey as
    /// near-black, which reads as the HUD being broken rather than mis-encoded.
    #[test]
    fn colours_are_encoded_on_the_way_out() {
        // Linear 0.5 is sRGB ~0.7358, or 188.
        let c = to_color([0.5, 0.5, 0.5]);
        assert!((186..=190).contains(&c.r()), "got {}", c.r());
        assert_eq!(to_color([1.0, 1.0, 1.0]).r(), 255);
        assert_eq!(to_color([0.0, 0.0, 0.0]).r(), 0);
    }
}
