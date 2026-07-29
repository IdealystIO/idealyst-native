//! Third-party `Video` SDK for the idealyst framework.
//!
//! Provides a `Video` primitive — typed props, `.bind(...)`-able
//! handle, `.with_style(...)` — rendered by each platform's native
//! player (`<video>` on web, AVPlayer on Apple, `VideoView` on
//! Android).
//!
//! # One authored surface, two cores
//!
//! The default build is the old-core implementation (`Element::External`
//! payload + per-backend `ExternalRegistry`), byte-moved into
//! [`oldcore`]; the `new-core` feature swaps in [`newcore`], which
//! re-expresses the SAME public call shape (`Video(VideoProps { .. })
//! .bind(v)` + a bootstrap `register`) over the scene registry — the
//! new core's unified primitive==external contract. Mutually exclusive
//! (same names), mirroring the svg/codeblock/table SDK precedents. The
//! new-core web leg reproduces the old web handler's `<video>` DOM
//! call-for-call; on non-web new-core hosts the SDK registers the
//! frozen External-placeholder degradation path (the native players are
//! old-core-only until new-core native boots grow an external story —
//! same posture as codeblock; see `newcore.rs`).
//!
//! # Usage (old core)
//!
//! ```ignore
//! // App bootstrap (one line per third-party SDK):
//! let mut backend = WebBackend::new("#app");
//! video::register(&mut backend);
//!
//! // Inside a `ui!` block:
//! let src = signal("https://example.com/clip.mp4".to_string());
//! let v: Ref<VideoHandle> = Ref::new();
//! ui! {
//!     view {
//!         { video::Video(VideoProps {
//!             source: video::url(move || src.get()),
//!             autoplay: true,
//!             controls: true,
//!             ..Default::default()
//!         }).bind(v.clone()) }
//!     }
//! }
//! // Imperative ops at any later point:
//! v.with(|h| h.play());
//! v.with(|h| h.seek(10.0));
//! ```
//!
//! On the new core the same author code compiles unchanged; only the
//! bootstrap seam moves — pass [`register`] to the boot entry
//! (`backend_web::newcore::start_in("#app", video::register, app)`).
#![deny(missing_docs)]

// Shared wasm32 helpers (pure DOM media plumbing, no core types) used
// by BOTH cores' web legs.
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

// Shared CoreGraphics RGBA→CGImage bridge for the Apple backends. The
// stream-display path is byte-identical on iOS and macOS (pure
// CoreGraphics), so both modules pull it from here.
#[cfg(all(
    any(target_os = "ios", target_os = "macos"),
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
mod cg_image;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32"), not(feature = "new-core")))]
mod ios;

#[cfg(all(
    target_os = "macos",
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
mod macos;

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
