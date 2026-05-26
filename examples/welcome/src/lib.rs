//! `welcome` — three-act cinematic intro driven by springs + tweens
//! + a raf-driven sun/vignette/planet pulse.
//!
//! - Act 1: "Welcome to Idealyst" rises into a light frame.
//! - Act 2: frame washes dark, a warm sun blooms from the top-right.
//! - Act 3: subtitle materializes below the shuffled-up headline.
//!
//! Each animated property is an `AnimatedValue` bound to a `Ref` via
//! [`AnimatedValue::bind`] (or `bind_color` / `bind_gradient_stop` /
//! `bind_text_color`). The framework owns all per-platform dispatch —
//! this project is pure platform-agnostic Rust; the per-target entry
//! points are in the wrapper crates the CLI generates at build time.

mod color;
#[macro_use]
mod components;
mod app;
mod constants;
mod coordinator;
mod style_helpers;
mod typeface;

pub use app::app;

// Per-target SDK-handler registration hook the CLI-generated wrappers
// invoke before mount. `welcome` doesn't depend on any third-party
// navigator/external SDKs, so each is intentionally empty — but the
// wrappers always call it, so the symbol must exist for each platform.
#[cfg(target_arch = "wasm32")]
pub fn register_extensions(_backend: &mut backend_web::WebBackend) {}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_ios::IosBackend) {}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn register_extensions(_backend: &mut backend_terminal::TerminalBackend) {}
