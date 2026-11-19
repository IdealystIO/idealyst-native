//! [`ListStates`] — the four things a list shows when it has no rows to
//! show (rules `blank-state`, `empty-state`, `loading-state`).

use std::rc::Rc;

use icons_lucide::{INBOX, SEARCH_X, TRIANGLE_ALERT};
use idea_ui::{variant, Button, Spinner};
use runtime_core::{
    component, memo, stylesheet, ui, AlignItems, Element, FlexDirection, IconData, IdealystSchema, JustifyContent,
    Length, Reactive,
};

use crate::blocks::StateMessage;
use crate::{Action, OptionalSlot, Slot};

/// Where a list's load stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ListPhase {
    /// The first load has not come back.
    #[default]
    Loading,
    /// The load failed, with the reason in the reader's words.
    Failed(String),
    /// The rows are in.
    Ready,
}

/// What a list says when no records exist at all: a glyph over a
/// message, with the one action that adds the first.
///
/// ```ignore
/// Blank::new("No materials yet").body("Add what your bids are priced with.").action(Action::new("Add a material", on_add))
/// ```
#[derive(Clone)]
pub struct Blank {
    title: String,
    body: Option<String>,
    icon: Option<IconData>,
    action: Option<Action>,
}

impl Blank {
    /// The message: what is not here yet.
    pub fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), body: None, icon: None, action: None }
    }

    /// The muted line under the title: what goes here.
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// The glyph over the message, where the list has one of its own.
    /// Without it: the empty tray.
    pub fn icon(mut self, icon: IconData) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The one action: the screen's own verb that creates the first
    /// record. Leave it off for a reader who may not add.
    pub fn action(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }
}

impl Default for Blank {
    fn default() -> Self {
        Self::new("Nothing here yet")
    }
}

/// Which of the five things the list's room holds.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Shown {
    Loading,
    Failed(String),
    Blank,
    NoMatch,
    Rows,
}

