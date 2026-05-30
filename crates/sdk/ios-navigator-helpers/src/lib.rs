// The entire crate is iOS-only; on non-iOS targets it compiles to
// an empty rlib so `cargo check --workspace` succeeds without
// dragging UIKit / objc2 into scope. Per-SDK crates already
// cfg-gate their `mod ios` references to `target_os = "ios"`, so
// nothing host-side touches this module.
#![cfg(target_os = "ios")]

//! Shared iOS-side machinery for the three first-party navigator SDKs
//! (stack / tab / drawer).
//!
//! # Model
//!
//! - **Stack**: backed by a `UINavigationController`. Push / pop /
//!   replace / reset hit the UIKit nav controller directly; a
//!   delegate observes interactive pops (swipe-back, system back
//!   chevron) and reconciles the rust-side stack against the
//!   controller's actual depth.
//! - **Tabs**: a plain `UIView` body that swaps its single child
//!   on `Select`. Tab chrome is rendered by author `.layout(...)`.
//! - **Drawer**: a plain `UIView` body wrapped in a self-owned
//!   `UINavigationController` (so the drawer has a native header
//!   regardless of whether it's nested in a parent stack), plus
//!   a sidebar `UIView` that slides in from the leading edge.
//!
//! # Substrate boundary
//!
//! The framework's navigator substrate (runtime-core) owns the
//! kind-agnostic command vocabulary, the per-screen scope mechanics,
//! and the reactive `NavState`. Everything kind-specific — chrome,
//! typed handles, the dispatcher mapping from `NavCommand` to native
//! action — lives in the SDK crates. This helper crate is the SDK-side
//! shared engine that all three first-party iOS SDKs (stack-navigator,
//! tab-navigator, drawer-navigator) call into for UIKit glue.

mod chrome;
mod stack;
mod tab_drawer;

use backend_ios::IosNode;
use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSObject};
use runtime_core::primitives::navigator::{
    MountResult, NavState, NavigatorControl, NavigatorHandle, NavigatorOps,
};
use runtime_core::{Color, Signal};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub use chrome::{apply_header_options, apply_header_options_with_nav};

// ---------------------------------------------------------------------------
// Local callback bundle types
// ---------------------------------------------------------------------------
//
// Mirrors the shape of the OLD `NavigatorCallbacks<N>` etc. that lived
// in runtime-core before the substrate refactor. Each SDK crate fills
// one of these in and passes it to `create_stack` / `create_tab` /
// `create_drawer`.

/// Kind-agnostic navigator callbacks. Every iOS SDK passes one of
/// these; the tab and drawer variants embed it.
pub struct IosNavCallbacks {
    pub initial_route: &'static str,
    pub initial_path: &'static str,
    pub mount_screen:
        Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<IosNode>>,
    pub release_screen: Rc<dyn Fn(u64)>,
    pub depth_changed: Rc<dyn Fn(usize)>,
    pub nav_state: NavState,
    pub defer_initial_mount: bool,
}

