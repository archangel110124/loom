//! The docked frame: which panels exist, where they start, and where the
//! scene ends up.
//!
//! **The scene is a *hole* in this layout, not a tab that draws anything.**
//! `Tab::Scene`'s body is empty on purpose: the renderer draws into the
//! rectangle the dock leaves, as a sub-rectangle of the swapchain (ADR 0025).
//! So the tab's whole job is to occupy space and report where it is.
//!
//! # The carve, and why the dock does not get the root `Ui`
//!
//! `DockArea::show_inside` allocates the *entire* available rect of whatever
//! `Ui` it is handed (`allocate_area_for_root_node`, egui_dock
//! `show/mod.rs:411`). Handing it the root `Ui` therefore empties
//! `root_ui_available_rect`, which egui writes once after `run_ui`
//! (egui `context.rs:803`) and which `is_pointer_over_egui` answers with
//! `!rect.contains(pos)` for anything on the background layer
//! (`context.rs:2841`). `run.rs` gates viewport picking on exactly that, so a
//! dock drawn on the root `Ui` makes egui claim the pointer over the whole
//! window and **every click in the viewport dies**.
//!
//! So the dock draws into a child `Ui` — which allocates nothing from the root
//! — and afterwards four empty, frameless `egui::Panel`s are laid on the root
//! to *carve* the layout back out of it. What is left over is exactly the
//! viewport tab's rectangle, which is then simultaneously:
//!
//! - what `Ui::layout` reads for `UiFrame::scene_rect`, so the renderer draws
//!   into the hole;
//! - what `is_pointer_over_egui` compares against, so clicks land;
//! - what `hud::draw` anchors to, so the HUD sits over the game and not over
//!   the hierarchy. The HUD needs no change and gets none.
//!
//! **When no viewport tab is visible the carve is skipped entirely** and the
//! root's whole area is allocated instead. A `Scene` tab dragged behind
//! `Console` reports nothing — `TabViewer::ui` only runs for a leaf's *active*
//! tab — and reusing the last rectangle would leave a click-through hole under
//! whatever panel now occupies it, plus a scene rendered underneath. Losing the
//! viewport for a frame is the correct answer: the placement goes degenerate
//! and egui owns the window.

use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use loom_render::egui;

use crate::panels::{self, PanelState, UiAction};

/// Every panel the editor has. **Fixed at eleven, once.**
///
/// Adding a variant later invalidates every saved layout — `egui_dock`'s tree
/// is persisted by tab identity — so this list is decided in one go rather
/// than grown. `Environment`, `Terrain`, `Events`, `Profiler` and `Foliage`
/// were all considered and cut: a tab whose body is empty is worse than no
/// tab, because it advertises a feature that is not there and it costs a
/// layout migration to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tab {
    /// The 3D viewport. **Draws nothing** — see the module docs.
    Scene,
    /// The running game, when playing. Also a hole.
    Game,
    Hierarchy,
    Inspector,
    Project,
    Console,
    Problems,
    History,
    Transactions,
    Prefabs,
    Agent,
}

impl Tab {
    /// Every variant, for the Window menu and for layout restoration.
    pub const ALL: [Self; 11] = [
        Self::Scene,
        Self::Game,
        Self::Hierarchy,
        Self::Inspector,
        Self::Project,
        Self::Console,
        Self::Problems,
        Self::History,
        Self::Transactions,
        Self::Prefabs,
        Self::Agent,
    ];