fn shown(phase: &ListPhase, is_empty: bool, narrowed: bool) -> Shown {
    match phase {
        ListPhase::Loading => Shown::Loading,
        ListPhase::Failed(reason) => Shown::Failed(reason.clone()),
        ListPhase::Ready if !is_empty => Shown::Rows,
        ListPhase::Ready if narrowed => Shown::NoMatch,
        ListPhase::Ready => Shown::Blank,
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct ListStatesProps {
    /// Where the load stands.
    pub phase: Reactive<ListPhase>,
    /// The loaded list has no rows.
    pub is_empty: Reactive<bool>,
    /// A search or a filter is on, so an empty list is the narrowing's
    /// doing (the EMPTY state) rather than the absence of any record
    /// (the BLANK state).
    pub narrowed: Reactive<bool>,
    /// The blank state's words, glyph and action.
    #[prop(static)]
    pub blank: Blank,
    /// The empty state's line.
    #[prop(static)]
    pub no_match: String,
    /// The failed state's title, in the screen's words ("Couldn't load
    /// the materials"). The reason from [`ListPhase::Failed`] goes under
    /// it.
    #[prop(static)]
    pub failed_title: String,
    /// The failed state's Retry.
    #[prop(static)]
    pub on_retry: Option<Rc<dyn Fn()>>,
    /// Builds the loading state: placeholder rows in the table's own
    /// shape (the same columns, row height and phone form as the rows
    /// that will replace them). `None` draws a spinner.
    #[prop(static)]
    pub loading: OptionalSlot,
    /// Builds the rows. Called when the list goes from having none to
    /// having some, never per row: one row changing must not rebuild the
    /// list.
    #[prop(static)]
    pub rows: Slot,
}

impl Default for ListStatesProps {
    fn default() -> Self {
        Self {
            phase: Reactive::Static(ListPhase::Loading),
            is_empty: Reactive::Static(true),
            narrowed: Reactive::Static(false),
            blank: Blank::default(),
            no_match: "No rows match.".to_string(),
            failed_title: "Couldn't load this list".to_string(),
            on_retry: None,
            loading: OptionalSlot::default(),
            rows: Slot::default(),
        }
    }
}

/// The room a list's rows stand in, holding the rows or the one state
/// that explains their absence. The same four states on every list, at
/// every width; a screen supplies the words and never draws a state.
///
/// - **Loading**: the `loading` placeholder.
/// - **Failed**: the title, the reason, and Retry.
/// - **Blank**: no records exist. The glyph, the message, and the one
///   action that adds the first.
/// - **Empty**: the filters removed every record. The search-✕ glyph
///   and the line, and no action: the toolbar above is the way back.
///
/// The toolbar is not this component's and stays in every state: a
/// screen never hides its search or filters because the list is blank.
///
/// ```ignore
/// ListStates(
///     phase = rx!(load.get()),
///     is_empty = rx!(rows.get().is_empty()),
///     narrowed = rx!(!search.get().is_empty()),
///     blank = Blank::new("No materials yet").action(Action::new("Add a material", on_add)),
///     failed_title = "Couldn't load the materials".to_string(),
///     on_retry = Some(reload),
///     rows = move || ui! { Table(…) },
/// )
/// ```
#[component]
pub fn ListStates(props: &ListStatesProps) -> Element {
    let phase = props.phase.clone();
    let is_empty = props.is_empty.clone();
    let narrowed = props.narrowed.clone();
    // A memo, so the branch below re-runs only when the STATE changes,
    // not on every write to the rows behind `is_empty`.
    let state = memo(move || shown(&phase.get(), is_empty.get(), narrowed.get()));
    let blank = props.blank.clone();
    let no_match = props.no_match.clone();
    let failed_title = props.failed_title.clone();
    let on_retry = props.on_retry.clone();
    let loading = props.loading.clone();
    let rows = props.rows.clone();
    ui! {
        match state.get() {
            Shown::Rows => { rows.build() }
            Shown::Loading => {
                {
                    match loading.build() {
                        Some(placeholder) => placeholder,
                        None => ui! { view(style = ListLoadingSeat(), test_id = "list-loading") { Spinner() } },
                    }
                }
            }
            Shown::Failed(reason) => {
                {
                    let retry = on_retry.clone().map(|run| ui! {
                        Button(label = "Retry", variant = variant::Outlined, on_click = run, test_id = Some("list-retry"))
                    });
                    ui! {
                        StateMessage(
                            icon = Some(TRIANGLE_ALERT),
                            title = failed_title.clone(),
                            body = Some(reason.clone()),
                            action = retry,
                            test_id = "list-failed",
                        )
                    }
                }
            }
            Shown::Blank => {
                {
                    let action = blank.action.clone().map(|a| ui! {
                        Button(label = a.label.clone(), on_click = a.on_press.clone(), leading_icon = a.icon, test_id = a.test_id)
                    });
                    ui! {
                        StateMessage(
                            icon = Some(blank.icon.unwrap_or(INBOX)),
                            title = blank.title.clone(),
                            body = blank.body.clone(),
                            action = action,
                            test_id = "list-blank",
                        )
                    }
                }
            }
            Shown::NoMatch => {
                StateMessage(icon = Some(SEARCH_X), title = no_match.clone(), test_id = "list-no-match")
            }
        }
    }
}

/// The spinner's least room, so the list's seat does not collapse while
/// its first page loads.
const LOADING_MIN_HEIGHT: f32 = 240.0;

stylesheet! {
    ListLoadingSeat<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            flex_grow: 1.0,
            min_height: Length::Px(LOADING_MIN_HEIGHT),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_rows_and_no_narrowing_is_blank() {
        assert_eq!(shown(&ListPhase::Ready, true, false), Shown::Blank);
    }

    #[test]
    fn no_rows_under_a_filter_is_empty_not_blank() {
        assert_eq!(shown(&ListPhase::Ready, true, true), Shown::NoMatch);
    }

    #[test]
    fn rows_show_whatever_the_narrowing() {
        assert_eq!(shown(&ListPhase::Ready, false, false), Shown::Rows);
        assert_eq!(shown(&ListPhase::Ready, false, true), Shown::Rows);
    }

    #[test]
    fn the_load_decides_before_the_rows_do() {
        assert_eq!(shown(&ListPhase::Loading, true, false), Shown::Loading);
        assert_eq!(shown(&ListPhase::Loading, false, true), Shown::Loading);
        assert_eq!(
            shown(&ListPhase::Failed("offline".into()), true, false),
            Shown::Failed("offline".into()),
        );
    }
}
