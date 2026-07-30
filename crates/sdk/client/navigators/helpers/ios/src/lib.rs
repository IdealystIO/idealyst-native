// The entire crate is iOS-only; on non-iOS targets it compiles to
// an empty rlib so `cargo check --workspace` succeeds without
// dragging UIKit / objc2 into scope. Per-SDK crates already
// cfg-gate their `mod ios` references to `target_os = "ios"`, so
// nothing host-side touches this module.
#![cfg(target_os = "ios")]

//! iOS `UINavigationController` stack engine for the outlet-model
//! stack navigator (`stack-navigator`'s `src/ios.rs`).
//!
//! **Internal — not author-facing.** Per-platform glue, not a public
//! API; the stack SDK pulls this crate in only on `target_os = "ios"`.
//! The whole crate is `#![cfg(target_os = "ios")]`, so on other hosts
//! it compiles to an empty rlib (keeping `cargo check --workspace`
//! free of UIKit / objc2).
//!
//! # Model
//!
//! A `UINavigationController` seated inside the author layout's
//! outlet. Push / pop / replace / reset hit the UIKit nav controller
//! directly; a delegate observes interactive pops (swipe-back, system
//! back chevron) and reconciles the rust-side stack against the
//! controller's actual depth. The native bar is hidden by the SDK
//! (`header_shown: Some(false)`) — chrome is the author `StackHeader`.
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

use backend_ios::IosNode;
use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSObject};
use runtime_shared::primitives::navigator::{
    MountResult, NavState, NavigatorControl, NavigatorHandle, NavigatorOps,
};
use runtime_shared::Color;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub use chrome::{apply_header_options, apply_header_options_with_nav};

// ---------------------------------------------------------------------------
// Local callback bundle types
// ---------------------------------------------------------------------------
//
// Mirrors the shape of the OLD `NavigatorCallbacks<N>` that lived in
// runtime-core before the substrate refactor; the stack SDK fills one
// in and passes it to `create_stack`.

/// Navigator callbacks the stack SDK hands the engine.
pub struct IosNavCallbacks {
    pub initial_route: &'static str,
    pub initial_path: &'static str,
    pub mount_screen:
        Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<IosNode>>,
    pub release_screen: Rc<dyn Fn(u64)>,
    pub depth_changed: Rc<dyn Fn(usize)>,
    pub nav_state: NavState,
    pub defer_initial_mount: bool,
    /// Fired with the TOP screen's `scope_id` after every transition
    /// that changes which screen is visible — push/pop/replace/reset,
    /// the initial attach, deep-link back-stack reconstruction, AND
    /// UIKit-initiated pops (swipe-back / back-chevron, via the
    /// `didShow` delegate). The outlet-model stack (`stack-navigator`)
    /// uses it to publish the revealed screen's author-header state
    /// (`screen_chrome`).
    pub top_changed: Option<Rc<dyn Fn(u64)>>,
}

// ---------------------------------------------------------------------------
// Local kind-specific enums + structs
// ---------------------------------------------------------------------------
//
// These used to live in runtime-core but are SDK-side concepts after
// the substrate refactor. They live here so each iOS SDK doesn't have
// to redeclare them — the three first-party SDKs share this helper
// crate and these definitions.

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
    /// Whether the system back affordance (swipe-back gesture + nav-bar
    /// back chevron) may pop this screen. `None`/`Some(true)` ⇒ normal;
    /// `Some(false)` ⇒ the stack engine disables
    /// `interactivePopGestureRecognizer` and hides the back chevron
    /// while this screen is on top. Honored by the stack engine only
    /// (tab/drawer have no native back affordance to lock).
    pub back_enabled: Option<bool>,
    /// Whether this screen is full-screen while active. `Some(true)` ⇒
    /// the stack engine calls `runtime_shared::set_fullscreen(true)` when
    /// this screen is the top one and `false` when a non-full-screen
    /// screen becomes top (including on pop-back). Applied per active
    /// screen, alongside the back-gesture re-sync.
    pub fullscreen: Option<bool>,
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
    /// Retained ObjC objects (callback targets, gesture-recognizer
    /// targets, NSTimer scheduling targets) the helpers need to keep
    /// alive past the helpers' construction calls. Mirrors
    /// `IosBackend.callback_targets` from before the refactor.
    pub(crate) static CALLBACK_TARGETS: RefCell<Vec<Retained<NSObject>>> =
        RefCell::new(Vec::new());
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

struct IosNavigatorOps;
impl NavigatorOps for IosNavigatorOps {}
static IOS_NAV_OPS: IosNavigatorOps = IosNavigatorOps;

// ---------------------------------------------------------------------------
// Slot styling — stack header / title / button; drawer sidebar
// ---------------------------------------------------------------------------

/// Apply the stack navigator's "body" slot style: the
/// `UINavigationController`'s root `view.backgroundColor`. The stack's
/// screen-outlet IS that view (push/pop swap child VCs inside it), so
/// painting it here gives `HeaderStyle.body_background` the same
/// behavior as Android's `apply_body_style` and the drawer's
/// `apply_drawer_body_style`.
pub fn apply_stack_body_style(
    navigator: &IosNode,
    style: &Rc<runtime_shared::StyleRules>,
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