    /// The tab's title, and the string a saved layout stores.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Scene => "Scene",
            Self::Game => "Game",
            Self::Hierarchy => "Hierarchy",
            Self::Inspector => "Inspector",
            Self::Project => "Project",
            Self::Console => "Console",
            Self::Problems => "Problems",
            Self::History => "History",
            Self::Transactions => "Transactions",
            Self::Prefabs => "Prefabs",
            Self::Agent => "Agent",
        }
    }

    /// The inverse of [`Self::title`], for reading a saved layout back.
    ///
    /// Returns `None` for a title this build does not know, which is how a
    /// layout saved by a newer build degrades instead of failing: the unknown
    /// tab is dropped and the rest of the arrangement survives.
    #[must_use]
    pub fn from_title(title: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.title() == title)
    }

    /// Whether the renderer draws into this tab rather than egui.
    ///
    /// The two holes are the reason `ViewportPlacement` exists.
    #[must_use]
    pub fn is_viewport(self) -> bool {
        matches!(self, Self::Scene | Self::Game)
    }
}

/// How tall the bottom node starts, in points.
///
/// **One bottom node, not two.** Unity's four regions are worth copying
/// because they buy the only free familiarity available, but Unity has no
/// full-width Project strip: 180 pt of console plus 160 pt of project under a
/// menu bar, a toolbar and a status bar leaves a 42% viewport in an editor
/// whose subject is a 3D scene. `Project` is a tab of this node instead.
pub const BOTTOM_HEIGHT: f32 = 280.0;

/// Fractions of the window the three side regions take initially.
pub const LEFT_FRACTION: f32 = 0.18;
pub const RIGHT_FRACTION: f32 = 0.26;

/// The editor's docked layout, and the only thing that draws panels.
///
/// Owns nothing but the arrangement. Every panel is still a pure function of a
/// borrowed [`PanelState`] returning [`UiAction`]s, so the dock cannot grow a
/// write path (never-do #16) and needs no trait to dispatch to a panel — one
/// `match` in [`Shell::ui`] is the whole of it (never-do #12).
pub struct Dock {
    state: DockState<Tab>,
    /// Last frame's play state, so entering play can focus `Game` **once**
    /// rather than re-focusing it every frame — which would make the `Scene`
    /// tab unclickable for as long as the game ran.
    was_playing: bool,
    /// Whether this session reads and writes the human's saved layout.
    ///
    /// False under `--frames`: the gate must not depend on the last splitter
    /// drag, and must not overwrite it either.
    persist: bool,
}

/// The per-frame bridge between `egui_dock` and the panels.
///
/// Borrowed state in, actions and one rectangle out. It exists for the frame
/// and is dropped; nothing about the arrangement lives here.
struct Shell<'a, 'b> {
    state: &'a PanelState<'b>,
    actions: Vec<UiAction>,
    /// Where the visible viewport tab's body landed, in egui points.
    ///
    /// `None` when neither hole is the active tab of any leaf drawn this
    /// frame. See the module docs for why that is not the same as "reuse the
    /// last one".
    viewport: Option<egui::Rect>,
    playing: bool,
}

