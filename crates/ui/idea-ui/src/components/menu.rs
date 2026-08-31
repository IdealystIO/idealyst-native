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
//! // `Field` reports edits; the CALLER owns the write that re-renders the rows.
//! let on_query: Rc<dyn Fn(String)> = Rc::new(move |v| query.set(v));
//! let rows = rx!({
//!     let q = query.get().to_lowercase();
//!     options.iter().filter(|o| o.label.to_lowercase().contains(&q)).collect()
//! });
//! ui! {
//!     Menu(
//!         target = AnchorTarget::from(trigger),
//!         on_dismiss = Some(on_dismiss),
//!         header = Some(ui! { Field(value = query, on_change = on_query) }),
//!     ) {
//!         // …one MenuItem per surviving option, plus a "No matches" row
//!         // when the query filters everything out.
//!     }
//! }
//! ```
//!
//! A menu with either slot set preserves focus across row presses, so the
//! search box keeps its caret when a row is clicked.
//!
//! # A searchable SUBmenu
//!
//! `SubMenu` takes the same pair, as builders (its flyout mounts on hover, so
//! its contents are constructed per open — [`SubMenuSlot`]), and its `items`
//! take a LIVE list so a keystroke re-renders the rows without rebuilding the
//! panel the field sits on:
//!
//! ```ignore
//! let query = signal(String::new());
//! ui! {
//!     Menu(target = AnchorTarget::from(trigger), on_dismiss = Some(on_dismiss)) {
//!         SubMenu(
//!             label = "Move to…",
//!             // Built per open. The handler is built INSIDE the builder: an
//!             // `Rc` can't be moved in from the enclosing reactive scope,
//!             // while `query` — a `Copy` signal — can.
//!             header = Some(SubMenuSlot::new(move |_cx| {
//!                 let on_query: Rc<dyn Fn(String)> = Rc::new(move |v| query.set(v));
//!                 ui! { Field(value = query, on_change = on_query) }
//!             })),
//!             items = rx!({
//!                 let q = query.get().to_lowercase();
//!                 folders.iter().filter(|f| f.name.to_lowercase().contains(&q))
//!                     .map(|f| MenuEntry::new(f.name.clone(), on_move(f))).collect()
//!             }),
//!         )
//!     }
//! }
//! ```
//!
//! A slotted flyout also LATCHES once the pointer has been inside it, so
//! hover-out can't close the box you're typing in; it collapses on a row
//! pick, on Escape / an outside click, or when another submenu opens. A
//! SLOTLESS submenu is unchanged — pure hover, as before.

use std::cell::{Cell, RefCell};
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

/// The anchored panel surface shared by `Menu` and each `SubMenu` flyout,
/// with their optional pinned `header`/`footer` slots. Delegates to the
/// family-wide scrolling panel so a long item list (or a SubMenu with many
/// entries) scrolls within a height-capped panel instead of running off the
/// bottom of the viewport; passing `None` for both slots is the plain
/// scrolling panel, so a slotless menu or flyout is unchanged.
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

// =============================================================================
// SubMenu slots
// =============================================================================

/// Context handed to a [`SubMenu`] slot builder each time the flyout opens.
#[derive(Clone)]
pub struct SubMenuSlotCx {
    /// Collapse the flyout — the same path a picked row takes. A header
    /// field's "done" action or a footer's "Add ‹what you typed›" calls it
    /// once the host-side work is finished; without it a slot has no way to
    /// reach the flyout's open state, which the component owns.
    pub dismiss: Rc<dyn Fn()>,
}

/// A pinned flyout slot ([`SubMenuProps::header`] / [`SubMenuProps::footer`]):
/// a builder invoked with a [`SubMenuSlotCx`] each time the flyout opens.
///
/// A builder rather than a plain `Element` for the same reason
/// [`SubMenuProps::items`] is data: the flyout mounts conditionally, so its
/// contents are constructed fresh on every open and an `Element` can only be
/// mounted once. Mirrors `Autocomplete`'s slots and `Modal`'s content.
/// (`Menu`'s own slots ARE plain `Element`s — its panel is built once, with
/// the caller gating the mount.)
#[derive(Clone)]
pub struct SubMenuSlot(Rc<dyn Fn(SubMenuSlotCx) -> Element>);

