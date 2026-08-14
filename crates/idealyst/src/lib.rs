//! Build and run an idealyst app with plain `cargo build`.
//!
//! # What changed
//!
//! Every platform used to be built through an ephemeral *wrapper*
//! crate the CLI wrote into `target/idealyst/<app>/<platform>/wrapper/`.
//! The wrapper owned the entry point, the per-platform dependencies,
//! and the boot sequence; the app crate was a plain `rlib` that could
//! not be built or run on its own.
//!
//! That cost more than it looked like:
//!
//! - **Two dependency graphs.** The wrapper resolved `runtime-core`
//!   independently of the app. Any disagreement — a different git rev,
//!   a workspace-inherited dep the resolver-by-hand couldn't read —
//!   produced two `runtime_core` crates and the notorious "expected
//!   `Element`, found `Element`" at the wrapper→app boundary.
//! - **Unreadable failures.** Build errors landed inside generated code
//!   the author had never seen and couldn't grep for.
//! - **A tool in the loop.** `cargo build` alone couldn't produce a
//!   runnable app, so every workflow — CI, `cargo test`, an IDE's
//!   check-on-save — went through the CLI or didn't work.
//!
//! Now the app crate *is* the artifact. It declares one dependency
//! (this crate), writes one line in `src/main.rs`, and builds with
//! cargo like anything else.
//!
//! # Layout
//!
//! ```ignore
//! // src/lib.rs — your components, unchanged
//! pub fn app() -> Element { … }
//! pub fn register_scene_extensions<H>(registry: &mut Registry<H>) where H: … { … }
//!
//! // src/main.rs — the whole entry point
//! idealyst::entry!(my_app);
//! ```
//!
//! ```toml
//! # Cargo.toml
//! [dependencies]
//! idealyst = "1.2"
//!
//! [package.metadata.idealyst.app]
//! name      = "My App"
//! bundle_id = "com.example.myapp"
//! ```
//!
//! # Choosing a platform
//!
//! By config, never by code. The target triple settles most of it:
//!
//! ```toml
//! # .cargo/config.toml
//! [build]
//! target = "wasm32-unknown-unknown"
//! ```
//!
//! `cargo build` now produces a web app; drop the file and it produces
//! a native one. The shells that *share* a triple (a terminal app and a
//! desktop-window app are both `target_os = "macos"`) are picked by a
//! feature on this crate — `features = ["terminal"]`.
//!
//! # Choosing an allocator
//!
//! Also by config. A web bundle that is size-bound rather than
//! allocation-bound can trade throughput for ~10KB:
//!
//! ```toml
//! [package.metadata.idealyst.app]
//! allocator = "small"   # "default" (dlmalloc) | "small" (free list)
//! ```
//!
//! `entry!` emits the matching `#[global_allocator]`. See the `alloc`
//! module (wasm builds only) for what each one costs and why this is
//! metadata rather than a feature.
//!
//! # What still needs a tool
//!
//! `cargo build` produces the artifact for every platform. It does not
//! *package* one: a web app still needs `wasm-bindgen` run over the
//! `.wasm`, plus `index.html` and asset staging (and `wasm-split` +
//! `wasm-opt` for release); iOS and Android need their bundles
//! assembled. That work moved to the dev/bundle server, which watches,
//! runs `cargo build`, post-processes and serves. It is a *consumer* of
//! the build rather than a precondition for it — which is the whole
//! difference.

pub mod boot;

// Web-only: a native shell uses the system allocator, and the crates
// behind these types are wasm deps.
#[cfg(target_arch = "wasm32")]
pub mod alloc;

/// The entry-point macro. See [`macro@entry`].
pub use idealyst_macros::entry;

#[doc(inline)]
pub use boot::{AppConfig, SceneExtensions, SceneHost};

// Re-exported so an app needs exactly one framework dependency, and so
// `entry!`'s expansion can name these paths without assuming the app
// depends on them directly.
pub use runtime_core;
pub use runtime_scene;
pub use runtime_vocabulary;
