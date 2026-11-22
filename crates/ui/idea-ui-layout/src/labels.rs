//! The words this crate's components draw themselves: the accessible
//! names of their glyph buttons (a dialog's ✕, a header's back arrow, a
//! toolbar's magnifier) and the defaults a screen can override per call
//! (a search's placeholder, a list's empty line). A component never bakes
//! one in; it reads [`layout_labels`], which is English until the app
//! installs its own.
//!
//! ```ignore
//! let mut fr = idea_ui_layout::LayoutLabels::english();
//! fr.close = "Fermer".to_string();
//! fr.search = "Rechercher".to_string();
//! idea_ui_layout::install_layout_labels(fr);
//! ```
//!
//! idea-ui's own components read [`idea_ui::UiLabels`]; an app in
//! another language installs both.
//!
//! Read at build: install before mounting. To change language while the
//! app runs, install the new set and rebuild what shows it.

use std::cell::RefCell;
use std::rc::Rc;

use crate::FlowLabels;

/// Every word this crate's components draw that the app did not pass.
/// Start from [`LayoutLabels::english`] and replace what you translate.
#[derive(Clone)]
pub struct LayoutLabels {
    /// A ✕ that closes: a [`Dialog`](crate::Dialog)'s, a split view's
    /// side pane's.
    pub close: String,
    /// A back arrow: a dialog of stages, a drill sheet's pane, a page
    /// header.
    pub back: String,
    /// A [`ToolbarSearch`](crate::ToolbarSearch)'s magnifier, and the
    /// default of its `placeholder`.
    pub search: String,
    /// The ✕ in a search field that empties it.
    pub clear_search: String,
    /// The ☰ that opens the sidebar where it is a drawer: the default of
    /// `SidebarToggle`'s `label`.
    pub menu: String,
    /// The desktop sidebar control while the sidebar is open.
    pub collapse_sidebar: String,
    /// The desktop sidebar control while it is collapsed.
    pub expand_sidebar: String,
    /// The ⋮ of an [`ActionMenu`](crate::ActionMenu): the default of
    /// its `label`.
    pub more_actions: String,
    /// A page header's ⋯ that holds the breadcrumbs with no room.
    pub show_pages_above: String,
    /// The default of a [`Blank`](crate::Blank)'s title.
    pub nothing_here_yet: String,
    /// The default of [`ListStates`](crate::ListStates)' `no_match`.
    pub no_rows_match: String,
    /// The default of [`ListStates`](crate::ListStates)' `failed_title`.
    pub failed_to_load: String,
    /// The default of a [`Flow`](crate::Flow)'s `labels`.
    pub flow: FlowLabels,
}

impl LayoutLabels {
    pub fn english() -> Self {
        Self {
            close: "Close".to_string(),
            back: "Back".to_string(),
            search: "Search".to_string(),
            clear_search: "Clear search".to_string(),
            menu: "Menu".to_string(),
            collapse_sidebar: "Collapse sidebar".to_string(),
            expand_sidebar: "Expand sidebar".to_string(),
            more_actions: "More actions".to_string(),
            show_pages_above: "Show the pages above this one".to_string(),
            nothing_here_yet: "Nothing here yet".to_string(),
            no_rows_match: "No rows match.".to_string(),
            failed_to_load: "Couldn't load this list".to_string(),
            flow: FlowLabels::english(),
        }
    }
}

impl Default for LayoutLabels {
    fn default() -> Self {
        Self::english()
    }
}

thread_local! {
    // Thread-wide like the installed theme: the app installs one set at
    // boot, before any world mounts.
    static INSTALLED: RefCell<Option<Rc<LayoutLabels>>> = const { RefCell::new(None) };
}

/// Make `labels` the words every component of this crate draws from now
/// on. Call it at boot, before mounting.
pub fn install_layout_labels(labels: LayoutLabels) {
    INSTALLED.with(|c| *c.borrow_mut() = Some(Rc::new(labels)));
}

/// The installed labels, or [`LayoutLabels::english`] when the app
/// installed none.
pub fn layout_labels() -> Rc<LayoutLabels> {
    INSTALLED.with(|c| c.borrow_mut().get_or_insert_with(|| Rc::new(LayoutLabels::english())).clone())
}
