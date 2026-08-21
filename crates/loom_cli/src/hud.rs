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

/// How far the drop shadow sits below and right of the glyphs it backs.
///
/// Two pixels, flat rather than scaled with `size`: every `Hud` in this project
/// is between 19 and 26 points, the outline a shadow reads as is about a pixel
/// wide at any of them, and a scaled offset would drift into a second visible
/// line on a large one.
const SHADOW: egui::Vec2 = egui::vec2(2.0, 2.0);

/// One resolved line, ready to draw.
pub(crate) struct Element {
    anchor: egui::Align2,
    offset: egui::Vec2,
    text: String,
    size: f32,
    color: egui::Color32,
}

#[cfg(test)]
impl Element {
    /// The line after `{name}` substitution — what a player would read.
    ///
    /// Test-only, and `#[cfg(test)]` rather than `#[allow(dead_code)]` so it
    /// cannot quietly become a second way to read an element at runtime. The
    /// shape-counting technique elsewhere in this file proves a line reached
    /// the painter; this is for asking whether the demo still says what its
    /// keys do, which no rendered shape can answer.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
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
    title: bool,
) -> Vec<Element> {
    let flagged = |component: &serde_json::Value, name: &str| {
        component
            .get(name)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    world
        .hud_elements()
        .into_iter()
        // Three states, not two: playing, on the title screen, and neither —
        // which is `loom run` with no game, and is why a title cannot simply
        // be an element with `only_in_play = false`.
        .filter(|component| {
            (playing || !flagged(component, "only_in_play"))
                && (title || !flagged(component, "only_on_title"))
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
            let font = egui::FontId::proportional(element.size);
            // **A shadow under every line, because a HUD has no control over
            // what is behind it.** The demo's inventory row is authored cream
            // (`[0.86, 0.89, 0.82]`) and at the opening heading it lands on a
            // near-white stone cylinder: the row naming one of that demo's four
            // features was, photographed at 4x, ghost glyphs. `Hud` has
            // `color`, `size`, `offset` and `anchor` and no shadow or scrim
            // field, so no authored colour can fix it — the same text is over
            // sky one second and over pale deck the next.
            //
            // Two pixels down-right, painted first so the bright glyphs sit on
            // top. Cheap: the overlay is a few hundred vertices and this
            // doubles it. Drawn only in `loom run`, so no reference PNG can
            // move.
            painter.text(
                at + SHADOW,
                element.anchor,
                &element.text,
                font.clone(),
                egui::Color32::from_black_alpha(190),
            );
            painter.text(at, element.anchor, &element.text, font, element.color)
        })
        .collect();

    (viewport, painted)
}

/// What the human picked in the pause menu, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PauseChoice {
    Resume,
    Quit,
}

/// Wide enough for the longest word on it at the default button font, and
/// narrow enough not to read as a panel.
const MENU_WIDTH: f32 = 180.0;

/// How dark the pause menu's dimming is.
///
/// Heavy on purpose: it is covering a game the player must stop acting on, and
/// the frame behind it is doing nothing for them any more.
const SCRIM: u8 = 170;

/// How dark the title screen's dimming is.
///
/// **Much lighter than the pause menu's, because the shot behind the title is
/// the whole point of the title.** A front end that dims its own photograph to
/// a grey plate is a screenshot with text on it.
///
/// **The number is right; the comparison that first chose it was made in the
/// wrong colour space and the note it left was wrong.** The overlay blends
/// premultiplied into a `B8G8R8A8_SRGB` swapchain, so the hardware converts to
/// linear, blends, and converts back — an alpha of `a` leaves the picture at
/// `(1 - a/255)^(1/2.2)` of its *displayed* brightness, not `1 - a/255`. So 90
/// is 82%, not 65%, and 170 would have been 61%, not 33%: the claim that 170
/// "loses the sun and the glitter path" was an artifact of compositing on the
/// bytes. Both would in fact have kept the picture.
///
/// What 90 actually buys, measured on the shipped shot under the word's box
/// (mean sRGB 103 clear, 84 dimmed): white-on-background contrast rises from
/// **5.7:1 to 7.6:1**. That is the case for it — a readability margin on text
/// laid over water whose brightness nobody controls — and it holds regardless
/// of which space the arithmetic is done in. The word needs no *more* than
/// that, because [`draw`] paints a dark copy under every line.
const TITLE_SCRIM: u8 = 90;

