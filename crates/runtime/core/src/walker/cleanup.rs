//! RAII cleanup wrappers used by the build walker.
//!
//! Each wrapper holds an `Rc<RefCell<B>>` + a `B::Node` and calls the
//! matching `Backend::release_*` hook on drop. The walker installs
//! these by capturing them in an otherwise-empty `Effect` whose
//! lifetime matches the surrounding `Scope`; when the scope drops
//! (`when()` branch flip, list-item recycling, `Owner` teardown) the
//! effect drops, the wrapper drops, and the backend gets its release
//! callback fired exactly once. This is the only path that keeps
//! browser-queued / native-queued late events from firing into
//! freed `Signal`/`Effect` slots.
//!
//! ## Re-entrancy
//!
//! Every wrapper releases through [`release_or_defer`]: these Drops
//! can run WHILE the backend is already mutably borrowed. The
//! concrete case: `Backend::release_virtualizer` (called under the
//! walker's borrow) synchronously shuts down its data source, which
//! drops the per-row scopes — and a row that contains a graphics /
//! portal / external node (or any styled node, see `StyleHandle`)
//! re-enters the backend RefCell from ITS cleanup drop. That was the
//! iOS "SIGABRT already-borrowed on navigating away from a docs page"
//! crash. Releasing via microtask when the borrow is held is safe —
//! the release hooks are teardown notifications keyed by node.

use crate::backend::Backend;
use std::cell::RefCell;
use std::rc::Rc;

/// Run `release(backend, node)` now if the backend is borrowable,
/// else defer it one microtask (re-checking the borrow there; a
/// backend still borrowed on the microtask would mean a borrow held
/// across turns — a leak far worse than a skipped release note).
fn release_or_defer<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    release: fn(&mut B, &B::Node),
) {
    match backend.try_borrow_mut() {
        Ok(mut b) => release(&mut b, node),
        Err(_) => {
            let backend = backend.clone();
            let node = node.clone();
            crate::schedule_microtask(move || {
                if let Ok(mut b) = backend.try_borrow_mut() {
                    release(&mut b, &node);
                }
            });
        }
    }
}

/// RAII wrapper that calls `Backend::release_graphics` when dropped.
/// Installed unconditionally per Graphics primitive (i.e. doesn't
/// depend on a user-supplied style) by a dedicated cleanup `Effect`
/// in the build walker. When the surrounding scope drops — `when()`
/// branch flip, list-item recycling, `Owner` teardown — the effect
/// drops, this handle drops, and the backend tears down its wgpu
/// state.
pub(super) struct GraphicsHandleCleanup<B: Backend + 'static> {
    pub(super) backend: Rc<RefCell<B>>,
    pub(super) node: B::Node,
}

impl<B: Backend + 'static> Drop for GraphicsHandleCleanup<B> {
    fn drop(&mut self) {
        release_or_defer(&self.backend, &self.node, |b, n| b.release_graphics(n));
    }
}

/// RAII wrapper that calls `Backend::release_virtualizer` when
/// dropped. Same lifecycle shape as `GraphicsHandleCleanup`:
/// installed per Virtualizer primitive by the walker via an empty
/// `Effect`; when that effect's scope drops, the backend detaches
/// listeners + drops the closures it handed the JS shim. Critical
/// for preventing "signal used after its scope was dropped"
/// panics from late-firing scroll/resize events whose Rust
/// callbacks captured the now-freed `Signal`.
pub(super) struct VirtualizerHandleCleanup<B: Backend + 'static> {
    pub(super) backend: Rc<RefCell<B>>,
    pub(super) node: B::Node,
}

impl<B: Backend + 'static> Drop for VirtualizerHandleCleanup<B> {
    fn drop(&mut self) {
        release_or_defer(&self.backend, &self.node, |b, n| b.release_virtualizer(n));
    }
}

/// RAII wrapper that calls `Backend::release_portal` when dropped.
/// Installed per Portal primitive by a dedicated `Effect` in the
/// build walker. When the surrounding scope drops — host's
/// open-state signal flips, `when` rebuilds the surrounding branch,
/// this scope drops — the backend tears down its floating layer
/// (detaches the portal node, removes Escape/back listeners, drops
/// the wasm-bindgen / JNI closure handles wired to system dismiss
/// events).
///
/// Without this, browser-queued dismissal events or anchor-tracking
/// observers firing after the scope dropped would invoke Rust
/// callbacks against freed `Signal` / `Effect` slots — same failure
/// mode `release_virtualizer` was added to prevent.
pub(super) struct PortalHandleCleanup<B: Backend + 'static> {
    pub(super) backend: Rc<RefCell<B>>,
    pub(super) node: B::Node,
}

impl<B: Backend + 'static> Drop for PortalHandleCleanup<B> {
    fn drop(&mut self) {
        release_or_defer(&self.backend, &self.node, |b, n| b.release_portal(n));
    }
}

/// RAII guard that calls `Backend::release_external` when dropped.
/// Mirrors the portal/virtualizer cleanup pattern so third-party
/// primitives get scope-tied teardown without per-handler boilerplate.
pub(super) struct ExternalHandleCleanup<B: Backend + 'static> {
    pub(super) backend: Rc<RefCell<B>>,
    pub(super) node: B::Node,
}

impl<B: Backend + 'static> Drop for ExternalHandleCleanup<B> {
    fn drop(&mut self) {
        release_or_defer(&self.backend, &self.node, |b, n| b.release_external(n));
    }
}