impl SubMenuSlot {
    /// Build a slot from a closure:
    /// `SubMenuSlot::new(move |cx| ui! { … })`.
    pub fn new(build: impl Fn(SubMenuSlotCx) -> Element + 'static) -> Self {
        Self(Rc::new(build))
    }

    /// Invoke the builder for one flyout-open cycle.
    fn build(&self, cx: SubMenuSlotCx) -> Element {
        (self.0)(cx)
    }
}

// =============================================================================
// SubMenu flyout: level coordination + hover-out policy
// =============================================================================

/// The submenu flyout currently open on the menu level, if any.
///
/// A `Menu`'s children are constructed by the CALLER before `Menu` itself
/// runs, so a sibling `SubMenu` cannot be reached through `provide`/`inject`
/// (there is no ancestor scope to hang the context on by the time the rows
/// exist) — this thread-local is the level. One slot is enough because a
/// `SubMenu`'s rows are [`MenuEntry`] data, which cannot nest another
/// `SubMenu`: every submenu in a menu is a sibling on the same level.
///
/// Hover alone used to keep the "one open submenu at a time" invariant —
/// hovering a sibling means the pointer LEFT this row, which collapsed it.
/// A latching flyout ([`HoverLatch`]) deliberately stops obeying hover-out,
/// so the invariant needs an explicit holder: opening any flyout collapses
/// whichever one held the level.
///
/// Not a signal and not world context: the claim is made from hover
/// callbacks, which backends dispatch OUTSIDE `World::enter` (the same
/// constraint that parks `toast.rs`'s queue handle in a thread-local).
thread_local! {
    static OPEN_FLYOUT: RefCell<Option<OpenFlyout>> = const { RefCell::new(None) };
    static NEXT_SUBMENU_ID: Cell<u64> = const { Cell::new(0) };
}

/// The level's open flyout: who holds it, and how to collapse it.
struct OpenFlyout {
    id: u64,
    /// Liveness probe for `close`. The holder's component can unmount with
    /// its claim still standing (the parent menu was dismissed), and a stale
    /// kernel handle PANICS on write — so the closer only runs while the
    /// signal it writes is still alive.
    open: Signal<bool>,
    close: Rc<dyn Fn()>,
}

/// A fresh per-`SubMenu` identity for the level claim.
fn next_submenu_id() -> u64 {
    NEXT_SUBMENU_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    })
}

/// Make `id` the level's open flyout, collapsing whichever one held it.
/// Re-claiming for the same `id` (the pointer travelling trigger → flyout
/// re-enters `open_now`) leaves it alone.
fn claim_flyout_level(id: u64, open: Signal<bool>, close: Rc<dyn Fn()>) {
    // Swap FIRST, then close the displaced holder outside the borrow: its
    // `close` releases the level, and would otherwise both re-enter this
    // RefCell and clear the claim just made.
    let prev = OPEN_FLYOUT.with(|c| c.borrow_mut().replace(OpenFlyout { id, open, close }));
    if let Some(prev) = prev {
        if prev.id != id && prev.open.is_alive() {
            (prev.close)();
        }
    }
}

/// Hand the level back, if `id` still holds it. A no-op when another submenu
/// has already claimed it — that one is open and must not be cleared.
fn release_flyout_level(id: u64) {
    OPEN_FLYOUT.with(|c| {
        let mut slot = c.borrow_mut();
        if slot.as_ref().is_some_and(|f| f.id == id) {
            *slot = None;
        }
    });
}

/// Hover-out policy for one `SubMenu`'s flyout.
///
/// A slotless flyout collapses on hover-out after the grace — the desktop
/// standard, and the only way a pointer-driven nested menu tidies itself up.
/// A flyout carrying a slot cannot afford that: a slot is exactly where a
/// search field goes, and the pointer drifting off the panel mid-typing
/// would close the box under the user.
///
/// So a SLOTTED flyout LATCHES once the pointer has actually been inside it.
/// Before that it behaves normally (hover the trigger, drift away, it
/// collapses — no stray panel left behind); after it, hover-out stops
/// closing and the flyout collapses only on a row pick, Escape / an outside
/// click, or another submenu on the level opening
/// ([`claim_flyout_level`]).
struct HoverLatch {
    /// True for a flyout with a header or footer slot.
    latching: bool,
    engaged: Cell<bool>,
}

