//! Ambient context that follows a reactive region into its later
//! realizations.
//!
//! A `Dyn` / `Keyed` region realizes its content twice over: once
//! synchronously at mount, inside whatever ambient state the enclosing
//! handlers set up, and again on every later driver-effect run — a branch
//! swap, a new keyed row — when that ambient state is long gone. Anything a
//! mount handler reads from a thread-local "where am I being mounted" stack
//! reads it EMPTY on those later runs.
//!
//! The robot registry is the case that forced this: its parent stack is
//! that kind of ambient state, so every element a region re-realized
//! registered as a detached root, and `find_all` returned a form step's
//! inputs in recycled-slot order instead of screen order (see the
//! vocabulary's `regression_region_rerealize_keeps_parent_and_order`).
//!
//! The seam is generic so the scene takes no dependency on any consumer:
//! a provider installed with [`install_region_context`] is asked for a
//! [`RegionScope`] when a region is CREATED (mount time, ambient state
//! intact), and the region runs every realize of its content inside it.

use std::cell::RefCell;
use std::rc::Rc;

/// Re-enter a captured ambient context around `f`. Must call `f` exactly
/// once.
pub type RegionScope = Rc<dyn Fn(&mut dyn FnMut())>;

type Provider = Rc<dyn Fn() -> RegionScope>;

thread_local! {
    static PROVIDERS: RefCell<Vec<Provider>> = const { RefCell::new(Vec::new()) };
}

/// Install a capture provider for this thread. Called at region creation;
/// the scope it returns wraps each realize of that region's content.
/// Providers accumulate (each consumer installs its own once) and nest in
/// installation order.
pub fn install_region_context(provider: impl Fn() -> RegionScope + 'static) {
    PROVIDERS.with(|p| p.borrow_mut().push(Rc::new(provider)));
}

/// The scopes captured for one region, outermost first.
#[derive(Clone, Default)]
pub(crate) struct CapturedRegion(Vec<RegionScope>);

/// Capture every installed provider's context. Cheap when none are
/// installed (one thread-local read, no allocation).
pub(crate) fn capture() -> CapturedRegion {
    let providers = PROVIDERS.with(|p| p.borrow().clone());
    CapturedRegion(providers.iter().map(|p| p()).collect())
}

impl CapturedRegion {
    /// Run `f` inside every captured scope.
    pub(crate) fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        enter_from(&self.0, f)
    }
}

fn enter_from<R>(scopes: &[RegionScope], f: impl FnOnce() -> R) -> R {
    let Some((first, rest)) = scopes.split_first() else {
        return f();
    };
    let mut f = Some(f);
    let mut out = None;
    first(&mut || {
        if let Some(f) = f.take() {
            out = Some(enter_from(rest, f));
        }
    });
    out.expect("a RegionScope must invoke the closure it is given")
}
