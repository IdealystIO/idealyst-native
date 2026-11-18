//! The rows a [`Sheet`](super::Sheet) holds. Four kinds: a row that
//! DOES something and closes, a row that TOGGLES something and stays,
//! a row that PICKS one of a set, and a row that PUSHES a pane.

use std::rc::Rc;

use icons_lucide::{CHECK, CHEVRON_RIGHT};
use idea_ui::{menu_checkbox, typography_kind, Icon, MenuLabel, Stack, StackAlign, StackAxis, StackGap, Typography};
use runtime_core::{
    component, icon, pressable, stylesheet, ui, AlignItems, Element, FlexDirection, IdealystSchema, IntoElement,
    JustifyContent, Length, Reactive, StyleApplication,
};

/// A sheet row's least height: a touch target, not a menu row.
pub const SHEET_ROW_MIN_HEIGHT: f32 = 48.0;

/// The push row's chevron, in px.
const PUSH_CHEVRON_PX: f32 = 16.0;

/// The pick row's check, and the slot kept for it, in px.
const PICK_CHECK_PX: f32 = 18.0;

stylesheet! {
    // Edge to edge, so the whole width is the target rather than the
    // label. `SpaceBetween` serves the push row's trailing half; the
    // other rows have one child and are unaffected.
    SheetRow<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            min_height: SHEET_ROW_MIN_HEIGHT,
            padding_horizontal: t.spacing.sm(),
            border_radius: t.radius.md(),
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SheetActionRowProps {
    /// What the row does.
    #[prop(static)]
    pub label: String,
    /// An icon before the label. `None` mounts nothing.
    #[prop(static)]
    pub leading: Option<Element>,
    /// Runs on press. Closing the sheet is this handler's to do.
    #[prop(static)]
    pub on_press: Rc<dyn Fn()>,
}

impl Default for SheetActionRowProps {
    fn default() -> Self {
        Self { label: String::new(), leading: None, on_press: Rc::new(|| {}) }
    }
}

/// A row that DOES something. Closing the sheet is the caller's half of
/// the handler, because whether an action closes (a delete does) or
/// keeps the sheet up is the action's own contract.
///
/// ```ignore
/// SheetActionRow(
///     label = "Delete".to_string(),
///     leading = Some(ui! { Icon(data = TRASH, size = 18.0) }),
///     on_press = run.clone(),
/// )
/// ```
#[component]
pub fn SheetActionRow(props: SheetActionRowProps) -> Element {
    let label = props.label;
    let leading = props.leading;
    let on_press = props.on_press;
    let body = ui! {
        Stack(axis = StackAxis::Row, align = StackAlign::Center, gap = StackGap::Md) {
            if let Some(leading) = leading {
                leading
            }
            Typography(content = label, kind = typography_kind::Body)
        }
    };
    // `pressable` takes its children by value: `ui!` owns no pressable
    // tag, so the vec is the primitive's signature.
    pressable(vec![body], move || on_press())
        .with_style(StyleApplication::new(SheetRow::sheet()))
        .into_element()
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SheetCheckRowProps {
    /// What the row toggles.
    #[prop(static)]
    pub label: String,
    /// An icon between the tick and the label. `None` mounts nothing.
    #[prop(static)]
    pub leading: Option<Element>,
    /// Whether the tick shows. Derived from where the truth lives
    /// (`rx!(selected.get().contains(&value))`), not a per-row signal.
    pub checked: bool,
    /// Runs on press. The sheet stays up.
    #[prop(static)]
    pub on_press: Rc<dyn Fn()>,
}

impl Default for SheetCheckRowProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            leading: None,
            checked: Reactive::Static(false),
            on_press: Rc::new(|| {}),
        }
    }
}

/// A row that TOGGLES something and stays up for the next one.
///
/// Not a `Checkbox`: that control owns a `Signal<bool>` it writes back
/// to, and a per-row mirror is a second source of truth a "Reset" cannot
/// reach. The tick here re-reads `checked`.
///
/// ```ignore
/// SheetCheckRow(
///     label = "Fair".to_string(),
///     checked = rx!(selected.get().contains("fair")),
///     on_press = toggle.clone(),
/// )
/// ```
#[component]
pub fn SheetCheckRow(props: SheetCheckRowProps) -> Element {
    let label = props.label;
    let leading = props.leading;
    let checked = props.checked;
    let on_press = props.on_press;
    let body = ui! {
        Stack(axis = StackAxis::Row, align = StackAlign::Center, gap = StackGap::Sm) {
            menu_checkbox(checked)
            if let Some(leading) = leading {
                leading
            }
            Typography(content = label, kind = typography_kind::Body)
        }
    };
    pressable(vec![body], move || on_press())
        .with_style(StyleApplication::new(SheetRow::sheet()))
        .into_element()
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SheetPickRowProps {
    /// The choice.
    #[prop(static)]
    pub label: String,
    /// An icon before the label. `None` mounts nothing.
    #[prop(static)]
    pub leading: Option<Element>,
    /// Whether this is the one chosen. Derived from where the choice
    /// lives (`rx!(theme.get() == Theme::Dark)`), not a per-row signal.
    pub selected: bool,
    /// Choose it. Closing the sheet is this handler's to do.
    #[prop(static)]
    pub on_press: Rc<dyn Fn()>,
}

impl Default for SheetPickRowProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            leading: None,
            selected: Reactive::Static(false),
            on_press: Rc::new(|| {}),
        }
    }
}