// `TabViewer` is `egui_dock`'s trait, not one this crate invented, so never-do
// #12 does not apply: implementing an external trait is how the crate is used.
impl TabViewer for Shell<'_, '_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match *tab {
            Tab::Scene | Tab::Game => {
                // Both can be visible at once, in different leaves. The one
                // that matches the play state wins, because that is the one
                // the human is looking at — and `max_rect` is the true body
                // rect only because `tab_style_override` zeroed the margin.
                let wanted = if self.playing { Tab::Game } else { Tab::Scene };
                if self.viewport.is_none() || *tab == wanted {
                    self.viewport = Some(ui.max_rect());
                }
            }
            Tab::Hierarchy => panels::hierarchy(ui, self.state, &mut self.actions),
            Tab::Inspector => panels::inspector(ui, self.state, &mut self.actions),
            Tab::Project => panels::assets(ui, self.state, &mut self.actions),
            Tab::Console => panels::console_column(ui, self.state.console, &mut self.actions),
            Tab::Transactions => panels::transactions(ui, self.state.history),
            // Bodies land in Stage 4 (Problems, History), Stage 6 (Agent) and
            // Stage 7 (Prefabs). A named empty tab is deliberate: the eleven
            // variants are fixed once because adding one invalidates every
            // saved layout, so the tab exists before its panel does.
            Tab::Problems | Tab::History | Tab::Prefabs | Tab::Agent => {
                ui.weak("not built yet");
            }
        }
    }

    /// The renderer fills the two holes, so egui must not paint over them.
    fn clear_background(&self, tab: &Tab) -> bool {
        !tab.is_viewport()
    }

    /// **No scroll bars on a viewport**, or the wheel scrolls the tab body
    /// instead of dollying the camera.
    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        [!tab.is_viewport(); 2]
    }

    /// Eleven tabs, all of them permanent. Closing one is a layout the Window
    /// menu can restore but nothing in the UI explains, which is worse than
    /// not offering it.
    fn is_closeable(&self, _tab: &Tab) -> bool {
        false
    }

    /// **Both overrides are required, not cosmetic.**
    ///
    /// The tab body is drawn inside a `ScrollArea` and a `Frame` carrying
    /// `tab_body.inner_margin` (egui_dock `show/leaf.rs`), so without zeroing
    /// it `ui.max_rect()` is inset and the carve is off by the margin — the
    /// renderer would draw a rectangle that does not match the hole. And the
    /// body's *stroke* is painted unconditionally, outside the
    /// `clear_background` check, so leaving it on draws a border over the
    /// rendered scene.
    fn tab_style_override(
        &self,
        tab: &Tab,
        global: &egui_dock::TabStyle,
    ) -> Option<egui_dock::TabStyle> {
        if !tab.is_viewport() {
            return None;
        }
        let mut style = global.clone();
        style.tab_body.inner_margin = egui::Margin::ZERO;
        style.tab_body.stroke = egui::Stroke::NONE;
        Some(style)
    }
}

/// Unity's four regions, with **one** bottom node rather than two — see
/// [`BOTTOM_HEIGHT`].
///
/// `height` is the window's logical height, because the bottom node is
/// authored in points and `egui_dock` splits by fraction.
///
/// **The fraction's direction is not what the crate's doc comment says.** It
/// claims the fraction is the share the *old* node keeps, and that is true for
/// `Right` and `Below`, where `split` assigns `[parent.left(), parent.right()]`
/// and `compute_rect_sizes` gives the fraction to the left/top child. For
/// `Left` and `Above` the assignment is reversed — `[parent.right(),
/// parent.left()]` — so the fraction lands on the **new** node. Hence
/// `LEFT_FRACTION` passed as-is and `1.0 - RIGHT_FRACTION` for the mirror
/// case. `the_fractions_go_to_the_nodes_they_name` pins it.
fn default_layout(height: f32) -> DockState<Tab> {
    let mut state = DockState::new(vec![Tab::Scene, Tab::Game]);
    let surface = state.main_surface_mut();

    // Clamped so a very short window cannot ask for a bottom node taller than
    // the window: `split` panics outside 0..=1.
    let upper_share = 1.0 - BOTTOM_HEIGHT / height.max(2.0 * BOTTOM_HEIGHT);
    let [upper, _bottom] = surface.split_below(
        NodeIndex::root(),
        upper_share,
        vec![
            Tab::Console,
            Tab::Project,
            Tab::Problems,
            Tab::History,
            Tab::Transactions,
            Tab::Agent,
        ],
    );
    let [upper, _left] = surface.split_left(upper, LEFT_FRACTION, vec![Tab::Hierarchy]);
    let [_centre, _right] =
        surface.split_right(upper, 1.0 - RIGHT_FRACTION, vec![Tab::Inspector, Tab::Prefabs]);
    state
}

impl Dock {
    /// A dock, from the saved layout when `persist` and from the default
    /// arrangement otherwise.
    #[must_use]
    pub fn new(persist: bool, height: f32) -> Self {
        Self {
            state: if persist { load(height) } else { default_layout(height) },
            was_playing: false,
            persist,
        }
    }

