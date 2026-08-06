//! # The backend-author surface
//!
//! One import root for writing a backend — in-tree or out-of-tree. It
//! gathers the three layers a backend spans:
//!
//! - **structure** — [`Host`] and the [`Registry`] / [`MountCx`] mount
//!   path (from `runtime-scene`),
//! - **capabilities** — the [`caps`] traits and the [`BuiltinSet`] lever
//!   (this crate),
//! - **substrate** — the boot-time installs (re-exported from
//!   [`runtime_shared::backend`], which documents the required order).
//!
//! A backend crate needs no other framework import. In particular it must
//! **not** depend on `runtime-core`: that root is the *author* surface and
//! its `glue` re-export deliberately shadows several substrate names with
//! authoring wrappers.
//!
//! ## Minimum viable backend
//!
//! 1. [`Host`] — `type Node` plus the seven structural ops. Answer
//!    `supports_splice` honestly; `false` is always correct.
//! 2. [`caps::ViewOps`], [`caps::TextOps`], [`caps::StyleOps`], and
//!    [`caps::LifecycleOps::finish`] — enough for a real tree. Every other
//!    cap trait is `impl Trait for MyBackend {}`, since all their methods
//!    have defaults.
//! 3. A boot entry generic over [`BuiltinSet`] — see [`install_env_services`]
//!    and `runtime_shared::backend`'s boot-order docs.
//! 4. The flush driver. Skipping it is the one failure that compiles,
//!    mounts, and then silently never commits an author write.
//!
//! `crates/backend/terminal/src/newcore.rs` is the smallest complete
//! in-tree worked example; `docs/backend.md` is the long-form contract.

use std::cell::RefCell;
use std::rc::Rc;

// --- structure -------------------------------------------------------------
pub use runtime_scene::{realize, Element, Host, MountCx, Realized, Registry};

/// The reactive kernel a boot entry owns. Realize inside `world.enter(…)`
/// so free `signal()` / `effect()` calls in the root build resolve, and
/// keep the `World` alive for as long as the `Realized` — its field order
/// is its drop order, and the tree must unmount before the slots it reads.
pub use runtime_world::World;

// --- capabilities ----------------------------------------------------------
pub use crate::caps;
pub use crate::caps::AllCaps;
pub use crate::handlers::{register_builtins_with, AllBuiltins, BuiltinSet};

// --- substrate -------------------------------------------------------------
pub use runtime_shared::backend::*;

/// The substrate crate itself, so a backend needs exactly ONE framework
/// dependency (`runtime-vocabulary`) to reach every value type in the
/// capability signatures — `StyleRules`, `AccessibilityProps`, `Action`,
/// `IconData`, `primitives::*`, `animation::AnimProp`, and the rest. A
/// backend that would rather name `runtime-shared` directly can; it is a
/// public crate and the paths are identical.
pub use runtime_shared;

/// Wire the booting backend's environment capabilities into the ambient
/// thread-locals that author code reads.
///
/// Five author-facing free functions — `platform()`, `color_scheme()`,
/// `open_url()`, `set_fullscreen()`, `announce()` — are readable from any
/// component body, effect, or event handler without a backend reference.
/// They read thread-local slots, and this is what fills those slots from
/// the backend's own [`caps::AppEnvOps`] / [`caps::A11yOps`] impls.
///
/// **Call it from every boot entry, before the root build.** A component
/// body may read `platform()` while constructing, so seeding it after the
/// build is too late. Nothing else calls it: there is no central `mount()`
/// to hang it on — the backend's boot entry *is* the mount path.
///
/// A backend that leaves a capability at its trait default (`url_opener`
/// → `None`, etc.) gets the documented no-op behavior for that one
/// service; the others still work.
///
/// ## Why it is unconditional
///
/// This anchors the backend's `url_opener` / `fullscreen_setter` closures
/// and the AX announce path in every bundle, including one that never
/// calls them — the linker cannot prove an author never reaches
/// `open_url()`, because the install is what makes it reachable.
///
/// It is deliberately NOT routed through a `BuiltinSet` gate the way
/// `nav_services` is. A `BuiltinSet` selects *primitive families*, and
/// these five reads are not tied to a primitive: `open_url()` is callable
/// from an app whose whole vocabulary is `view` + `text`. Gating them
/// would re-introduce exactly the silent no-op this seam exists to fix,
/// for the apps least likely to notice.
///
/// Measured cost on a release wasm build of `benchmark/idealyst-native`
/// (raw `cargo build --release --target wasm32-unknown-unknown`, before
/// the CLI's wasm-opt/wasm-split passes, which shrink it further):
/// 1,736,430 → 1,740,405 bytes, **+3,975 (+0.23%)**. That is the price of
/// four author APIs that were no-ops on every backend.
///
/// ## Why the announcer is different
///
/// `url_opener` / `fullscreen_setter` hand back self-contained closures —
/// opening a URL is a stateless platform call on every backend, so the
/// closure captures platform handles, never the view tree.
/// `A11yOps::announce_for_accessibility` takes `&mut self`, so the
/// announcer has to hold the backend. It is captured **weakly**: the
/// thread-local outlives the app (it is never cleared on teardown), so a
/// strong `Rc` would pin the backend and its entire view tree for the
/// life of the thread. After teardown the upgrade fails and `announce()`
/// degrades to the same no-op as an AX-less backend.
pub fn install_env_services<H>(backend: &Rc<RefCell<H>>)
where
    H: caps::AppEnvOps + caps::A11yOps + 'static,
{
    {
        let b = backend.borrow();
        runtime_shared::host::install_current_platform(caps::AppEnvOps::platform(&*b));
        runtime_shared::host::install_current_color_scheme(caps::AppEnvOps::color_scheme(&*b));
        runtime_shared::host::install_url_opener(caps::AppEnvOps::url_opener(&*b));
        runtime_shared::host::install_fullscreen_setter(caps::AppEnvOps::fullscreen_setter(&*b));
    }

    // Weak, and re-entrancy-safe: the borrow is confined to the forward
    // call. An announcer that re-entered framework code while we held a
    // borrow would trip a RefCell double-borrow on the backend — the same
    // reason `announce()` itself clones the Rc out before invoking.
    let weak = Rc::downgrade(backend);
    runtime_shared::host::install_announcer(Some(Rc::new(
        move |msg: &str, priority: runtime_shared::accessibility::LiveRegionPriority| {
            if let Some(backend) = weak.upgrade() {
                caps::A11yOps::announce_for_accessibility(&mut *backend.borrow_mut(), msg, priority);
            }
        },
    )));
}
