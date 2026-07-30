//! macOS leg — the `MacosBackend`-concrete scene handler.
//!
//! Reuses the AppKit NSToolbar machinery in [`crate::macos_shared`]
//! (delegate, toolbar construction, wipe+repopulate item updates, the
//! 0-size placeholder) and contributes only the runtime-facing half:
//!
//! - the reactive items subscription via [`runtime_world::effect`]
//!   (created during realize — the world is entered — so it runs once
//!   immediately and is collected into the enclosing subtree, dying at
//!   unmount; the effect closure's `NativeToolbar` capture is what
//!   keeps the weakly-held delegate retained), and
//! - the click-callback discipline: every toolbar-button `on_click` is
//!   wrapped so `backend_macos::newcore::schedule_flush()` runs after
//!   the author code returns. NSToolbar target-action fires OUTSIDE
//!   the runtime's wrapped dispatch sites, so without this a signal
//!   write in the click handler stays staged forever. This closes the
//!   "third-party macOS glue must call `schedule_flush` after author
//!   callbacks" residual named in `backend-macos/src/newcore.rs`'s
//!   flush-driver docs.
//!
//! # Test coverage (CLAUDE.md rule 8 note)
//!
//! The pure item translation + click-wrap ordering are unit-tested
//! (here and in `macos_shared`). The NSToolbar build/attach path
//! itself has NO tighter test: AppKit objects (NSToolbar, NSWindow,
//! the MainThreadOnly delegate) can only be constructed on the main
//! thread of a running app, and cargo-test threads are not it — the
//! same limitation every `backend-macos` imp/ module documents. The
//! live gate for that path is the `toolbar-demo` example
//! (`host_appkit::newcore::run_with` + `toolbar::register`).

use std::rc::Rc;

use backend_macos::{MacosBackend, MacosNode};
use runtime_scene::{Element, MountCx};

use crate::macos_shared;
use crate::{finish_mount, ToolbarPrim};

/// Mount handler for `Registry<MacosBackend>`: NSToolbar + delegate
/// built and attached to the host window, reactive items via a world
/// effect, 0-size layer-backed placeholder registered with the backend
/// and returned as the in-tree node, then the standard mount tail
/// (style / ref fill / `release_external` teardown).
pub(crate) fn mount_toolbar_macos(
    cx: &mut MountCx<'_, MacosBackend>,
    prim: &Rc<ToolbarPrim>,
    _children: Vec<Element>,
) -> MacosNode {
    let backend = cx.backend().clone();

    // Window lookup + toolbar build need only a shared borrow; the
    // borrow must drop before `register_external_view` (&mut) below.
    let (mtm, window) = {
        let b = backend.borrow();
        let mtm = b.mtm();
        // host_root's window is non-nil by the time handlers fire
        // (`setContentView:` runs before render starts); a windowless
        // mount degrades to a detached toolbar that never displays.
        let window = b.host_root().and_then(macos_shared::window_of_view);
        (mtm, window)
    };
    let native = macos_shared::build_native_toolbar(mtm, window.as_deref(), prim.props.visible);

    // Reactive items: `runtime_world::effect` runs once immediately and
    // re-fires when signals read inside the author's `items` closure
    // change. The NativeToolbar clone captured here anchors the delegate
    // (see macos_shared's lifetime model).
    let native_for_effect = native.clone();
    let props_for_effect = prim.props.clone();
    runtime_world::effect(move || {
        let items = (props_for_effect.items)();
        let plan = macos_shared::plan_items(items, &|cb| {
            wrap_click(cb, backend_macos::newcore::schedule_flush)
        });
        macos_shared::apply_items(&native_for_effect, plan);
    });

    // In-tree node: a zero-sized, layer-backed NSView registered with
    // the backend (frame/lifecycle bookkeeping) and returned as
    // `MacosNode::View`.
    let placeholder = macos_shared::make_placeholder_view(mtm);
    backend.borrow_mut().register_external_view(&placeholder);
    let node = MacosNode::View(placeholder);

    finish_mount(&backend, &node, prim);
    node
}

/// Wrap an author click callback so `flush` runs AFTER the author code
/// returns — mirroring `backend_macos::newcore`'s `flushing0`
/// dispatch-site wrapper (the caps impls wrap first-party callbacks
/// there; NSToolbar target-action is a third-party dispatch site the
/// backend can't see, so the wrap happens here). `flush` is
/// parameterized (production passes
/// `backend_macos::newcore::schedule_flush`) so the ordering contract
/// is unit-testable without a booted world.
fn wrap_click(cb: Rc<dyn Fn()>, flush: fn()) -> Rc<dyn Fn()> {
    Rc::new(move || {
        cb();
        flush();
    })
}

// =========================================================================
// Tests — the click-wrap ordering contract (the schedule_flush glue's
// shape, mirroring backend-macos's own `flushing_wrappers_run_author_
// code_then_queue_one_flush`). The NSToolbar attach path has no unit
// seam — see the module docs.
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static ORDER: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    fn record_flush() {
        ORDER.with(|o| o.borrow_mut().push("flush"));
    }

    /// The wrapper must run the author callback FIRST, then the flush —
    /// on EVERY invocation (a re-clicked button schedules a flush each
    /// time; the dedup lives inside `schedule_flush`, not here).
    #[test]
    fn wrap_click_runs_author_then_flush_every_invocation() {
        ORDER.with(|o| o.borrow_mut().clear());
        let wrapped = wrap_click(
            Rc::new(|| ORDER.with(|o| o.borrow_mut().push("author"))),
            record_flush,
        );
        wrapped();
        wrapped();
        ORDER.with(|o| {
            assert_eq!(
                *o.borrow(),
                vec!["author", "flush", "author", "flush"],
                "author code must complete before the flush is scheduled"
            );
        });
    }

    /// The production wiring routes clicks through
    /// `backend_macos::newcore::schedule_flush` via `plan_items`'s
    /// wrap seam. `schedule_flush` is documented safe pre-boot (no
    /// mounted world → the queued flush is a no-op), so invoking the
    /// planned record's wrapped handler here proves the full
    /// plan→wrap→author→schedule_flush chain executes without a booted
    /// app. (On a test thread with no installed scheduler,
    /// `schedule_microtask` runs synchronously on native — the flush
    /// no-ops against the empty world slot.)
    #[test]
    fn planned_clicks_reach_schedule_flush_without_a_booted_world() {
        let fired = Rc::new(RefCell::new(0u32));
        let fired_in = fired.clone();
        let plan = macos_shared::plan_items(
            vec![crate::ToolbarItem::button("Go")
                .on_click(move || *fired_in.borrow_mut() += 1)
                .into()],
            &|cb| wrap_click(cb, backend_macos::newcore::schedule_flush),
        );
        let on_click = plan.records[0].on_click.clone().expect("handler kept");
        on_click();
        assert_eq!(*fired.borrow(), 1, "author callback ran through the wrap");
    }
}
