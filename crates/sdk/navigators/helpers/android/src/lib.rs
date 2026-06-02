// The crate is Android-only; on non-Android targets it compiles to an
// empty rlib so workspace-wide `cargo check` succeeds without dragging
// JNI / `backend-android-mobile` into scope on hosts. Per-SDK crates
// already cfg-gate their `mod android` references to Android, so
// nothing host-side touches this module.
#![cfg(all(target_os = "android", not(target_arch = "wasm32")))]

//! Shared Android-side machinery for the three first-party navigator
//! SDKs (stack / tab / drawer).
//!
//! **Internal — not author-facing.** This crate is per-platform glue,
//! not a public API. Apps never depend on it directly; they use
//! `stack-navigator`, `tab-navigator`, or `drawer-navigator`, which pull
//! this crate in only on `target_os = "android"` and call into it. The
//! whole crate is `#![cfg(all(target_os = "android", not(target_arch =
//! "wasm32")))]`, so on other hosts it compiles to an empty rlib
//! (keeping `cargo check --workspace` free of JNI /
//! `backend-android-mobile`).
//!
//! # Model
//!
//! Two flavors of native chrome:
//!
//! - **Stack navigator** — `io.idealyst.runtime.RustNavigator` wraps a
//!   `FrameLayout` and the Activity's `FragmentManager`. Push / pop /
//!   replace / reset map to fragment transactions;
//!   `RustHostFragment.onDestroyView` trampolines back through JNI to
//!   release per-screen scopes. See [`create_stack`].
//!
//! - **Tab / Drawer navigators** — plain view-swap, no FragmentManager.
//!   - Tabs: navigator node is a `FrameLayout`; the active tab's screen
//!     view is the single child. Author chrome (tab bar) lives in a
//!     `.layout(...)` slot.
//!   - Drawers: navigator node is a `RustExactFrameLayout` wrapping a
//!     `RustDrawerLayout` (androidx DrawerLayout subclass), with a
//!     body `LinearLayout` for the active screen + Toolbar, and the
//!     drawer view attached separately via [`drawer_attach_sidebar`].
//!     See [`create_drawer`].
//!
//! Per-instance state lives in thread-local registries (one for stack,
//! one for tab/drawer), mirroring how web-navigator-helpers stores its
//! `NavigatorInstance` per `data-navigator-id`. The SDK handler retains
//! the container `GlobalRef` and looks up the instance via the
//! JObject* pointer-derived key.
//!
//! # Substrate boundary
//!
//! `runtime-core` owns the kind-agnostic command vocabulary, per-screen
//! scope mechanics, and reactive `NavState`. Everything kind-specific —
//! chrome construction, typed handles, the dispatcher mapping from
//! `NavCommand` to native action — lives in the SDK crates. This
//! helper crate is the Android-side shared engine the three first-party
//! Android SDKs (stack-navigator, tab-navigator, drawer-navigator) call
//! into for JNI glue.

use jni::objects::GlobalRef;
use runtime_core::primitives::navigator::{MountResult, NavState, NavigatorControl, NavigatorHandle};
use runtime_core::Signal;
use std::any::Any;
use std::rc::Rc;

mod stack;
mod tab_drawer;

// =============================================================================
// Local callback bundle types — paralleling web-navigator-helpers.
// =============================================================================
//
// Mirrors the shape of the OLD `NavigatorCallbacks<N>` /
// `TabNavigatorCallbacks<N>` / `DrawerNavigatorCallbacks<N>` that lived
// in runtime-core before the substrate refactor. Each SDK fills one of
// these in and passes it to `create_stack` / `create_tab` /
// `create_drawer`.

/// Kind-agnostic Android navigator callbacks. Every Android SDK passes
/// one of these; the tab and drawer variants embed it.
pub struct AndroidNavCallbacks {
    pub initial_route: &'static str,
    pub initial_path: &'static str,
    pub mount_screen:
        Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<GlobalRef>>,
    pub release_screen: Rc<dyn Fn(u64)>,
    pub match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>>,
    pub depth_changed: Rc<dyn Fn(usize)>,
    pub nav_state: NavState,
    pub defer_initial_mount: bool,
}

