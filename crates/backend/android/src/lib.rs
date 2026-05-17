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
//!   - `imp/primitives/*.rs` — one module per `Primitive` kind.
//! - [`stub`] — non-Android `unreachable!()` stub so the workspace
//!   `cargo check`s on host platforms without an NDK.
//!
//! # Threading
//!
//! The framework's reactive arena is thread-local (see
//! `framework-core/src/reactive.rs`). All `Backend` calls happen on
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

#[cfg(not(target_os = "android"))]
mod stub;

#[cfg(target_os = "android")]
pub use imp::AndroidBackend;

#[cfg(not(target_os = "android"))]
pub use stub::AndroidBackend;

/// Optional AAS-client glue. Compiled in only when the `aas-shell`
/// Cargo feature is on. The module exposes `attach` / `drain` /
/// `detach` entry points; the consuming staticlib crate defines its
/// own JNI exports (with package-qualified names like
/// `Java_<pkg>_NativeBridge_attachAas`) that trampoline into these.
#[cfg(all(target_os = "android", feature = "aas-shell"))]
pub mod aas;