impl HoverLatch {
    fn new(latching: bool) -> Self {
        Self { latching, engaged: Cell::new(false) }
    }

    /// The pointer reached the flyout panel itself.
    fn engage(&self) {
        self.engaged.set(true);
    }

    /// The flyout closed — drop the latch so the next open starts fresh.
    fn reset(&self) {
        self.engaged.set(false);
    }

    /// Does a hover-out still schedule the collapse?
    fn closes_on_hover_out(&self) -> bool {
        !(self.latching && self.engaged.get())
    }
}

// =============================================================================
// SubMenu
// =============================================================================

// Reactive-by-default: `label` is already reactive; `items` is declared
// `Reactive<Vec<MenuEntry>>` by hand (`#[props]` skips `Vec`, and a bare
// `Vec` snapshot can't be filtered — see the field's docs); `side` is
// structural overlay positioning, kept bare via `#[prop(static)]`; the slots
// are ELEMENT-BUILDERS (the *children* category, reactive internally via the
// slot cx), `#[prop(static)]` for the same reason as `Autocomplete`'s.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct SubMenuProps {
    /// Trigger row label.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
    /// Flyout contents. Passed as reconstructable [`MenuEntry`] data (not
    /// composed children) because the flyout mounts conditionally — the
    /// `when`-gated builder must be able to rebuild it on each open.
    /// Selecting an entry runs its `on_select` and closes the flyout.
    ///
    /// A bare `Vec` is a snapshot taken when the flyout opens. Pass a LIVE
    /// list (`rx!(…)`) to filter the rows from a header field: the read then
    /// happens inside the flyout, so a keystroke re-renders the rows alone
    /// and leaves the panel — and the caret in the field — untouched. A
    /// `Vec` rebuilt by the caller's own scope instead would remount the
    /// whole menu on every character.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[schema(constraint = "reactive: static Vec<MenuEntry> or rx!")]
    pub items: Reactive<Vec<MenuEntry>>,
    /// Which side the flyout opens toward. Default `End` (right in LTR).
    // TODO(reactive-sweep): route `side` to anchored_overlay `.side()`
    // (structural positioning, not a style sink). Kept bare for now.
    #[prop(static)]
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub side: ElementSide,
    /// Optional slot pinned ABOVE the flyout's scrolling row area — it stays
    /// put while the rows scroll under it. A search field narrowing a long
    /// `items` list is the motivating case: the flyout holds the box, the
    /// CALLER owns the query signal and feeds the survivors back through a
    /// live `items`.
    ///
    /// Built per flyout-open with a [`SubMenuSlotCx`]. A flyout with either
    /// slot set also LATCHES once the pointer has been inside it, so hover
    /// can no longer collapse it under a typing user — see [`SubMenu`].
    #[prop(static)]
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub header: Option<SubMenuSlot>,
    /// Optional slot pinned BELOW the scrolling row area — see
    /// [`SubMenuProps::header`]. Typically a "Clear" / "Add ‹what you typed›"
    /// action that must stay reachable without scrolling to the end.
    #[prop(static)]
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub footer: Option<SubMenuSlot>,
}

impl Default for SubMenuProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            items: Reactive::Static(Vec::new()),
            side: ElementSide::End,
            header: None,
            footer: None,
        }
    }
}

/// One flyout row: the entry's optional checkbox and label in a pressable.
/// A checkable row keeps the flyout open — multi-select toggles stack
/// without re-hovering (the checkbox itself re-marks reactively).
fn flyout_row(entry: MenuEntry, close: Rc<dyn Fn()>) -> Element {
    let on_select = entry.on_select;
    let keep_open = entry.checked.is_some();
    let mut kids: Vec<Element> = Vec::with_capacity(2);
    if let Some(c) = entry.checked {
        kids.push(menu_checkbox(c));
    }
    kids.push(runtime_core::text(entry.label).into_element());
    runtime_core::pressable(kids, move || {
        (on_select)();
        if !keep_open {
            (close)();
        }
    })
    .with_style(|| StyleApplication::new(MenuItemRow::sheet()))
    .into_element()
}

