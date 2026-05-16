//! Route constants + the sidebar index. One entry per page; the
//! sidebar renders these in order, and the Navigator wires each
//! route to its page component.

use framework_core::Route;

pub const OVERVIEW_ROUTE: Route<()> = Route::<()>::new("overview", "/");
pub const THEMES_ROUTE: Route<()> = Route::<()>::new("themes", "/themes");
pub const LAYOUT_ROUTE: Route<()> = Route::<()>::new("layout", "/layout");
pub const TYPOGRAPHY_ROUTE: Route<()> = Route::<()>::new("typography", "/typography");
pub const ACTIONS_ROUTE: Route<()> = Route::<()>::new("actions", "/actions");
pub const INPUTS_ROUTE: Route<()> = Route::<()>::new("inputs", "/inputs");
pub const FEEDBACK_ROUTE: Route<()> = Route::<()>::new("feedback", "/feedback");
pub const OVERLAYS_ROUTE: Route<()> = Route::<()>::new("overlays", "/overlays");
pub const STATEFUL_ROUTE: Route<()> = Route::<()>::new("stateful", "/stateful");

pub struct IndexEntry {
    pub name: &'static str,
    pub label: &'static str,
}

/// The sidebar uses these — order matters; matched against
/// `LayoutProps::active_route` to highlight the current page.
pub const INDEX: &[IndexEntry] = &[
    IndexEntry { name: "overview", label: "Overview" },
    IndexEntry { name: "themes", label: "Themes & Intents" },
    IndexEntry { name: "layout", label: "Layout" },
    IndexEntry { name: "typography", label: "Typography" },
    IndexEntry { name: "actions", label: "Actions" },
    IndexEntry { name: "inputs", label: "Inputs" },
    IndexEntry { name: "feedback", label: "Feedback" },
    IndexEntry { name: "overlays", label: "Overlays" },
    IndexEntry { name: "stateful", label: "Stateful" },
];
