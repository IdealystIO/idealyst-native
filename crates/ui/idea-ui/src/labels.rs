//! The words idea-ui's components draw themselves: the accessible names
//! of their glyph buttons, their empty and loading rows, a picker's
//! fallback text. A component never bakes one in; it reads
//! [`ui_labels`], which is English until the app installs its own.
//!
//! ```ignore
//! let mut fr = idea_ui::UiLabels::english();
//! fr.clear = "Effacer".to_string();
//! fr.no_results = "Aucun résultat".to_string();
//! idea_ui::install_ui_labels(fr);
//! ```
//!
//! A prop that names the same thing on one instance (`Pagination`'s
//! `previous_label`, `Autocomplete`'s `empty_text`, a date input's
//! `labels`) still wins over the installed set.
//!
//! Read at build: install before mounting. To change language while the
//! app runs, install the new set and rebuild what shows it.

use std::cell::RefCell;
use std::rc::Rc;

use crate::date::DateLabels;

/// Every word idea-ui's components draw that the app did not pass.
/// Start from [`UiLabels::english`] and replace what you translate.
#[derive(Clone)]
pub struct UiLabels {
    /// Month and weekday names and the calendar's chevrons: the default
    /// of every date component's `labels`, and what [`format_date`]
    /// and [`parse_date`] read.
    ///
    /// [`format_date`]: crate::date::format_date
    /// [`parse_date`]: crate::date::parse_date
    pub date: Rc<DateLabels>,
    /// A ✕ that empties a field (`Autocomplete`), and a date picker's
    /// Clear.
    pub clear: String,
    /// An autocomplete's chevron, which opens and closes its options.
    pub show_options: String,
    /// The menu row of an autocomplete whose query matches nothing.
    pub no_results: String,
    /// The menu row of an autocomplete that is loading.
    pub loading: String,
    /// A date picker's trigger with no value and no `placeholder`.
    pub select_date: String,
    /// A date-and-time picker's trigger with no value and no
    /// `placeholder`.
    pub select_date_time: String,
    /// A range picker's trigger with no value and no `placeholder`.
    pub select_range: String,
    /// A date input's message for text that is not a date.
    pub invalid_date: String,
    /// A date-and-time input's message for text that is not one.
    pub invalid_date_time: String,
    /// A time input's message for text that is not a time.
    pub invalid_time: String,
    /// The ‹ stepper of `Pagination` and a table's pager.
    pub previous_page: String,
    /// The › stepper of `Pagination` and a table's pager.
    pub next_page: String,
    /// A table's select-all box.
    pub select_all_rows: String,
    /// A table row's box, where the table has no `select_row_label`.
    pub select_row: String,
    /// The card list's select-all line, from how many rows are picked:
    /// "Select all" at none, "3 selected" after.
    pub selection: Rc<dyn Fn(usize) -> String>,
    /// A collapsed group's toggle, from the group's title: "Expand
    /// Drilling".
    pub expand_group: Rc<dyn Fn(&str) -> String>,
    /// An expanded group's toggle: "Collapse Drilling".
    pub collapse_group: Rc<dyn Fn(&str) -> String>,
    /// The caption before a table pager's page-size picker.
    pub rows_per_page: String,
    /// A table pager's count, from the visible window, the total and
    /// the table's noun. See [`PagerCount`].
    pub pager_count: Rc<dyn Fn(PagerCount<'_>) -> (String, String)>,
}

/// What a table pager's count says, as its bold lead and muted tail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagerCount<'a> {
    /// No rows: `("No projects", "")`.
    None { noun: &'a str },
    /// One page holds them all: `("6 projects", "")`.
    All { total: usize, noun: &'a str },
    /// A page of several: `("26–50", "of 312 projects")`, where
    /// `window` is the "26–50" already written.
    Window { window: &'a str, total: usize, noun: &'a str },
}

// The English words some components also publish as constants, for a
// robot or test that finds a control by its default name.
pub(crate) const EN_CLEAR: &str = "Clear";
pub(crate) const EN_SHOW_OPTIONS: &str = "Show options";
pub(crate) const EN_PREVIOUS_PAGE: &str = "Previous page";
pub(crate) const EN_NEXT_PAGE: &str = "Next page";
pub(crate) const EN_SELECT_ALL_ROWS: &str = "Select all rows";
pub(crate) const EN_SELECT_ROW: &str = "Select row";

impl UiLabels {
    pub fn english() -> Self {
        Self {
            date: DateLabels::english(),
            clear: EN_CLEAR.to_string(),
            show_options: EN_SHOW_OPTIONS.to_string(),
            no_results: "No results".to_string(),
            loading: "Loading…".to_string(),
            select_date: "Select date".to_string(),
            select_date_time: "Select date & time".to_string(),
            select_range: "Select range".to_string(),
            invalid_date: "Invalid date".to_string(),
            invalid_date_time: "Invalid date/time".to_string(),
            invalid_time: "Invalid time".to_string(),
            previous_page: EN_PREVIOUS_PAGE.to_string(),
            next_page: EN_NEXT_PAGE.to_string(),
            select_all_rows: EN_SELECT_ALL_ROWS.to_string(),
            select_row: EN_SELECT_ROW.to_string(),
            selection: Rc::new(|count| match count {
                0 => "Select all".to_string(),
                n => format!("{n} selected"),
            }),
            expand_group: Rc::new(|title| format!("Expand {title}")),
            collapse_group: Rc::new(|title| format!("Collapse {title}")),
            rows_per_page: "Rows".to_string(),
            pager_count: Rc::new(|count| match count {
                PagerCount::None { noun } => (format!("No {noun}"), String::new()),
                PagerCount::All { total, noun } => (format!("{total} {noun}"), String::new()),
                PagerCount::Window { window, total, noun } => (window.to_string(), format!("of {total} {noun}")),
            }),
        }
    }
}

impl Default for UiLabels {
    fn default() -> Self {
        Self::english()
    }
}

thread_local! {
    // Thread-wide like the installed theme: the app installs one set at
    // boot, before any world mounts.
    static INSTALLED: RefCell<Option<Rc<UiLabels>>> = const { RefCell::new(None) };
}

/// Make `labels` the words every idea-ui component draws from now on.
/// Call it at boot, before mounting.
pub fn install_ui_labels(labels: UiLabels) {
    INSTALLED.with(|c| *c.borrow_mut() = Some(Rc::new(labels)));
}

/// The installed labels, or [`UiLabels::english`] when the app installed
/// none.
pub fn ui_labels() -> Rc<UiLabels> {
    INSTALLED.with(|c| c.borrow_mut().get_or_insert_with(|| Rc::new(UiLabels::english())).clone())
}
