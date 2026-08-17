//! The docked frame: which panels exist, where they start, and where the
//! scene ends up.
//!
//! **The scene is a *hole* in this layout, not a tab that draws anything.**
//! `Tab::Scene`'s body is empty on purpose: the renderer draws into the
//! rectangle the dock leaves, as a sub-rectangle of the swapchain (ADR 0025).
//! So the tab's whole job is to occupy space and report where it is.

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

#[cfg(test)]
mod tests {
    use super::Tab;

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
}
