//! [`AppFrame`] — the app's sidebar and the screen beside it — with the
//! [`SidebarState`] they share, the [`SidebarItem`]s the sidebar holds
//! and the [`SidebarToggle`] a screen's header works it with.

use std::rc::Rc;

use icons_lucide::{MENU, PANEL_LEFT_CLOSE, PANEL_LEFT_OPEN};
use idea_ui::{tone, variant, Icon, IconButton, IconButtonSize, ListItem};
use idea_ui_nav::AppShell;
use runtime_core::{
    component, derived, rx, signal, stylesheet, ui, AlignItems, Breakpoint, ChildList, Element, FlexDirection,
    IconData, IdealystSchema, JustifyContent, Length, Overflow, Reactive, Shadow, Signal,
};

use crate::width::{form_factor, FormFactor};

/// The sidebar's width, open or as a drawer.
pub const SIDEBAR_WIDTH: f32 = 264.0;

/// The sidebar's width collapsed to icons: an icon button and its
/// margins.
pub const SIDEBAR_RAIL_WIDTH: f32 = 56.0;

/// A sidebar item's glyph beside its label, in px.
const ITEM_ICON_PX: f32 = 18.0;

/// What a pinned sidebar becomes when it is collapsed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema)]
pub enum SidebarCollapse {
    /// It does not collapse: on a desktop the sidebar is always open.
    #[default]
    Off,
    /// It goes away, and the screen takes its room.
    Hidden,
    /// It narrows to a rail of its items' icons.
    Icons,
}

/// The sidebar's state, shared by the [`AppFrame`] that draws it, the
/// [`SidebarItem`]s in it and the [`SidebarToggle`] in a screen's
/// header. The app makes one and hands it to all three. Passed down
/// rather than looked up, because the sidebar's content is built before
/// the frame around it. `Copy`: signals and a choice.
///
/// It has two states, open and collapsed, and they mean different
/// things at different widths:
///
/// | | Open | Collapsed |
/// |---|---|---|
/// | Desktop | pinned at full width | per [`SidebarCollapse`]: full width, gone, or an icon rail |
/// | Tablet, phone | the drawer is over the screen | the drawer is away |
///
/// The icon rail is a desktop form only. Below 1024 the sidebar is a
/// drawer, and an open drawer is always the full sidebar with its
/// labels.
#[derive(Clone, Copy)]
pub struct SidebarState {
    /// Whether the drawer is open. Only means anything while the
    /// sidebar is a drawer.
    pub drawer: Signal<bool>,
    /// Whether the pinned sidebar is collapsed. Only means anything on
    /// a desktop, and only with a [`SidebarCollapse`] other than `Off`.
    /// Write it from a saved preference to restore the reader's choice.
    pub collapsed: Signal<bool>,
    collapse: SidebarCollapse,
}

impl SidebarState {
    /// A sidebar whose collapsed form on a desktop is `collapse`.
    pub fn new(collapse: SidebarCollapse) -> Self {
        Self { drawer: signal(false), collapsed: signal(false), collapse }
    }

    /// The sidebar's collapsed form.
    pub fn collapse(self) -> SidebarCollapse {
        self.collapse
    }

    /// The sidebar is a drawer at this width. TRACKED.
    pub fn is_drawer(self) -> bool {
        form_factor() != FormFactor::Desktop
    }

    /// The sidebar is drawn as a rail of icons right now. TRACKED. For
    /// the sidebar's own content that is not a [`SidebarItem`] (a logo
    /// that drops its wordmark, a footer that hides).
    pub fn is_rail(self) -> bool {
        self.collapse == SidebarCollapse::Icons && !self.is_drawer() && self.collapsed.get()
    }

    /// What a press on the [`SidebarToggle`] does: open the drawer where
    /// the sidebar is one, flip open and collapsed where it is pinned.
    pub fn toggle(self) {
        if runtime_core::untrack(|| self.is_drawer()) {
            self.drawer.set(true);
        } else if self.collapse != SidebarCollapse::Off {
            self.collapsed.set(!self.collapsed.peek());
        }
    }

    /// The width the sidebar takes right now. TRACKED.
    fn width(self, open: f32) -> f32 {
        if self.is_drawer() || !self.collapsed.get() {
            return open;
        }
        match self.collapse {
            SidebarCollapse::Off => open,
            SidebarCollapse::Hidden => 0.0,
            SidebarCollapse::Icons => SIDEBAR_RAIL_WIDTH,
        }
    }
}

