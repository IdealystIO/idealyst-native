//! `on_cleanup` semantics — registration order, run order, lifetimes.
//!
//! `on_cleanup(callback)` registers a teardown that fires before the
//! surrounding Effect's next re-run AND on final disposal. Catching
//! regressions here matters: callers rely on cleanup for resource
//! release (timers, sockets, native handles), and a missed cleanup is
//! a silent leak.

use std::cell::RefCell;
use std::rc::Rc;

use framework_core::{on_cleanup, signal, Effect, Signal};

/// Cleanup fires before the effect's next re-run.
#[test]
fn cleanup_fires_before_re_run() {
    let trigger: Signal<i32> = signal!(0);
    let trace: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));

    let trace_for_effect = trace.clone();
    let _e = Effect::new(move || {
        trace_for_effect.borrow_mut().push("body");
        let _ = trigger.get();
        let trace_for_cleanup = trace_for_effect.clone();
        on_cleanup(move || {
            trace_for_cleanup.borrow_mut().push("cleanup");
        });
    });

    // Initial run: body once, no cleanup yet (cleanup runs before
    // the NEXT body, not at the end of this one).
    assert_eq!(*trace.borrow(), vec!["body"]);

    trigger.set(1);
    assert_eq!(
        *trace.borrow(),
        vec!["body", "cleanup", "body"],
        "cleanup fires before the next body re-run"
    );

    trigger.set(2);
    assert_eq!(
        *trace.borrow(),
        vec!["body", "cleanup", "body", "cleanup", "body"]
    );
}

/// Cleanup fires when the Effect's owning scope drops, even without a
/// preceding re-run.
#[test]
fn cleanup_fires_on_effect_drop() {
    let fired: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let fired_for_cleanup = fired.clone();

    {
        let _e = Effect::new(move || {
            let fired = fired_for_cleanup.clone();
            on_cleanup(move || {
                *fired.borrow_mut() = true;
            });
        });
        assert_eq!(*fired.borrow(), false, "not yet fired");
        // Effect dropped at end of block.
    }

    assert_eq!(*fired.borrow(), true, "cleanup fires on Effect drop");
}

/// Multiple `on_cleanup` calls within one Effect run all fire on
/// re-run, in LIFO order (last-registered first).
#[test]
fn multiple_cleanups_fire_in_lifo_order() {
    let trigger: Signal<i32> = signal!(0);
    let order: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));

    let order_for_effect = order.clone();
    let _e = Effect::new(move || {
        let _ = trigger.get();
        for i in 0..4 {
            let o = order_for_effect.clone();
            on_cleanup(move || {
                o.borrow_mut().push(i);
            });
        }
    });

    // First run registers four cleanups but doesn't fire them.
    assert!(order.borrow().is_empty());

    trigger.set(1);
    // Before the second body, all four cleanups fire in LIFO order.
    assert_eq!(
        *order.borrow(),
        vec![3, 2, 1, 0],
        "cleanups fire LIFO: last-registered first"
    );
}

/// Cleanup registered on run N runs before run N+1's body, NOT after.
/// And the cleanup registered on run N+1 doesn't run yet — it'll
/// run before N+2 or on disposal.
#[test]
fn cleanup_does_not_double_fire() {
    let trigger: Signal<i32> = signal!(0);
    let counter: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));

    let counter_for_effect = counter.clone();
    let _e = Effect::new(move || {
        let _ = trigger.get();
        let c = counter_for_effect.clone();
        on_cleanup(move || {
            *c.borrow_mut() += 1;
        });
    });

    assert_eq!(*counter.borrow(), 0);
    trigger.set(1);
    assert_eq!(*counter.borrow(), 1, "one cleanup ran before run 2");
    trigger.set(2);
    assert_eq!(*counter.borrow(), 2, "one cleanup ran before run 3");
    trigger.set(3);
    assert_eq!(*counter.borrow(), 3);
}

/// Framework behavior: outside a render scope, `Effect::new` returns
/// a handle that owns the slot — dropping the handle frees the slot
/// and fires its cleanups. So a nested `Effect::new(...)` whose
/// returned handle isn't held drops at end of its lexical block,
/// firing cleanups immediately.
///
/// Inside a render scope (where `Effect::new` slots are adopted by
/// the active `Scope`), the returned handle's drop is a no-op and
/// the Effect lives until the scope drops. That path is covered by
/// the walker tests; this test pins the standalone-handle behavior.
#[test]
fn nested_effect_outside_scope_drops_at_block_end() {
    let parent_trigger: Signal<i32> = signal!(0);
    let nested_cleanups: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));

    let nested_for_effect = nested_cleanups.clone();
    let _e = Effect::new(move || {
        let _ = parent_trigger.get();
        // The inner Effect's handle is dropped at end of this block.
        // Without a render scope to adopt it, the handle owns the
        // slot — drop = fire cleanups + free.
        let _nested = {
            let nested = nested_for_effect.clone();
            Effect::new(move || {
                let n = nested.clone();
                on_cleanup(move || {
                    *n.borrow_mut() += 1;
                });
            })
        };
        // _nested is dropped here, firing its cleanup.
    });

    // Initial parent run: nested created, body ran, cleanup fired
    // when nested handle dropped at the inner-block boundary.
    assert_eq!(
        *nested_cleanups.borrow(),
        1,
        "nested Effect drop fires cleanup at end of its lexical scope"
    );

    parent_trigger.set(1);
    assert_eq!(*nested_cleanups.borrow(), 2, "parent re-run creates + drops a new nested Effect");

    parent_trigger.set(2);
    assert_eq!(*nested_cleanups.borrow(), 3);
}

/// Cleanup registered outside an Effect — i.e. called at the top
/// level of a render scope but not inside an `Effect::new` body — is
/// a no-op (or specifically: not attached to an Effect, can't fire
/// from one). The framework should not panic in that case.
#[test]
fn cleanup_outside_effect_does_not_panic() {
    // No scope, no effect — just a top-level call. Should be a
    // graceful no-op.
    on_cleanup(|| {
        // Never invoked — there's no parent reactive context.
    });
}
