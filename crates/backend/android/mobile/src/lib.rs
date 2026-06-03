//! Android backend: drives the framework's `View` tree by calling
//! into the Android Java View hierarchy via JNI.
//!
//! # File layout
//!
//! - [`imp`] — full Android implementation, gated on
//!   `target_os = "android"`. Split into:
//!   - `imp/mod.rs` — `JNI_OnLoad`, `with_env`, `AndroidBackend`
//!     struct, the `Backend` impl block (delegates to per-primitive
//!     modules).
//!   - `imp/callbacks.rs` — leaked-box wrappers for Click / State /
//!     TextChange / ToggleChange / SliderChange callbacks.
//!   - `imp/jni_exports.rs` — every `Java_io_idealyst_runtime_*`
//!     trampoline the Kotlin runtime calls back into.
//!   - `imp/helpers.rs` — small shared utilities (`with_env` callers,
//!     `set_text`, `dp_to_px`, `parse_color`, default LayoutParams).
//!   - `imp/style.rs` — `apply_rules` plus the GradientDrawable path.
//!   - `imp/animation.rs` — animator builders + the
//!     `Easing → Interpolator` mapping.
//!   - `imp/primitives/*.rs` — one module per `Element` kind.
//! - [`stub`] — non-Android `unreachable!()` stub so the workspace
//!   `cargo check`s on host platforms without an NDK.
//!
//! # Threading
//!
//! The framework's reactive arena is thread-local (see
//! `runtime-core/src/reactive.rs`). All `Backend` calls happen on
//! the Android UI thread (where the app started `render`), so
//! `AndroidBackend` is `!Send`/`!Sync` and assumes single-threaded
//! access.
//!
//! JNI access is acquired lazily per call by attaching the current
//! thread to the cached `JavaVM`. The `JavaVM` is captured in
//! `JNI_OnLoad` and stashed in a `static`. This is what lets
//! `AndroidBackend: 'static` — there's no `'local` lifetime tied to a
//! `JNIEnv` living on the stack.

#![allow(unused_imports)]

#[cfg(target_os = "android")]
mod imp;

/// Phase-timer wrapper around `runtime_core::debug`'s aggregator.
/// Zero-cost stub when `debug-stats` is off (the macro expansion is
/// a let-binding the optimizer elides). Enabled by passing
/// `--features debug-stats` to the variant at build time. See
/// `phase_timer.rs` for the on/off shapes.
#[cfg(target_os = "android")]
mod phase_timer;

/// Pure-compute helpers + host-runnable regression coverage for
/// `Position::Sticky` on Android. The full registry / JNI-driven
/// pipeline lives in `imp::sticky` (target_os = "android"); this
/// module mirrors the math + the lifecycle invariants in a form
/// that compiles and runs on the host so `cargo test
/// -p backend-android-mobile` exercises the regression coverage
/// from any platform.
///
/// The iOS reference pins all of its sticky tests inside the
/// `cfg(target_os = "ios")` `imp` gate, which means they don't
/// run from host. We deliberately diverge here: the math
/// regression and the empty-registry invariant don't need JNI
/// types, so there's no reason to make them target-gated.
mod sticky_compute;

/// Pure layout-scheduling policy (when an `insert` must kick a layout pass),
/// kept un-gated like `sticky_compute` so its regression coverage runs on the
/// host. The JNI-driven insert path that consumes it lives in `imp`.
mod layout_policy;

#[cfg(not(target_os = "android"))]
mod stub;

#[cfg(target_os = "android")]
pub use imp::{install_global_self, set_animated_color, set_animated_f32, AndroidBackend};

/// SDK extension point: leaked-box callback wrapper for header bar
/// buttons. Constructed by the navigator helpers crate when building
/// per-screen Toolbars from `attach_initial` options; the pointer
/// flows through `RustActionBarHelper.buildToolbar` and is invoked by
/// the JNI export
/// `Java_io_idealyst_runtime_RustActionBarHelper_nativeInvoke`.
///
/// Exposed `pub` so `android-navigator-helpers` can hand the same
/// concrete type to the JNI export — the export dereferences the
/// pointer as `*const HeaderButtonCallback`, so the box layout must
/// match exactly.
#[cfg(target_os = "android")]
pub use imp::callbacks::HeaderButtonCallback;

/// Stable key for a node's animation/instance state — derived from the
/// `JObject*` pointer the `GlobalRef` wraps. SDK helpers crates index
/// per-instance state by this so lookups match what the backend uses
/// internally.
#[cfg(target_os = "android")]
pub fn node_key_of(node: &jni::objects::GlobalRef) -> usize {
    node.as_obj().as_raw() as usize
}

