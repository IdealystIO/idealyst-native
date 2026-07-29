//! Third-party SVG renderer SDK for the idealyst framework.
//!
//! Provides an `Svg` primitive rendered from SVG markup on every
//! backend; the mechanism differs (native browser SVG on web, resvg +
//! tiny-skia-free vector walk on iOS/Android) but the output converges.
//!
//! # One authored surface, two cores
//!
//! The default build is the old-core implementation (`Element::External`
//! payload + per-backend `ExternalRegistry`), byte-moved into
//! [`oldcore`]; the `new-core` feature swaps in [`newcore`], which
//! re-expresses the SAME public call shape (`Svg(SvgProps { .. })
//! .bind(r)` + a bootstrap `register`) over the scene registry — the
//! new core's unified primitive==external contract. Mutually exclusive
//! (same names), mirroring the codeblock/table SDK precedents. The
//! new-core web leg reproduces the old web handler's wrapper-`<div>` +
//! `innerHTML` DOM call-for-call; on non-web new-core hosts the SDK
//! registers the frozen External-placeholder degradation path (the
//! native vector handlers are old-core-only until new-core native boots
//! grow an external story — same posture as codeblock).
//!
//! # Usage (old core)
//!
//! ```ignore
//! // App bootstrap — one line per third-party SDK:
//! let mut backend = WebBackend::new("#app");
//! svg::register(&mut backend);
//!
//! // Inside a `ui!` block. `Svg` interpolates as an expression — the
//! // macro only knows the closed first-party set, so third-party
//! // primitives come in via `{ ... }` interpolation.
//! let markup = signal(LOGO_SVG.to_string());
//! let r: Ref<SvgHandle> = Ref::new();
//! ui! {
//!     view {
//!         { svg::Svg(SvgProps {
//!             markup: svg::markup(move || markup.get()),
//!             on_load: Some(Rc::new(|| log::info!("svg parsed"))),
//!             ..Default::default()
//!         }).bind(r.clone()) }
//!     }
//! }
//! // Read intrinsic dimensions from the parsed SVG:
//! let size = r.with(|h| h.intrinsic_size());
//! ```
//!
//! On the new core the same author code compiles unchanged; only the
//! bootstrap seam moves — pass [`register`] to the boot entry
//! (`backend_web::newcore::start_in("#app", svg::register, app)`).
#![deny(missing_docs)]

// Shared wasm32 helpers (pure DOM introspection, no core types) used by
// BOTH cores' web legs.
#[cfg(target_arch = "wasm32")]
pub(crate) mod web_util;

#[cfg(all(target_arch = "wasm32", not(feature = "new-core")))]
mod web;

#[cfg(all(
    target_os = "android",
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
mod android;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32"), not(feature = "new-core")))]
mod ios;

// The walker translates a parsed `usvg::Tree` into trait-driven calls
// against per-backend native vector primitives. Only the iOS and
// Android old-core backends consume it.
#[cfg(all(
    any(target_os = "ios", target_os = "android"),
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
pub(crate) mod tree_walker;

// One authored surface, two cores: the default build is the old-core
// implementation, byte-moved into `oldcore`; the `new-core` feature
// swaps in `newcore`, which re-expresses the SAME public names over the
// scene registry. Mutually exclusive (same names).
#[cfg(not(feature = "new-core"))]
mod oldcore;
#[cfg(not(feature = "new-core"))]
pub use oldcore::*;

#[cfg(feature = "new-core")]
mod newcore;
#[cfg(feature = "new-core")]
pub use newcore::*;
