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

/// One resolved element, ready to draw — ADR 0110.
pub(crate) struct Element {
    anchor: egui::Align2,
    offset: egui::Vec2,
    text: String,
    size: f32,
    color: egui::Color32,
    kind: loom_scene::components::HudKind,
    /// Width and height in points, for a bar or a panel.
    extent: egui::Vec2,
    /// How full a bar is, 0..=1, already mapped through its authored range.
    fill: f32,
    /// A panel's fill alpha and a bar's track alpha.
    opacity: f32,
}

#[cfg(test)]
impl Default for Element {
    /// A plain text element — what every `Hud` was before ADR 0110, and the
    /// base the placement tests below vary one field of.
    fn default() -> Self {
        Self {
            anchor: egui::Align2::LEFT_TOP,
            offset: egui::Vec2::ZERO,
            text: String::new(),
            size: 22.0,
            color: egui::Color32::WHITE,
            kind: loom_scene::components::HudKind::Text,
            extent: egui::vec2(180.0, 14.0),
            fill: 1.0,
            opacity: 0.55,
        }
    }
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

            // **A missing number reads full, not empty.** A bar that vanished
            // because the rules script had not written `oxygen` yet would look
            // like a broken bar rather than an unstarted game — and in the
            // editor, where there is no game at all, every bar would be an
            // invisible rectangle you could not find to move.
            let value = component
                .get("value")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let range = pair("range", defaults.range);
            let fill = if value.is_empty() {
                1.0
            } else {
                #[allow(clippy::cast_possible_truncation)]
                match state.number(value) {
                    Some(v) => {
                        let span = range[1] - range[0];
                        if span.abs() < f32::EPSILON {
                            1.0
                        } else {
                            ((v as f32 - range[0]) / span).clamp(0.0, 1.0)
                        }
                    }
                    None => 1.0,
                }
            };
            let kind = component
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .and_then(|k| match k {
                    "bar" => Some(loom_scene::components::HudKind::Bar),
                    "panel" => Some(loom_scene::components::HudKind::Panel),
                    _ => None,
                })
                .unwrap_or(loom_scene::components::HudKind::Text);

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
                kind,
                extent: {
                    let e = pair("extent", defaults.extent);
                    egui::vec2(e[0], e[1])
                },
                fill,
                opacity: scalar("opacity", defaults.opacity),
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

    // **Panels, then bars, then text** — ADR 0110. A backdrop authored after the
    // line it backs would otherwise paint over it, and "why did my label
    // disappear when I added a panel" is a question a scene author should never
    // have to ask. The order is what a human means by a backdrop rather than
    // something they have to arrange, and within a kind the scene's own order is
    // kept, so two panels still stack the way they were written.
    let mut ordered: Vec<&Element> = elements.iter().collect();
    ordered.sort_by_key(|e| match e.kind {
        loom_scene::components::HudKind::Panel => 0,
        loom_scene::components::HudKind::Bar => 1,
        loom_scene::components::HudKind::Text => 2,
    });

    let painted = ordered
        .iter()
        .map(|element| {
            let at = element.anchor.pos_in_rect(&viewport) + element.offset;
            let font = egui::FontId::proportional(element.size);

            // A rectangle sits *from* its anchor the way text does: the anchor
            // names the corner the offset is measured from, so the box grows
            // inward from that corner rather than being centred on the point.
            let boxed = |extent: egui::Vec2| {
                let min = egui::pos2(
                    match element.anchor.x() {
                        egui::Align::Max => at.x - extent.x,
                        egui::Align::Center => at.x - extent.x * 0.5,
                        egui::Align::Min => at.x,
                    },
                    match element.anchor.y() {
                        egui::Align::Max => at.y - extent.y,
                        egui::Align::Center => at.y - extent.y * 0.5,
                        egui::Align::Min => at.y,
                    },
                );
                egui::Rect::from_min_size(min, extent)
            };

            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let alpha = |a: f32| (a.clamp(0.0, 1.0) * 255.0) as u8;
            match element.kind {
                loom_scene::components::HudKind::Panel => {
                    let rect = boxed(element.extent);
                    // The authored colour at the authored opacity. Unmultiplied,
                    // because `color` is the colour a human picked in the swatch
                    // and premultiplying it here would darken it as it faded.
                    let [r, g, b, _] = element.color.to_array();
                    painter.rect_filled(
                        rect,
                        4.0,
                        egui::Color32::from_rgba_unmultiplied(r, g, b, alpha(element.opacity)),
                    );
                    return rect;
                }
                loom_scene::components::HudKind::Bar => {
                    let track = boxed(element.extent);
                    // The track first, at the authored opacity, so an empty bar
                    // is still a visible thing in a place.
                    painter.rect_filled(
                        track,
                        2.0,
                        egui::Color32::from_black_alpha(alpha(element.opacity)),
                    );
                    let mut filled = track;
                    filled.set_right(track.left() + track.width() * element.fill);
                    painter.rect_filled(filled, 2.0, element.color);
                    return track;
                }
                loom_scene::components::HudKind::Text => {}
            }

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

/// One frame's worth of menu navigation, already resolved from the action map.
///
/// **A plain struct of booleans, so this module stays free of `loom_input`.**
/// The menus care that "down" happened, not that it came from `ArrowDown`, a
/// D-pad or a stick past its dead zone — that resolution belongs in `run.rs`
/// where the bindings live, and keeping it there is what lets these functions
/// be tested without an input map at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MenuNav {
    pub up: bool,
    pub down: bool,
    pub confirm: bool,
}

/// Which item a menu has selected — ADR 0087.
///
/// **Nothing is selected until the player moves.** A menu that highlights its
/// first item on open fights the mouse: the pointer is somewhere else, and the
/// highlight says "this is what Enter does" while the eye is on something the
/// pointer is over. So `engaged` starts false, the first `up`/`down` engages it,
/// and hovering hands control back to the mouse.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MenuFocus {
    index: usize,
    engaged: bool,
}

impl MenuFocus {
    /// Apply a frame of navigation to a menu of `len` items.
    ///
    /// Wraps at both ends, because a menu of two with a stick in your hand is
    /// exactly where an unwrapped list feels broken.
    pub(crate) fn step(&mut self, nav: MenuNav, len: usize) {
        if len == 0 {
            return;
        }
        let delta = i64::from(nav.down) - i64::from(nav.up);
        if delta == 0 {
            return;
        }
        if self.engaged {
            #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
            let next = (self.index as i64 + delta).rem_euclid(len as i64) as usize;
            self.index = next;
        } else {
            // The first press engages without moving: pressing down on a fresh
            // menu should light up the first item, not the second.
            self.engaged = true;
            self.index = if delta > 0 { 0 } else { len - 1 };
        }
    }