    /// Draw the toolbar, the dock and the overlays, and carve the viewport
    /// back out of `root`. See the module docs for what the carve is for.
    pub fn draw(&mut self, root: &mut egui::Ui, state: &PanelState<'_>) -> Vec<UiAction> {
        let mut actions = Vec::new();
        // The toolbar and the banner are chrome rather than tabs: they are
        // above the whole arrangement, in every editor, and a human cannot
        // close or move them.
        panels::toolbar(root, state, &mut actions);
        if state.conflict {
            panels::conflict_banner(root, &mut actions);
        }

        let playing = state.playing.is_some();
        // Rising edge only. Pressing Play brings the Game tab forward once;
        // after that the human is free to look at the Scene while it runs.
        if playing && !self.was_playing && let Some(path) = self.state.find_tab(&Tab::Game) {
            // An error here means the tab vanished between the find and the
            // set, which cannot happen — and if it somehow did, the layout is
            // still perfectly usable.
            let _ = self.state.set_active_tab(path);
        }
        self.was_playing = playing;

        let area = root.available_rect_before_wrap();
        let mut shell = Shell { state, actions, viewport: None, playing };
        {
            let mut child = root.new_child(egui::UiBuilder::new().max_rect(area));
            DockArea::new(&mut self.state)
                .show_close_buttons(false)
                .show_add_buttons(false)
                .show_inside(&mut child, &mut shell);
        }
        let actions = shell.actions;

        match shell.viewport.map(|rect| rect.intersect(area)) {
            Some(hole) if hole.is_positive() => carve(root, area, hole),
            // No viewport tab visible: egui owns the lot. `available_rect`
            // collapses, `ViewportPlacement` clamps it to a degenerate 1×1 and
            // no click can reach a scene that is not on screen.
            _ => {
                root.allocate_rect(area, egui::Sense::hover());
            }
        }

        // Painted on top of everything, into the background layer, clipped to
        // window pixels the caller already projected. They allocate nothing,
        // so they cannot disturb the carve.
        panels::gizmo_overlay(root, state);
        panels::agent_overlay(root, state);
        actions
    }

    /// Write the layout to XDG state. Called from `exiting`, never per frame.
    pub fn save(&self) {
        if !self.persist {
            return;
        }
        let Some(path) = layout_path() else { return };
        let text = match serde_json::to_string_pretty(&for_saving(&self.state)) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("loom: could not encode the dock layout: {e}");
                return;
            }
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("loom: could not save the dock layout to {}: {e}", path.display());
        }
    }
}

/// The writable form of a layout: titles, and not one infinity.
///
/// **Titles, not the enum.** The persistence format is a string per tab
/// (`Tab::from_title` is its inverse), which is what lets a layout written by a
/// build with a twelfth tab still load here, and what keeps `serde` off this
/// crate's dependency list.
///
/// **And every rectangle is zeroed.** A node that has never been drawn holds
/// `Rect::NOTHING`, whose infinities `serde_json` writes as `null` and then
/// refuses to read back as `f32` — egui_dock documents exactly this hazard in
/// `window_state.rs`, and it is reachable here: a session that quits before its
/// first frame saves a file that never loads again. Rects are recomputed on the
/// first frame anyway, so they are noise as well as a trap. `Node::set_rect`
/// covers split nodes, which carry the same default and which a leaf-only walk
/// would miss.
fn for_saving(state: &DockState<Tab>) -> DockState<String> {
    let mut saved = state.map_tabs(|tab| tab.title().to_owned());
    zero_rects(&mut saved);
    saved
}

/// See [`for_saving`]: an undrawn rect is `Rect::NOTHING`, and JSON cannot
/// carry an infinity.
fn zero_rects<T>(state: &mut DockState<T>) {
    for (_, node) in state.iter_all_nodes_mut() {
        node.set_rect(egui::Rect::ZERO);
        if let Some(leaf) = node.get_leaf_mut() {
            leaf.viewport = egui::Rect::ZERO;
        }
    }
}

