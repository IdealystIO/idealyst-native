//! Build and run an idealyst app with plain `cargo build`.
//!
//! # Why the app crate is the artifact
//!
//! The app crate declares one dependency (this crate), writes one line in
//! `src/main.rs`, and builds with cargo like anything else — no generated
//! wrapper crate owning the entry point and boot sequence. A wrapper
//! resolves the framework in a second dependency graph (any disagreement
//! yields two `runtime_core` crates and "expected `Element`, found
//! `Element`"), puts build errors in generated code the author never saw,
//! and keeps `cargo build` / `cargo test` / check-on-save from producing a
//! runnable app without the CLI.
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
//! *package* one: a web app needs its JS written from the `.wasm` (the
//! web-glue pass; the `wasm-bindgen` CLI too, only for an app that still
//! links wasm-bindgen), plus `index.html` and asset staging (and
//! `wasm-split` + `wasm-opt` for release); iOS and Android need their bundles
//! assembled. That work is the dev/bundle server's: it watches,
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