    /// The selected index, or `None` while the mouse is still in charge.
    pub(crate) fn selected(self) -> Option<usize> {
        self.engaged.then_some(self.index)
    }

    /// Hand control back to the pointer.
    pub(crate) fn disengage(&mut self) {
        self.engaged = false;
    }
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
pub(crate) fn title_menu(
    root: &mut egui::Ui,
    nav: MenuNav,
    focus: &mut MenuFocus,
) -> Option<TitleChoice> {
    let viewport = root.available_rect_before_wrap();
    let mut choice = None;
    let items = [("Start", TitleChoice::Start), ("Quit", TitleChoice::Quit)];
    focus.step(nav, items.len());
    egui::Area::new(egui::Id::new("loom_title_menu"))
        .order(egui::Order::Foreground)
        // Below the word, which the scene anchors just under centre.
        .fixed_pos(viewport.center() + egui::vec2(-MENU_WIDTH * 0.5, 96.0))
        .show(root.ctx(), |ui| {
            ui.set_width(MENU_WIDTH);
            ui.vertical_centered(|ui| {
                for (index, (label, picked)) in items.into_iter().enumerate() {
                    // **Twenty points, not egui's thirteen.** The default is
                    // sized for an inspector row, and under a 96-point word it
                    // reads as a tooltip somebody left on. These two are the
                    // only controls on the screen and the largest target on it
                    // should not be the smallest text.
                    let chosen = focus.selected() == Some(index);
                    let button = egui::Button::new(egui::RichText::new(label).size(20.0))
                        .selected(chosen);
                    let response = ui.add_sized([MENU_WIDTH, 40.0], button);
                    if response.hovered() {
                        focus.disengage();
                    }
                    if response.clicked() || (chosen && nav.confirm) {
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
pub(crate) fn pause_menu(
    root: &mut egui::Ui,
    nav: MenuNav,
    focus: &mut MenuFocus,
) -> Option<PauseChoice> {
    let viewport = root.available_rect_before_wrap();
    dim(root, SCRIM);

    let mut choice = None;
    let items = [("Resume", PauseChoice::Resume), ("Quit", PauseChoice::Quit)];
    focus.step(nav, items.len());
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
                for (index, (label, picked)) in items.into_iter().enumerate() {
                    let chosen = focus.selected() == Some(index);
                    let button = egui::Button::new(label).selected(chosen);
                    let response = ui.add_sized([MENU_WIDTH, 34.0], button);
                    // The pointer takes the highlight back the moment it is
                    // over something, so the two never disagree about what
                    // Enter would do.
                    if response.hovered() {
                        focus.disengage();
                    }
                    if response.clicked() || (chosen && nav.confirm) {
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

// ---------------------------------------------------------------------------
// **THE CREEL: A SPATIAL INVENTORY, PAINTED, CLAIMING NOTHING.**
// ---------------------------------------------------------------------------
//
// `Hud` is `anchor`, `offset`, `text`, `size` and `color`. A grid of items that
// occupy more than one cell is not expressible in it and never will be, so this
// is the second thing `loom_cli` draws over a game — and it is drawn the same
// way the HUD is, for the same three reasons.
//
// **Painted into the root `Ui`, never an `egui::Area`.** An `Area` claims
// `wants_pointer_input` across its whole rect even with painted-only content,
// and a claimed pointer is consumed before the input map ever sees it: that is
// exactly how a visible crosshair ends up attached to a trigger that does
// nothing, and `hud::draw`'s own comment records the round it cost. The root
// `Ui` claims nothing. `nothing_in_the_creel_claims_the_pointer` and
// `nothing_in_the_creel_claims_the_keyboard` are what keep that true.
//
// **No scrim and no full-screen panel.** The demo's frame is open water, the
// water is what this engine is being built to show, and the world does not stop
// while this is up — a panel across the middle hides the thing you most need to
// see. 138 px square in a corner is 0.9% of a 1920x1080 frame.
//
// **Drawn only in `loom run`.** `Ui::new` occurs exactly once in this tree and
// it is behind a `winit::window::Window`; the offscreen renderer never
// constructs one. No reference PNG can move because of anything in this file.
// If one does, the change touched something else and the change is wrong.

/// Cell, gutter and pad in points. 3 * 38 + 2 * 4 + 2 * 8 = 138 square.
const CELL: f32 = 38.0;
const GUTTER: f32 = 4.0;
const PAD: f32 = 8.0;
/// In from the bottom-left corner of whatever the panels left over.
const INSET: egui::Vec2 = egui::vec2(24.0, -24.0);

/// What is in the hand, which is one item held on the cursor.
pub(crate) struct Hand {
    pub label: String,
    /// Footprint **after** any turn, in cells, straight from the rules script —
    /// so the preview paints the cells a press would actually fill.
    pub w: usize,
    pub h: usize,
    /// Whether it would land where the cursor is. Green or red, nothing else.
    pub fits: bool,
}

/// The creel as the overlay needs it: a grid of characters and the labels they
/// stand for.
pub(crate) struct Creel {
    /// One character per cell, `.` for empty. **One letter per placement, not
    /// per kind**, which is what lets two sprats side by side draw as two
    /// objects: the border between two cells is drawn iff their characters
    /// differ, which is one four-neighbour test and no extra data.
    rows: Vec<Vec<char>>,
    /// One label per placement, in the order the letters run.
    labels: Vec<String>,
    pub open: bool,
    pub cursor: (usize, usize),
    pub hand: Option<Hand>,
}

impl Creel {
    /// Parse the two strings the rules script keeps. `None` for a scene that
    /// keeps neither — which is every scene in this project but one, and is why
    /// nothing else has to know this exists.
    ///
    /// A ragged grid is refused rather than drawn short: a row of the wrong
    /// length is a packer bug, and half a grid on screen is a worse way to find
    /// out than nothing on screen.
    fn from_text(cells: &str, kinds: &str) -> Option<Self> {
        let rows: Vec<Vec<char>> = cells.split('/').map(|r| r.chars().collect()).collect();
        let width = rows.first()?.len();
        if width == 0 || rows.iter().any(|r| r.len() != width) {
            return None;
        }
        Some(Self {
            rows,
            labels: kinds
                .split_whitespace()
                .map(std::string::ToString::to_string)
                .collect(),
            open: false,
            cursor: (0, 0),
            hand: None,
        })
    }

    /// Read it out of the running game's own state. The same `GameState` the
    /// HUD already interpolates — no new plumbing, and nothing in `loom_scene`.
    pub(crate) fn read(state: &GameState) -> Option<Self> {
        let mut creel = Self::from_text(
            &state.text("creel_cells")?,
            &state.text("creel_kinds").unwrap_or_default(),
        )?;
        let flag = |name: &str| state.number(name).unwrap_or(0.0) > 0.5;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let index = |name: &str| state.number(name).unwrap_or(0.0).max(0.0) as usize;
        creel.open = flag("creel_open");
        creel.cursor = (index("creel_cx"), index("creel_cy"));
        if flag("creel_hand") {
            creel.hand = Some(Hand {
                label: state.text("creel_hand_label").unwrap_or_default(),
                w: index("creel_hand_w").max(1),
                h: index("creel_hand_h").max(1),
                fits: flag("creel_fits"),
            });
        }
        Some(creel)
    }

    fn width(&self) -> usize {
        self.rows.first().map_or(0, Vec::len)
    }

    fn at(&self, x: usize, y: usize) -> char {
        self.rows
            .get(y)
            .and_then(|row| row.get(x))
            .copied()
            .unwrap_or('.')
    }

    /// The label a cell's character stands for, if any.
    fn label(&self, ch: char) -> Option<&str> {
        let i = usize::from(u8::try_from(ch).ok()?.checked_sub(b'a')?);
        self.labels.get(i).map(String::as_str)
    }
}

/// Fill for an occupied cell. A catch is blue and everything else is green,
/// off the label rather than off a kind: the item table is authored in rhai and
/// a second copy of it here is the divergence this whole project is built to
/// avoid.
fn cell_fill(label: Option<&str>) -> egui::Color32 {
    if label == Some("><>") {
        egui::Color32::from_rgb(38, 66, 88)
    } else {
        egui::Color32::from_rgb(48, 62, 52)
    }
}

/// Draw the creel, and return the plate and every cell — which is what makes
/// the layout testable without a window.
///
/// Returns `None` for a `Creel` with no rows, so a caller cannot draw an empty
/// plate over a game that has no inventory.
pub(crate) fn creel(
    root: &mut egui::Ui,
    view: &Creel,
) -> Option<(egui::Rect, Vec<egui::Rect>)> {
    let (w, h) = (view.width(), view.rows.len());
    if w == 0 || h == 0 {
        return None;
    }
    let viewport = root.available_rect_before_wrap();
    let painter = root.painter().with_clip_rect(viewport);

    #[allow(clippy::cast_precision_loss)]
    let span = |n: usize| n as f32 * CELL + (n as f32 - 1.0) * GUTTER;
    let plate = egui::Rect::from_min_size(
        egui::pos2(
            viewport.left() + INSET.x,
            viewport.bottom() + INSET.y - (span(h) + 2.0 * PAD),
        ),
        egui::vec2(span(w) + 2.0 * PAD, span(h) + 2.0 * PAD),
    );
    // **Dimmer when it is shut, not hidden.** There is no open/close animation
    // to build, no "where has my stuff gone" moment, and the shapes stay
    // legible at a glance in the middle of a fight.
    let plate_alpha = if view.open { 150 } else { 90 };
    painter.rect_filled(plate, 6.0, egui::Color32::from_black_alpha(plate_alpha));

    let origin = plate.min + egui::vec2(PAD, PAD);
    #[allow(clippy::cast_precision_loss)]
    let cell_rect = |x: usize, y: usize| {
        egui::Rect::from_min_size(
            origin + egui::vec2(x as f32 * (CELL + GUTTER), y as f32 * (CELL + GUTTER)),
            egui::Vec2::splat(CELL),
        )
    };

    let mut cells = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let rect = cell_rect(x, y);
            cells.push(rect);
            let ch = view.at(x, y);
            if ch == '.' {
                painter.rect_filled(rect, 2.0, egui::Color32::from_white_alpha(18));
            } else {
                painter.rect_filled(rect, 0.0, cell_fill(view.label(ch)));
            }
        }
    }

    // **One outline per item, not one per cell.** A 3x2 conger is a closed
    // rectangle and not six squares, and the whole rule is: draw the edge
    // between two cells only where their characters differ. Out of bounds
    // counts as different, which is what closes the outer perimeter.
    let edge = egui::Stroke::new(1.0, egui::Color32::from_gray(190));
    for y in 0..h {
        for x in 0..w {
            let ch = view.at(x, y);
            if ch == '.' {
                continue;
            }
            let r = cell_rect(x, y);
            // Half a gutter out, so two abutting items do not share a line and
            // an item's own interior seams stay closed.
            let g = GUTTER * 0.5;
            let differs = |dx: isize, dy: isize| {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    return true;
                }
                #[allow(clippy::cast_sign_loss)]
                let other = view.at(nx as usize, ny as usize);
                other != ch
            };
            if differs(0, -1) {
                let y0 = r.top() - g;
                painter.line_segment(
                    [egui::pos2(r.left() - g, y0), egui::pos2(r.right() + g, y0)],
                    edge,
                );
            }
            if differs(0, 1) {
                let y1 = r.bottom() + g;
                painter.line_segment(
                    [egui::pos2(r.left() - g, y1), egui::pos2(r.right() + g, y1)],
                    edge,
                );
            }
            if differs(-1, 0) {
                let x0 = r.left() - g;
                painter.line_segment(
                    [egui::pos2(x0, r.top() - g), egui::pos2(x0, r.bottom() + g)],
                    edge,
                );
            }
            if differs(1, 0) {
                let x1 = r.right() + g;
                painter.line_segment(
                    [egui::pos2(x1, r.top() - g), egui::pos2(x1, r.bottom() + g)],
                    edge,
                );
            }
        }
    }

    // Labels, once per item, centred on the whole footprint — found as the
    // top-left cell of each run of a character, which is the only cell whose
    // up and left neighbours both differ.
    let font = egui::FontId::monospace(13.0);
    for y in 0..h {
        for x in 0..w {
            let ch = view.at(x, y);
            if ch == '.' {
                continue;
            }
            let up_same = y > 0 && view.at(x, y - 1) == ch;
            let left_same = x > 0 && view.at(x - 1, y) == ch;
            if up_same || left_same {
                continue;
            }
            let mut ex = x;
            while ex + 1 < w && view.at(ex + 1, y) == ch {
                ex += 1;
            }
            let mut ey = y;
            while ey + 1 < h && view.at(x, ey + 1) == ch {
                ey += 1;
            }
            let Some(label) = view.label(ch) else { continue };
            let at = cell_rect(x, y).union(cell_rect(ex, ey)).center();
            // The same dark copy every HUD line pays for, for the same reason:
            // what is behind this is whatever the sea is doing.
            painter.text(
                at + SHADOW,
                egui::Align2::CENTER_CENTER,
                label,
                font.clone(),
                egui::Color32::from_black_alpha(190),
            );
            painter.text(
                at,
                egui::Align2::CENTER_CENTER,
                label,
                font.clone(),
                egui::Color32::from_rgb(226, 232, 220),
            );
        }
    }

    if view.open {
        // The held item's footprint at the cursor, green if a press would land
        // it and red if it would not. Painted before the cursor ring so the
        // ring stays the brightest thing in the grid.
        if let Some(hand) = &view.hand {
            let (cx, cy) = view.cursor;
            let ex = (cx + hand.w - 1).min(w.saturating_sub(1));
            let ey = (cy + hand.h - 1).min(h.saturating_sub(1));
            let area = cell_rect(cx, cy).union(cell_rect(ex, ey));
            let tint = if hand.fits {
                egui::Color32::from_rgba_unmultiplied(90, 200, 110, 150)
            } else {
                egui::Color32::from_rgba_unmultiplied(214, 84, 72, 150)
            };
            painter.rect_filled(area, 2.0, tint);
            painter.text(
                area.center() + SHADOW,
                egui::Align2::CENTER_CENTER,
                &hand.label,
                font.clone(),
                egui::Color32::from_black_alpha(190),
            );
            painter.text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                &hand.label,
                font,
                egui::Color32::WHITE,
            );
        }
        let (cx, cy) = view.cursor;
        painter.rect_stroke(
            cell_rect(cx.min(w - 1), cy.min(h - 1)),
            2.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 214, 120)),
            egui::StrokeKind::Outside,
        );
    }

    Some((plate, cells))
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
            ..Element::default()
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
            ..Element::default()
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
            ..Element::default()
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

    // -----------------------------------------------------------------------
    // **THE CREEL.** Same technique as everything above: a real `egui::Context`
    // with no window anywhere, two passes, and assertions on the rects that
    // came back and the shapes that were emitted.
    // -----------------------------------------------------------------------

    /// A viewport with a side panel in it, the way the viewer actually looks.
    fn creel_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            ..egui::RawInput::default()
        }
    }

    /// Lay the creel out and hand back the plate, the cells, and every shape.
    fn creel_drawn(view: &Creel) -> (egui::Rect, Vec<egui::Rect>, Vec<egui::Shape>) {
        let ctx = egui::Context::default();
        let input = creel_input();
        let mut placed = (egui::Rect::NOTHING, Vec::new());
        let mut shapes = Vec::new();
        // Twice: the first pass of a fresh context has no fonts and no area
        // sizes, exactly as the shadow and pause-menu tests found.
        for _ in 0..2 {
            let out = ctx.run_ui(input.clone(), |root| {
                placed = creel(root, view).expect("the creel has rows");
            });
            shapes = out.shapes.iter().map(|c| c.shape.clone()).collect();
        }
        (placed.0, placed.1, shapes)
    }

    fn open_creel(cells: &str, kinds: &str) -> Creel {
        let mut view = Creel::from_text(cells, kinds).expect("a rectangular grid");
        view.open = true;
        view
    }

    /// The arithmetic, as a test, because every other test here is written
    /// against these numbers. 3 * 38 + 2 * 4 + 2 * 8 = 138 square, and cell
    /// (2, 2) is 2 * (38 + 4) = 84 from the origin in both axes.
    #[test]
    fn the_creel_is_nine_cells_where_the_arithmetic_says() {
        let view = open_creel("abc/d../...", "BAIT LINE LAMP FLSK");
        let (plate, cells, _) = creel_drawn(&view);

        assert_eq!(cells.len(), 9);
        assert!(
            (plate.width() - 138.0).abs() < 0.01 && (plate.height() - 138.0).abs() < 0.01,
            "the plate is {plate:?}, not 138 square"
        );
        let origin = plate.min + egui::vec2(PAD, PAD);
        assert!((cells[0].min - origin).length() < 0.01, "cell (0,0) is {:?}", cells[0]);
        let far = cells[8].min - origin;
        assert!(
            (far.x - 84.0).abs() < 0.01 && (far.y - 84.0).abs() < 0.01,
            "cell (2,2) is {far:?} from the origin, not (84, 84)"
        );
        assert!((cells[0].width() - CELL).abs() < 0.01);
    }

    /// **A 3x2 item is one outline, not six squares**, and that is the whole
    /// reason the grid is projected as one letter per *placement* rather than
    /// per kind. The border between two cells is drawn iff their characters
    /// differ, so a closed 3x2 has a perimeter of 2 * (3 + 2) = 10 segments;
    /// six separate one-cell items would be 24.
    #[test]
    fn a_three_by_two_fish_draws_one_outline_and_not_six() {
        let joined = open_creel("aaa/aaa/...", "><>");
        let (_, _, shapes) = creel_drawn(&joined);
        let segments = shapes
            .iter()
            .filter(|s| matches!(s, egui::Shape::LineSegment { .. }))
            .count();
        assert_eq!(segments, 10, "a joined 3x2 should have a 10-segment perimeter");

        let apart = open_creel("abc/def/...", "A B C D E F");
        let (_, _, shapes) = creel_drawn(&apart);
        let segments = shapes
            .iter()
            .filter(|s| matches!(s, egui::Shape::LineSegment { .. }))
            .count();
        assert_eq!(segments, 24, "six separate items should have six perimeters");
    }

    /// One label per item, not one per cell — a 3x2 fish is one word, centred
    /// on the whole footprint rather than on its top-left cell.
    #[test]
    fn a_multi_cell_item_is_labelled_once_and_in_the_middle() {
        let view = open_creel("aaa/aaa/...", "><>");
        let (plate, cells, shapes) = creel_drawn(&view);
        let texts: Vec<&egui::epaint::TextShape> = shapes
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(t) => Some(t),
                _ => None,
            })
            .collect();
        // Two: the dark copy and the bright one. See `SHADOW`.
        assert_eq!(texts.len(), 2, "one item should paint one label, twice");
        let face = texts[1].pos;
        let middle = cells[0].union(cells[5]).center();
        // `Align2::CENTER_CENTER` reports the galley's top-left, so compare on
        // the axis the centring is unambiguous in.
        assert!(
            (face.x - middle.x).abs() < 12.0 && (face.y - middle.y).abs() < 12.0,
            "the label landed at {face:?}, not near the footprint's centre {middle:?}"
        );
        assert!(plate.contains(face));
    }

    /// **It does not own the frame.** The world does not stop while this is up,
    /// so a panel across the middle hides the thing the player most needs to
    /// see. The plate lives in the bottom-left quadrant and nothing it paints
    /// covers the viewport.
    #[test]
    fn the_creel_stays_in_its_corner_and_draws_no_scrim() {
        let view = open_creel("abc/d../...", "BAIT LINE LAMP FLSK");
        let (plate, _, shapes) = creel_drawn(&view);
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0));