/// Dim the game's view behind the title screen.
///
/// Its own entry point so the caller needs no constant: the front end draws
/// this *before* the HUD, because the game's name is written across the sea
/// and must be dimmed against rather than dimmed. The pause menu dims after,
/// because a score behind a pause menu is as stale as the game is.
pub(crate) fn title_scrim(root: &mut egui::Ui) {
    dim(root, TITLE_SCRIM);
}

/// Paint `alpha` of black over the game's view.
///
/// `available_rect_before_wrap`, not the whole window: a scrim leaves the
/// editor's panels alone, which is deliberate here and deliberately the
/// opposite in [`fade`].
fn dim(root: &mut egui::Ui, alpha: u8) {
    let viewport = root.available_rect_before_wrap();
    root.painter()
        .rect_filled(viewport, 0.0, egui::Color32::from_black_alpha(alpha));
}

/// What the human picked on the title screen, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TitleChoice {
    Start,
    Quit,
}

/// Draw the front end's two buttons over the title shot.
///
/// **The game's name is not here.** It is a `Hud` element with
/// `only_on_title`, so the word, its size, its colour and where it sits are
/// authored in the scene like every other line of text in this project and
/// `loom_cli` never learns what game it is running. This function owns the
/// chrome and nothing else. The caller draws the scrim *before* the HUD, so
/// the word is dimmed-against rather than dimmed.
///
/// **Start and Quit, and no Options.** There is no settings system in this
/// tree — no persisted config, no volume, no keybind store — so an Options
/// item would be a promise made on the first screen a player ever sees and
/// broken on the second. `0683990` made the same call for the pause menu one
/// commit earlier, and a title offering Options over a pause menu that does
/// not would have the two disagree about what this is.
///
/// A near-copy of [`pause_menu`] rather than a shared helper: the two differ
/// in position, heading and items, and unifying two callers costs a
/// six-parameter function to save thirty lines that will never both change.
/// If a third menu appears, unify then.
pub(crate) fn title_menu(root: &mut egui::Ui) -> Option<TitleChoice> {
    let viewport = root.available_rect_before_wrap();
    let mut choice = None;
    let items = [("Start", TitleChoice::Start), ("Quit", TitleChoice::Quit)];
    egui::Area::new(egui::Id::new("loom_title_menu"))
        .order(egui::Order::Foreground)
        // Below the word, which the scene anchors just under centre.
        .fixed_pos(viewport.center() + egui::vec2(-MENU_WIDTH * 0.5, 96.0))
        .show(root.ctx(), |ui| {
            ui.set_width(MENU_WIDTH);
            ui.vertical_centered(|ui| {
                for (label, picked) in items {
                    // **Twenty points, not egui's thirteen.** The default is
                    // sized for an inspector row, and under a 96-point word it
                    // reads as a tooltip somebody left on. These two are the
                    // only controls on the screen and the largest target on it
                    // should not be the smallest text.
                    let button = egui::Button::new(egui::RichText::new(label).size(20.0));
                    if ui.add_sized([MENU_WIDTH, 40.0], button).clicked() {
                        choice = Some(picked);
                    }
                    ui.add_space(10.0);
                }
            });
        });

    // **Escape closes the window here and that has to be said.**
    // `escape_means(false, false, false)` is `Close`, which is the right answer
    // for a top-level menu and a nasty surprise undocumented.
    //
    // **At the bottom edge, not under the buttons.** It belongs with the rest
    // of the legend: a scene writes its own control lines as `Hud` elements
    // with `only_on_title`, and `deeper_demo` puts its two at 40 and 68 px up,
    // so this sits at 16 and the three read as one block. **A title screen that
    // authors its own lines should leave the bottom 32 px to this one.** Under
    // the buttons it was a stray fourteen-point line between two things that
    // were not it.
    //
    // Painted, with `draw`'s shadow, for `draw`'s reasons: this lands on open
    // water whose brightness the engine does not choose, and a painter claims
    // no click — which is what
    // `the_title_screen_does_not_claim_clicks_in_the_viewport` is about.
    let painter = root.painter().with_clip_rect(viewport);
    let at = viewport.center_bottom() - egui::vec2(0.0, 16.0);
    let font = egui::FontId::proportional(15.0);
    painter.text(
        at + SHADOW,
        egui::Align2::CENTER_BOTTOM,
        ESC_HINT,
        font.clone(),
        egui::Color32::from_black_alpha(190),
    );
    painter.text(
        at,
        egui::Align2::CENTER_BOTTOM,
        ESC_HINT,
        font,
        egui::Color32::from_gray(190),
    );
    choice
}

