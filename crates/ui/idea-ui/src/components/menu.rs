//! `Menu` and its building blocks — an anchored command surface.
//!
//! `Menu` is an anchored panel (like [`Popover`](crate::components::popover::Popover))
//! whose contents you compose from [`MenuItem`], [`MenuLabel`],
//! [`MenuSeparator`], and [`SubMenu`]:
//!
//! ```ignore
//! let trigger: Ref<PressableHandle> = Ref::new();
//! let open = signal!(false);
//! ui! {
//!     Button(label = "Actions", on_click = move || open.set(true), bind_to = Some(trigger))
//!     if open.get() {
//!         Menu(target = AnchorTarget::from(trigger), on_dismiss = move || open.set(false)) {
//!             MenuLabel(text = "Edit")
//!             MenuItem(label = "Rename", on_select = on_rename)
//!             MenuItem(label = "Duplicate", on_select = on_dup)
//!             MenuSeparator()
//!             SubMenu(label = "Move to…", items = folders)
//!             MenuItem(label = "Delete", on_select = on_delete)
//!         }
//!     }
//! }
//! ```
//!
//! A `SubMenu` flyout mounts conditionally, so its contents are passed
//! as reconstructable [`MenuEntry`] data (mirroring `Select`'s menu),
//! whereas top-level `Menu` contents are composed children. See the
//! note on `SubMenuProps::items`.

use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    component, signal, ui, ChildList, IdealystSchema, IntoElement, PressableHandle, Element,
    Reactive, Ref, Signal, StyleApplication,
};

use crate::stylesheets::{MenuChevron, MenuItemRow, MenuLabel as MenuLabelStyle, MenuSeparator as MenuSeparatorStyle, SelectMenu, Spacer};

/// Right-pointing chevron shown on SubMenu rows.
const CHEVRON: &str = "\u{203A}";

fn grow() -> Element {
    runtime_core::view(Vec::new())
        .with_style(|| StyleApplication::new(Spacer::sheet()))
        .into_element()
}

fn panel(children: Vec<Element>) -> Element {
    ui! { view(style = SelectMenu()) { children } }
}

// =============================================================================
// Menu
// =============================================================================

// Reactive-by-default: `#[props]` would wrap each scalar field, but every
// data prop here is STRUCTURAL/positioning — `target`/`side`/`align`/`offset`
// feed the `anchored_overlay` builder once at render, not a reactive style
// sink. They're held bare via `#[prop(static)]`; routing them reactively is a
// separate structural change (TODO below), not a style-prop sweep.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct MenuProps {
    /// Element to anchor against — `AnchorTarget::from(some_ref)`.
    /// Required; the component panics if `None`.
    // TODO(reactive-sweep): route `target` to the anchored_overlay anchor
    // (structural — a live target re-anchors the panel). Kept bare for now.
    #[prop(static)]
    pub target: Option<AnchorTarget>,
    /// Fires on click-outside / Escape; flip your open-state signal.
    pub on_dismiss: Option<Rc<dyn Fn()>>,
    /// Which side of the anchor the panel opens toward. Default `Below`.
    // TODO(reactive-sweep): route `side` to anchored_overlay `.side()`
    // (structural positioning, not a style sink). Kept bare for now.
    #[prop(static)]
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub side: ElementSide,
    /// Alignment along the anchor's edge. Default `Start`.
    // TODO(reactive-sweep): route `align` to anchored_overlay `.align()`.
    #[prop(static)]
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub align: ElementAlign,
    /// Gap in pixels between the anchor and the panel. Default 4.
    // TODO(reactive-sweep): route `offset` to anchored_overlay `.offset()`.
    #[prop(static)]
    #[schema(constraint = "pixels, >= 0")]
    pub offset: f32,
    /// Panel contents — compose [`MenuItem`], [`MenuLabel`],
    /// [`MenuSeparator`], and [`SubMenu`] children.
    pub children: Vec<Element>,
}