/// Reserve everything around `hole` on `root`, so what is left is the hole.
///
/// Four empty panels rather than one call, because egui has no "shrink to this
/// rectangle" on a `Ui`: a panel is how a rect is taken out of a parent, and
/// each one moves the parent's cursor along its own axis only. Order is
/// irrelevant for the same reason.
fn carve(root: &mut egui::Ui, area: egui::Rect, hole: egui::Rect) {
    let sides = [
        (egui::Panel::left("loom_carve_left"), hole.min.x - area.min.x),
        (egui::Panel::right("loom_carve_right"), area.max.x - hole.max.x),
        (egui::Panel::top("loom_carve_top"), hole.min.y - area.min.y),
        (egui::Panel::bottom("loom_carve_bottom"), area.max.y - hole.max.y),
    ];
    for (panel, size) in sides {
        panel
            // `exact_size` is a `Rangef::point`, which the resize clamp then
            // pins the panel to — this is what makes the carve land on the
            // dock's edges rather than near them.
            .exact_size(size.max(0.0))
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE)
            .show(root, |_| {});
    }
}

/// `$XDG_STATE_HOME/loom/layouts/default.json`, or the spec's fallback.
///
/// Read from the environment rather than through a crate: two `env::var`s and
/// a `join` is the whole of the XDG state rule this needs, and a dependency
/// that resolves five other directories is not worth linking for it.
fn layout_path() -> Option<std::path::PathBuf> {
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(dir) if !dir.is_empty() => std::path::PathBuf::from(dir),
        _ => std::path::PathBuf::from(std::env::var_os("HOME")?).join(".local/state"),
    };
    Some(base.join("loom/layouts/default.json"))
}

/// The saved layout, degrading to [`default_layout`] rather than failing.
///
/// A layout is a convenience. Every way of not getting one back — no file, a
/// truncated file, a file from a build whose tabs are all unknown — ends in the
/// default arrangement, because refusing to open the editor over a stale
/// cache file would be absurd.
fn load(height: f32) -> DockState<Tab> {
    let Some(path) = layout_path() else {
        return default_layout(height);
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return default_layout(height);
    };
    from_json(&text, height)
}