/// Attach the current thread to the JVM (cached `JavaVM` captured at
/// `JNI_OnLoad`) and run `f` with the resulting `JNIEnv`. Public entry
/// point for third-party SDK code (e.g. `webview`'s `Effect` closures
/// that fire outside the build path and need to reach Java without a
/// `&mut AndroidBackend` in scope).
///
/// Panics if `JNI_OnLoad` hasn't fired — which can only happen if the
/// library wasn't loaded by an Android process. Don't call from
/// non-Android code; gate at the SDK level on `cfg(target_os = "android")`.
#[cfg(target_os = "android")]
pub fn with_jni_env<R>(f: impl FnOnce(&mut jni::JNIEnv) -> R) -> R {
    imp::with_env(f)
}

/// Re-export the JNI `GlobalRef` so author-level animation drivers
/// can downcast `view_handle.as_any()` without having to depend on
/// `jni` directly. Matches the iOS backend's `IosNode` re-export.
#[cfg(target_os = "android")]
pub use jni::objects::GlobalRef as AndroidNode;

#[cfg(all(target_os = "android", feature = "async-driver"))]
pub use backend_android_core::render_loop::install_render_loop;

/// Install the Android scheduler (Handler.postDelayed on the main
/// Looper). Must be called once before `runtime_core::render(...)`
/// so timer-driven features (long-press recognizer, presence
/// animations, anything calling `after_ms` / `schedule_microtask`)
/// delay correctly instead of firing synchronously.
#[cfg(target_os = "android")]
pub use imp::scheduler::install_scheduler;

/// Schedule a layout pass retry. The host scheduling layer
/// already wraps this in a retry loop for the initial 0×0 case;
/// SDK code that mutates the view tree outside the normal
/// build path (e.g. drawer's deferred sidebar attach) calls this
/// to force a Taffy → apply_frames cycle once the tree settles.
#[cfg(target_os = "android")]
pub fn schedule_layout_pass() {
    imp::scheduler::schedule_layout_pass_retry(0);
}

/// Synchronously run a Taffy compute + apply_frames pass against
/// the global backend self-handle. Unlike [`schedule_layout_pass`]
/// this does NOT post to the main looper — it runs on the calling
/// thread immediately. Use when the next user-visible frame must
/// reflect a layout change you just made and you can't afford the
/// async hop (drawer screen swap is the canonical case: without a
/// sync layout the new screen flashes briefly with default LPs).
#[cfg(target_os = "android")]
pub fn run_layout_now() {
    if let Some(weak) = imp::backend_self_weak() {
        if let Some(rc) = weak.upgrade() {
            rc.borrow_mut().run_layout();
        }
    }
}

#[cfg(not(target_os = "android"))]
pub fn run_layout_now() {}

#[cfg(not(target_os = "android"))]
pub fn schedule_layout_pass() {}

/// Notify the backend that the host configuration changed (rotation,
/// multi-window resize, density change, etc.). Schedules a layout
/// pass against the host root's current dimensions; the existing
/// retry loop covers the brief window where `getWidth/getHeight`
/// still report the pre-change values. Call from the JNI bridge's
/// `notifyConfigChanged` symbol (the generated wrapper crate routes
/// MainActivity.onConfigurationChanged → here).
#[cfg(target_os = "android")]
pub fn notify_config_changed() {
    log::info!("[layout] notify_config_changed → scheduling layout pass");
    imp::scheduler::schedule_layout_pass_retry(0);
}

#[cfg(not(target_os = "android"))]
pub fn notify_config_changed() {}

#[cfg(not(target_os = "android"))]
pub use stub::AndroidBackend;

/// Non-Android no-op so cross-compile of host code still type-checks.
#[cfg(all(not(target_os = "android"), feature = "async-driver"))]
pub fn install_render_loop() {}

/// Non-Android no-op so cross-compile of host code still type-checks.
#[cfg(not(target_os = "android"))]
pub fn install_scheduler() {}

/// Non-Android stub for cross-compile. The wasm/iOS targets pull in
/// the host's `app()` and re-export the welcome example's
/// drive-AV bridge under `cfg(target_os = "android")`; this stub
/// keeps non-Android `cargo check` passing.
#[cfg(not(target_os = "android"))]
pub struct AndroidNode;

#[cfg(not(target_os = "android"))]
pub fn install_global_self(_weak: std::rc::Weak<std::cell::RefCell<AndroidBackend>>) {}

#[cfg(not(target_os = "android"))]
pub fn set_animated_f32(
    _node: &AndroidNode,
    _prop: runtime_core::animation::AnimProp,
    _value: f32,
) {}

#[cfg(not(target_os = "android"))]
pub fn set_animated_color(
    _node: &AndroidNode,
    _prop: runtime_core::animation::AnimProp,
    _value: [f32; 4],
) {}

/// Optional runtime-server-client glue. Compiled in only when the `runtime-server`
/// Cargo feature is on. The module exposes `attach` / `drain` /
/// `detach` entry points; the consuming staticlib crate defines its
/// own JNI exports (with package-qualified names like
/// `Java_<pkg>_NativeBridge_attachRuntimeServer`) that trampoline into these.
#[cfg(all(target_os = "android", feature = "runtime-server"))]
pub mod runtime_server;