impl Default for MenuProps {
    fn default() -> Self {
        Self {
            target: None,
            on_dismiss: None,
            side: ElementSide::Below,
            align: ElementAlign::Start,
            offset: 4.0,
            children: Vec::new(),
        }
    }
}

/// Renders an anchored, backdrop-less command panel containing the
/// composed menu children, positioned relative to `target`.
#[component(children)]
pub fn Menu(props: MenuProps) -> Element {
    let target = props
        .target
        .expect("Menu: required `target` prop missing — set it to an AnchorTarget from a Ref");

    let mut content: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut content);
    }

    let mut bound = runtime_core::anchored_overlay(target, vec![panel(content)])
        .side(props.side)
        .align(props.align)
        .offset(props.offset)
        .backdrop(BackdropMode::None)
        .trap_focus(false);
    if let Some(d) = props.on_dismiss {
        bound = bound.on_dismiss(move || (d)());
    }
    bound.into_element()
}

// =============================================================================
// MenuItem
// =============================================================================

// Reactive-by-default: `#[props]` wraps the scalar `active` → `Reactive<bool>`
// (routes to the row style sink below). `label` is already reactive; the
// handler (`on_select`) and element slots (`leading`/`trailing`) auto-skip.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct MenuItemProps {
    /// Row label. `Reactive<String>` — static or live.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
    /// Fires when the row is chosen. Typically also closes the menu.
    pub on_select: Rc<dyn Fn()>,
    /// Optional leading element (icon, avatar).
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub leading: Option<Element>,
    /// Optional trailing element (shortcut hint, badge), pushed right.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub trailing: Option<Element>,
    /// Renders the row in its highlighted/active state. `Reactive<bool>` —
    /// static or live (signal/`rx!`); the row re-styles in place.
    pub active: bool,
}

impl Default for MenuItemProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            on_select: Rc::new(|| {}),
            leading: None,
            trailing: None,
            active: Reactive::Static(false),
        }
    }
}

/// Renders one selectable menu row: optional leading element, label,
/// and optional right-pushed trailing element, in a pressable.
#[component]
pub fn MenuItem(props: MenuItemProps) -> Element {
    let active = props.active.clone();
    let on_select = props.on_select.clone();

    let mut kids: Vec<Element> = Vec::with_capacity(4);
    if let Some(l) = props.leading {
        kids.push(l);
    }
    kids.push(runtime_core::text(props.label).into_element());
    if let Some(tr) = props.trailing {
        kids.push(grow());
        kids.push(tr);
    }

    // `active` reads live INSIDE the style closure so the apply-style Effect
    // subscribes when it's a signal; a static stays the build-time fast path.
    let style_is_reactive = !active.is_static();
    let make_style = move || {
        StyleApplication::new(MenuItemRow::sheet())
            .with("active", if active.get() { "on" } else { "off" }.to_string())
    };

    let bound = runtime_core::pressable(kids, move || (on_select)());
    if style_is_reactive {
        bound.with_style(make_style).into_element()
    } else {
        bound.with_style(make_style()).into_element()
    }
}

// =============================================================================
// MenuLabel / MenuSeparator
// =============================================================================

// Reactive-by-default: only data field (`text`) is already `Reactive`;
// `#[props]` is a no-op here but kept for uniformity with the family.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct MenuLabelProps {
    /// Section-heading text. `Reactive<String>` — static or live.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub text: Reactive<String>,
}
impl Default for MenuLabelProps {
    fn default() -> Self {
        Self { text: Reactive::Static(String::new()) }
    }
}