/// The half of [`load`] that touches no filesystem, so it can be tested.
fn from_json(text: &str, height: f32) -> DockState<Tab> {
    let saved: DockState<String> = match serde_json::from_str(text) {
        Ok(saved) => saved,
        Err(e) => {
            eprintln!("loom: ignoring the saved dock layout ({e})");
            return default_layout(height);
        }
    };
    let mut state = saved.filter_map_tabs(|title| Tab::from_title(title));
    // **Guarded, because `main_surface()` indexes rather than looks up.** A
    // file whose tabs this build all rejects — or one hand-edited down to
    // nothing — filters to a state with no tabs, and every repair below would
    // panic on it.
    if state
        .get_surface(egui_dock::SurfaceIndex::main())
        .and_then(egui_dock::Surface::node_tree)
        .is_none_or(|tree| tree.num_tabs() == 0)
    {
        return default_layout(height);
    }
    // A tab this build has and the file does not — because the file predates
    // it, or because a future build let the human close one — comes back
    // somewhere visible rather than being silently unreachable.
    for tab in Tab::ALL {
        if state.find_tab(&tab).is_none() {
            state.main_surface_mut().push_to_first_leaf(tab);
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::{
        BOTTOM_HEIGHT, Dock, LEFT_FRACTION, PanelState, RIGHT_FRACTION, Tab, default_layout,
        egui, from_json,
    };
    use egui_dock::{DockState, Node, NodeIndex};

    /// **Eleven, and the count is the assertion.** A twelfth variant added
    /// later invalidates every saved layout, so this failing is the reminder
    /// that adding one is a migration rather than an edit.
    #[test]
    fn the_tab_list_is_fixed_at_eleven_ending_in_agent() {
        assert_eq!(Tab::ALL.len(), 11);
        assert_eq!(*Tab::ALL.last().unwrap(), Tab::Agent);
    }

    /// Titles are the persistence format, so they must round-trip and must be
    /// unique — two tabs sharing a title would silently collapse into one on
    /// restore.
    #[test]
    fn titles_round_trip_and_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for tab in Tab::ALL {
            assert!(seen.insert(tab.title()), "duplicate title {}", tab.title());
            assert_eq!(Tab::from_title(tab.title()), Some(tab));
        }
        assert_eq!(seen.len(), 11);
    }

    /// A title from a newer build is dropped rather than fatal.
    #[test]
    fn an_unknown_title_is_ignored() {
        assert_eq!(Tab::from_title("Foliage"), None);
        assert_eq!(Tab::from_title(""), None);
    }

    /// Exactly two tabs are holes the renderer draws into. If a third ever
    /// became one, `ViewportPlacement` would need to know which is focused.
    #[test]
    fn exactly_two_tabs_are_viewports() {
        let holes: Vec<&str> = Tab::ALL
            .into_iter()
            .filter(|t| t.is_viewport())
            .map(Tab::title)
            .collect();
        assert_eq!(holes, ["Scene", "Game"]);
    }

    /// Every tab in the tree, in tree order.
    fn tabs_of(state: &DockState<Tab>) -> Vec<Tab> {
        let mut tabs: Vec<Tab> = state.iter_all_tabs().map(|(_, tab)| *tab).collect();
        tabs.sort_unstable();
        tabs
    }

    /// **Each of the eleven, exactly once.** A tab that is in the enum and not
    /// in the layout is unreachable — there is no Window menu yet — and a tab
    /// placed twice is two bodies fighting over one `Id`.
    #[test]
    fn the_default_layout_places_every_tab_once() {
        let mut expected = Tab::ALL.to_vec();
        expected.sort_unstable();
        assert_eq!(tabs_of(&default_layout(900.0)), expected);
    }

    /// **The fraction goes to the node the split names, and the crate's doc
    /// comment says otherwise.** It claims the fraction is what the *old* node
    /// keeps; for `Split::Left` and `Split::Above` the code assigns the old
    /// node to `parent.right()` and hands the fraction to the left/top child —
    /// the new one. Reading the doc and "fixing" `split_left` to
    /// `1.0 - LEFT_FRACTION` gives an 82% hierarchy, which is what this test is
    /// here to catch.
    #[test]
    fn the_fractions_go_to_the_nodes_they_name() {
        let state = default_layout(900.0);
        let surface = state.main_surface();

        // Root: the horizontal split between everything and the bottom node.
        // `split_below` puts the old node on the left, so the fraction is the
        // upper share and this direction is the one the doc gets right.
        let Node::Vertical(root) = &surface[NodeIndex::root()] else {
            panic!("the root should be a vertical split");
        };
        assert!((root.fraction - (1.0 - BOTTOM_HEIGHT / 900.0)).abs() < 1e-6);

        // Upper: hierarchy on the left, at LEFT_FRACTION — passed as-is.
        let upper = NodeIndex::root().left();
        let Node::Horizontal(split) = &surface[upper] else {
            panic!("the upper node should be a horizontal split");
        };
        assert!((split.fraction - LEFT_FRACTION).abs() < 1e-6);
        let left = surface[upper.left()].get_leaf().expect("hierarchy leaf");
        assert_eq!(left.tabs, vec![Tab::Hierarchy]);

        // Centre: inspector on the right, so the *centre* keeps the fraction
        // and the mirror is spelled `1.0 - RIGHT_FRACTION`.
        let centre = upper.right();
        let Node::Horizontal(split) = &surface[centre] else {
            panic!("the centre node should be a horizontal split");
        };
        assert!((split.fraction - (1.0 - RIGHT_FRACTION)).abs() < 1e-6);
        let right = surface[centre.right()].get_leaf().expect("inspector leaf");
        assert_eq!(right.tabs, vec![Tab::Inspector, Tab::Prefabs]);
    }

    /// A layout survives a round trip through JSON: a tab this build does not
    /// know is dropped, a tab it knows and the file lacks comes back, and the
    /// rest keep their places.
    #[test]
    fn a_saved_layout_loads_back_with_the_tabs_this_build_has() {
        let mut saved = super::for_saving(&default_layout(900.0));
        saved.main_surface_mut().push_to_first_leaf("Foliage".to_owned());
        let path = saved.find_tab(&"Prefabs".to_owned()).expect("prefabs is placed");
        saved.remove_tab(path);

        let json = serde_json::to_string(&saved).expect("titles encode");
        let loaded = from_json(&json, 900.0);

        let mut expected = Tab::ALL.to_vec();
        expected.sort_unstable();
        // Foliage gone, Prefabs back, everything else exactly once.
        assert_eq!(tabs_of(&loaded), expected);
        // And the *arrangement* survived rather than falling back to default:
        // hierarchy is still alone on the left.
        let upper = NodeIndex::root().left();
        let left = loaded.main_surface()[upper.left()].get_leaf().expect("hierarchy leaf");
        assert_eq!(left.tabs, vec![Tab::Hierarchy]);
    }

    /// **A layout saved before its first frame must still load.** Undrawn
    /// nodes hold `Rect::NOTHING`, `serde_json` writes `f32::INFINITY` as
    /// `null`, and `null` is not an `f32` on the way back. Zeroing is the
    /// whole fix; this fails the moment it is dropped.
    #[test]
    fn an_undrawn_layout_writes_no_nulls() {
        let undrawn = default_layout(900.0);
        // The hazard, demonstrated rather than asserted from the crate's
        // comment: an untouched layout writes infinities as `null` and does
        // not read back. (`focused_surface` is legitimately `null` and reads
        // back fine, which is why this checks the parse and not the text.)
        let raw = serde_json::to_string(&undrawn.map_tabs(|t| t.title().to_owned()))
            .expect("titles encode");
        assert!(
            serde_json::from_str::<DockState<String>>(&raw).is_err(),
            "an unzeroed layout was expected to be unreadable"
        );

        let json = serde_json::to_string(&super::for_saving(&undrawn)).expect("titles encode");
        serde_json::from_str::<DockState<String>>(&json).expect("zeroed rects read back");
    }

    /// Garbage is a missing layout, not a crash — and neither is a file whose
    /// every tab this build rejects, which filters down to a state with no
    /// tabs at all and would panic every repair in `from_json`.
    #[test]
    fn an_unreadable_layout_falls_back_to_the_default() {
        let mut expected = Tab::ALL.to_vec();
        expected.sort_unstable();
        assert_eq!(tabs_of(&from_json("{", 900.0)), expected);

        let mut all_unknown = DockState::new(vec!["Foliage".to_owned(), "Terrain".to_owned()]);
        super::zero_rects(&mut all_unknown);
        let json = serde_json::to_string(&all_unknown).expect("encodes");
        assert_eq!(tabs_of(&from_json(&json, 900.0)), expected);
    }

    /// Draw a dock over egui's test context and report what the root `Ui` has
    /// left afterwards — which is the rectangle the renderer draws into, the
    /// one `is_pointer_over_egui` compares a click against, and the one the HUD
    /// anchors to. All three are the same rectangle, so testing one tests all.
    fn leftover_after_drawing(state: DockState<Tab>) -> (egui::Rect, egui::Rect) {
        let scene = loom_scene::Scene::parse("[scene]\nformat = 1\n\n[[node]]\nname = \"Root\"\n")
            .expect("a one-node scene");
        let registry = loom_reflect::TypeRegistry::new();
        let panels = PanelState {
            scene: &scene,
            paths: &[],
            picks: &std::collections::BTreeMap::new(),
            assets: &[],
            object_count: 0,
            selected: &[],
            history: &[],
            can_undo: false,
            can_redo: false,
            dirty: false,
            conflict: false,
            editable: true,
            registry: &registry,
            mode: crate::gizmo::Mode::Move,
            handles: &[],
            dragging: None,
            fps: 60.0,
            agent_marks: &[],
            overrides: &std::collections::BTreeMap::new(),
            console: &[],
            tick_seconds: 1.0 / 60.0,
            playing: None,
        };
        let mut dock = Dock { state, was_playing: false, persist: false };
        let mut result = (egui::Rect::ZERO, egui::Rect::ZERO);
        egui::__run_test_ui(|root| {
            let before = root.available_rect_before_wrap();
            dock.draw(root, &panels);
            result = (before, root.available_rect_before_wrap());
        });
        result
    }

    /// **The carve, end to end.** Nothing else in this file proves the two
    /// halves agree: the dock draws into a child `Ui` (allocating nothing) and
    /// four empty panels then take back everything but the viewport tab.
    ///
    /// Fails both ways it can be got wrong. Hand `DockArea` the root `Ui` and
    /// the leftover is empty — no click ever reaches the scene again. Drop the
    /// carve and the leftover is the whole window — the scene is drawn over the
    /// hierarchy, which is how the HUD ended up there before.
    #[test]
    fn the_carve_leaves_the_viewport_and_nothing_else() {
        let (area, hole) = leftover_after_drawing(default_layout(900.0));
        assert!(hole.is_positive(), "the viewport was carved away entirely");
        assert!(area.contains_rect(hole), "the hole escaped the root's area");

        // **The four insets are the fractions, within a couple of points of
        // separator.** Fractions rather than absolutes because the test
        // context's viewport is not the window's. `close` is 2% of the axis,
        // which is far tighter than the ways this goes wrong: a reversed split
        // moves an inset by 60% of the axis, and handing `DockArea` the root
        // `Ui` leaves a 27-point strip where a 6849-point viewport should be.
        let close = |got: f32, want: f32, name: &str| {
            assert!(
                (got - want).abs() < 0.02,
                "the {name} inset was {got:.3} of the axis, not {want:.3}"
            );
        };
        close((hole.min.x - area.min.x) / area.width(), LEFT_FRACTION, "left");
        let centre = area.width() - (hole.min.x - area.min.x);
        close((area.max.x - hole.max.x) / centre, RIGHT_FRACTION, "right");
        close(
            (area.max.y - hole.max.y) / area.height(),
            BOTTOM_HEIGHT / 900.0,
            "bottom",
        );
        // The toolbar is the only thing above it, so the hole keeps the rest.
        assert!(
            hole.height() > 0.5 * area.height(),
            "the viewport was {} tall of {} — the dock took the root's rect",
            hole.height(),
            area.height()
        );
    }

    /// **No sticky rectangle.** `TabViewer::ui` runs only for a leaf's *active*
    /// tab, so a `Scene` dragged behind `Console` reports nothing this frame.
    /// Reusing the last rectangle would carve a click-through hole under the
    /// console and render the scene beneath it; consuming everything is the
    /// answer, and `ViewportPlacement` clamps the degenerate result.
    #[test]
    fn a_hidden_viewport_leaves_nothing_at_all() {
        // Console is tab 0 and therefore active; Scene never draws.
        let buried = DockState::new(vec![Tab::Console, Tab::Scene]);
        let (_, hole) = leftover_after_drawing(buried);
        assert!(!hole.is_positive(), "a hidden viewport still carved {hole:?}");
    }
}
