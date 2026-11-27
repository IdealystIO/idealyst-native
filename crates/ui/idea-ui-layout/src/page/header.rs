//! [`PageHeader`] — a screen's title row: where it is, the way back,
//! and the verbs that act on it.

use std::rc::Rc;

use icons_lucide::{ARROW_LEFT, CHEVRON_RIGHT, ELLIPSIS};
use idea_ui::{
    tone, typography_kind, variant, Button, Crumb, IconButton, IconButtonSize, Spacer, Typography,
};
use runtime_core::Tokens as _;
use runtime_core::{
    accessibility::Role, component, icon, platform, pressable, stylesheet, ui, AlignItems, Cursor, Element,
    FlexDirection, IdealystSchema, IntoElement, Length, Platform, Reactive, StyleApplication,
};

use crate::overlay::ActionMenu;
use crate::width::is_phone;
use crate::{Action, OptionalSlot};
use crate::LayoutTheme;

/// The most verbs a header holds (rule `header-actions`): a fourth
/// means the screen has not decided what its primary act is.
pub const HEADER_MAX_ACTIONS: usize = 3;

/// The header row's least height, so a header with no action is as tall
/// as one with a button and the body does not shift between screens.
const HEADER_MIN_HEIGHT: f32 = 48.0;

/// The chevron between two crumbs, in px.
const CRUMB_CHEVRON_PX: f32 = 14.0;

/// Which form a [`HeaderWayBack`] takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema)]
pub enum WayBack {
    /// What the platform's own apps do: [`Arrow`](Self::Arrow) on iOS
    /// and Android, [`Trail`](Self::Trail) everywhere else.
    #[default]
    Auto,
    /// The ancestors as a breadcrumb trail, each a link, and no arrow: a
    /// browser already has Back, and a trail names and links every level
    /// where an arrow is one anonymous step.
    Trail,
    /// A back arrow and no trail: on a phone a trail costs the title
    /// its room and its links are below touch size.
    Arrow,
}