/// Renders a non-interactive section heading inside a menu panel.
#[component]
pub fn MenuLabel(props: MenuLabelProps) -> Element {
    ui! { text(style = MenuLabelStyle()) { props.text.clone() } }
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct MenuSeparatorProps {}
impl Default for MenuSeparatorProps {
    fn default() -> Self {
        Self {}
    }
}

/// Renders a thin horizontal divider between groups of menu rows.
#[component]
pub fn MenuSeparator(_props: MenuSeparatorProps) -> Element {
    ui! { view(style = MenuSeparatorStyle()) {} }
}

// =============================================================================
// SubMenu
// =============================================================================

/// One row in a [`SubMenu`] flyout. `MenuEntry::new(label, on_select)`.
#[derive(Clone, IdealystSchema)]
pub struct MenuEntry {
    /// Flyout row label. `Reactive<String>` — static or live.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
    /// Fires when this flyout row is chosen (also closes the flyout).
    pub on_select: Rc<dyn Fn()>,
}

impl MenuEntry {
    pub fn new(label: impl Into<Reactive<String>>, on_select: Rc<dyn Fn()>) -> Self {
        Self { label: label.into(), on_select }
    }
}

// Reactive-by-default: `label` is already reactive; `items` is a LIST
// (`Vec<MenuEntry>`, auto-skipped — the flyout structure, not a style sink);
// `side` is structural overlay positioning, kept bare via `#[prop(static)]`.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct SubMenuProps {
    /// Trigger row label.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
    /// Flyout contents. Passed as reconstructable [`MenuEntry`] data
    /// (not composed children) because the flyout mounts conditionally
    /// — the `when`-gated builder must be able to rebuild it on each
    /// open. Selecting an entry runs its `on_select` and closes the
    /// flyout.
    // TODO(reactive-sweep): route `items` to the flyout rows (list/structural
    // — a live items list would rebuild the flyout). Kept bare for now.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub items: Vec<MenuEntry>,
    /// Which side the flyout opens toward. Default `End` (right in LTR).
    // TODO(reactive-sweep): route `side` to anchored_overlay `.side()`
    // (structural positioning, not a style sink). Kept bare for now.
    #[prop(static)]
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub side: ElementSide,
}

impl Default for SubMenuProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            items: Vec::new(),
            side: ElementSide::End,
        }
    }
}

/// Renders a menu row with a trailing chevron that, on click, opens a
/// nested flyout panel built from `items` to the configured `side`.
#[component]
pub fn SubMenu(props: SubMenuProps) -> Element {
    let open: Signal<bool> = signal!(false);
    let trigger_ref: Ref<PressableHandle> = Ref::new();
    let items = Rc::new(props.items);
    let side = props.side;

    // Trigger row: label + chevron, click toggles the flyout.
    let chevron = runtime_core::text(CHEVRON.to_string())
        .with_style(MenuChevron())
        .into_element();
    let label_node = runtime_core::text(props.label).into_element();
    let trigger = runtime_core::pressable(
        vec![label_node, grow(), chevron],
        move || open.set(!open.get()),
    )
    .with_style(move || {
        StyleApplication::new(MenuItemRow::sheet())
            .with("active", if open.get() { "on" } else { "off" }.to_string())
    })
    .bind(trigger_ref)
    .into_element();

    // Flyout — rebuilt from `items` data each time it opens.
    let flyout = runtime_core::when(
        move || open.get(),
        {
            let items = items.clone();
            move || {
                let close = move || open.set(false);
                let mut rows: Vec<Element> = Vec::with_capacity(items.len());
                for entry in items.iter() {
                    let on_select = entry.on_select.clone();
                    let label = entry.label.clone();
                    let row = runtime_core::pressable(
                        vec![runtime_core::text(label).into_element()],
                        move || {
                            (on_select)();
                            close();
                        },
                    )
                    .with_style(|| StyleApplication::new(MenuItemRow::sheet()))
                    .into_element();
                    rows.push(row);
                }
                runtime_core::anchored_overlay(AnchorTarget::from(trigger_ref), vec![panel(rows)])
                    .side(side)
                    .align(ElementAlign::Start)
                    .offset(2.0)
                    .backdrop(BackdropMode::None)
                    .trap_focus(false)
                    .on_dismiss(move || open.set(false))
                    .into_element()
            }
        },
        || ui! { view {} }.into_element(),
    );

    ui! {
        view {
            trigger
            flyout
        }
    }
}