/// Tab-navigator-specific callbacks. The iOS engine treats tabs as
/// "screen-swap with author chrome" — the registrations are kept for
/// SDK-side decisions (active highlighting, route mapping) but the
/// helper itself only needs `placement` + `mount_policy` to wire the
/// outlet, and `active_changed` to notify the SDK when the active tab
/// changes.
pub struct IosTabCallbacks {
    pub navigator: IosNavCallbacks,
    pub tabs: Vec<TabRegistration>,
    pub placement: TabPlacement,
    pub mount_policy: MountPolicy,
    pub active_changed: Rc<dyn Fn(&'static str)>,
}

/// Drawer-navigator-specific callbacks. Same screen-swap engine as
/// tabs, plus the drawer chrome (sidebar, scrim, animation) and the
/// open-state signal the drawer SDK exposes through `DrawerHandle`.
pub struct IosDrawerCallbacks {
    pub navigator: IosNavCallbacks,
    pub side: DrawerSide,
    pub drawer_type: DrawerType,
    pub drawer_width: f32,
    pub swipe_to_open: bool,
    pub mount_policy: MountPolicy,
    pub is_open: Signal<bool>,
    /// Deferred sidebar builder — invoked from a `schedule_microtask`
    /// after `create_drawer` returns. `None` ⇒ no sidebar is built.
    pub build_content: Option<Rc<dyn Fn() -> IosNode>>,
    pub active_changed: Rc<dyn Fn(&'static str)>,
    pub open_changed: Rc<dyn Fn(bool)>,
    pub background_color: Option<Rc<dyn Fn() -> Color>>,
}

// ---------------------------------------------------------------------------
// Local kind-specific enums + structs
// ---------------------------------------------------------------------------
//
// These used to live in runtime-core but are SDK-side concepts after
// the substrate refactor. They live here so each iOS SDK doesn't have
// to redeclare them — the three first-party SDKs share this helper
// crate and these definitions.

/// Where the tab bar lives relative to the screen content. Currently
/// informational from the helper's standpoint; the author's
/// `.layout(...)` closure owns actual positioning.
#[derive(Clone, Copy, Debug)]
pub enum TabPlacement {
    Auto,
    Top,
    Bottom,
    Sidebar,
}

/// When to materialize a screen's subtree relative to navigation, and
/// what happens to it on switch.
///
/// - `EagerPersistent`: mount at navigator creation time, keep across
///   switches.
/// - `LazyPersistent`: mount on first activation, keep across switches.
/// - `LazyDisposing`: mount on first activation, tear down on switch.
#[derive(Clone, Copy, Debug)]
pub enum MountPolicy {
    EagerPersistent,
    LazyPersistent,
    LazyDisposing,
}

/// Which side of the screen the drawer slides in from.
#[derive(Clone, Copy, Debug)]
pub enum DrawerSide {
    Start,
    End,
}

/// Visual presentation style for the drawer chrome.
#[derive(Clone, Copy, Debug)]
pub enum DrawerType {
    /// Slides over the content; backdrop dims the content.
    Front,
    /// Pushes the content sideways; no backdrop.
    Slide,
}

/// Identifier + display metadata for a single tab. Currently
/// informational on iOS — the helper itself doesn't render tabs
/// (authors build their own tab bar via the layout closure).
pub struct TabRegistration {
    pub route: &'static str,
    pub label: String,
    pub icon: Option<String>,
    pub badge: Option<Rc<dyn Fn() -> String>>,
}

/// Icon-based header bar button. SDK callers translate their own
/// `BarButton` into this shape before passing into `attach_initial`.
#[derive(Clone)]
pub struct BarButton {
    pub icon: String,
    pub on_press: Rc<dyn Fn()>,
    pub tint: Option<Color>,
}

/// Per-screen iOS header chrome options. The SDK iOS handler
/// translates its kind-specific options (`StackScreenOptions`,
/// `DrawerScreenOptions`) into this shape, then passes it through to
/// `attach_initial`. Color fields stay as closures so the per-VC
/// re-tint Effect can re-resolve them on theme swap.
#[derive(Default, Clone)]
pub struct IosScreenOptions {
    pub title: Option<String>,
    pub header_shown: Option<bool>,
    pub header_left: Option<BarButton>,
    pub header_right: Option<BarButton>,
    pub header_background: Option<Rc<dyn Fn() -> Color>>,
    pub header_tint: Option<Rc<dyn Fn() -> Color>>,
    pub title_color: Option<Rc<dyn Fn() -> Color>>,
    /// Per-screen override of the navigator's [`IosNavCallbacks::mount_policy`].
    /// `None` defers to the navigator-global default. When the SDK's
    /// per-screen `mount_policy` builder is used, the platform handler
    /// fills this so `select_screen` can branch on it for cache-vs-dispose.
    pub mount_policy: Option<MountPolicy>,
}

/// Drawer-specific commands ridden across the substrate's
/// `NavCommand::Custom` channel. The drawer SDK builds one of these
/// inside an `Rc<dyn Any>`, dispatches it, and the helper's dispatcher
/// downcasts to drive open/close/toggle.
#[derive(Clone, Copy, Debug)]
pub enum DrawerCmd {
    Open,
    Close,
    Toggle,
}

// ---------------------------------------------------------------------------
// Per-instance state stored in thread-locals
// ---------------------------------------------------------------------------
//
// Mirrors the web helpers crate's `NAVIGATOR_INSTANCES`. Keyed by the
// container view's pointer (the same `view_key()` the framework uses).

thread_local! {
    pub(crate) static STACK_INSTANCES:
        RefCell<HashMap<usize, Rc<RefCell<stack::StackEntry>>>> =
        RefCell::new(HashMap::new());
    pub(crate) static TAB_DRAWER_INSTANCES:
        RefCell<HashMap<usize, Rc<RefCell<tab_drawer::TabDrawerEntry>>>> =
        RefCell::new(HashMap::new());
    /// Retained ObjC objects (callback targets, gesture-recognizer
    /// targets, NSTimer scheduling targets) the helpers need to keep
    /// alive past the helpers' construction calls. Mirrors
    /// `IosBackend.callback_targets` from before the refactor.
    pub(crate) static CALLBACK_TARGETS: RefCell<Vec<Retained<NSObject>>> =
        RefCell::new(Vec::new());
}

/// Push a retained ObjC object onto the thread-local lifetime anchor.
/// Used by the helpers to keep callback targets alive past local
/// scope (UIKit holds them weakly via `setTarget:`).
pub(crate) fn retain_target(obj: Retained<NSObject>) {
    CALLBACK_TARGETS.with(|t| t.borrow_mut().push(obj));
}

// ---------------------------------------------------------------------------
// Public API — mirrors web-navigator-helpers
// ---------------------------------------------------------------------------

/// Stack navigator entry point. Builds a `UINavigationController`,
/// installs the per-instance dispatcher on `control`, registers a
/// delegate that observes interactive pops, and stashes per-instance
/// state in the thread-local registry.
pub fn create_stack(
    mtm: MainThreadMarker,
    callbacks: IosNavCallbacks,
    control: Rc<NavigatorControl>,
) -> IosNode {
    stack::create(mtm, callbacks, control)
}

/// Tab navigator entry point. Builds a plain `UIView` body, installs
/// the per-instance `Select`-dispatcher on `control`, and stashes
/// per-instance state in the thread-local registry.
pub fn create_tab(
    mtm: MainThreadMarker,
    callbacks: IosTabCallbacks,
    control: Rc<NavigatorControl>,
) -> IosNode {
    tab_drawer::create_tab(mtm, callbacks, control)
}

/// Drawer navigator entry point. Builds the outer container + scrim +
/// embedded `UINavigationController` (for the header bar), installs
/// the per-instance dispatcher on `control`, and stashes per-instance
/// state in the thread-local registry. If `build_content` is
/// supplied, it's invoked from a `schedule_microtask` shortly after
/// this returns to materialize the sidebar.
pub fn create_drawer(
    mtm: MainThreadMarker,
    callbacks: IosDrawerCallbacks,
    control: Rc<NavigatorControl>,
) -> IosNode {
    tab_drawer::create_drawer(mtm, callbacks, control)
}

/// Attach the framework-realized initial stack screen. Wraps the
/// screen view in a fresh `UIViewController`, sets it as the nav
/// controller's only view controller, and applies the per-screen
/// header chrome.
pub fn stack_attach_initial(
    mtm: MainThreadMarker,
    navigator: &IosNode,
    screen: IosNode,
    scope_id: u64,
    options: &IosScreenOptions,
) {
    stack::attach_initial(mtm, navigator, screen, scope_id, options);
}

/// Attach the framework-realized initial tab screen. Mounts directly
/// into the body without going through `Select` (no animation, no
/// auto-close, etc.).
pub fn tab_attach_initial(
    navigator: &IosNode,
    screen: IosNode,
    scope_id: u64,
) {
    tab_drawer::tab_attach_initial(navigator, screen, scope_id);
}

/// Attach the framework-realized initial drawer screen. Same shape
/// as `tab_attach_initial`, plus per-screen header chrome (drawer
/// owns its own `UINavigationController`).
pub fn drawer_attach_initial(
    mtm: MainThreadMarker,
    navigator: &IosNode,
    screen: IosNode,
    scope_id: u64,
    options: &IosScreenOptions,
) {
    tab_drawer::drawer_attach_initial(mtm, navigator, screen, scope_id, options);
}

/// Attach the deferred-built sidebar UIView to the drawer. Called
/// from the SDK handler's microtask after `host.build_node`
/// materializes the SDK's sidebar Element into an `IosNode`.
pub fn drawer_attach_sidebar(
    mtm: MainThreadMarker,
    navigator: &IosNode,
    sidebar: IosNode,
) {
    tab_drawer::drawer_attach_sidebar(mtm, navigator, sidebar);
}

/// Tear down a stack navigator: drop the per-instance entry from the
/// thread-local registry, which releases the `UINavigationController`,
/// the delegate, and every still-mounted screen scope. The framework
/// has already called `release_screen` for any screens it owns; this
/// path is just the UIKit + retainer cleanup.
pub fn release_stack(node: &IosNode) {
    STACK_INSTANCES.with(|m| {
        m.borrow_mut().remove(&node.view_key());
    });
}

/// Tear down a tab or drawer navigator. Same shape as `release_stack`
/// but on a different thread-local.
pub fn release_tab_drawer(node: &IosNode) {
    TAB_DRAWER_INSTANCES.with(|m| {
        m.borrow_mut().remove(&node.view_key());
    });
}

/// Build a `NavigatorHandle` for the stack navigator identified by
/// `node`. SDK crates wrap this in their own typed handle
/// (`StackHandle`) that exposes the kind-specific methods. Returns an
/// inert (no-control) handle when `node` isn't a registered navigator.
pub fn make_stack_handle(node: &IosNode) -> NavigatorHandle {
    let control = STACK_INSTANCES.with(|m| {
        m.borrow()
            .get(&node.view_key())
            .map(|e| e.borrow().control.clone())
    });
    match control {
        Some(c) => NavigatorHandle::with_control(Rc::new(()), &IOS_NAV_OPS, c),
        None => NavigatorHandle::new(Rc::new(()), &IOS_NAV_OPS),
    }
}

/// Build a `NavigatorHandle` for the tab navigator identified by
/// `node`. Same shape as `make_stack_handle`.
pub fn make_tab_handle(node: &IosNode) -> NavigatorHandle {
    make_tab_drawer_handle(node)
}

/// Build a `NavigatorHandle` for the drawer navigator identified by
/// `node`. Same shape as `make_stack_handle`.
pub fn make_drawer_handle(node: &IosNode) -> NavigatorHandle {
    make_tab_drawer_handle(node)
}

fn make_tab_drawer_handle(node: &IosNode) -> NavigatorHandle {
    let control = TAB_DRAWER_INSTANCES.with(|m| {
        m.borrow()
            .get(&node.view_key())
            .map(|e| e.borrow().control.clone())
    });
    match control {
        Some(c) => NavigatorHandle::with_control(Rc::new(()), &IOS_NAV_OPS, c),
        None => NavigatorHandle::new(Rc::new(()), &IOS_NAV_OPS),
    }
}

struct IosNavigatorOps;
impl NavigatorOps for IosNavigatorOps {}
static IOS_NAV_OPS: IosNavigatorOps = IosNavigatorOps;

// ---------------------------------------------------------------------------
// Slot styling — stack header / title / button; drawer sidebar
// ---------------------------------------------------------------------------

/// Apply the stack navigator's "header" slot style: the
/// `UINavigationBar` background color (and the nav view background +
/// top VC view background so the themed color fills behind the bar /
/// status area / home indicator).
pub fn apply_stack_header_style(
    navigator: &IosNode,
    style: &Rc<runtime_core::StyleRules>,
) {
    let entry = STACK_INSTANCES.with(|m| m.borrow().get(&navigator.view_key()).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();
    chrome::apply_nav_header_style(&entry.controller, navigator.as_view(), style);
}

/// Apply the stack navigator's "title" slot style: the
/// `UINavigationBar` titleTextAttributes (color + font).
pub fn apply_stack_title_style(
    navigator: &IosNode,
    style: &Rc<runtime_core::StyleRules>,
) {
    let entry = STACK_INSTANCES.with(|m| m.borrow().get(&navigator.view_key()).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();
    chrome::apply_nav_title_style(&entry.controller, style);
}

/// Apply the stack navigator's "button" slot style: the
/// `UINavigationBar` tintColor.
pub fn apply_stack_button_style(
    navigator: &IosNode,
    style: &Rc<runtime_core::StyleRules>,
) {
    let entry = STACK_INSTANCES.with(|m| m.borrow().get(&navigator.view_key()).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();
    chrome::apply_nav_button_style(&entry.controller, style);
}

/// Apply the stack navigator's "body" slot style: the
/// `UINavigationController`'s root `view.backgroundColor`. The stack's
/// screen-outlet IS that view (push/pop swap child VCs inside it), so
/// painting it here gives `HeaderStyle.body_background` the same
/// behavior as Android's `apply_body_style` and the drawer's
/// `apply_drawer_body_style`.
pub fn apply_stack_body_style(
    navigator: &IosNode,
    style: &Rc<runtime_core::StyleRules>,
) {
    let entry = STACK_INSTANCES.with(|m| m.borrow().get(&navigator.view_key()).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();
    let Some(view) = entry.controller.view() else { return };
    if let Some(ref bg) = style.background {
        let bg_val = bg.resolve();
        let c = backend_ios_core::style::color_to_uicolor(&bg_val);
        view.setBackgroundColor(Some(&c));
    }
}

/// Apply the drawer navigator's "sidebar" slot style: the sidebar
/// UIView's `backgroundColor`.
pub fn apply_drawer_sidebar_style(
    navigator: &IosNode,
    style: &Rc<runtime_core::StyleRules>,
) {
    let entry =
        TAB_DRAWER_INSTANCES.with(|m| m.borrow().get(&navigator.view_key()).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();
    let Some(ref sidebar) = *entry.sidebar.borrow() else {
        return;
    };
    if let Some(ref bg) = style.background {
        let bg_val = bg.resolve();
        let c = backend_ios_core::style::color_to_uicolor(&bg_val);
        sidebar.setBackgroundColor(Some(&c));
    }
}

/// Apply the drawer/tab navigator's "body" slot style: the
/// screen-outlet UIView's `backgroundColor`. Mirrors Android's
/// `apply_body_style` so that `HeaderStyle.body_background` paints
/// the active-screen container's background uniformly across
/// backends (rule 7 — backend implementations diverge in mechanism
/// but converge in observable behavior).
pub fn apply_drawer_body_style(
    navigator: &IosNode,
    style: &Rc<runtime_core::StyleRules>,
) {
    let entry =
        TAB_DRAWER_INSTANCES.with(|m| m.borrow().get(&navigator.view_key()).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();
    if let Some(ref bg) = style.background {
        let bg_val = bg.resolve();
        let c = backend_ios_core::style::color_to_uicolor(&bg_val);
        entry.body.setBackgroundColor(Some(&c));
    }
}

/// Recover a `&UINavigationController` from the type-erased
/// `Retained<NSObject>` the drawer entry stores. The entry struct
/// hides the concrete type so it can be shared across navigator
/// kinds; we constructed it as a `UINavigationController` in
/// `create_drawer`, so this cast is sound.
fn drawer_nav_ctrl(
    obj: &Retained<NSObject>,
) -> &objc2_ui_kit::UINavigationController {
    // SAFETY: `header_nav_ctrl` is stored as NSObject only to keep the
    // entry struct uniform across navigator kinds. The pointer was
    // populated from a real `UINavigationController::new(mtm)` in
    // `create_drawer`, so this pointer-cast back is sound.
    unsafe {
        &*(Retained::as_ptr(obj) as *const objc2_ui_kit::UINavigationController)
    }
}

/// Apply the drawer navigator's "header" slot style: the embedded
/// `UINavigationController`'s nav-bar background. Mirrors
/// `apply_stack_header_style` — the drawer wraps its body in a
/// self-owned `UINavigationController`, so the same chrome helpers
/// work.
pub fn apply_drawer_header_style(
    navigator: &IosNode,
    style: &Rc<runtime_core::StyleRules>,
) {
    let entry =
        TAB_DRAWER_INSTANCES.with(|m| m.borrow().get(&navigator.view_key()).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();
    let Some(ref nav_obj) = entry.header_nav_ctrl else { return };
    let nav_ctrl = drawer_nav_ctrl(nav_obj);
    let Some(nav_view) = nav_ctrl.view() else { return };
    chrome::apply_nav_header_style(nav_ctrl, &nav_view, style);
}

/// Apply the drawer navigator's "title" slot style: title color +
/// font on the embedded `UINavigationController`'s nav bar.
pub fn apply_drawer_title_style(
    navigator: &IosNode,
    style: &Rc<runtime_core::StyleRules>,
) {
    let entry =
        TAB_DRAWER_INSTANCES.with(|m| m.borrow().get(&navigator.view_key()).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();
    let Some(ref nav_obj) = entry.header_nav_ctrl else { return };
    chrome::apply_nav_title_style(drawer_nav_ctrl(nav_obj), style);
}

/// Apply the drawer navigator's "button" slot style: tint color on
/// the embedded `UINavigationController`'s nav bar (back chevron +
/// bar-button items, including the hamburger).
pub fn apply_drawer_button_style(
    navigator: &IosNode,
    style: &Rc<runtime_core::StyleRules>,
) {
    let entry =
        TAB_DRAWER_INSTANCES.with(|m| m.borrow().get(&navigator.view_key()).cloned());
    let Some(entry) = entry else { return };
    let entry = entry.borrow();
    let Some(ref nav_obj) = entry.header_nav_ctrl else { return };
    chrome::apply_nav_button_style(drawer_nav_ctrl(nav_obj), style);
}
