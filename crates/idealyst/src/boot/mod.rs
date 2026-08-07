//! Per-platform boot sequences — the bodies the CLI used to generate.
//!
//! Each submodule holds one platform's `run`, and `run` is re-exported
//! from here under whichever `cfg` selects that platform. That keeps
//! platform choice in **one place** (this file's `cfg` arms) instead of
//! smeared across a build tool, and it keeps the boot sequence in
//! ordinary source you can read, test, and set a breakpoint in.
//!
//! The uniform signature across platforms is:
//!
//! ```ignore
//! pub fn run<E: SceneExtensions, S: BuiltinSet>(app: impl FnOnce() -> Element, config: AppConfig)
//! ```
//!
//! `S` is the set of builtin primitives the app registers. Anything
//! outside it is never named, so LLVM drops its handler along with the
//! imports and JS glue only that handler reached — measured at
//! 195,255 → 126,813 bytes brotli on a `view`+`text` app. `entry!`
//! reads it from `[package.metadata.idealyst.app].primitives` and
//! defaults to the full vocabulary. Native shells ignore it: it exists
//! to shrink a shipped bundle, and a native binary has none.

use runtime_scene::Registry;
// Only the "no shell selected" arm below names `Element` directly —
// every real platform's `run` lives in its own module.
#[cfg(all(not(target_arch = "wasm32"), not(feature = "terminal")))]
use runtime_scene::Element;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::run;

#[cfg(all(not(target_arch = "wasm32"), feature = "terminal"))]
mod terminal;
#[cfg(all(not(target_arch = "wasm32"), feature = "terminal"))]
pub use terminal::run;

/// What a scene backend must provide for an app to boot on it.
///
/// An alias trait rather than a bound repeated at every call site: the
/// app's own `register_scene_extensions` carries these same supertraits,
/// and having one name for the set means adding a capability to the
/// seam is a one-line change here instead of an edit in every app.
pub trait SceneHost:
    runtime_scene::Host
    + runtime_vocabulary::style_attach::StyleServices
    + runtime_vocabulary::caps::InputOps
    + 'static
{
}

impl<T> SceneHost for T where
    T: runtime_scene::Host
        + runtime_vocabulary::style_attach::StyleServices
        + runtime_vocabulary::caps::InputOps
        + 'static
{
}

/// The app's scene-registry seam, lifted to a type.
///
/// SDK payloads (tables, forms, canvases, …) lower to scene items whose
/// handlers must be registered before realize — an unregistered payload
/// panics naming a raw `TypeId`. Apps expose that as a *generic*
/// function, `register_scene_extensions<H>(&mut Registry<H>)`, because
/// each backend realizes them differently.
///
/// A generic function can't be passed as a value, so a uniform
/// `run(app, register, config)` is impossible. Implementing this trait
/// on a zero-sized type moves the genericity into the type system,
/// where each platform's `run` monomorphizes it against its own
/// backend. `entry!` generates the impl; hand-written entry points
/// implement it directly.
pub trait SceneExtensions {
    fn register<H>(registry: &mut Registry<H>)
    where
        H: SceneHost;
}

/// Values read from `[package.metadata.idealyst.app]` at compile time.
///
/// Populated by `entry!`, which can see the consuming crate's
/// `Cargo.toml`. Fields are `&'static str` because they're baked in —
/// there is no runtime source for them on a platform like wasm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppConfig {
    /// Human-readable app name. Window title, task-switcher label.
    pub name: &'static str,
    /// Reverse-DNS identity (`com.example.app`). Used by the platforms
    /// that require one — macOS/iOS bundles, Android packages.
    pub bundle_id: &'static str,
    /// Web only: the CSS selector this app mounts into.
    pub mount_selector: &'static str,
    /// Terminal only: px per character cell, for translating the layout
    /// engine's pixel geometry to a character grid. `None` leaves the
    /// natural 1px = 1 cell.
    pub cell_size: Option<(f32, f32)>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            name: "idealyst-app",
            bundle_id: "ai.idealyst.app",
            mount_selector: "#app",
            cell_size: None,
        }
    }
}

// ---------------------------------------------------------------------
// No shell selected.
//
// A native target with no shell feature has no `run` to offer. This
// arm still COMPILES and fails at run time, deliberately.
//
// The compile-time version (an unsatisfiable trait bound with
// `#[diagnostic::on_unimplemented]`) was strictly worse in practice:
// building the app's binary for the host is completely routine —
// `cargo test`, `cargo check`, and every rust-analyzer save do it — so
// a hard error there breaks the app's own test suite and reddens the
// IDE for a web app that is configured perfectly correctly. Failing at
// run time costs nothing (a native binary of a web app was never going
// to do anything useful) and the message is just as actionable.
// ---------------------------------------------------------------------

#[cfg(all(not(target_arch = "wasm32"), not(feature = "terminal")))]
pub fn run<E: SceneExtensions, S: runtime_vocabulary::BuiltinSet>(
    _app: impl FnOnce() -> Element,
    _config: AppConfig,
) {
    panic!(
        "no idealyst shell is selected for this target.\n\n\
         This binary was built for the HOST, not for a shell that can \
         display anything.\n\n\
         For a web app, set the target in .cargo/config.toml:\n    \
         [build]\n    target = \"wasm32-unknown-unknown\"\n\n\
         For a terminal app, enable the shell feature in Cargo.toml:\n    \
         idealyst = {{ version = \"1.2\", features = [\"terminal\"] }}"
    )
}
