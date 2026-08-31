//! `Menu` and its building blocks — an anchored command surface.
//!
//! `Menu` is an anchored panel (like [`Popover`](crate::components::popover::Popover))
//! whose contents you compose from [`MenuItem`], [`MenuLabel`],
//! [`MenuSeparator`], and [`SubMenu`]:
//!
//! ```ignore
//! let trigger: Ref<PressableHandle> = Ref::new();
//! let open = signal(false);
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
//!
//! # A searchable menu
//!
//! `header` and `footer` pin an element outside the scrolling row area, so it
//! stays put while a long list scrolls. The framework pins the slot; the
//! CALLER owns the filtering — which keeps the slot equally usable for a
//! section heading, a hint, or a bulk action:
//!
//! ```ignore
//! let query = signal(String::new());
//! let rows = rx!({
//!     let q = query.get().to_lowercase();
//!     options.iter().filter(|o| o.label.to_lowercase().contains(&q)).collect()
//! });
//! ui! {
//!     Menu(
//!         target = AnchorTarget::from(trigger),
//!         on_dismiss = Some(on_dismiss),
//!         header = Some(ui! { Field(value = query, placeholder = "Search…") }),
//!     ) {
//!         // …one MenuItem per surviving option, plus a "No matches" row
//!         // when the query filters everything out.
//!     }
//! }
//! ```
//!
//! A menu with either slot set preserves focus across row presses, so the
//! search box keeps its caret when a row is clicked. Note that `SubMenu` has
//! no slots by design: its flyout opens and closes on HOVER, so a box you
//! type into while the pointer drifts off would close under you. Anchor a
//! searchable menu to a CLICK instead.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    after_ms, component, signal, ui, ChildList, IdealystSchema, IntoElement, Element, Reactive,
    Ref, ScheduledTask, Signal, StyleApplication, ViewHandle,
};

/// Grace period before a hovered-out submenu collapses. Bridges the small gap
/// between a SubMenu's trigger row and its flyout panel so moving the pointer
/// from one to the other doesn't flicker the flyout shut. Standard "hover
/// intent" delay.
const SUBMENU_HOVER_GRACE_MS: i32 = 120;

use crate::stylesheets::{MenuCheckMark, MenuCheckbox, MenuChevron, MenuItemRow, MenuLabel as MenuLabelStyle, MenuSeparator as MenuSeparatorStyle, Spacer};

/// Right-pointing chevron shown on SubMenu rows.
const CHEVRON: &str = "\u{203A}";

fn grow() -> Element {
    runtime_core::view(Vec::new())
        .with_style(|| StyleApplication::new(Spacer::sheet()))
        .into_element()
}

/// The anchored panel surface shared by `Menu` and each `SubMenu` flyout.
/// Delegates to the family-wide scrolling panel so a long item list (or a
/// SubMenu with many entries) scrolls within a height-capped panel instead of
/// running off the bottom of the viewport.
fn panel(children: Vec<Element>) -> Element {
    crate::components::menu_panel::scrolling_menu_panel(children)
}

/// [`panel`] with `Menu`'s optional pinned `header`/`footer` slots. Passing
/// `None` for both is byte-for-byte [`panel`] — a slotless menu is unchanged.
fn slotted_panel(
    children: Vec<Element>,
    header: Option<Element>,
    footer: Option<Element>,
) -> Element {
    crate::components::menu_panel::slotted_menu_panel(children, header, footer)
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
    /// Optional element pinned ABOVE the scrolling row area — it stays put
    /// while the rows scroll under it. A search field is the motivating case:
    /// the panel holds the box, and the CALLER owns what typing in it does
    /// (filter your own rows and pass the survivors as `children`). The
    /// framework deliberately knows nothing about the query, so a header is
    /// equally a section heading, a hint, or a bulk action.
    ///
    /// A menu with either slot set preserves focus across row presses, so a
    /// focusable header keeps its caret when a row is clicked.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub header: Option<Element>,
    /// Optional element pinned BELOW the scrolling row area — see
    /// [`MenuProps::header`]. Typically a "Clear" / "Add ‹what you typed›"
    /// action that must stay reachable without scrolling to the end.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub footer: Option<Element>,
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
            header: None,
            footer: None,
        }
    }
}