/// What Escape does on the title screen, in one place so the test can name it.
const ESC_HINT: &str = "Esc to quit";

/// Seconds of fade to black once Start is clicked.
///
/// Short: the human just acted and the screen owes them an answer. These are
/// deliberately **not** a UI library's numbers — 300 ms is right for a control
/// sliding in and reads as a flicker on a scene change.
pub(crate) const FADE_OUT: f32 = 0.5;

/// Seconds of black beyond however long building the world blocks for.
///
/// **A floor, not a ceiling.** Starting the game runs on the first frame of
/// this window, and if it costs longer the black simply lasts longer — which
/// is the whole reason the expensive work goes here.
pub(crate) const HOLD: f32 = 0.25;

/// Seconds back up. Longer than the way out on purpose: leaving is a
/// dismissal, arriving is an introduction.
pub(crate) const FADE_IN: f32 = 0.8;

/// The whole transition, click to playable.
pub(crate) const TRANSITION: f32 = FADE_OUT + HOLD + FADE_IN;

/// How black the screen is, `elapsed` seconds into a transition.
///
/// **The exponent is the mechanism, not a taste knob.** `Color32` is
/// premultiplied and the target is `_SRGB`, so the scrim blends in *linear*
/// light: alpha scales radiance rather than display brightness. A linear ramp
/// therefore leaves the screen at 73% of its brightness at the halfway point
/// of its own fade and then plunges — nothing, nothing, slam — and the instinct
/// then is to lengthen the duration, when the duration was never the problem.
/// `1 - (1 - t)^2.2` is the complement of the display gamma, so what falls
/// evenly is what the eye is actually watching.
///
/// One expression serves both directions: on the way up `t` runs 1 to 0 and
/// the curve mirrors exactly. The window opening is the same function started
/// at `FADE_OUT + HOLD`, which is why there is no second one.
pub(crate) fn curtain(elapsed: f32) -> f32 {
    let ramp = |t: f32| 1.0 - (1.0 - t.clamp(0.0, 1.0)).powf(2.2);
    if elapsed < FADE_OUT {
        ramp(elapsed / FADE_OUT)
    } else if elapsed < FADE_OUT + HOLD {
        1.0
    } else {
        ramp(1.0 - (elapsed - FADE_OUT - HOLD) / FADE_IN)
    }
}

/// Black over the whole window at `alpha` in 0..=1.
///
/// **A painter, not an `Area`, and its own layer above every menu.** A
/// full-viewport interactive region once ate every click in this codebase —
/// that is `the_overlay_does_not_claim_clicks_in_the_viewport` — and a painter
/// creates no region at all. `Order::Tooltip` sits above `Order::Foreground`,
/// where both menus are: a fade that does not cover the thing it is fading out
/// is not a fade. And the whole window rather than the game's viewport,
/// because a transition must take the editor's panels with it where a scrim
/// must not — `layer_painter` clips to `content_rect` in any case, which on
/// this platform is the same rectangle minus safe-area insets of zero.
pub(crate) fn fade(root: &egui::Ui, alpha: f32) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    if a == 0 {
        return;
    }
    let ctx = root.ctx();
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("loom_curtain"),
    ))
    .rect_filled(ctx.viewport_rect(), 0.0, egui::Color32::from_black_alpha(a));
}