        assert!(plate.center().x < viewport.center().x, "not on the left");
        assert!(plate.center().y > viewport.center().y, "not at the bottom");
        for shape in &shapes {
            if let egui::Shape::Rect(r) = shape {
                assert!(
                    r.rect.width() < 500.0,
                    "something in the creel is {} px wide — that is a scrim",
                    r.rect.width()
                );
            }
        }
    }

    /// **Nothing in it claims the pointer**, which is the regression that would
    /// silently kill `fire` for the whole bottom-left corner of the screen. It
    /// is painted into the root `Ui`; an `egui::Area` claims its whole rect
    /// even with painted-only content — **injected, and this test is what
    /// caught it**, which is the whole reason the creel is painted rather than
    /// laid out.
    #[test]
    fn nothing_in_the_creel_claims_the_pointer() {
        let view = open_creel("abc/d../...", "BAIT LINE LAMP FLSK");
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            // Inside the plate: 24 in from the left, 24 up from the bottom of
            // 600, so (60, 520) is well within the grid.
            events: vec![egui::Event::PointerMoved(egui::pos2(60.0, 520.0))],
            ..creel_input()
        };
        let mut claimed = false;
        for _ in 0..2 {
            let _ = ctx.run_ui(input.clone(), |root| {
                let _ = creel(root, &view);
            });
            claimed = ctx.is_pointer_over_egui();
        }
        assert!(!claimed, "the creel claimed a pointer over the grid");
    }

    /// **And nothing in it claims the keyboard.** The moment a focusable widget
    /// exists in here, egui eats every key and the player stops moving — which
    /// matters more than usual, because W/A/S/D are the cursor.
    ///
    /// **What it does NOT catch is an `egui::Area`**, and that is measured
    /// rather than assumed: wrapping the grid in one — the obvious way to do
    /// this, and what `pause_menu` does — fails
    /// `nothing_in_the_creel_claims_the_pointer` and leaves this one green,
    /// because an empty `Area` claims the pointer and not the keys. The two
    /// tests guard two different mistakes and neither substitutes for the
    /// other.
    #[test]
    fn nothing_in_the_creel_claims_the_keyboard() {
        let view = open_creel("abc/d../...", "BAIT LINE LAMP FLSK");
        let ctx = egui::Context::default();
        let input = creel_input();
        let mut wants = false;
        for _ in 0..2 {
            let _ = ctx.run_ui(input.clone(), |root| {
                let _ = creel(root, &view);
            });
            wants = ctx.egui_wants_keyboard_input();
        }
        assert!(!wants, "the creel claimed the keyboard");
    }

    /// The cursor ring is on the cell the game names, and there is one of it.
    #[test]
    fn the_cursor_ring_sits_on_the_cell_the_game_names() {
        let mut view = open_creel("abc/d../...", "BAIT LINE LAMP FLSK");
        view.cursor = (2, 1);
        let (_, cells, shapes) = creel_drawn(&view);
        let rings: Vec<&egui::epaint::RectShape> = shapes
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Rect(r) if r.stroke.width > 1.5 => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(rings.len(), 1, "one cursor, not {}", rings.len());
        // Row 1, column 2, in a row-major list of nine.
        let wanted = cells[3 + 2];
        assert!(
            (rings[0].rect.min - wanted.min).length() < 0.01,
            "the ring is at {:?}, not on cell (2,1) at {:?}",
            rings[0].rect,
            wanted
        );
    }

    /// **Green when it would land, red when it would not**, and the preview is
    /// the held item's real footprint — 2x3 covers six cells, not one.
    #[test]
    fn the_held_item_previews_its_own_footprint_in_green_or_red() {
        let green = egui::Color32::from_rgba_unmultiplied(90, 200, 110, 150);
        let red = egui::Color32::from_rgba_unmultiplied(214, 84, 72, 150);

        for (fits, wanted) in [(true, green), (false, red)] {
            let mut view = open_creel(".../.../...", "");
            view.cursor = (1, 0);
            view.hand = Some(Hand { label: "><>".to_owned(), w: 2, h: 3, fits });
            let (_, cells, shapes) = creel_drawn(&view);
            let preview = shapes
                .iter()
                .find_map(|s| match s {
                    egui::Shape::Rect(r) if r.fill == wanted => Some(r.rect),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no {wanted:?} preview was painted"));
            let footprint = cells[1].union(cells[2 * 3 + 2]);
            assert!(
                (preview.min - footprint.min).length() < 0.01
                    && (preview.max - footprint.max).length() < 0.01,
                "the preview is {preview:?}, not the 2x3 at the cursor {footprint:?}"
            );
        }
    }

    /// **A scene with no creel draws nothing at all**, which is every scene in
    /// this project but one, and is why nothing else has to know this exists.
    /// A ragged grid is refused rather than drawn short: a row of the wrong
    /// length is a packer bug, and half a grid on screen is a worse way to find
    /// out than nothing on screen.
    #[test]
    fn a_missing_or_ragged_creel_is_not_drawn() {
        assert!(Creel::read(&GameState::default()).is_none(), "no state, no creel");
        assert!(Creel::from_text("ab/c../...", "A B C").is_none(), "ragged");
        assert!(Creel::from_text("", "").is_none(), "empty");
        assert!(Creel::from_text("abc/d../...", "A B C D").is_some(), "the good case");
    }

    /// Every label in the creel is painted twice and the first one is dark, the
    /// same rule `hud::draw` follows and for the same reason: what is behind it
    /// is whatever the sea is doing.
    #[test]
    fn every_creel_label_is_backed_by_a_dark_copy_of_itself() {
        let view = open_creel("ab./.../...", "BAIT ><>");
        let (_, _, shapes) = creel_drawn(&view);
        let texts: Vec<egui::Pos2> = shapes
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(t) => Some(t.pos),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 4, "two labels should paint four texts");
        for pair in texts.chunks(2) {
            assert!(
                (pair[0] - pair[1] - SHADOW).length() < 0.01,
                "shadow at {:?} is not {SHADOW:?} from the face at {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    /// Shut, it is dimmer and it has no cursor — there is no open/close
    /// animation to build and no "where has my stuff gone" moment, and the
    /// shapes stay legible at a glance in the middle of a fight.
    #[test]
    fn a_shut_creel_is_dimmer_and_has_no_cursor() {
        let mut view = open_creel("abc/d../...", "BAIT LINE LAMP FLSK");
        view.open = false;
        let (_, _, shapes) = creel_drawn(&view);
        let rings = shapes
            .iter()
            .filter(|s| matches!(s, egui::Shape::Rect(r) if r.stroke.width > 1.5))
            .count();
        assert_eq!(rings, 0, "a shut creel drew a cursor");
        let plate = shapes
            .iter()
            .find_map(|s| match s {
                egui::Shape::Rect(r) if r.rect.width() > 100.0 => Some(r.fill),
                _ => None,
            })
            .expect("a plate");
        assert_eq!(plate, egui::Color32::from_black_alpha(90));
    }

    /// **A fresh menu selects nothing**, so the highlight never disagrees with
    /// where the pointer is. The first press engages without moving: pressing
    /// down on a menu you just opened should light the *first* item, not skip
    /// past it to the second.
    #[test]
    fn the_first_press_engages_without_skipping() {
        let mut focus = super::MenuFocus::default();
        assert_eq!(focus.selected(), None, "nothing is selected until you move");
        focus.step(super::MenuNav { down: true, ..Default::default() }, 2);
        assert_eq!(focus.selected(), Some(0), "down engages on the first item");
        focus.step(super::MenuNav { down: true, ..Default::default() }, 2);
        assert_eq!(focus.selected(), Some(1));
    }

    /// Up from nothing lands on the last item, which is what a player expects
    /// from pressing up on a fresh menu.
    #[test]
    fn up_from_nothing_lands_on_the_last_item() {
        let mut focus = super::MenuFocus::default();
        focus.step(super::MenuNav { up: true, ..Default::default() }, 2);
        assert_eq!(focus.selected(), Some(1));
    }

    /// Wrapping at both ends. A menu of two with a stick in your hand is
    /// exactly where an unwrapped list feels broken.
    #[test]
    fn the_selection_wraps_at_both_ends() {
        let mut focus = super::MenuFocus::default();
        focus.step(super::MenuNav { down: true, ..Default::default() }, 2);
        focus.step(super::MenuNav { down: true, ..Default::default() }, 2);
        focus.step(super::MenuNav { down: true, ..Default::default() }, 2);
        assert_eq!(focus.selected(), Some(0), "past the end wraps to the start");
        focus.step(super::MenuNav { up: true, ..Default::default() }, 2);
        assert_eq!(focus.selected(), Some(1), "and back off the start wraps to the end");
    }

    /// The pointer takes the highlight back, so the two never disagree about
    /// what confirm would do.
    #[test]
    fn hovering_hands_control_back_to_the_mouse() {
        let mut focus = super::MenuFocus::default();
        focus.step(super::MenuNav { down: true, ..Default::default() }, 2);
        assert!(focus.selected().is_some());
        focus.disengage();
        assert_eq!(focus.selected(), None);
    }

    /// An empty menu must not panic or divide by zero — a menu built from a
    /// filtered list can be empty, and this is cheaper than every caller
    /// remembering.
    #[test]
    fn an_empty_menu_is_inert() {
        let mut focus = super::MenuFocus::default();
        focus.step(super::MenuNav { down: true, ..Default::default() }, 0);
        assert_eq!(focus.selected(), None);
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
                let _ = pause_menu(root, MenuNav::default(), &mut MenuFocus::default());
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
                let _ = title_menu(root, MenuNav::default(), &mut MenuFocus::default());
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
                let _ = title_menu(root, MenuNav::default(), &mut MenuFocus::default());
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

    /// **A bar reads the game's own number through its authored range**
    /// — ADR 0110. The range is the point: a game keeps oxygen in seconds and a
    /// hold in kilograms, and a bar that assumed 0..1 would be full from the
    /// first tick of every game ever written.
    #[test]
    fn a_bar_fills_from_the_state_through_its_range() {
        let world = loom_ecs::World::from_scene(
            &loom_scene::Scene::parse(
                "[scene]\nformat = 1\nid = \"9d2e1a63-8b45-4c79-a1f2-3056b9d48e01\"\n\n\
                 [[node]]\nname = \"Root\"\n\n\
                   [node.components.Hud]\n  kind = \"bar\"\n  value = \"oxygen\"\n\
                   range = [0.0, 180.0]\n",
            )
            .expect("valid scene"),
        );
        let mut host = loom_script::ScriptHost::default();
        host.compile("r", "state.oxygen = 45.0;").expect("valid");
        let mut state = GameState::default();
        let view = loom_script::WorldView { positions: &[], events: &[], submersion: &[] };
        host.rules("r", 1, 1.0 / 60.0, &view, &mut state).expect("runs");

        let resolved = elements(&world, &state, true, false);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].kind, loom_scene::components::HudKind::Bar);
        assert!(
            (resolved[0].fill - 0.25).abs() < 1e-5,
            "45 of 180 is a quarter, got {}",
            resolved[0].fill
        );
    }

    /// **A number the game has not written yet reads full, not empty.** A bar
    /// that vanished before its rules script ran would look like a broken bar;
    /// in the editor, where there is no game at all, every bar would be an
    /// invisible rectangle nobody could find to move.
    #[test]
    fn a_bar_with_no_number_yet_reads_full() {
        let world = loom_ecs::World::from_scene(
            &loom_scene::Scene::parse(
                "[scene]\nformat = 1\nid = \"1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d\"\n\n\
                 [[node]]\nname = \"Root\"\n\n\
                   [node.components.Hud]\n  kind = \"bar\"\n  value = \"never_written\"\n",
            )
            .expect("valid scene"),
        );

        let resolved = elements(&world, &GameState::default(), false, false);

        assert!((resolved[0].fill - 1.0).abs() < 1e-5, "got {}", resolved[0].fill);
    }

    /// **A box grows inward from its anchor, like text does** — ADR 0110.
    ///
    /// The anchor names the corner the offset is measured from, so a panel
    /// anchored bottom-right must extend up and left from that point. Centred
    /// on the point, or grown down-right from it, a corner panel would sit half
    /// off the screen — and that is four cases per axis to get right, which is
    /// exactly the kind of thing that looks fine in the one corner it was
    /// written against.
    #[test]
    fn a_panel_grows_inward_from_whichever_corner_it_is_anchored_to() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            ..egui::RawInput::default()
        };

        for (anchor, offset, name) in [
            (egui::Align2::LEFT_TOP, egui::vec2(20.0, 10.0), "top left"),
            (egui::Align2::RIGHT_BOTTOM, egui::vec2(-20.0, -10.0), "bottom right"),
            (egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0), "centre"),
        ] {
            let element = Element {
                anchor,
                offset,
                kind: loom_scene::components::HudKind::Panel,
                extent: egui::vec2(200.0, 100.0),
                ..Element::default()
            };
            let mut painted = egui::Rect::NOTHING;
            for _ in 0..2 {
                let _ = ctx.run_ui(input.clone(), |root| {
                    painted = draw(root, std::slice::from_ref(&element)).1[0];
                });
            }

            assert!(
                (painted.width() - 200.0).abs() < 0.5 && (painted.height() - 100.0).abs() < 0.5,
                "{name}: the box is not its authored extent: {painted:?}"
            );
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0));
            assert!(
                viewport.contains_rect(painted),
                "{name}: the box left the viewport: {painted:?}"
            );
        }
    }

    /// **A panel draws behind the text it backs, whatever order it was authored
    /// in.** Otherwise "why did my label disappear when I added a backdrop" is a
    /// question every scene author asks once.
    #[test]
    fn a_panel_is_drawn_before_the_text_even_when_authored_after_it() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            ..egui::RawInput::default()
        };
        // Authored text first, panel second — the order that used to hide the
        // text under the backdrop.
        let elements = vec![
            Element { text: "on top".to_owned(), ..Element::default() },
            Element { kind: loom_scene::components::HudKind::Panel, ..Element::default() },
        ];
        let mut painted = Vec::new();
        for _ in 0..2 {
            let _ = ctx.run_ui(input.clone(), |root| {
                painted = draw(root, &elements).1;
            });
        }

        // Two shapes, and the panel's rect is the first of them: `draw` returns
        // them in the order it painted.
        assert_eq!(painted.len(), 2);
        assert!(
            painted[0].width() > 100.0,
            "the first thing painted should be the 180-point panel, got {:?}",
            painted[0]
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
            ..Element::default()
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