impl Default for SidebarState {
    fn default() -> Self {
        Self::new(SidebarCollapse::Off)
    }
}

/// The width the sidebar pins from: the ladder's desktop.
const PIN_AT: Breakpoint = Breakpoint::Lg;

/// The raised content card's lift off the ground: a shadow's drop and
/// blur.
const RAISED_SHADOW_Y: f32 = 1.0;
const RAISED_SHADOW_BLUR: f32 = 3.0;

/// How the screen sits against the sidebar on a desktop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema)]
pub enum FrameSurface {
    /// The sidebar and the screen are one surface, parted by a rule.
    #[default]
    Flat,
    /// The screen is a card on the app's ground, and the sidebar sits on
    /// that ground: a layer behind the content.
    Raised,
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct AppFrameProps {
    /// The sidebar's content: the app's destinations. Built once; the
    /// same nodes serve the pinned column and the drawer.
    pub sidebar: Vec<Element>,
    /// The sidebar's state: the same one its [`SidebarItem`]s and the
    /// headers' [`SidebarToggle`]s hold.
    #[prop(static)]
    pub state: SidebarState,
    /// How the screen sits against the sidebar. Normally the app's look,
    /// chosen once; a signal restyles the frame in place and remounts
    /// nothing.
    pub surface: FrameSurface,
    /// The sidebar's width, open or as a drawer.
    #[prop(static)]
    pub sidebar_width: f32,
    /// The screen on show: a navigator's outlet, or a `Page`.
    pub children: Vec<Element>,
}

impl Default for AppFrameProps {
    fn default() -> Self {
        Self {
            sidebar: Vec::new(),
            state: SidebarState::default(),
            surface: Reactive::Static(FrameSurface::Flat),
            sidebar_width: SIDEBAR_WIDTH,
            children: Vec::new(),
        }
    }
}

/// The app's frame (rule `app-frame`): the sidebar in its seats and the
/// screen beside it. On a desktop the sidebar is pinned, and collapses
/// as its [`SidebarState`] says (rule `sidebar`); on a tablet and a
/// phone it is a drawer over the screen, which then runs edge to edge.
///
/// `surface` is the one switch between the two looks. `Flat` parts the
/// sidebar from the screen with a rule. `Raised` puts the screen on a
/// card and the sidebar on the ground behind it. The card is drawn only
/// while the sidebar is pinned: with the sidebar away there is nothing
/// for the screen to be raised above, and a phone has no room to spend
/// on a margin.
///
/// ```ignore
/// let sidebar = SidebarState::new(SidebarCollapse::Icons);
/// AppFrame(
///     state = sidebar,
///     sidebar = vec![ui! {
///         SidebarItem(sidebar = sidebar, icon = Some(FOLDER), label = "Projects".to_string(), on_press = to_projects)
///         SidebarItem(sidebar = sidebar, icon = Some(USERS), label = "Workers".to_string(), on_press = to_workers)
///     }],
///     surface = FrameSurface::Raised,
/// ) {
///     { nav.outlet }
/// }
/// ```
#[component(children)]
pub fn AppFrame(props: AppFrameProps) -> Element {
    let AppFrameProps { sidebar, state, surface, sidebar_width, children } = props;
    // The `#[component(children)]` slot's flatten, not a staged list.
    let mut screen = Vec::with_capacity(children.len());
    for c in children {
        ChildList::append_to(c, &mut screen);
    }
    let mut destinations = Vec::with_capacity(sidebar.len());
    for c in sidebar {
        ChildList::append_to(c, &mut destinations);
    }
    let ground = {
        let surface = surface.clone();
        derived(move || match surface.get() {
            FrameSurface::Flat => FrameGroundSurface::Flat,
            FrameSurface::Raised => FrameGroundSurface::Raised,
        })
    };
    let side = {
        let surface = surface.clone();
        derived(move || match surface.get() {
            FrameSurface::Flat => FrameSidebarSurface::Flat,
            FrameSurface::Raised => FrameSidebarSurface::Raised,
        })
    };
    // One sheet per look for the content rather than a variant: a
    // variant arm cannot be scoped to a breakpoint, and the card is the
    // desktop's alone. The seat picks between the two sheets, so a
    // change of look restyles the one node the screen is mounted in.
    let content_style = move || match surface.get() {
        FrameSurface::Flat => FrameContentFlat().into_style_application(),
        FrameSurface::Raised => FrameContentRaised().into_style_application(),
    };
    // A sidebar that collapses has a live width: collapsing moves the
    // panel, the screen's offset and the drawer's slide together. One
    // that does not keeps a STATIC width, which the shell bakes into its
    // sheets: a live width is laid out in Rust, so a server-rendered
    // page would paint its first frame for the dump's viewport.
    let width: Reactive<f32> = match state.collapse() {
        SidebarCollapse::Off => Reactive::Static(sidebar_width),
        _ => rx!(state.width(sidebar_width)),
    };
    ui! {
        view(style = FrameGround().surface(ground)) {
            AppShell(
                sidebar = vec![ui! { view(style = FrameSidebar().surface(side)) { destinations } }],
                is_open = state.drawer,
                pin_at = PIN_AT,
                width = width,
            ) {
                view(style = content_style) { screen }
            }
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SidebarToggleProps {
    /// The [`AppFrame`]'s sidebar.
    #[prop(static)]
    pub sidebar: SidebarState,
    /// The button's accessible name.
    #[prop(static)]
    pub label: String,
}

impl Default for SidebarToggleProps {
    fn default() -> Self {
        Self { sidebar: SidebarState::default(), label: "Menu".to_string() }
    }
}

/// The control that works the [`AppFrame`]'s sidebar, for a
/// [`PageHeader`](crate::PageHeader)'s `leading` slot.
///
/// - Tablet, phone: ☰, which opens the drawer.
/// - Desktop: a panel glyph that collapses the sidebar and opens it
///   again. Not drawn where the sidebar does not collapse
///   ([`SidebarCollapse::Off`]): it is always there, and the control
///   would do nothing.
///
/// ```ignore
/// PageHeader(title = "Workers".to_string(), leading = Some(ui! { SidebarToggle(sidebar = sidebar) }))
/// ```
#[component]
pub fn SidebarToggle(props: &SidebarToggleProps) -> Element {
    let sidebar = props.sidebar;
    let label = props.label.clone();
    let press: Rc<dyn Fn()> = Rc::new(move || sidebar.toggle());
    let collapses = sidebar.collapse() != SidebarCollapse::Off;
    let form = move || match (sidebar.is_drawer(), collapses) {
        (true, _) => ToggleForm::Menu,
        (false, false) => ToggleForm::None,
        (false, true) if sidebar.collapsed.get() => ToggleForm::Expand,
        (false, true) => ToggleForm::Collapse,
    };
    // The forms hold different glyphs, or nothing, so they are a
    // `match`.
    ui! {
        match form() {
            ToggleForm::None => { view {} }
            shown => {
                IconButton(
                    icon = Some(shown.glyph()),
                    variant = variant::Ghost,
                    tone = tone::Neutral,
                    label = Some(label.clone()),
                    on_click = press.clone(),
                    test_id = Some("sidebar-toggle"),
                )
            }
        }
    }
}

/// What a [`SidebarToggle`] is drawn as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToggleForm {
    /// Not drawn: a pinned sidebar that does not collapse.
    None,
    /// ☰: opens the drawer.
    Menu,
    /// Collapses the open sidebar.
    Collapse,
    /// Opens the collapsed sidebar.
    Expand,
}

impl ToggleForm {
    fn glyph(self) -> IconData {
        match self {
            ToggleForm::Expand => PANEL_LEFT_OPEN,
            ToggleForm::Collapse => PANEL_LEFT_CLOSE,
            ToggleForm::Menu | ToggleForm::None => MENU,
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SidebarItemProps {
    /// The [`AppFrame`]'s sidebar.
    #[prop(static)]
    pub sidebar: SidebarState,
    /// The destination's glyph: drawn before its label, and ALONE in
    /// the icon rail. Without one the rail draws the label's first
    /// letter.
    #[prop(static)]
    pub icon: Option<IconData>,
    /// The destination's name. In the icon rail it is the item's
    /// tooltip and accessible name.
    #[prop(static)]
    pub label: String,
    /// This is the destination on show.
    pub active: bool,
    /// Go there. The drawer closes itself first.
    #[prop(static)]
    pub on_press: Rc<dyn Fn()>,
    /// The item's robot/E2E id.
    #[prop(static)]
    pub test_id: Option<&'static str>,
}

impl Default for SidebarItemProps {
    fn default() -> Self {
        Self {
            sidebar: SidebarState::default(),
            icon: None,
            label: String::new(),
            active: Reactive::Static(false),
            on_press: Rc::new(|| {}),
            test_id: None,
        }
    }
}

/// One destination in the sidebar (rule `sidebar`). Its glyph and label
/// in a row while the sidebar is open or a drawer; its glyph alone,
/// centred, with the label as a tooltip, in the icon rail. One call for
/// both: the item reads the form from the [`SidebarState`].
///
/// Pressing it in a drawer closes the drawer before going.
///
/// ```ignore
/// SidebarItem(
///     sidebar = sidebar,
///     icon = Some(FOLDER),
///     label = "Projects".to_string(),
///     active = rx!(route.get() == "projects"),
///     on_press = to_projects,
/// )
/// ```
#[component]
pub fn SidebarItem(props: &SidebarItemProps) -> Element {
    let sidebar = props.sidebar;
    let glyph = props.icon;
    let label = props.label.clone();
    let active = props.active.clone();
    let test_id = props.test_id;
    let go = props.on_press.clone();
    let press: Rc<dyn Fn()> = Rc::new(move || {
        sidebar.drawer.set(false);
        go();
    });
    // What a rail draws for an item with no glyph of its own.
    let initial: String = label.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    // The two forms hold different elements, so they are a `match`.
    ui! {
        match sidebar.is_rail() {
            true => {
                view(style = RailSeat()) {
                    IconButton(
                        icon = glyph,
                        glyph = initial.clone(),
                        variant = variant::Ghost,
                        tone = tone::Neutral,
                        size = IconButtonSize::Md,
                        selected = active.clone(),
                        label = Some(label.clone()),
                        tooltip = Some(label.clone()),
                        on_click = press.clone(),
                        test_id = test_id,
                    )
                }
            }
            false => {
                ListItem(
                    label = label.clone(),
                    leading = glyph.map(|data| ui! { Icon(data = data, size = ITEM_ICON_PX) }),
                    active = active.clone(),
                    on_press = Some(press.clone()),
                    test_id = test_id,
                )
            }
        }
    }
}

stylesheet! {
    // A rail item, centred in the rail's width.
    RailSeat<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
    }
}

stylesheet! {
    // What shows wherever nothing else paints: round the raised card,
    // and behind the sidebar.
    FrameGround<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
        }
        variant surface {
            #[default]
            flat(t) { background: t.color.surface() }
            raised(t) { background: t.color.background() }
        }
    }
}

stylesheet! {
    // Fills the shell's panel and paints over it, so the sidebar takes
    // the frame's look at both of its seats. Flat: the screen's surface
    // and a rule. Raised: the ground, and no rule, since the card's own
    // edge parts them.
    FrameSidebar<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: Length::Px(0.0),
            // A collapsing sidebar clips what does not fit its width,
            // down to nothing where it is hidden.
            overflow: Overflow::Hidden,
        }
        variant surface {
            #[default]
            flat(t) {
                background: t.color.surface(),
                border_right_width: 1.0,
                border_right_color: t.color.border(),
            }
            raised(t) { background: t.color.background() }
        }
    }
}

stylesheet! {
    // The screen's seat. A column with a floor of zero, so a bounded
    // page inside it is bounded by the window.
    FrameContentFlat<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            height: Length::pct(100.0),
            min_height: Length::Px(0.0),
            min_width: Length::Px(0.0),
        }
    }
}

stylesheet! {
    // The same seat, a card from `lg`: where the sidebar is pinned
    // beside it. No margin on the sidebar's side, so the card's edge
    // stands where the flat look's rule does. `overflow: hidden` clips
    // the page's square corners to the card's.
    FrameContentRaised<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            height: Length::pct(100.0),
            min_height: Length::Px(0.0),
            min_width: Length::Px(0.0),
        }
        breakpoint lg(t) {
            height: Length::Auto,
            flex_grow: 1.0,
            margin_top: t.spacing.sm(),
            margin_right: t.spacing.sm(),
            margin_bottom: t.spacing.sm(),
            border_radius: t.radius.lg(),
            border_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
            overflow: Overflow::Hidden,
            shadow: Shadow { x: 0.0, y: RAISED_SHADOW_Y, blur: RAISED_SHADOW_BLUR, color: t.effect.shadow() },
        }
    }
}