/// Draw the pause menu over the frozen game and report what was clicked.
///
/// **Two items, because a demo's pause menu is Resume and Quit.** Settings,
/// save slots and a volume slider are a settings system, which is a different
/// job and nobody asked for one.
///
/// An `Area` rather than a `Window`: a window is draggable, collapsible and
/// closable, and all three are wrong for something Escape already closes.
/// Foreground order, so it sits over the editor's panels under `--edit`.
///
/// The scrim goes into `root`, which is the background layer — so it dims the
/// scene and the HUD and leaves the panels alone. That is what a player wants
/// in `--play` and what an editor wants in `--edit`.
pub(crate) fn pause_menu(root: &mut egui::Ui) -> Option<PauseChoice> {
    let viewport = root.available_rect_before_wrap();
    dim(root, SCRIM);

    let mut choice = None;
    let items = [("Resume", PauseChoice::Resume), ("Quit", PauseChoice::Quit)];
    egui::Area::new(egui::Id::new("loom_pause_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(viewport.center() - egui::vec2(MENU_WIDTH * 0.5, 90.0))
        .show(root.ctx(), |ui| {
            ui.set_width(MENU_WIDTH);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("PAUSED")
                        .size(30.0)
                        .color(egui::Color32::WHITE),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Esc to resume")
                        .size(14.0)
                        .color(egui::Color32::from_gray(180)),
                );
                ui.add_space(16.0);
                for (label, picked) in items {
                    let button = egui::Button::new(label);
                    if ui.add_sized([MENU_WIDTH, 34.0], button).clicked() {
                        choice = Some(picked);
                    }
                    ui.add_space(8.0);
                }
            });
        });
    choice
}

