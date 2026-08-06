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
// A native target with no shell feature has no `run` to offer. Rather
// than omit it (leaving "cannot find function `run`", which says nothing
// about the actual mistake) this arm defines `run` with an unsatisfiable
// bound. The error then fires *at the call site*, with a message naming
// the fix — and `cargo check -p idealyst` on a bare native build stays
// green, because the bound is only checked when someone calls it.
// ---------------------------------------------------------------------

#[cfg(all(not(target_arch = "wasm32"), not(feature = "terminal")))]
mod no_shell {
    /// Never implemented for anything. See the module comment.
    #[diagnostic::on_unimplemented(
        message = "no idealyst shell is selected for this target",
        label = "needs a shell",
        note = "this is a NATIVE build (no `--target wasm32-unknown-unknown`).",
        note = "For a web app, set the target in .cargo/config.toml:",
        note = "    [build]",
        note = "    target = \"wasm32-unknown-unknown\"",
        note = "For a terminal app, enable the shell feature in Cargo.toml:",
        note = "    idealyst = { version = \"1.2\", features = [\"terminal\"] }"
    )]
    pub trait ShellSelected {}

    /// The type that never satisfies [`ShellSelected`].
    pub struct NoShell;
}

// The bound sits on the GENERIC parameter, not on a concrete type.
// `where NoShell: ShellSelected` would be a "trivial bound" — rustc
// evaluates predicates over concrete types eagerly and rejects the
// definition itself (E0277), so the error would fire for anyone merely
// compiling this crate. Hanging it on `E` defers the check to the call
// site, which is where the mistake actually is.
#[cfg(all(not(target_arch = "wasm32"), not(feature = "terminal")))]
pub fn run<E, S: runtime_vocabulary::BuiltinSet>(_app: impl FnOnce() -> Element, _config: AppConfig)
where
    E: SceneExtensions + no_shell::ShellSelected,
{
    unreachable!("unsatisfiable bound — this body is never reachable")
}
