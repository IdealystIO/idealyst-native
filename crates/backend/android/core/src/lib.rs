//! Shared Android substrate.
//!
//! Houses the bits that both `backend-android-mobile` (touch /
//! phone+tablet) and `backend-android-tv` (Leanback / D-pad focus)
//! reuse unchanged: JNI helpers and the render-thread `RenderLoopDriver`.
//! Higher pieces — the `AndroidBackend` struct, per-primitive
//! modules, navigator/tab-drawer chrome — stay in the leaf crates
//! because they bake in input semantics that differ between mobile
//! and TV.
//!
//! Modules are gated on `cfg(target_os = "android")`; on the host
//! target the crate compiles as an empty rlib so workspace-wide
//! `cargo check` keeps working.

// Pure style-decision helpers (effective text color). Cross-target so
// the decision logic is host-testable; the JNI `style` path in
// `backend-android-mobile` calls into it.
pub mod style_diff;

#[cfg(target_os = "android")]
pub mod helpers;

#[cfg(all(target_os = "android", feature = "async-driver"))]
pub mod render_loop;
