//! Third-party `Toolbar` SDK for the idealyst framework.
//!
//! Provides a `Toolbar` primitive that attaches to the host window's
//! chrome on native desktop hosts — title bar on macOS (`NSToolbar`),
//! command bar on Windows, HeaderBar on GTK. On every other platform
//! (iOS, Android, web, terminal, wgpu, ESP, CPU) `register` is a
//! no-op and the in-tree primitive renders zero-size.
//!
//! That posture follows the project's mobile-first philosophy
//! ([[feedback_mobile_first_philosophy]]): toolbar / menu chrome
//! belongs in third-party SDKs, not the core Backend trait.
//!
//! # One authored surface, two cores
//!
//! The default build is the old-core implementation (`Element::External`
//! payload + per-backend `ExternalRegistry`), byte-moved into
//! [`oldcore`]; the `new-core` feature swaps in [`newcore`], which
//! re-expresses the SAME public call shape (`Toolbar(ToolbarProps
//! { .. }).bind(r)` + a bootstrap `register`) over the scene registry —
//! the new core's unified primitive==external contract. Mutually
//! exclusive (same names), mirroring the svg/codeblock/table SDK
//! precedents. The macOS NSToolbar machinery itself is CORE-FREE and
//! shared by both legs (`macos_shared`); each leg contributes only its
//! reactive wrapper (old: `runtime_core::effect!`; new:
//! `runtime_world::effect`) and its click-callback discipline (the new
//! core wraps every button `on_click` with
//! `backend_macos::newcore::schedule_flush`).
//!
//! # Usage (old core)
//!
//! ```ignore
//! // App bootstrap: pass an `register_extensions` closure to host_appkit::run_with.
//! host_appkit::run_with(
//!     app,
//!     host_appkit::RunOptions::default(),
//!     |backend| {
//!         toolbar::register(backend);
//!         // other SDKs that need backend.register_external::<T>(...)
//!     },
//! )?;
//!
//! // Inside a `ui!` block — the toolbar's in-tree footprint is zero,
//! // so its position in the tree doesn't matter visually. Convention:
//! // mount near the root so the items closure is owned by a long-
//! // lived scope. `.into()` lifts each `ToolbarButton` builder into
//! // the enum so `vec![]` accepts mixed kinds (buttons + spacers).
//! let count = signal(0_i32);
//! ui! {
//!     view {
//!         { toolbar::Toolbar(toolbar::ToolbarProps {
//!             items: Box::new(move || vec![
//!                 toolbar::ToolbarItem::button("Save")
//!                     .icon("square.and.arrow.down")
//!                     .on_click({ let c = count.clone(); move || c.set(c.get() + 1) })
//!                     .into(),
//!                 toolbar::ToolbarItem::flexible_space(),
//!                 toolbar::ToolbarItem::button("Reload")
//!                     .on_click(|| log::info!("reload"))
//!                     .into(),
//!             ]),
//!             ..Default::default()
//!         }) }
//!         // ... rest of the app
//!     }
//! }
//! ```
//!
//! On the new core the same author code compiles unchanged; only the
//! bootstrap seam moves — pass [`register`] to the boot entry
//! (`host_appkit::newcore::run_with(build, opts, |registry|
//! toolbar::register(registry))`).
#![deny(missing_docs)]

// Core-free item model (ToolbarItem/ToolbarButton + builders), shared
// by both core legs and by the macOS native machinery.
mod items;

// Old-core per-backend handler modules. Gated off under `new-core`
// (they drive the old `ExternalRegistry`, which the new core replaces
// with the scene registry).
#[cfg(all(target_os = "macos", not(feature = "new-core")))]
mod macos;
#[cfg(all(target_os = "windows", not(feature = "new-core")))]
mod windows;
#[cfg(all(target_os = "linux", not(feature = "new-core")))]
mod linux;

// Shared macOS NSToolbar machinery (delegate, item construction,
// wipe+repopulate update, placeholder view, imperative ops). Core-free:
// no reactive imports, no feature gate — compiled for BOTH cores so the
// AppKit code cannot fork.
#[cfg(target_os = "macos")]
mod macos_shared;

// New-core macOS leg: the MacosBackend-concrete scene handler that
// drives `macos_shared` through `runtime_world::effect` + the
// schedule_flush click discipline.
#[cfg(all(target_os = "macos", feature = "new-core"))]
mod macos_newcore;

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
