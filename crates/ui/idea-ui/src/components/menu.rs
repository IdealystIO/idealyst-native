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
    component, signal, ui, ChildList, IntoElement, PressableHandle, Element, Reactive, Ref, Signal,
    StyleApplication,
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

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct MenuProps {
    /// Element to anchor against — `AnchorTarget::from(some_ref)`.
    pub target: Option<AnchorTarget>,
    /// Fires on click-outside / Escape; flip your open-state signal.
    pub on_dismiss: Option<Rc<dyn Fn()>>,
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub side: ElementSide,
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub align: ElementAlign,
    pub offset: f32,
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

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct MenuItemProps {
    /// Row label. `Reactive<String>` — static or live.
    pub label: Reactive<String>,
    /// Fires when the row is chosen. Typically also closes the menu.
    pub on_select: Rc<dyn Fn()>,
    /// Optional leading element (icon, avatar).
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub leading: Option<Element>,
    /// Optional trailing element (shortcut hint, badge), pushed right.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub trailing: Option<Element>,
    /// Renders the row in its highlighted/active state.
    pub active: bool,
}

impl Default for MenuItemProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            on_select: Rc::new(|| {}),
            leading: None,
            trailing: None,
            active: false,
        }
    }
}

#[component]
pub fn MenuItem(props: MenuItemProps) -> Element {
    let active = props.active;
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

    runtime_core::pressable(kids, move || (on_select)())
        .with_style(move || {
            StyleApplication::new(MenuItemRow::sheet())
                .with("active", if active { "on" } else { "off" }.to_string())
        })
        .into_element()
}

// =============================================================================
// MenuLabel / MenuSeparator
// =============================================================================

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct MenuLabelProps {
    pub text: Reactive<String>,
}
impl Default for MenuLabelProps {
    fn default() -> Self {
        Self { text: Reactive::Static(String::new()) }
    }
}

#[component]
pub fn MenuLabel(props: MenuLabelProps) -> Element {
    ui! { text(style = MenuLabelStyle()) { props.text.clone() } }
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct MenuSeparatorProps {}
impl Default for MenuSeparatorProps {
    fn default() -> Self {
        Self {}
    }
}

#[component]
pub fn MenuSeparator(_props: MenuSeparatorProps) -> Element {
    ui! { view(style = MenuSeparatorStyle()) {} }
}

// =============================================================================
// SubMenu
// =============================================================================

/// One row in a [`SubMenu`] flyout. `MenuEntry::new(label, on_select)`.
#[derive(Clone)]
pub struct MenuEntry {
    pub label: Reactive<String>,
    pub on_select: Rc<dyn Fn()>,
}

impl MenuEntry {
    pub fn new(label: impl Into<Reactive<String>>, on_select: Rc<dyn Fn()>) -> Self {
        Self { label: label.into(), on_select }
    }
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct SubMenuProps {
    /// Trigger row label.
    pub label: Reactive<String>,
    /// Flyout contents. Passed as reconstructable [`MenuEntry`] data
    /// (not composed children) because the flyout mounts conditionally
    /// — the `when`-gated builder must be able to rebuild it on each
    /// open. Selecting an entry runs its `on_select` and closes the
    /// flyout.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub items: Vec<MenuEntry>,
    /// Which side the flyout opens toward. Default `End` (right in LTR).
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
