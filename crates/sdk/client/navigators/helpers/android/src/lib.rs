// The crate is Android-only; on non-Android targets it compiles to an
// empty rlib so workspace-wide `cargo check` succeeds without dragging
// JNI / `backend-android-mobile` into scope on hosts. Per-SDK crates
// already cfg-gate their `mod android` references to Android, so
// nothing host-side touches this module.
#![cfg(all(target_os = "android", not(target_arch = "wasm32")))]

//! Shared Android-side machinery for the three first-party navigator
//! SDKs.
//!
//! **Internal — not author-facing.** This crate is per-platform glue,
//! not a public API. Apps never depend on it directly; they use
//! `stack-navigator`, which pulls this crate in only on
//! `target_os = "android"` and calls into it. The
//! whole crate is `#![cfg(all(target_os = "android", not(target_arch =
//! "wasm32")))]`, so on other hosts it compiles to an empty rlib
//! (keeping `cargo check --workspace` free of JNI /
//! `backend-android-mobile`).
//!
//! # The live path: the stack presenter
//!
//! [`AndroidStackPresenter`] (`presenter.rs`, `RustStackPresenter.kt`)
//! is what the outlet-model `stack-navigator` uses: it implements the
//! vocabulary's `nav_native_push` seam, giving the stack its push / pop /
//! replace transitions and system Back. Everything below this section
//! describes the legacy `NavCommand` engine (`create_stack`), which no
//! current navigator calls.
//!
//! # Legacy model
//!
//! `io.idealyst.runtime.RustNavigator` wraps a `FrameLayout` and the
//! Activity's `FragmentManager`. Push / pop / replace / reset map to
//! fragment transactions; `RustHostFragment.onDestroyView` trampolines
//! back through JNI to release per-screen scopes. See [`create_stack`].
//!
//! Per-instance state lives in a thread-local registry; the caller
//! retains the container `GlobalRef` and the instance is looked up via
//! the JObject* pointer-derived key.

use jni::objects::GlobalRef;
use runtime_shared::primitives::navigator::{MountResult, NavState, NavigatorControl, NavigatorHandle};
use runtime_shared::Signal;
use std::any::Any;
use std::rc::Rc;

mod presenter;
mod stack;

pub use presenter::{install as install_stack_presenter, AndroidStackPresenter};

// =============================================================================
// Callback bundle for the legacy engine.
// =============================================================================

/// Android navigator callbacks, passed to [`create_stack`].
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

// =============================================================================
// Kind-specific enums + structs.
// =============================================================================

/// When to materialize a screen's subtree relative to navigation.
#[derive(Clone, Copy, Debug)]
pub enum MountPolicy {
    EagerPersistent,
    LazyPersistent,
    LazyDisposing,
}

// =============================================================================
// Per-screen options — translated by the caller before calling
// [`attach_initial`]. Stack screens don't render Android-side header
// chrome (see stack module).
// =============================================================================

/// Icon-based header bar button. The closure is leaked via
/// `HeaderButtonCallback` so the Toolbar's OnClickListener can call
/// back any number of times.
#[derive(Clone)]
pub struct BarButton {
    pub icon: String,
    pub on_press: Rc<dyn Fn()>,
}

/// Per-screen options handed to [`attach_initial`] — title,
/// header_left/right buttons, color closures — a shared representation
/// the JNI-side Toolbar builder consumes, translated from a navigator's
/// own typed options.
#[derive(Default, Clone)]
pub struct AndroidScreenOptions {
    pub title: Option<String>,
    pub header_shown: Option<bool>,
    pub header_left: Option<BarButton>,
    pub header_right: Option<BarButton>,
    pub header_background: Option<Rc<dyn Fn() -> runtime_shared::Color>>,
    pub header_tint: Option<Rc<dyn Fn() -> runtime_shared::Color>>,
    pub title_color: Option<Rc<dyn Fn() -> runtime_shared::Color>>,
    /// Per-screen override of the navigator's global mount policy.
    /// `None` defers to the navigator-global default. Mirrors
    /// `IosScreenOptions::mount_policy`.
    pub mount_policy: Option<MountPolicy>,
    /// Whether the system back affordance may pop this screen
    /// (`StackScreenOptions::back_enabled`, defaulting to `true`).
    /// `Some(false)` ⇒ the stack's `RustNavigator` arms an
    /// `OnBackPressedCallback` that swallows the edge-swipe-back *and*
    /// the system back button while this screen is on top — Android
    /// routes both through the same `OnBackPressedDispatcher`, so the
    /// lock is necessarily all-or-nothing. Honored by the stack
    /// navigator only.
    pub back_enabled: Option<bool>,
    /// Whether this screen is full-screen (immersive) while active.
    /// `Some(true)` ⇒ the stack's `RustNavigator` drives
    /// `RustSystemUi.setFullscreen` (hide bars + full-edge gesture
    /// exclusion) when this screen is on top, and restores chrome for
    /// screens that aren't full-screen (including on pop-back).
    pub fullscreen: Option<bool>,
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

/// Mount the framework-built initial screen into a freshly-created
/// navigator, after the caller has translated its typed options to
/// [`AndroidScreenOptions`].
pub fn attach_initial(
    navigator: &GlobalRef,
    screen: GlobalRef,
    scope_id: u64,
    options: &AndroidScreenOptions,
) {
    // `back_enabled == Some(false)` ⇒ lock back on the root screen.
    // Absent / `Some(true)` ⇒ back works normally.
    let back_locked = options.back_enabled.map(|enabled| !enabled).unwrap_or(false);
    let fullscreen = options.fullscreen.unwrap_or(false);
    let _ = stack::attach_initial(navigator, &screen, scope_id, back_locked, fullscreen);
}

/// Tear down a navigator: release every still-mounted screen scope,
/// drop the instance entry, free any leaked listener boxes.
pub fn release(node: &GlobalRef) {
    let _ = stack::release(node);
}

/// Build a `NavigatorHandle` for the navigator identified by `node`.
/// A navigator SDK wraps this in its own typed handle. Returns an inert
/// (no-control) handle
/// when `node` isn't a registered navigator.
pub fn make_handle(node: &GlobalRef) -> NavigatorHandle {
    if let Some(handle) = stack::make_handle(node) {
        return handle;
    }
    NavigatorHandle::new(Rc::new(()), &NOOP_OPS)
}

// =============================================================================
// Internal helpers.
// =============================================================================

struct NoopOps;
impl runtime_shared::primitives::navigator::NavigatorOps for NoopOps {}
static NOOP_OPS: NoopOps = NoopOps;

/// Stable key for an `instance` table — JObject* pointer. Mirrors the
/// backend's internal `node_key_of` (re-exported as
/// `backend_android_mobile::node_key_of`); we re-derive here so the
/// helpers crate doesn't need a runtime crossing for every lookup.
pub(crate) fn node_key(node: &GlobalRef) -> usize {
    node.as_obj().as_raw() as usize
}