/// Renders an anchored command panel containing the composed menu children,
/// positioned relative to `target`. Dismisses on Escape and on **outside
/// click** (a fullscreen transparent catcher behind the panel fires
/// `on_dismiss`, the universal dropdown/menu behavior — mirrors `Popover`).
#[component(children)]
pub fn Menu(props: MenuProps) -> Element {
    let target = props
        .target
        .expect("Menu: required `target` prop missing — set it to an AnchorTarget from a Ref");

    let mut content: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut content);
    }

    let on_dismiss = props.on_dismiss;

    // The anchored panel itself carries NO backdrop — the catcher (below) owns
    // outside-click dismissal; this `on_dismiss` is the Escape-key path.
    let mut bound =
        runtime_core::anchored_overlay(target, vec![slotted_panel(content, props.header, props.footer)])
        .side(props.side)
        .align(props.align)
        .offset(props.offset)
        .backdrop(BackdropMode::None)
        .trap_focus(false);
    if let Some(d) = on_dismiss.clone() {
        bound = bound.on_dismiss(move || (d)());
    }
    let anchored = bound.into_element();

    // Fullscreen transparent catcher BEHIND the panel: a tap anywhere off the
    // panel fires `on_dismiss`. A tap ON the panel lands on the panel (rendered
    // after the catcher, so above it) and doesn't dismiss. Same proven pattern
    // as `Popover`; without it the menu only closed on Escape / item-select.
    let catcher = crate::components::popover::dismiss_catcher(on_dismiss);

    // Out-of-flow wrapper so the two portals don't occupy a flex slot and shift
    // the trigger's siblings on open/close (see `out_of_flow_wrapper_sheet`).
    runtime_core::view(vec![catcher, anchored])
        .with_style(crate::components::popover::out_of_flow_wrapper_sheet())
        .into_element()
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

/// One row in a [`SubMenu`] flyout. `MenuEntry::new(label, on_select)`
/// for a classic pick-and-close row, `MenuEntry::checkable(label,
/// checked, on_select)` for a multi-select row.
#[derive(Clone, IdealystSchema)]
pub struct MenuEntry {
    /// Flyout row label. `Reactive<String>` — static or live.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
    /// Fires when this flyout row is chosen (also closes the flyout,
    /// unless the row is checkable).
    pub on_select: Rc<dyn Fn()>,
    /// `Some` renders a leading checkbox reflecting the value LIVE
    /// (pass a signal/`rx!` so toggles re-mark the row in place), and
    /// selecting the row does NOT close the flyout — multi-select rows
    /// keep it open for the next toggle. `None` is the classic row.
    #[schema(constraint = "reactive: static bool or Signal/rx!")]
    pub checked: Option<Reactive<bool>>,
}

impl MenuEntry {
    pub fn new(label: impl Into<Reactive<String>>, on_select: Rc<dyn Fn()>) -> Self {
        Self { label: label.into(), on_select, checked: None }
    }

    /// A multi-select row: leading checkbox bound to `checked`, and the
    /// flyout stays open across selects.
    pub fn checkable(
        label: impl Into<Reactive<String>>,
        checked: impl Into<Reactive<bool>>,
        on_select: Rc<dyn Fn()>,
    ) -> Self {
        Self { label: label.into(), on_select, checked: Some(checked.into()) }
    }
}