/// The flyout's rows.
///
/// A STATIC `items` builds its rows once — no effect, the shape `SubMenu`
/// always had. A LIVE one goes through `switch`, which rebuilds the rows in
/// place WITHOUT touching the panel around them; that boundary is the whole
/// reason a header search field survives the list it filters (rebuilding the
/// panel would remount the field and drop its caret and focus).
fn flyout_rows(items: &Reactive<Vec<MenuEntry>>, close: Rc<dyn Fn()>) -> Vec<Element> {
    if items.is_static() {
        return items
            .get_untracked()
            .into_iter()
            .map(|e| flyout_row(e, close.clone()))
            .collect();
    }
    let identity = items.clone();
    let build = items.clone();
    vec![runtime_core::switch(
        // The row list's identity: its labels, in order. Cheap to compare,
        // and `switch`'s `PartialEq` dedup means a keystroke that doesn't
        // change the matches leaves the mounted rows alone. Reading a
        // reactive label here subscribes to it, so a label that changes
        // re-renders the list too.
        move || identity.get().iter().map(|e| e.label.get()).collect::<Vec<String>>(),
        move |_| {
            // Untracked: the identity read above is what this region tracks.
            // Re-reading the source tracked here would subscribe the region
            // twice, past its own dedup.
            let close = close.clone();
            runtime_core::fragment(
                build
                    .get_untracked()
                    .into_iter()
                    .map(|e| flyout_row(e, close.clone()))
                    .collect(),
            )
        },
    )]
}

/// The flyout's panel: the rows in the family's capped scroller, with the
/// optional slots pinned OUTSIDE it (so a header stays put while the rows
/// scroll) on a focus-preserving surface (so a row press can't blur a header
/// field mid-typing). Both come from the shared [`slotted_panel`].
///
/// Split out of the `when` builder because a reactive hole is opaque to the
/// element mirror — this is the seam the slot-pinning tests can reach.
fn flyout_panel(
    items: &Reactive<Vec<MenuEntry>>,
    header: Option<&SubMenuSlot>,
    footer: Option<&SubMenuSlot>,
    close: Rc<dyn Fn()>,
) -> Element {
    let cx = SubMenuSlotCx { dismiss: close.clone() };
    let header = header.map(|s| s.build(cx.clone()));
    let footer = footer.map(|s| s.build(cx));
    slotted_panel(flyout_rows(items, close), header, footer)
}