impl WayBack {
    /// The form `Auto` takes on `platform`. Keyed to the platform, never
    /// the width: a narrow browser still has a URL bar and Back.
    fn resolve(self, platform: Platform) -> WayBack {
        match self {
            WayBack::Auto => match platform {
                Platform::Ios | Platform::Android => WayBack::Arrow,
                _ => WayBack::Trail,
            },
            chosen => chosen,
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct PageHeaderProps {
    /// The page's name. It gives ground: anything sharing its row lets
    /// it truncate.
    pub title: String,
    /// SLOT, in place of the title's text: for an app that names the
    /// page some other way (the last crumb of its own trail, a title
    /// over a subtitle). It sits in the title's seat, which truncates
    /// before anything else in the row does.
    pub title_content: Option<Element>,
    /// SLOT, before everything: the app's way around (a
    /// [`SidebarToggle`](crate::SidebarToggle), a logo).
    pub leading: Option<Element>,
    /// SLOT, between `leading` and the title: the way back to where the
    /// screen came from. The app's choice: a [`HeaderWayBack`] for the
    /// kit's own, a [`HeaderTrail`] or a [`HeaderBackButton`] for one
    /// form at every platform, or anything of the app's.
    pub nav: Option<Element>,
    /// SLOT, after the title and the record's menu: the record's status
    /// (a chip, a progress bar). A phone does not draw it.
    #[prop(static)]
    pub status: OptionalSlot,
    /// SLOT, at the far end before the verbs: something that is not a
    /// verb (an avatar stack, a sync indicator). A phone does not draw
    /// it.
    #[prop(static)]
    pub trailing: OptionalSlot,
    /// The verbs that act on the page, at most [`HEADER_MAX_ACTIONS`],
    /// at most one of them [`primary`](Action::primary).
    #[prop(static)]
    pub actions: Vec<Action>,
    /// What applies to the whole record (Archive, Delete, "Save as
    /// template"): a ⋮ menu right after the title.
    #[prop(static)]
    pub menu: Vec<Action>,
}

impl Default for PageHeaderProps {
    fn default() -> Self {
        Self {
            title: Reactive::Static(String::new()),
            title_content: None,
            leading: None,
            nav: None,
            status: OptionalSlot::default(),
            trailing: OptionalSlot::default(),
            actions: Vec::new(),
            menu: Vec::new(),
        }
    }
}

/// A screen's title row (rules `header-slots`, `header-actions`,
/// `record-menu`). The header is slots in a fixed order; a screen fills
/// them and the header arranges them for the width.
///
/// ```text
/// wide   [leading] [nav] Title  ⋮ [status]            [trailing] [Export] [Add heading]
/// phone  [leading] [nav] Title                                             (+)  ⋮
/// ```
///
/// - What goes in a slot is the app's. The header decides only where
///   each slot stands, which ones a phone has room for, and that the
///   title gives ground first.
/// - The verbs are labelled buttons from 640 up, the primary last. On a
///   phone the primary is its glyph alone and every other verb folds
///   into ONE ⋮ with the record's menu. A label is never shortened.
/// - The `status` and `trailing` slots are drawn from 640 up.
///
/// The menus are [`ActionMenu`]s, so on a phone they open as action
/// sheets.
///
/// ```ignore
/// PageHeader(
///     title = rx!(project.get().name),
///     leading = Some(ui! { SidebarToggle(sidebar = sidebar) }),
///     nav = Some(ui! { HeaderWayBack(crumbs = vec![Crumb::linked("Projects", to_projects)], on_back = Some(pop)) }),
///     status = || ui! { Tag(label = "Active") },
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
    let nav = props.nav;
    let title_content = props.title_content;
    let status = props.status;
    let trailing = props.trailing;
    let menu = props.menu;
    let primary: Option<Action> = props.actions.iter().find(|a| a.primary).cloned();
    let others: Vec<Action> = props.actions.iter().filter(|a| !a.primary).cloned().collect();
    // The phone's one menu: the verbs that lost their button, then what
    // the record's menu holds.
    let folded: Vec<Action> = others.iter().chain(menu.iter()).cloned().collect();
    // An empty menu is left out, not drawn empty: even with no trigger it
    // is a node in the row, which takes the row's gap (the primary then
    // stopped a gap short of the gutter, off the toolbar's edge).
    let folded_menu: Option<Vec<Action>> = (!folded.is_empty()).then(|| folded.clone());
    let record_menu: Option<Vec<Action>> = (!menu.is_empty()).then(|| menu.clone());
    ui! {
        view(style = HeaderRow()) {
            if let Some(leading) = leading {
                leading
            }
            if let Some(nav) = nav {
                nav
            }
            view(style = HeaderTitle()) {
                if let Some(content) = title_content {
                    content
                } else {
                    Typography(
                        content = title,
                        kind = typography_kind::H3,
                        lines = Some(1_u32),
                        a11y_role = Some(Role::Header),
                    )
                }
            }
            // The two forms hold different elements, so they are a
            // `match`; it lives here so no screen writes one.
            match is_phone() {
                true => {
                    Spacer()
                    if let Some(a) = primary.clone() {
                        HeaderPrimary(action = a, compact = true)
                    }
                    if let Some(actions) = folded_menu.clone() {
                        ActionMenu(actions = actions, size = IconButtonSize::Md)
                    }
                }
                false => {
                    if let Some(actions) = record_menu.clone() {
                        ActionMenu(actions = actions, size = IconButtonSize::Md)
                    }
                    if let Some(build) = status.0.clone() {
                        { build() }
                    }
                    Spacer()
                    if let Some(build) = trailing.0.clone() {
                        { build() }
                    }
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
#[derive(IdealystSchema)]
pub struct HeaderWayBackProps {
    /// The page's ancestors, root first, each normally
    /// [`Crumb::linked`]. Drawn where the form is a trail.
    #[prop(static)]
    pub crumbs: Vec<Crumb>,
    /// Pop one level. Drawn as an arrow where the form is one.
    #[prop(static)]
    pub on_back: Option<Rc<dyn Fn()>>,
    /// Which form to take. Leave it `Auto`.
    #[prop(static)]
    pub form: WayBack,
}

impl Default for HeaderWayBackProps {
    fn default() -> Self {
        Self { crumbs: Vec::new(), on_back: None, form: WayBack::Auto }
    }
}

/// The kit's own way back, for a [`PageHeader`]'s `nav` slot (rule
/// `header-nav`): a screen hands it both the ancestors and a pop, and it
/// draws the one its platform's apps use. A [`HeaderTrail`] on the web
/// and the desktop; a [`HeaderBackButton`] on iOS and Android.
///
/// An app that wants one form everywhere, or its own, fills the slot
/// with that instead.
///
/// ```ignore
/// HeaderWayBack(crumbs = vec![Crumb::linked("Projects", to_projects)], on_back = Some(pop))
/// ```
#[component]
pub fn HeaderWayBack(props: &HeaderWayBackProps) -> Element {
    // The platform cannot change under a mounted screen, so the form is
    // decided once.
    match (props.form.resolve(platform()), props.on_back.clone()) {
        (WayBack::Arrow, Some(back)) => ui! { HeaderBackButton(on_back = back) },
        (WayBack::Arrow, None) => ui! { view {} },
        _ => ui! { HeaderTrail(crumbs = props.crumbs.clone()) },
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct HeaderBackButtonProps {
    /// Pop one level.
    #[prop(static)]
    pub on_back: Rc<dyn Fn()>,
}

impl Default for HeaderBackButtonProps {
    fn default() -> Self {
        Self { on_back: Rc::new(|| {}) }
    }
}

/// A back arrow, for a [`PageHeader`]'s `nav` slot.
///
/// ```ignore
/// HeaderBackButton(on_back = pop)
/// ```
#[component]
pub fn HeaderBackButton(props: &HeaderBackButtonProps) -> Element {
    ui! {
        IconButton(
            icon = Some(ARROW_LEFT),
            variant = variant::Ghost,
            tone = tone::Neutral,
            size = IconButtonSize::Md,
            label = Some(crate::layout_labels().back.clone()),
            on_click = props.on_back.clone(),
            test_id = Some("header-back"),
        )
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct HeaderTrailProps {
    /// The page's ancestors, root first. Empty draws nothing.
    #[prop(static)]
    pub crumbs: Vec<Crumb>,
    /// On a phone, fold the ancestors into one "…" menu so the row
    /// stays one line. Off: every ancestor is drawn at every width, and
    /// keeping the row to one line is the app's.
    #[prop(static)]
    pub fold_on_phone: bool,
}

impl Default for HeaderTrailProps {
    fn default() -> Self {
        Self { crumbs: Vec::new(), fold_on_phone: true }
    }
}

/// A breadcrumb trail of the page's ancestors, for a [`PageHeader`]'s
/// `nav` slot. Each ancestor in a row with a chevron after it; the page
/// itself is the header's title, after the last chevron.
///
/// With `fold_on_phone` (the default), a phone draws ONE "…" that lists
/// the ancestors, nearest last, as an action sheet. That is one answer
/// to a trail that does not fit; an app with another passes its own
/// element to the slot.
///
/// ```ignore
/// HeaderTrail(crumbs = vec![Crumb::linked("Projects", to_projects)])
/// ```
#[component]
pub fn HeaderTrail(props: &HeaderTrailProps) -> Element {
    let crumbs = props.crumbs.clone();
    let has_crumbs = !crumbs.is_empty();
    let fold = props.fold_on_phone;
    let hidden: Vec<Action> = crumbs
        .iter()
        .map(|c| Action::new(c.label.get(), c.on_press.clone().unwrap_or_else(|| Rc::new(|| {}))))
        .collect();
    ui! {
        if has_crumbs {
            match fold && is_phone() {
                true => {
                    ActionMenu(
                        actions = hidden.clone(),
                        icon = Some(ELLIPSIS),
                        label = crate::layout_labels().show_pages_above.clone(),
                        size = IconButtonSize::Md,
                        test_id = Some("header-crumbs-more"),
                    )
                    CrumbChevron()
                }
                false => {
                    for c in crumbs.iter().enumerate(), key = c.0 {
                        HeaderCrumb(crumb = c.1.clone())
                        CrumbChevron()
                    }
                }
            }
        }
    }
}

/// The chevron after an ancestor.
///
/// ```ignore
/// CrumbChevron()
/// ```
#[component]
fn CrumbChevron() -> Element {
    // The `icon` builder rather than `Icon`: the colour is a token
    // closure, which is the primitive's own spelling.
    icon(CHEVRON_RIGHT)
        .size(CRUMB_CHEVRON_PX)
        .color(|| idea_ui::tokens().color.text_muted().resolve())
        .into_element()
}

#[runtime_core::props]
#[derive(IdealystSchema)]
struct HeaderCrumbProps {
    /// The ancestor.
    #[prop(static)]
    pub crumb: Crumb,
}

impl Default for HeaderCrumbProps {
    fn default() -> Self {
        Self { crumb: Crumb::new(String::new()) }
    }
}

/// One ancestor: a link where it has somewhere to go, muted text where
/// it does not.
///
/// ```ignore
/// HeaderCrumb(crumb = Crumb::linked("Projects", to_projects))
/// ```
#[component]
fn HeaderCrumb(props: &HeaderCrumbProps) -> Element {
    let label = props.crumb.label.clone();
    let text = ui! { Typography(content = label, muted = true) };
    match props.crumb.on_press.clone() {
        // `pressable` takes its children by value: `ui!` owns no
        // pressable tag, so the vec is the primitive's signature.
        Some(go) => pressable(vec![text], move || go())
            .with_style(StyleApplication::new(CrumbLink::sheet()))
            .into_element(),
        None => text,
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
    // A link with no box of its own, so the first crumb's text stands
    // on the gutter's edge, where the body's does.
    CrumbLink<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_shrink: 0.0,
            cursor: Cursor::Pointer,
        }
        state hovered(_t) { opacity: 0.7 }
        state pressed(_t) { opacity: 0.5 }
        transitions { opacity: 120ms EaseOut }
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
            padding_left: LayoutTheme::tokens().gutter.phone(),
            padding_right: LayoutTheme::tokens().gutter.phone(),
            padding_top: t.spacing.sm(),
            padding_bottom: t.spacing.sm(),
        }
        breakpoint sm(t) {
            padding_left: LayoutTheme::tokens().gutter.tablet(),
            padding_right: LayoutTheme::tokens().gutter.tablet(),
        }
        breakpoint lg(t) {
            padding_left: LayoutTheme::tokens().gutter.desktop(),
            padding_right: LayoutTheme::tokens().gutter.desktop(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_is_a_back_arrow_on_ios_and_android_and_a_trail_elsewhere() {
        assert_eq!(WayBack::Auto.resolve(Platform::Ios), WayBack::Arrow);
        assert_eq!(WayBack::Auto.resolve(Platform::Android), WayBack::Arrow);
        assert_eq!(WayBack::Auto.resolve(Platform::Web), WayBack::Trail);
        assert_eq!(WayBack::Auto.resolve(Platform::MacOs), WayBack::Trail);
    }

    #[test]
    fn a_chosen_form_holds_on_every_platform() {
        assert_eq!(WayBack::Arrow.resolve(Platform::Web), WayBack::Arrow);
        assert_eq!(WayBack::Trail.resolve(Platform::Ios), WayBack::Trail);
    }
}
