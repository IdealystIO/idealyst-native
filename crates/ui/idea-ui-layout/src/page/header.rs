//! [`PageHeader`] — a screen's title row: where it is, and the verbs
//! that act on it.

use idea_ui::{tone, typography_kind, variant, Button, IconButton, IconButtonSize, Spacer, Typography};
use runtime_core::{
    component, stylesheet, ui, AlignItems, Element, FlexDirection, IdealystSchema, Length, Reactive,
};

use crate::overlay::ActionMenu;
use crate::width::is_phone;
use crate::{Action, OptionalSlot};

/// The most verbs a header holds (rule `header-actions`): a fourth
/// means the screen has not decided what its primary act is.
pub const HEADER_MAX_ACTIONS: usize = 3;

/// The header row's least height, so a header with no action is as tall
/// as one with a button and the body does not shift between screens.
const HEADER_MIN_HEIGHT: f32 = 48.0;

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct PageHeaderProps {
    /// The page's name. It gives ground: anything sharing its row lets
    /// it truncate.
    pub title: String,
    /// Before the title: the app's way out or way around (a back arrow,
    /// the drawer's ☰, a breadcrumb trail's ancestors).
    pub leading: Option<Element>,
    /// The verbs that act on the page, at most [`HEADER_MAX_ACTIONS`],
    /// at most one of them [`primary`](Action::primary).
    #[prop(static)]
    pub actions: Vec<Action>,
    /// What applies to the whole record (Archive, Delete, "Save as
    /// template"): a ⋮ menu right after the title.
    #[prop(static)]
    pub menu: Vec<Action>,
    /// The record's status (a chip, a progress bar), after the menu. A
    /// phone does not draw it.
    #[prop(static)]
    pub status: OptionalSlot,
}

impl Default for PageHeaderProps {
    fn default() -> Self {
        Self {
            title: Reactive::Static(String::new()),
            leading: None,
            actions: Vec::new(),
            menu: Vec::new(),
            status: OptionalSlot::default(),
        }
    }
}

/// A screen's title row (rules `header-actions`, `record-menu`).
///
/// - Tablet, desktop: the title, the record's ⋮ menu and status after
///   it, and the verbs as labelled buttons at the far end, the primary
///   last.
/// - Phone: the title, then the primary as its glyph alone, then ONE ⋮
///   holding every other verb followed by the record's menu. The
///   primary never folds into the menu, and a label is never shortened:
///   a verb collapses to its glyph or moves into the menu whole.
///
/// The menu is an [`ActionMenu`], so on a phone it opens as an action
/// sheet.
///
/// ```ignore
/// PageHeader(
///     title = rx!(project.get().name),
///     leading = Some(back_button),
///     actions = vec![
///         Action::new("Add heading", on_add).icon(PLUS).primary(),
///         Action::new("Export", on_export).icon(DOWNLOAD),
///     ],
///     menu = vec![Action::new("Archive", on_archive).icon(ARCHIVE)],
/// )
/// ```
#[component]
pub fn PageHeader(props: PageHeaderProps) -> Element {
    debug_assert!(
        props.actions.len() <= HEADER_MAX_ACTIONS,
        "rule `header-actions`: at most {HEADER_MAX_ACTIONS} actions",
    );
    debug_assert!(
        props.actions.iter().filter(|a| a.primary).count() <= 1,
        "rule `header-actions`: one filled primary at most",
    );
    let title = props.title;
    let leading = props.leading;
    let status = props.status;
    let menu = props.menu;
    let primary: Option<Action> = props.actions.iter().find(|a| a.primary).cloned();
    let others: Vec<Action> = props.actions.iter().filter(|a| !a.primary).cloned().collect();
    // The phone's one menu: the verbs that lost their button, then what
    // the record's menu holds.
    let folded: Vec<Action> = others.iter().chain(menu.iter()).cloned().collect();
    ui! {
        view(style = HeaderRow()) {
            if let Some(leading) = leading {
                leading
            }
            view(style = HeaderTitle()) {
                Typography(content = title, kind = typography_kind::H3, lines = Some(1_u32))
            }
            // The two forms hold different elements, so they are a
            // `match`; it lives here so no screen writes one.
            match is_phone() {
                true => {
                    Spacer()
                    if let Some(a) = primary.clone() {
                        HeaderPrimary(action = a, compact = true)
                    }
                    ActionMenu(actions = folded.clone(), size = IconButtonSize::Md)
                }
                false => {
                    ActionMenu(actions = menu.clone(), size = IconButtonSize::Md)
                    if let Some(build) = status.0.clone() {
                        { build() }
                    }
                    Spacer()
                    for a in others.iter(), key = a.label.clone() {
                        Button(
                            label = a.label.clone(),
                            on_click = a.on_press.clone(),
                            tone = a.tone.clone().unwrap_or_else(|| tone::Neutral.into()),
                            variant = variant::Outlined,
                            loading = a.busy_now(),
                            disabled = a.disabled_now(),
                            leading_icon = a.icon,
                            test_id = a.test_id,
                        )
                    }
                    if let Some(a) = primary.clone() {
                        HeaderPrimary(action = a, compact = false)
                    }
                }
            }
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema, Default)]
struct HeaderPrimaryProps {
    /// The screen's primary verb.
    #[prop(static)]
    pub action: Action,
    /// The phone form: the glyph alone, when the verb has one.
    #[prop(static)]
    pub compact: bool,
}

/// The header's filled verb: a labelled button, or its glyph alone on a
/// phone. A verb with no glyph keeps its label there, whole.
///
/// ```ignore
/// HeaderPrimary(action = add, compact = true)
/// ```
#[component]
fn HeaderPrimary(props: &HeaderPrimaryProps) -> Element {
    let a = props.action.clone();
    let tone = a.tone.clone().unwrap_or_else(|| tone::Primary.into());
    match (props.compact, a.icon) {
        (true, Some(glyph)) => ui! {
            IconButton(
                icon = Some(glyph),
                on_click = a.on_press.clone(),
                tone = tone,
                variant = variant::Filled,
                size = IconButtonSize::Md,
                disabled = a.disabled_now(),
                label = Some(a.label.clone()),
                test_id = a.test_id,
            )
        },
        _ => ui! {
            Button(
                label = a.label.clone(),
                on_click = a.on_press.clone(),
                tone = tone,
                loading = a.busy_now(),
                disabled = a.disabled_now(),
                leading_icon = a.icon,
                test_id = a.test_id,
            )
        },
    }
}

stylesheet! {
    // In the page gutter, so the title's left edge is the body's.
    HeaderRow<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            min_height: Length::Px(HEADER_MIN_HEIGHT),
            padding_left: t.spacing.lg(),
            padding_right: t.spacing.lg(),
            padding_top: t.spacing.sm(),
            padding_bottom: t.spacing.sm(),
        }
        breakpoint sm(t) {
            padding_left: t.spacing.xl(),
            padding_right: t.spacing.xl(),
        }
        breakpoint lg(t) {
            padding_left: t.spacing.xxl(),
            padding_right: t.spacing.xxl(),
        }
    }
}

stylesheet! {
    // Shrinks before anything else in the row does, and never grows:
    // the record's menu follows the title, not the row's far end.
    HeaderTitle<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_shrink: 1.0,
            min_width: Length::Px(0.0),
        }
    }
}