/// Renders a menu row with a trailing chevron whose nested flyout opens on
/// HOVER — the desktop/web standard for nested menus. Only one submenu per
/// level is open at a time: hovering a sibling row means the pointer has LEFT
/// this one, which collapses it (after a short grace) while the sibling
/// opens, and the sibling's open also claims the level explicitly (see
/// [`claim_flyout_level`]) so a latched flyout can't outlive it. The grace
/// bridges the gap between the trigger row and its flyout so moving the
/// pointer into the flyout doesn't dismiss it. Hovering off the row/flyout
/// collapses just this submenu; the parent Menu's catcher still closes the
/// whole menu on an outside click.
///
/// # Slots
///
/// `header` / `footer` pin an element above / below the scrolling rows, the
/// same surface `Menu` offers — a search field that narrows a long `items`
/// list is the motivating case. A slotted flyout LATCHES once the pointer
/// has been inside it: hover-out stops collapsing it, because a box you are
/// typing into must not close when the pointer drifts. It then closes on a
/// row pick, on Escape / an outside click, or when another submenu on the
/// level opens. Pair a slot with a LIVE `items` (`rx!`) — a plain `Vec` is
/// snapshotted at open and no query will move it.
///
/// Touch has no hover (`on_hover` is a no-op on iOS/Android), so the flyout
/// won't expand there — mobile menus are a separate consideration.
#[component]
pub fn SubMenu(props: SubMenuProps) -> Element {
    let open: Signal<bool> = signal(false);
    let trigger_ref: Ref<ViewHandle> = Ref::new();
    let items = props.items;
    let side = props.side;
    let header = props.header;
    let footer = props.footer;

    // Identity for the level claim, and the hover-out policy: a flyout that
    // carries a slot latches once the pointer has been inside it.
    let level_id = next_submenu_id();
    let latch = Rc::new(HoverLatch::new(header.is_some() || footer.is_some()));

    // Pending hover-out close. Stored (not detached) so a re-hover of the
    // trigger OR the flyout cancels it — the "hover intent" bridge. Owned by
    // this component scope; drops (cancelling any pending close) on unmount.
    let close_task: Rc<RefCell<Option<ScheduledTask>>> = Rc::new(RefCell::new(None));

    // The one collapse path, whatever asked for it (grace timer, row pick,
    // Escape, a slot's `dismiss`, a sibling claiming the level): drop the
    // pending close, drop the latch, hand the level back, close.
    let close: Rc<dyn Fn()> = {
        let ct = close_task.clone();
        let latch = latch.clone();
        Rc::new(move || {
            if let Some(mut t) = ct.borrow_mut().take() {
                t.cancel();
            }
            latch.reset();
            release_flyout_level(level_id);
            open.set(false);
        })
    };

    // Hover-in: cancel any pending close, take the level (collapsing any
    // other open flyout), and open immediately.
    let open_now = {
        let ct = close_task.clone();
        let close = close.clone();
        move || {
            if let Some(mut t) = ct.borrow_mut().take() {
                t.cancel();
            }
            claim_flyout_level(level_id, open, close.clone());
            open.set(true);
        }
    };
    // Hover-out: collapse after the grace, unless re-hovered first — or
    // unless the flyout has latched (a slot is being used).
    let schedule_close = {
        let ct = close_task.clone();
        let latch = latch.clone();
        let close = close.clone();
        move || {
            if !latch.closes_on_hover_out() {
                return;
            }
            if let Some(mut t) = ct.borrow_mut().take() {
                t.cancel();
            }
            let close = close.clone();
            let task = after_ms(SUBMENU_HOVER_GRACE_MS, move || (close)());
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
    // there) without the grace timer collapsing it; reaching the panel is
    // also what engages a slotted flyout's latch.
    let flyout = runtime_core::when(
        move || open.get(),
        {
            let items = items.clone();
            let header = header.clone();
            let footer = footer.clone();
            let close = close.clone();
            let latch = latch.clone();
            let open_now = open_now.clone();
            let schedule_close = schedule_close.clone();
            move || {
                let panel =
                    flyout_panel(&items, header.as_ref(), footer.as_ref(), close.clone());
                let on_enter = open_now.clone();
                let on_leave = schedule_close.clone();
                let latch = latch.clone();
                let panel_view = runtime_core::view(vec![panel])
                    .on_hover(move |entering| {
                        if entering {
                            latch.engage();
                            on_enter();
                        } else {
                            on_leave();
                        }
                    })
                    .into_element();
                let dismiss = close.clone();
                runtime_core::anchored_overlay(AnchorTarget::from(trigger_ref), vec![panel_view])
                    .side(side)
                    .align(ElementAlign::Start)
                    .offset(2.0)
                    .backdrop(BackdropMode::None)
                    .trap_focus(false)
                    .on_dismiss(move || (dismiss)())
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

    // =========================================================================
    // SubMenu slots
    // =========================================================================

    /// A `SubMenu`'s `header`/`footer` slots must reach the flyout panel
    /// PINNED — direct panel children around the scrolling row area — and the
    /// panel must preserve focus, exactly as `Menu`'s do. That pair is what
    /// makes a search field on a flyout usable: it stays on screen while the
    /// rows it filters scroll under it, and a row press doesn't blur it. If a
    /// refactor routes the flyout back through the slotless panel the header
    /// silently disappears.
    #[test]
    fn submenu_slots_reach_the_flyout_panel_pinned_around_the_scroller() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());

            let noop: Rc<dyn Fn()> = Rc::new(|| {});
            let items = Reactive::Static(vec![MenuEntry::new("Inbox", noop.clone())]);
            let panel = flyout_panel(
                &items,
                Some(&SubMenuSlot::new(|_cx| {
                    runtime_core::text("Search".to_string()).into_element()
                })),
                Some(&SubMenuSlot::new(|_cx| {
                    runtime_core::text("Clear".to_string()).into_element()
                })),
                noop,
            );

            let P::View { preserves_focus, mut children, .. } = classify(panel) else {
                panic!("the flyout panel is a View (the SelectMenu surface)");
            };
            assert!(
                preserves_focus,
                "a slotted flyout preserves focus so a row press can't blur the header field"
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

    /// A slotless flyout is byte-for-byte what it always was: one scroller
    /// child, no focus-preservation. The slots are additive — `SubMenu`'s
    /// existing hover-only behaviour must not pick up a focus mark it never
    /// had.
    #[test]
    fn a_slotless_flyout_panel_is_unchanged() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());

            let noop: Rc<dyn Fn()> = Rc::new(|| {});
            let items = Reactive::Static(vec![MenuEntry::new("Inbox", noop.clone())]);
            let panel = flyout_panel(&items, None, None, noop);

            let P::View { preserves_focus, children, .. } = classify(panel) else {
                panic!("the flyout panel is a View (the SelectMenu surface)");
            };
            assert!(!preserves_focus, "a slotless flyout keeps its original focus behaviour");
            assert_eq!(children.len(), 1, "the panel holds a single scroller child");
    });
    }

    /// A slot builder gets a `dismiss` that actually collapses the flyout.
    /// Without it a slot can't reach the open state (the component owns it),
    /// so an "Add ‹query›" footer would leave the flyout hanging open after
    /// doing its work.
    #[test]
    fn a_slot_builder_receives_a_dismiss_that_closes_the_flyout() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());

            let closed = Rc::new(Cell::new(false));
            let flag = closed.clone();
            let close: Rc<dyn Fn()> = Rc::new(move || flag.set(true));
            let items: Reactive<Vec<MenuEntry>> = Reactive::Static(Vec::new());
            let panel = flyout_panel(
                &items,
                None,
                Some(&SubMenuSlot::new(|cx| {
                    let dismiss = cx.dismiss.clone();
                    runtime_core::pressable(
                        vec![runtime_core::text("Add".to_string()).into_element()],
                        move || (dismiss)(),
                    )
                    .into_element()
                })),
                close,
            );

            let P::View { mut children, .. } = classify(panel) else {
                panic!("the flyout panel is a View");
            };
            let footer = children.remove(1);
            let P::Pressable { on_click, .. } = classify(footer) else {
                panic!("the footer slot built a pressable");
            };
            assert!(!closed.get(), "building the slot must not close anything");
            (on_click)();
            assert!(closed.get(), "the slot's `cx.dismiss` collapses the flyout");
    });
    }

    /// The point of a live `items`: the read must happen INSIDE the flyout's
    /// reactive region, not at build time. A snapshot taken while building
    /// the rows can never move again, so a header field would filter nothing
    /// — the bug this guards. A STATIC list keeps the original build-once
    /// shape (no reactive hole, no effect).
    #[test]
    fn a_live_items_list_is_read_inside_a_reactive_region_not_snapshotted() {
        with_test_world(|| {
            let noop: Rc<dyn Fn()> = Rc::new(|| {});

            let reads = Rc::new(Cell::new(0));
            let counted = reads.clone();
            let entry = noop.clone();
            let live: Reactive<Vec<MenuEntry>> = Reactive::derive(move || {
                counted.set(counted.get() + 1);
                vec![MenuEntry::new("Inbox", entry.clone())]
            });
            let rows = flyout_rows(&live, noop.clone());
            assert_eq!(rows.len(), 1, "a live list contributes one reactive region");
            assert!(
                matches!(classify(rows.into_iter().next().expect("one region")), P::Other(_)),
                "the rows sit in a reactive hole, so a keystroke re-renders them alone"
            );
            assert_eq!(
                reads.get(),
                0,
                "the list must not be read while building — that would freeze the rows"
            );

            let stat = Reactive::Static(vec![
                MenuEntry::new("Inbox", noop.clone()),
                MenuEntry::new("Archive", noop.clone()),
            ]);
            let rows = flyout_rows(&stat, noop);
            assert_eq!(rows.len(), 2, "a static list still builds its rows directly");
    });
    }

    /// Picking a plain row closes the flyout; picking a CHECKABLE one does
    /// not — multi-select toggles stack without re-hovering.
    #[test]
    fn a_checkable_row_keeps_the_flyout_open_and_a_plain_row_closes_it() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());

            let closed = Rc::new(Cell::new(false));
            let flag = closed.clone();
            let close: Rc<dyn Fn()> = Rc::new(move || flag.set(true));
            let noop: Rc<dyn Fn()> = Rc::new(|| {});

            let P::Pressable { on_click, .. } =
                classify(flyout_row(MenuEntry::checkable("Starred", true, noop.clone()), close.clone()))
            else {
                panic!("a flyout row is a pressable");
            };
            (on_click)();
            assert!(!closed.get(), "a checkable row keeps the flyout open for the next toggle");

            let P::Pressable { on_click, .. } =
                classify(flyout_row(MenuEntry::new("Inbox", noop), close))
            else {
                panic!("a flyout row is a pressable");
            };
            (on_click)();
            assert!(closed.get(), "a plain row closes the flyout after running its handler");
    });
    }

    // =========================================================================
    // SubMenu hover-out policy + level coordination
    // =========================================================================

    /// A SLOTLESS flyout always collapses on hover-out — the desktop
    /// standard, and the only way a pointer-driven nested menu tidies itself
    /// up. Entering the panel must not make it sticky.
    #[test]
    fn a_slotless_flyout_always_collapses_on_hover_out() {
        let latch = HoverLatch::new(false);
        assert!(latch.closes_on_hover_out());
        latch.engage();
        assert!(latch.closes_on_hover_out(), "no slot, no latch — hover still rules");
    }

    /// Regression: a slotted flyout must stop collapsing on hover-out once
    /// the pointer has been inside it. Without the latch, a search field in
    /// the header closes under the user the moment the pointer drifts off the
    /// panel mid-typing — the reason SubMenu had no slots at all before.
    /// Before the pointer ever reaches the flyout it still auto-collapses, so
    /// merely brushing the trigger leaves no stray panel behind; and a close
    /// resets the latch, so the next open starts fresh.
    #[test]
    fn regression_a_slotted_flyout_latches_once_the_pointer_reaches_it() {
        let latch = HoverLatch::new(true);
        assert!(
            latch.closes_on_hover_out(),
            "brushing the trigger without reaching the flyout still collapses it"
        );
        latch.engage();
        assert!(
            !latch.closes_on_hover_out(),
            "a pointer that drifts off the panel must not close the box being typed in"
        );
        latch.reset();
        assert!(latch.closes_on_hover_out(), "closing drops the latch for the next open");
    }

    /// Only one submenu per level is open at a time. Hover alone used to
    /// carry that (hovering a sibling means the pointer LEFT this row), but a
    /// latched flyout ignores hover-out — so opening any flyout must
    /// explicitly collapse whichever one held the level, or two panels
    /// overlap.
    #[test]
    fn opening_a_flyout_collapses_the_one_that_held_the_level() {
        with_test_world(|| {
            let first_closed = Rc::new(Cell::new(0));
            let second_closed = Rc::new(Cell::new(0));
            let a = first_closed.clone();
            let b = second_closed.clone();
            let first: Rc<dyn Fn()> = Rc::new(move || a.set(a.get() + 1));
            let second: Rc<dyn Fn()> = Rc::new(move || b.set(b.get() + 1));
            let open_a = signal(true);
            let open_b = signal(true);

            claim_flyout_level(1, open_a, first.clone());
            assert_eq!(first_closed.get(), 0, "the first claim closes nothing");

            // Re-hovering the same submenu (trigger → flyout) re-claims: it
            // must not close itself.
            claim_flyout_level(1, open_a, first.clone());
            assert_eq!(first_closed.get(), 0, "re-claiming for the same submenu is a no-op");

            claim_flyout_level(2, open_b, second);
            assert_eq!(first_closed.get(), 1, "a sibling opening collapses the held flyout");
            assert_eq!(second_closed.get(), 0);

            // The displaced submenu releasing (its own close path ran) must
            // not clear the level the sibling now holds.
            release_flyout_level(1);
            claim_flyout_level(3, signal(true), Rc::new(|| {}));
            assert_eq!(second_closed.get(), 1, "the level still named the sibling");

            release_flyout_level(3);
            claim_flyout_level(4, signal(true), Rc::new(|| {}));
            assert_eq!(
                second_closed.get(),
                1,
                "a released level holds nobody — the next open closes nothing"
            );
    });
    }
}