/// A row that PICKS one of a set: a theme, a team, a sort order. A check
/// at the row's end marks the one chosen. Its slot is kept on every row,
/// so choosing another moves nothing.
///
/// Not a [`SheetCheckRow`]: a box on every row reads as if two could be
/// ticked at once.
///
/// ```ignore
/// SheetPickRow(
///     label = "Dark".to_string(),
///     selected = rx!(theme.get() == Theme::Dark),
///     on_press = pick_dark.clone(),
/// )
/// ```
#[component]
pub fn SheetPickRow(props: SheetPickRowProps) -> Element {
    let label = props.label;
    let leading = props.leading;
    let selected = props.selected;
    let on_press = props.on_press;
    let body = ui! {
        Stack(axis = StackAxis::Row, align = StackAlign::Center, gap = StackGap::Md) {
            if let Some(leading) = leading {
                leading
            }
            Typography(content = label, kind = typography_kind::Body)
        }
    };
    let mark = ui! {
        view(style = PickCheckSlot()) {
            if selected.get() {
                Icon(data = CHECK, size = PICK_CHECK_PX)
            }
        }
    };
    // Two children of the row, parted by its `SpaceBetween`.
    pressable(vec![body, mark], move || on_press())
        .with_style(StyleApplication::new(SheetRow::sheet()))
        .into_element()
}

stylesheet! {
    // The check's room, held whether or not the check is drawn.
    PickCheckSlot<idea_ui::IdeaThemeRef> {
        base(_t) {
            width: Length::Px(PICK_CHECK_PX),
            height: Length::Px(PICK_CHECK_PX),
            flex_shrink: 0.0,
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SheetPushRowProps {
    /// What the pane is about.
    #[prop(static)]
    pub label: String,
    /// What the pane currently says. Live: it follows the pane's state
    /// while the sheet is up.
    pub summary: String,
    /// Push the pane: `cx.push(i)` from a [`DrillCx`](super::DrillCx).
    #[prop(static)]
    pub on_press: Rc<dyn Fn()>,
}

impl Default for SheetPushRowProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            summary: Reactive::Static(String::new()),
            on_press: Rc::new(|| {}),
        }
    }
}

/// A row that PUSHES a pane: the label, what the pane currently says,
/// and the chevron that says a press goes somewhere. The phone's
/// submenu row.
///
/// ```ignore
/// SheetPushRow(
///     label = "Condition".to_string(),
///     summary = rx!(summary()),
///     on_press = { let f: Rc<dyn Fn()> = Rc::new(move || push(2)); f },
/// )
/// ```
#[component]
pub fn SheetPushRow(props: &SheetPushRowProps) -> Element {
    let label = props.label.clone();
    let summary = props.summary.clone();
    let on_press = props.on_press.clone();
    // Two children of the ROW, not one stack with a spacer: a stack in a
    // pressable is content-sized, so a spacer in it has nothing to push
    // against. The row's `SpaceBetween` parts them.
    let leading = ui! { Typography(content = label, kind = typography_kind::Body) };
    // The `icon` builder rather than `Icon`: the colour is a token
    // closure, which is the primitive's own spelling.
    let chevron = icon(CHEVRON_RIGHT)
        .size(PUSH_CHEVRON_PX)
        .color(|| idea_ui::tokens().color.text_muted().resolve())
        .into_element();
    let trailing = ui! {
        Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
            Typography(content = summary, kind = typography_kind::BodySm, muted = true)
            chevron
        }
    };
    pressable(vec![leading, trailing], move || on_press())
        .with_style(StyleApplication::new(SheetRow::sheet()))
        .into_element()
}

#[runtime_core::props]
#[derive(IdealystSchema, Default)]
pub struct SheetHeadingProps {
    /// What the rows under it are about.
    #[prop(static)]
    pub text: String,
}

/// A heading line inside a sheet: what the rows under it are about.
///
/// ```ignore
/// SheetHeading(text = "Group by".to_string())
/// ```
#[component]
pub fn SheetHeading(props: &SheetHeadingProps) -> Element {
    let text = props.text.clone();
    ui! { MenuLabel(text = text) }
}
