//! `welcome` — three-act cinematic intro, driven by the framework's
//! animation system (springs + tweens) rather than `Presence`'s
//! enter/exit transitions.
//!
//! Act 1 — "Welcome to Idealyst" rises into a light frame, settles.
//! Act 2 — The frame washes to dark; a warm sun-glare blooms in
//!         from the top-right corner; the welcome phrase exits.
//! Act 3 — Content scales from oversized down to rest, reading as
//!         "focus pulling in."
//!
//! Each animated property is its own `AnimatedValue<f32>` bound to
//! a `Ref<ViewHandle>` via `subscribe_and_apply`. Per-frame the
//! subscriber writes `opacity` / `transform: translate(...) scale(...)`
//! as inline CSS via `web::set_animated_f32`. The act-sequence
//! `effect!` fires `av.animate(SpringTo::new(...))` (or `TweenTo`)
//! calls at the right moments — that's the whole orchestration.
//!
//! Springs (`SpringTo::new(target).stiffness(s).damping(d)`) carry
//! the entrances; tweens with cubic-bezier easing carry fades and
//! exits.
//!
//! `app()` is invoked via `framework_core::mount(backend, super::app)`
//! (see `src/web.rs`) so the `effect!` below adopts the root scope.
//!
//! ## Module layout
//!
//! - [`app`] — the `app()` function: orchestrates AVs, refs, the
//!   timeline and the per-frame pulse driver, then builds the tree.
//! - [`components`] — one submodule per visual element (page, dark
//!   layer, vignette, sun glare, planet, welcome phrase, subtitle,
//!   content layer). Each owns its stylesheets and any constants
//!   that are purely about its appearance.
//! - [`animation_bridge`] — wires `AnimatedValue` outputs to per-
//!   platform `set_animated_*` writers on the bound nodes.
//! - [`color`], [`style_helpers`] — small reusable helpers.
//! - [`constants`] — cross-cutting timing constants used by the act
//!   schedule.
//! - [`typeface`] — bundles the Inter font into the binary.

#[cfg(target_arch = "wasm32")]
mod web;

mod animation_bridge;
mod app;
mod color;
mod components;
mod constants;
mod style_helpers;
mod typeface;

pub use app::app;