/// The small checkbox glyph checkable [`MenuEntry`]s render — exported
/// so composed [`MenuItem`]s (a chip's value menu, say) can carry the
/// same mark in their `leading` slot. Reactive: pass a signal/`rx!`
/// and the mark flips in place.
pub fn menu_checkbox(checked: impl Into<Reactive<bool>>) -> Element {
    let checked = checked.into();
    let mark_checked = checked.clone();
    let mark = runtime_core::text("✓".to_string())
        .with_style(move || {
            StyleApplication::new(MenuCheckMark::sheet())
                .with("checked", if mark_checked.get() { "on" } else { "off" }.to_string())
        })
        .into_element();
    runtime_core::view(vec![mark])
        .with_style(move || {
            StyleApplication::new(MenuCheckbox::sheet())
                .with("checked", if checked.get() { "on" } else { "off" }.to_string())
        })
        .into_element()
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

/// Renders a menu row with a trailing chevron whose nested flyout opens on
/// HOVER — the desktop/web standard for nested menus. Only one submenu per
/// level is open at a time: this falls out of hover naturally, since hovering
/// a sibling row means the pointer has LEFT this one, which closes it (after a
/// short grace) while the sibling opens. The grace bridges the gap between the
/// trigger row and its flyout so moving the pointer into the flyout doesn't
/// dismiss it. Hovering off the row/flyout collapses just this submenu; the
/// parent Menu's catcher still closes the whole menu on an outside click.
///
/// Touch has no hover (`on_hover` is a no-op on iOS/Android), so the flyout
/// won't expand there — mobile menus are a separate consideration.
#[component]
pub fn SubMenu(props: SubMenuProps) -> Element {
    let open: Signal<bool> = signal(false);
    let trigger_ref: Ref<ViewHandle> = Ref::new();
    let items = Rc::new(props.items);
    let side = props.side;

    // Pending hover-out close. Stored (not detached) so a re-hover of the
    // trigger OR the flyout cancels it — the "hover intent" bridge. Owned by
    // this component scope; drops (cancelling any pending close) on unmount.
    let close_task: Rc<RefCell<Option<ScheduledTask>>> = Rc::new(RefCell::new(None));

    // Hover-in: cancel any pending close and open immediately.
    let open_now = {
        let ct = close_task.clone();
        move || {
            if let Some(mut t) = ct.borrow_mut().take() {
                t.cancel();
            }
            open.set(true);
        }
    };
    // Hover-out: collapse after the grace, unless re-hovered first.
    let schedule_close = {
        let ct = close_task.clone();
        move || {
            if let Some(mut t) = ct.borrow_mut().take() {
                t.cancel();
            }
            let task = after_ms(SUBMENU_HOVER_GRACE_MS, move || open.set(false));
            *ct.borrow_mut() = Some(task);
        }
    };

    // Trigger row — a hover-tracking VIEW (`on_hover` is a view-only channel),
    // styled like a menu row and anchoring the flyout. The chevron marks it as
    // expandable; `active` highlights it while its flyout is open.
    let chevron = runtime_core::text(CHEVRON.to_string())
        .with_style(MenuChevron())
        .into_element();
    let label_node = runtime_core::text(props.label).into_element();
    let trigger = {
        let open_now = open_now.clone();
        let schedule_close = schedule_close.clone();
        runtime_core::view(vec![label_node, grow(), chevron])
            .with_style(move || {
                StyleApplication::new(MenuItemRow::sheet())
                    .with("active", if open.get() { "on" } else { "off" }.to_string())
            })
            .on_hover(move |entering| {
                if entering {
                    open_now();
                } else {
                    schedule_close();
                }
            })
            .bind(trigger_ref)
            .into_element()
    };

    // Flyout — rebuilt from `items` each time it opens. Its panel ALSO tracks
    // hover so the pointer can travel from the trigger into it (and dwell
    // there) without the grace timer collapsing it.
    let flyout = runtime_core::when(
        move || open.get(),
        {
            let items = items.clone();
            let open_now = open_now.clone();
            let schedule_close = schedule_close.clone();
            move || {
                let mut rows: Vec<Element> = Vec::with_capacity(items.len());
                for entry in items.iter() {
                    let on_select = entry.on_select.clone();
                    let label = entry.label.clone();
                    // Checkable rows keep the flyout open — multi-select
                    // toggles stack without re-hovering (the checkbox
                    // itself re-marks reactively).
                    let keep_open = entry.checked.is_some();
                    let mut kids: Vec<Element> = Vec::with_capacity(2);
                    if let Some(c) = entry.checked.clone() {
                        kids.push(menu_checkbox(c));
                    }
                    kids.push(runtime_core::text(label).into_element());
                    let row = runtime_core::pressable(kids, move || {
                        (on_select)();
                        if !keep_open {
                            open.set(false);
                        }
                    })
                    .with_style(|| StyleApplication::new(MenuItemRow::sheet()))
                    .into_element();
                    rows.push(row);
                }
                // Wrap the panel in a hover-tracking view so dwelling in the
                // flyout keeps it open (cancels the trigger's hover-out close).
                let on_enter = open_now.clone();
                let on_leave = schedule_close.clone();
                let panel_view = runtime_core::view(vec![panel(rows)])
                    .on_hover(move |entering| {
                        if entering {
                            on_enter();
                        } else {
                            on_leave();
                        }
                    })
                    .into_element();
                runtime_core::anchored_overlay(AnchorTarget::from(trigger_ref), vec![panel_view])
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;
    use runtime_core::primitives::portal::{PortalTarget, ViewportPlacement};
    use runtime_core::Ref;

    // (ViewHandle is the module's anchor handle type; any anchorable Ref works
    // as a Menu target here.)

    /// Regression: a Menu dismisses on OUTSIDE CLICK, not just Escape /
    /// item-select. Like `Popover`, it composes a FULLSCREEN transparent
    /// catcher (a viewport-sized `overlay()` portal whose backdrop is a
    /// `Pressable` wired to `on_dismiss`) BEHIND the anchored panel. Menu used
    /// to be a lone `anchored_overlay(BackdropMode::None)` with no catcher, so
    /// clicking away never closed it. If a refactor drops the catcher (or
    /// shrinks it from FullScreen), click-away silently disappears again.
    #[test]
    fn menu_has_fullscreen_catcher_behind_panel() {
        with_test_world(|| {
            let trigger: Ref<ViewHandle> = Ref::new();
            let el = Menu(MenuProps {
                target: Some(AnchorTarget::from(trigger)),
                children: vec![runtime_core::text("Item".to_string()).into_element()],
                ..Default::default()
            });

            // Menu = a View wrapping [catcher, anchored panel].
            let mut kids = match classify(el) {
                P::View { children, .. } => children,
                _ => panic!("a targeted Menu must wrap [catcher, anchored] in a View"),
            };
            assert_eq!(kids.len(), 2, "Menu = fullscreen catcher + anchored panel");

            // child[0]: fullscreen catcher portal with a tap-catching backdrop.
            match classify(kids.remove(0)) {
                P::Portal { mut children, target, .. } => {
                    assert!(
                        matches!(target, PortalTarget::Viewport(ViewportPlacement::FullScreen)),
                        "the catcher must be a FULLSCREEN viewport portal"
                    );
                    assert!(
                        !children.is_empty()
                            && matches!(classify(children.remove(0)), P::Pressable { .. }),
                        "the catcher's first child must be the backdrop Pressable"
                    );
                }
                _ => panic!("Menu's first child must be the fullscreen catcher Portal"),
            }

            // child[1]: the anchored panel portal.
            assert!(
                matches!(classify(kids.remove(0)), P::Portal { .. }),
                "Menu's second child must be the anchored panel Portal"
            );
    });
    }

    /// A `Menu`'s `header`/`footer` slots must reach the panel PINNED — direct
    /// panel children around the scrolling row area, not inside it — so a
    /// search field stays on screen while a long option list scrolls under it.
    /// This is the whole point of the slots; if a refactor routes `Menu` back
    /// through the slotless panel the header silently disappears.
    #[test]
    fn menu_slots_reach_the_panel_pinned_around_the_scroller() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());

            let trigger: Ref<ViewHandle> = Ref::new();
            let el = Menu(MenuProps {
                target: Some(AnchorTarget::from(trigger)),
                children: vec![runtime_core::text("Row".to_string()).into_element()],
                header: Some(runtime_core::text("Search".to_string()).into_element()),
                footer: Some(runtime_core::text("Clear".to_string()).into_element()),
                ..Default::default()
            });

            // Menu = View[catcher, anchored panel portal]; the panel is the
            // portal's child.
            let mut kids = match classify(el) {
                P::View { children, .. } => children,
                _ => panic!("a targeted Menu must wrap [catcher, anchored] in a View"),
            };
            let anchored = kids.remove(1);
            let mut portal_kids = match classify(anchored) {
                P::Portal { children, .. } => children,
                _ => panic!("Menu's second child must be the anchored panel Portal"),
            };
            // The portal's child is `anchored_overlay`'s positioning wrapper;
            // the SelectMenu panel is inside it.
            let mut positioned = match classify(portal_kids.remove(0)) {
                P::View { children, .. } => children,
                _ => panic!("the anchored portal's child is the positioning wrapper View"),
            };
            assert_eq!(positioned.len(), 1, "the positioner wraps the panel alone");
            let P::View { preserves_focus, mut children, .. } = classify(positioned.remove(0))
            else {
                panic!("the positioner's child is the SelectMenu panel View");
            };
            assert!(
                preserves_focus,
                "a slotted menu preserves focus so a row press can't blur the header field"
            );
            assert_eq!(children.len(), 3, "panel children are [header, scroller, footer]");
            match classify(children.remove(0)) {
                P::Text { text, .. } => assert_eq!(text.as_deref(), Some("Search")),
                _ => panic!("the header slot is pinned as the panel's first child"),
            }
            assert!(
                matches!(classify(children.remove(0)), P::ScrollView { .. }),
                "the rows still scroll between the pinned slots"
            );
            match classify(children.remove(0)) {
                P::Text { text, .. } => assert_eq!(text.as_deref(), Some("Clear")),
                _ => panic!("the footer slot is pinned as the panel's last child"),
            }
    });
    }

    /// Regression: neither portal a Menu builds may trap focus. The catcher
    /// comes from `overlay()`, whose trap defaults ON, and a trapping EMPTY
    /// portal bounces focus out of its SIBLING — so any focusable content on
    /// the panel (a search box, an input row) took the press and instantly
    /// lost focus. See `popover::dismiss_catcher`.
    #[test]
    fn regression_neither_menu_portal_traps_focus() {
        with_test_world(|| {
            let trigger: Ref<ViewHandle> = Ref::new();
            let el = Menu(MenuProps {
                target: Some(AnchorTarget::from(trigger)),
                children: vec![runtime_core::text("Item".to_string()).into_element()],
                ..Default::default()
            });
            let kids = match classify(el) {
                P::View { children, .. } => children,
                _ => panic!("a targeted Menu must wrap [catcher, anchored] in a View"),
            };
            for (i, child) in kids.into_iter().enumerate() {
                match classify(child) {
                    P::Portal { trap_focus, .. } => assert!(
                        !trap_focus,
                        "Menu portal {i} traps focus — focusable panel content becomes untypable"
                    ),
                    _ => panic!("both of a Menu's children must be Portals"),
                }
            }
    });
    }
}
