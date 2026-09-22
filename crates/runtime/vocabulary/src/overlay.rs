//! The dev-time **overlay**: the runtime half of `ui-overlay`.
//!
//! `ui!` has one lowering. What this adds, behind a cargo feature that
//! is off by default, is the ability to *address* what that lowering
//! built: every site registers its [`Descriptor`] and every node it
//! builds carries a [`NodeTag`]. A patch is a replacement descriptor for
//! one site; applying it edits the built `Element` in place.
//!
//! # Why a feature and not `cfg(debug_assertions)`
//!
//! An over-the-air path would want this in a release build. A feature
//! can be turned on there; `debug_assertions` cannot.
//!
//! # Why re-apply on every build
//!
//! A site's `Element` is rebuilt whenever its reactive scope re-runs — a
//! branch flips, a keyed row is re-rendered, a component re-renders. A
//! patch applied once to a tree that is then rebuilt from the compiled
//! code would silently revert. So [`apply`] runs at the point the site
//! produces its `Element`, every time, and is a no-op when no patch is
//! registered for that site.
//!
//! # Why tags and not positions
//!
//! A reactive branch, a keyed row or a `for` changes how many siblings
//! exist, so "the n-th child of this parent" is not a stable identity
//! across rebuilds. The tag is: it names the descriptor node that
//! produced this `Element`. Everything here looks nodes up by tag —
//! dynamic slots above all, which is what makes "patch the static half,
//! leave the code alone" sound.

use std::cell::RefCell;
use std::collections::HashMap;

use runtime_scene::Element;
use runtime_template::{Descriptor, Patch, Registry, SiteId};

thread_local! {
    /// Descriptors registered by the sites built so far, and the patches
    /// staged against them.
    ///
    /// Thread-local, not global: a scene is single-threaded (the whole
    /// reactive kernel is), and a `Registry` is not `Sync`.
    static STATE: RefCell<OverlayState> = RefCell::new(OverlayState::default());
}

#[derive(Default)]
struct OverlayState {
    registry: Registry,
    /// Staged patches by site hash — the same key the macro spells into
    /// each node's tag, so a tagged `Element` finds its patch without
    /// reconstructing a `SiteId`.
    patches: HashMap<String, Patch>,
}

/// Register a site's compiled descriptor.
///
/// Called at the head of every build of the site (a `static` cannot run
/// code, and this crate will not take a `ctor`/`linkme` dependency for a
/// dev-time feature). Idempotent and cheap: an occupancy check, then at
/// most one clone per site per process.
pub fn register(descriptor: &'static Descriptor, hash: &'static str) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.registry.contains(&descriptor.site) {
            return;
        }
        s.registry.register(descriptor.clone());
        let _ = hash;
    });
}

/// Attach an origin tag to a freshly built node.
///
/// Emitted around every node's expression under `ui-overlay`. Delegates
/// to `runtime_scene::with_tag`, which knows which `Element` variants
/// are nodes and which are structure.
///
/// Takes `impl IntoElement`, not `Element`: a primitive's expression is
/// still its BUILDER at that point (`GlueText`, `GlueView`, …) and only
/// coerces at the site's edge. Tagging has to accept the builder and do
/// the coercion itself — which does mean a tagged node reaches its
/// parent already an `Element` rather than a builder. That is
/// invisible to authors (every consuming position takes `impl
/// IntoElement` or `ChildList`) and is the one shape difference the
/// feature makes.
pub fn tag(element: impl crate::glue::IntoElement, site: &'static str, node: u32) -> Element {
    runtime_scene::with_tag(
        crate::glue::IntoElement::into_element(element),
        runtime_scene::NodeTag { site, node },
    )
}

/// Stage a patch, after checking it against the registered descriptor.
///
/// Returns the validation error rather than panicking: a bad patch is a
/// dev-loop event, not a program bug, and the running app must keep
/// serving the un-patched tree.
pub fn stage(patch: Patch) -> Result<(), runtime_template::ValidationError> {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        runtime_template::validate(&patch, &s.registry)?;
        s.patches.insert(patch.site.hash.to_string(), patch);
        Ok(())
    })
}

/// Drop a staged patch, reverting that site to its compiled form on the
/// next build.
pub fn unstage(site: &SiteId) {
    STATE.with(|s| {
        s.borrow_mut().patches.remove(site.hash.as_ref());
    });
}

/// How many sites are registered / patched. For the suite and for a
/// dev-tools readout.
pub fn stats() -> (usize, usize) {
    STATE.with(|s| {
        let s = s.borrow();
        (s.registry.len(), s.patches.len())
    })
}

/// Forget everything. Test-support: the state is thread-local and
/// per-process, so a suite that registers sites in one case must not
/// leak them into the next.
pub fn reset() {
    STATE.with(|s| *s.borrow_mut() = OverlayState::default());
}