/// Tab-navigator-specific callbacks.
pub struct AndroidTabCallbacks {
    pub navigator: AndroidNavCallbacks,
    pub tabs: Vec<TabRegistration>,
    pub placement: TabPlacement,
    pub mount_policy: MountPolicy,
    pub active_changed: Rc<dyn Fn(&'static str, String)>,
}

/// Drawer-navigator-specific callbacks.
pub struct AndroidDrawerCallbacks {
    pub navigator: AndroidNavCallbacks,
    pub side: DrawerSide,
    pub drawer_type: DrawerType,
    pub drawer_width: f32,
    pub swipe_to_open: bool,
    pub mount_policy: MountPolicy,
    pub is_open: Signal<bool>,
    pub active_changed: Rc<dyn Fn(&'static str, String)>,
    pub open_changed: Rc<dyn Fn(bool)>,
}

// =============================================================================
// Local kind-specific enums + structs — moved out of runtime-core into the
// helpers crate (SDK-side concepts after the substrate refactor).
// =============================================================================

/// Identifier + display metadata for a single tab. Mostly opaque to the
/// helper itself — the active-screen view-swap engine doesn't render
/// tab chrome (authors build their own bar via the layout slot).
pub struct TabRegistration {
    pub route: &'static str,
    pub path: &'static str,
    pub label: Option<String>,
    pub icon: Option<String>,
}

/// Where the tab bar lives relative to the screen content. Currently
/// informational on Android — author chrome owns positioning.
#[derive(Clone, Copy, Debug)]
pub enum TabPlacement {
    Auto,
    Top,
    Bottom,
    Sidebar,
}

/// When to materialize a screen's subtree relative to navigation.
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
    Front,
    Slide,
}

/// Drawer-specific commands ridden across the substrate's
/// `NavCommand::Custom` channel. The drawer SDK builds one of these
/// inside an `Rc<dyn Any>`, dispatches it, and the helper's dispatcher
/// downcasts to drive the native open/close/toggle action.
#[derive(Clone, Copy, Debug)]
pub enum DrawerCmd {
    Open,
    Close,
    Toggle,
}

// =============================================================================
// Per-screen options — translated by the SDK handler before calling
// [`attach_initial`]. Tabs and drawers consume these; stack screens
// currently don't render Android-side header chrome (see stack module).
// =============================================================================

/// Icon-based header bar button. The closure is leaked via
/// `HeaderButtonCallback` so the Toolbar's OnClickListener can call
/// back any number of times.
#[derive(Clone)]
pub struct BarButton {
    pub icon: String,
    pub on_press: Rc<dyn Fn()>,
}

/// Per-screen options the SDK handler hands to [`attach_initial`].
/// Mirrors the legacy `runtime_core::ScreenOptions` shape — title,
/// header_left/right buttons, color closures — but lives in the
/// helpers crate so each SDK can translate from its own typed options
/// (`StackScreenOptions`, `DrawerScreenOptions`, …) into a shared
/// representation the JNI-side Toolbar builder consumes.
#[derive(Default, Clone)]
pub struct AndroidScreenOptions {
    pub title: Option<String>,
    pub header_shown: Option<bool>,
    pub header_left: Option<BarButton>,
    pub header_right: Option<BarButton>,
    pub header_background: Option<Rc<dyn Fn() -> runtime_core::Color>>,
    pub header_tint: Option<Rc<dyn Fn() -> runtime_core::Color>>,
    pub title_color: Option<Rc<dyn Fn() -> runtime_core::Color>>,
    /// Per-screen override of the navigator's global mount policy
    /// (`AndroidDrawerCallbacks::mount_policy` / `AndroidTabCallbacks`).
    /// `None` defers to the navigator-global default. Mirrors
    /// `IosScreenOptions::mount_policy` so a single
    /// `DrawerScreenExt::mount_policy(...)` declaration works on
    /// both mobile backends.
    pub mount_policy: Option<MountPolicy>,
}

// =============================================================================
// Public API — dispatch surfaces used by SDK android.rs handlers.
// =============================================================================

use backend_android::AndroidBackend;

/// Stack navigator entry point. Creates the `RustNavigator` instance,
/// stashes per-instance state in the thread-local registry, installs
/// the kind-specific dispatcher on `control`, and returns the
/// container `GlobalRef` (which the SDK retains as its node).
pub fn create_stack(
    backend: &mut AndroidBackend,
    callbacks: AndroidNavCallbacks,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    stack::create(backend, callbacks, control)
}

/// Tab navigator entry point. Same posture as [`create_stack`] but
/// builds a plain `FrameLayout` (no FragmentManager) for view-swap on
/// `Select`.
pub fn create_tab(
    backend: &mut AndroidBackend,
    callbacks: AndroidTabCallbacks,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    tab_drawer::create_tab(backend, callbacks, control)
}

/// Drawer navigator entry point. Builds a `RustExactFrameLayout`
/// wrapping a `RustDrawerLayout` with a body `LinearLayout` for the
/// active screen + Toolbar. The sidebar is attached separately by the
/// SDK handler via [`drawer_attach_sidebar`] after the SDK
/// materializes it through `host.build_node`.
pub fn create_drawer(
    backend: &mut AndroidBackend,
    callbacks: AndroidDrawerCallbacks,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    tab_drawer::create_drawer(backend, callbacks, control)
}

/// Mount the framework-built initial screen into a freshly-created
/// navigator. The SDK handler calls this from
/// `NavigatorHandler::attach_initial` after translating its typed
/// options to [`AndroidScreenOptions`].
///
/// Works for all three kinds — the helpers crate dispatches based on
/// which thread-local registry holds the node.
pub fn attach_initial(
    navigator: &GlobalRef,
    screen: GlobalRef,
    scope_id: u64,
    options: &AndroidScreenOptions,
) {
    if stack::attach_initial(navigator, &screen, scope_id) {
        return;
    }
    tab_drawer::attach_initial(navigator, screen, scope_id, options);
}

/// Tear down a navigator: release every still-mounted screen scope,
/// drop the instance entry, free any leaked listener boxes. Works for
/// all three kinds via the same registry-dispatch as
/// [`attach_initial`].
pub fn release(node: &GlobalRef) {
    if stack::release(node) {
        return;
    }
    tab_drawer::release(node);
}

/// Attach a freshly-built sidebar view to a drawer navigator. Called
/// by the drawer SDK handler after `host.build_node` (deferred via
/// microtask) materializes the sidebar Element into a `GlobalRef`.
///
/// No-op on tab and stack navigators.
pub fn drawer_attach_sidebar(navigator: &GlobalRef, sidebar: GlobalRef) {
    tab_drawer::attach_sidebar(navigator, sidebar);
}

/// Build a `NavigatorHandle` for the navigator identified by `node`.
/// SDK crates wrap this in their own typed handle (`StackHandle`,
/// `TabsHandle`, `DrawerHandle`). Returns an inert (no-control) handle
/// when `node` isn't a registered navigator.
pub fn make_handle(node: &GlobalRef) -> NavigatorHandle {
    if let Some(handle) = stack::make_handle(node) {
        return handle;
    }
    if let Some(handle) = tab_drawer::make_handle(node) {
        return handle;
    }
    NavigatorHandle::new(Rc::new(()), &NOOP_OPS)
}

/// Apply a navigator header-slot style. The SDK handler routes its
/// `apply_slot_style("header", ...)` call here. No-op when `node`
/// isn't a registered navigator or the slot isn't supported on the
/// active kind.
pub fn apply_header_style(
    node: &GlobalRef,
    rules: &Rc<runtime_core::StyleRules>,
) {
    tab_drawer::apply_header_style(node, rules);
}

/// Apply a navigator title-slot style. Currently honors `rules.color`.
pub fn apply_title_style(
    node: &GlobalRef,
    rules: &Rc<runtime_core::StyleRules>,
) {
    tab_drawer::apply_title_style(node, rules);
}

/// Apply a navigator button-slot style. Tints the Toolbar's nav-icon
/// from `rules.color`.
pub fn apply_button_style(
    node: &GlobalRef,
    rules: &Rc<runtime_core::StyleRules>,
) {
    tab_drawer::apply_button_style(node, rules);
}

/// Apply a navigator body-slot style — paints the active-screen
/// container's background.
pub fn apply_body_style(
    node: &GlobalRef,
    rules: &Rc<runtime_core::StyleRules>,
) {
    tab_drawer::apply_body_style(node, rules);
}

// =============================================================================
// Internal helpers shared across stack + tab_drawer modules.
// =============================================================================

struct NoopOps;
impl runtime_core::primitives::navigator::NavigatorOps for NoopOps {}
static NOOP_OPS: NoopOps = NoopOps;

/// Stable key for an `instance` table — JObject* pointer. Mirrors the
/// backend's internal `node_key_of` (re-exported as
/// `backend_android_mobile::node_key_of`); we re-derive here so the
/// helpers crate doesn't need a runtime crossing for every lookup.
pub(crate) fn node_key(node: &GlobalRef) -> usize {
    node.as_obj().as_raw() as usize
}