/// Replace `{name}` with the game's own numbers — or strings.
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
                // Not a number, so try a string. **Numbers first and not the
                // other way round**: a number is the assertable axis and has
                // to keep its formatting, and a name is only ever one of the
                // two. This is what lets a rules script draw a row of
                // inventory slots — four cells and a fish — which four
                // counters cannot be.
                None => match state.text(name) {
                    Some(text) => out.push_str(&text),
                    None => out.push_str(&rest[start..=end]),
                },
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

    /// **Every line is drawn twice, and the first one is dark.** The demo's
    /// cream inventory row over a near-white stone was unreadable and no
    /// authored colour could fix it — the background is whatever the player is
    /// looking at. Counted rather than eyeballed, because "is it legible" is
    /// the one thing this project has never had an instrument for.
    #[test]
    fn every_line_is_backed_by_a_dark_copy_of_itself() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            ..egui::RawInput::default()
        };
        let element = Element {
            anchor: egui::Align2::LEFT_TOP,
            offset: egui::vec2(16.0, 14.0),
            text: "HOLD 2/4".to_owned(),
            size: 19.0,
            color: egui::Color32::from_rgb(219, 227, 209),
        };

        let mut texts = Vec::new();
        for _ in 0..2 {
            let out = ctx.run_ui(input.clone(), |root| {
                let _ = draw(root, std::slice::from_ref(&element));
            });
            texts = out
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(t) => Some(t.pos),
                    _ => None,
                })
                .collect();
        }

        assert_eq!(texts.len(), 2, "one line should paint a shadow and a face");
        let (shadow, face) = (texts[0], texts[1]);
        assert!(
            (shadow - face - SHADOW).length() < 0.01,
            "shadow at {shadow:?} is not {SHADOW:?} from the face at {face:?}"
        );
    }

    /// **An untested menu is one nobody knows is drawn.** Same technique as
    /// the shadow test above: lay it out through a real `egui::Context` with
    /// no window anywhere, and read the shapes that came out.
    #[test]
    fn the_pause_menu_offers_resume_and_quit() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            ..egui::RawInput::default()
        };

        // Twice: the first pass of a fresh context is a layout pass and areas
        // have no size yet, exactly as the shadow test found.
        let mut labels: Vec<String> = Vec::new();
        let mut scrims = 0;
        for _ in 0..2 {
            let out = ctx.run_ui(input.clone(), |root| {
                let _ = pause_menu(root);
            });
            labels = Vec::new();
            scrims = 0;
            for clipped in &out.shapes {
                match &clipped.shape {
                    egui::Shape::Text(t) => labels.push(t.galley.text().to_owned()),
                    egui::Shape::Rect(r) if r.rect.width() >= 1000.0 => scrims += 1,
                    _ => {}
                }
            }
        }

        for wanted in ["PAUSED", "Resume", "Quit"] {
            assert!(
                labels.iter().any(|l| l == wanted),
                "the menu drew {labels:?}, with no {wanted:?} on it"
            );
        }
        assert_eq!(scrims, 1, "the game behind the menu was not dimmed");
    }

    /// Lay the title screen out with no window anywhere and read the shapes
    /// back, the way `the_pause_menu_offers_resume_and_quit` does.
    ///
    /// `fade` is drawn at full black afterwards, so the same pass answers the
    /// second question: **a curtain that does not cover the menu it is
    /// dismissing is not a curtain.** egui emits shapes in layer order, so
    /// "after every glyph" is the checkable form of "on top", and it is what
    /// fails if the curtain is ever moved into the scrim's layer.
    #[test]
    fn the_title_offers_start_and_quit_under_a_curtain_that_covers_them() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            ..egui::RawInput::default()
        };

        // Twice: the first pass of a fresh context has no fonts and no area
        // sizes, exactly as the pause-menu and shadow tests found.
        let mut labels: Vec<String> = Vec::new();
        let mut scrims = 0;
        let mut last_glyph = 0;
        let mut curtains = Vec::new();
        for _ in 0..2 {
            let out = ctx.run_ui(input.clone(), |root| {
                title_scrim(root);
                let _ = title_menu(root);
                fade(root, 1.0);
            });
            labels = Vec::new();
            scrims = 0;
            last_glyph = 0;
            curtains = Vec::new();
            for (i, clipped) in out.shapes.iter().enumerate() {
                match &clipped.shape {
                    egui::Shape::Text(t) => {
                        labels.push(t.galley.text().to_owned());
                        last_glyph = i;
                    }
                    egui::Shape::Rect(r) if r.rect.width() >= 1000.0 => match r.fill.a() {
                        255 => curtains.push(i),
                        TITLE_SCRIM => scrims += 1,
                        _ => {}
                    },
                    _ => {}
                }
            }
        }

        for wanted in ["Start", "Quit", ESC_HINT] {
            assert!(
                labels.iter().any(|l| l == wanted),
                "the title drew {labels:?}, with no {wanted:?} on it"
            );
        }
        assert!(
            !labels.iter().any(|l| l == "DEEPER"),
            "the game's name belongs to the scene, not to {labels:?}"
        );
        assert_eq!(scrims, 1, "the shot behind the title was not dimmed");
        let [curtain] = curtains[..] else {
            panic!("expected exactly one full-black curtain, got {curtains:?}");
        };
        assert!(
            curtain > last_glyph,
            "the curtain was painted at {curtain}, under a glyph at {last_glyph}"
        );
    }

    /// The title screen is a painter and two buttons, so the viewport behind
    /// it is still the game's — the same guarantee the HUD gives, checked
    /// separately because the title is the one screen drawn over open water
    /// with nothing else to click.
    #[test]
    fn the_title_screen_does_not_claim_clicks_in_the_viewport() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            events: vec![egui::Event::PointerMoved(egui::pos2(200.0, 200.0))],
            ..egui::RawInput::default()
        };
        let mut claimed = false;
        for _ in 0..2 {
            let _ = ctx.run_ui(input.clone(), |root| {
                title_scrim(root);
                let _ = title_menu(root);
                fade(root, 0.4);
            });
            claimed = ctx.is_pointer_over_egui();
        }
        assert!(!claimed, "the title screen swallowed a click over the sea");
    }

    /// **A curtain that is even in alpha is not even to the eye.** The fill is
    /// premultiplied black over a linearised sRGB target, so alpha scales
    /// radiance and displayed brightness goes as `(1 - a)^(1/2.2)`. A linear
    /// ramp leaves the screen at 73% of its brightness at the halfway point of
    /// its own fade and then plunges.
    ///
    /// **The property, not one sample of it.** This was a threshold — `curtain(
    /// FADE_OUT * 0.5) > 0.75` — whose doc claimed it survived a retune, and
    /// it does not: the shipped value is `0.7824` and an exponent of `2.0`
    /// gives exactly `0.7500`, so a gentler curve went red for being gentler.
    /// What the fade actually has to be is *anywhere above a straight line*,
    /// which is true for every exponent past 1 and false the instant somebody
    /// replaces the curve with `t`.
    #[test]
    fn the_curtain_falls_evenly_to_the_eye_rather_than_to_the_alpha() {
        for i in 1..100u8 {
            let t = f32::from(i) / 100.0;
            let at = curtain(t * FADE_OUT);
            assert!(
                at > t,
                "{t:.2} of the way out the curtain is {at:.4}, at or under the \
                 straight line it exists to not be"
            );
        }
        assert!(curtain(0.0).abs() < 1e-6, "the fade starts on the picture");
        assert!((curtain(FADE_OUT) - 1.0).abs() < 1e-6, "black by the hold");
        assert!((curtain(FADE_OUT + HOLD) - 1.0).abs() < 1e-6, "still black");
        assert!(curtain(TRANSITION).abs() < 1e-6, "and clear again at the end");
        assert!(curtain(TRANSITION + 5.0).abs() < 1e-6, "and stays clear");

        // Monotone in each leg, or the picture would breathe on the way down.
        let leg = |a: f32, b: f32| {
            let mut previous = curtain(a);
            for i in 1..=100u8 {
                let at = (b - a).mul_add(f32::from(i) / 100.0, a);
                let now = curtain(at);
                assert!(now >= previous - 1e-6, "the curtain lifted at {at}");
                previous = now;
            }
        };
        leg(0.0, FADE_OUT);
        leg(TRANSITION, FADE_OUT + HOLD);
    }

    /// **A title is a line the scene authors, and the editor must never see
    /// it.** There are three states, not two: an element flagged
    /// `only_on_title` is absent while a game runs *and* absent in `loom run`
    /// with no game at all, which is what an `only_in_play = false` title
    /// would have got wrong.
    #[test]
    fn a_title_line_shows_on_the_title_screen_and_nowhere_else() {
        let world = loom_ecs::World::from_scene(
            &loom_scene::Scene::parse(
                "[scene]\nformat = 1\nid = \"7c1f0b52-9a34-4d68-b0e1-2f45a8c37d90\"\n\n\
                 [[node]]\nname = \"Root\"\n\n\
                   [node.components.Hud]\n  text = \"DEEPER\"\n\
                   only_on_title = true\n",
            )
            .expect("valid scene"),
        );

        let on_title = elements(&world, &GameState::default(), false, true);
        assert_eq!(on_title.len(), 1, "the word belongs on the title screen");
        assert_eq!(on_title[0].text, "DEEPER");

        assert!(
            elements(&world, &GameState::default(), false, false).is_empty(),
            "the title was drawn over the editor"
        );
        assert!(
            elements(&world, &GameState::default(), true, false).is_empty(),
            "the title was still up during the game"
        );
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

        let resolved = elements(&world, &GameState::default(), false, false);

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
        let view = loom_script::WorldView { positions: &[], events: &[], submersion: &[] };
        host.rules("r", 1, 1.0 / 60.0, &view, &mut state).expect("runs");

        assert_eq!(interpolate("Score {score}", &state), "Score 250");
    }

    #[test]
    fn status_and_message_interpolate_too() {
        let mut host = loom_script::ScriptHost::default();
        host.compile("r", r#"status = "won"; message = "nice";"#).expect("valid");
        let mut state = GameState::default();
        let view = loom_script::WorldView { positions: &[], events: &[], submersion: &[] };
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

    /// Lay one line out at its authored size and give back how wide egui
    /// actually made it.
    fn laid_out_width(text: &str, size: f32) -> f32 {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(4000.0, 600.0),
            )),
            ..egui::RawInput::default()
        };
        let element = Element {
            anchor: egui::Align2::CENTER_BOTTOM,
            offset: egui::vec2(0.0, 56.0),
            text: text.to_owned(),
            size,
            color: egui::Color32::WHITE,
        };
        let mut width = 0.0_f32;
        // Two passes: egui has no fonts loaded on the first.
        for _ in 0..2 {
            let out = ctx.run_ui(input.clone(), |root| {
                let _ = draw(root, std::slice::from_ref(&element));
            });
            width = out
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(t) => Some(t.galley.rect.width()),
                    _ => None,
                })
                .fold(0.0, f32::max);
        }
        width
    }

    /// **A `Hud` draws a string the rules script keeps, as well as a number.**
    ///
    /// `{message}` was this mechanism for one hard-coded name; the demo's hold
    /// row needs it for `{slots}`, because four slots showing what is *in* them
    /// is glyphs and four counters are not. Numbers keep their formatting, an
    /// unknown name is still left standing, and neither of those is a thing a
    /// string lookup may quietly take over — so all three are asserted here.
    #[test]
    fn a_hud_interpolates_a_string_a_number_and_neither() {
        let mut host = loom_script::ScriptHost::default();
        host.compile("r", r#"state.slots = "[BAIT ][ ><> ]"; state.stowed = 3;"#)
            .expect("valid");
        let mut state = GameState::default();
        let view = loom_script::WorldView { positions: &[], events: &[], submersion: &[] };
        host.rules("r", 1, 1.0 / 60.0, &view, &mut state).expect("runs");

        assert_eq!(
            interpolate("HOLD {slots}  BELOW {stowed}  CRATE {nope}", &state),
            "HOLD [BAIT ][ ><> ]  BELOW 3  CRATE {nope}"
        );
    }

    /// **The longest line the demo can say has to fit the window it is played
    /// in**, and until this existed nothing could tell.
    ///
    /// `scripts/green.sh` pins caption strings with `grep -q` on `loom sim`'s
    /// JSON, which proves a string was *produced* and cannot prove it was
    /// drawable. Both `Hud` rows in `deeper_demo.loom` are `bottom_center`, so
    /// an over-long line is clipped at *both* ends — it loses the verb as well
    /// as the object, which is the worst way for an instruction to fail.
    ///
    /// The four strings below are the worst cases of the four longest branches:
    /// the idle helm caption carrying a three-digit range, the two sentences the
    /// catch-on-E round made longest, and the `Hold` row with every slot filled.
    /// None is hypothetical — all assemble from parts reachable in one run, and
    /// 282 m is a measured position, not a guess.
    ///
    /// **Measured, one row at a time.** The helm caption lays out at **840 px**,
    /// the step-off line at **841**, the fish-on-the-line route line at **830**
    /// and the slot row at **508**. The step-off line is the worst case now and
    /// was not before: putting the catch on E added `E at the ` to it. The slot
    /// row *fell* — 621 px to 508 — because four cells of five characters are
    /// shorter than four names with a counter each.
    ///
    /// The round that budgeted for this worked from a per-character
    /// average read off a screenshot — 12.9 px/char, which puts the caption at
    /// 1135 px and predicts an overflow that does not happen. The real figure
    /// is 9.5 px/char, because the average was taken on a short all-caps
    /// string and the long branches are mostly lower case. A proportional font
    /// has no per-character width; laying the line out is the only way to ask.
    ///
    /// The instrument was cross-checked against the one photograph anyone has
    /// taken of this overlay: the inventory row of the day measured **619 px**
    /// on screen with a ruler, against **621 px** here. That agreement is what
    /// makes these numbers worth trusting; the row itself has since changed and
    /// nobody has photographed the new one.
    ///
    /// `MIN_VIEWPORT` is the narrowest window this demo is documented to work
    /// in, not the narrowest egui will open. 960 px leaves the caption 120 px
    /// of margin — about twelve more characters — so this bites well before a
    /// player loses anything. Widening a caption past it is allowed; it is
    /// then a decision about the minimum window, made here and written into
    /// the scene header, rather than a line whose ends a player silently
    /// loses.
    #[test]
    fn the_longest_demo_caption_fits_the_narrowest_documented_window() {
        const MIN_VIEWPORT: f32 = 960.0;

        let caption = "THE HELM   W ahead  S astern  A/D wheel  SPACE lets go   \
5 kn   HOME 282 m \u{2014} hold S";
        // **The round that put the catch on E made two of these longer**, which
        // is exactly the change this test exists to catch: `E at the ...` is
        // four characters of key name added to sentences that were already the
        // longest ones on screen, and the route line the fish-on-the-line state
        // draws is the longest string the file can produce at all.
        let on_the_line = "SHE IS ON THE LINE   E takes her off \u{2014} or go RIGHT round \
the bait box to the fish box";
        let step_off = "1 BELOW   SHE IS ALONGSIDE \u{2014} step off, E at the crate 12 m \
behind you, on your right";
        // **The hold row is four slots now, not four counters** — `{slots}`,
        // built by `slot_row` in `deeper_rules.rhai`. Worst case is every cell
        // filled with the widest word it can hold and two-digit tallies.
        let inventory = "HOLD  [FLASK][LAMP ][LINE ][ ><> ]      BELOW 99   CRATE 99";

        for (what, text, size) in [
            ("caption", caption, 22.0_f32),
            ("on-the-line caption", on_the_line, 22.0_f32),
            ("step-off caption", step_off, 22.0_f32),
            ("inventory", inventory, 19.0_f32),
            // The title is on the same list because it is on the same path —
            // a `Hud` line laid out by `draw` — and it is by far the largest
            // point size any scene in this project asks for.
            ("title", "DEEPER", 96.0_f32),
            // **And the two control lines under it**, which is where the demo
            // tells a first-timer what the keys are. They are the longest
            // strings the scene authors and nothing else measures them: a HUD
            // caption that overflows is one round of a fight, a title legend
            // that overflows is the first thing anybody sees.
            (
                "title keys",
                "WASD walk    MOUSE look    SPACE jump    SHIFT sprint",
                19.0_f32,
            ),
            (
                "title verbs",
                "CLICK casts, and again to set the hook    SHIFT reels    \
                 E takes the fish    TAB the creel",
                19.0_f32,
            ),
        ] {
            let width = laid_out_width(text, size);
            // A line measuring zero never reached the painter, which would
            // make the bound below vacuously true.
            assert!(
                width > 100.0,
                "the {what} row laid out {width:.0} px, which is not a line"
            );
            assert!(
                width <= MIN_VIEWPORT,
                "the {what} row lays out {width:.0} px at size {size}, wider than the \
                 {MIN_VIEWPORT:.0} px window this demo documents. It is centre-anchored, so it \
                 loses both ends. Shorten the branch in deeper_rules.rhai, or raise \
                 MIN_VIEWPORT here and say so in the scene header."
            );
        }
    }
}
