//! Author callbacks handed to a backend must go inert when the scope that
//! produced them is torn down.
//!
//! # What this pins, and why it is not a backend's problem to solve
//!
//! A backend receives `on_press` / `on_activate` / `on_change` /
//! `on_scroll` / the `attach_states` setter as plain `Rc<dyn Fn…>` and
//! stores them on native objects whose lifetime the toolkit owns. Those
//! objects outlive the mounting scope, and toolkits invoke them anyway:
//! GTK emits `focus-leave` *while* a focused widget is being unparented;
//! a callback deferred to a run-loop source fires after a route change
//! dropped the screen; a gesture recognizer survives its view.
//!
//! The closure then writes a freed signal slot and `runtime_world` raises
//! `idealyst[stale-signal-handle]`. That panic is correct, but it is
//! raised on a stack the framework does not own — a GObject signal
//! trampoline, an objc callback — which is `extern "C"` and cannot
//! unwind. The process ABORTS rather than reporting a panic. Observed on
//! GTK as:
//!
//! ```text
//! panicked: signal used through a stale handle (world 1, slot 139)
//! ...EventControllerFocus::connect_leave::leave_trampoline
//! panic in a function that cannot unwind
//! ```
//!
//! Fixing it per backend means N implementations with N sets of coverage
//! gaps, and a backend cannot do it properly anyway — it has no way to
//! ask whether the producing scope is still alive. So the framework hands
//! over callbacks that are already guarded, and this test holds that line
//! for every primitive that has one.
//!
//! The assertions deliberately invoke the callbacks the way a backend
//! would: through the handle the harness retained at mount, AFTER the
//! subtree is gone. Under the bug each of these aborts the test binary.

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_shared::{Length, StateBits, StyleRules, StyleSheet};
use runtime_vocabulary::builders::{link, pressable, scroll_view, text_input, toggle, view};
use runtime_world::{signal, Signal};

/// Mount `body` inside a structural hole, then return a closure that
/// unmounts it. The hole is what makes this a MID-LIFE teardown — the
/// world stays alive, only the inner scope dies, which is the shape a
/// route change has. Tearing the whole world down instead does NOT
/// reproduce the bug: everything goes at once and the write is
/// short-circuited for unrelated reasons.
struct Mounted {
    _realized: runtime_scene::Realized<host_mock::Node>,
    shown: Signal<bool>,
}

fn mount_in_hole(
    h: &Harness,
    body: impl Fn() -> runtime_scene::Element + 'static,
) -> Mounted {
    let slot: Rc<RefCell<Option<Signal<bool>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let realized = h.mount({
        let element = h.world.enter(|| {
            let shown = signal(true);
            *slot_for_build.borrow_mut() = Some(shown);
            view()
                .child(move || {
                    if shown.get() {
                        body()
                    } else {
                        view().build()
                    }
                })
                .build()
        });
        element
    });
    let shown = slot.borrow().expect("hole built");
    Mounted { _realized: realized, shown }
}

/// A sheet with a `hover` state axis, so the node takes the event-driven
/// state path and the backend is handed a real setter.
fn stateful_sheet() -> Rc<StyleSheet> {
    Rc::new(
        StyleSheet::new(|_| StyleRules {
            width: Some(Length::Px(10.0).into()),
            ..Default::default()
        })
        .variant("__state_hovered", "on", |_| StyleRules {
            width: Some(Length::Px(20.0).into()),
            ..Default::default()
        }),
    )
}

#[test]
fn regression_press_and_link_callbacks_go_inert_after_unmount() {
    let h = Harness::new();
    let hits = Rc::new(RefCell::new(Vec::<&'static str>::new()));
    let hits_for_build = hits.clone();

    let mounted = mount_in_hole(&h, move || {
        // `inner` is owned by the HOLE's scope, so it is freed when the
        // hole rebuilds — exactly the signal a stale callback would write.
        let inner = signal(0i32);
        let hp = hits_for_build.clone();
        let hl = hits_for_build.clone();
        view()
            .children(vec![
                pressable(move || {
                    hp.borrow_mut().push("press");
                    inner.set(inner.get() + 1);
                })
                .build(),
                link()
                    .on_activate(move || {
                        hl.borrow_mut().push("link");
                        inner.set(inner.get() + 1);
                    })
                    .build(),
            ])
            .build()
    });
    h.world.flush();

    // Backends retain these; grab them the way a backend holds them.
    let press = h.press_handler(0);
    let activate = h.link_activation(0);

    // Sanity: while mounted they really do fire, so a green result below
    // means "guarded", not "never wired".
    press();
    activate();
    h.world.flush();
    assert_eq!(*hits.borrow(), ["press", "link"], "callbacks must work while mounted");

    // Unmount the subtree. The world survives; the inner scope does not.
    mounted.shown.set(false);
    h.world.flush();
    hits.borrow_mut().clear();

    // A backend firing a retained callback now must be a NO-OP. Before the
    // fix each of these panicked with `stale-signal-handle` — and in a real
    // backend, aborted.
    press();
    activate();
    h.world.flush();
    assert!(
        hits.borrow().is_empty(),
        "a callback whose scope was torn down must not run: it would write a \
         freed signal slot, and that panic is raised inside a non-unwinding C \
         trampoline, which aborts the process",
    );
}

#[test]
fn regression_state_setter_goes_inert_after_unmount() {
    // The setter is framework-BUILT (not an author callback) and writes a
    // signal owned by the styled node's own scope. It is the one that
    // actually crashed on GTK: `focus-leave` fires while the framework
    // unparents a focused widget, i.e. during that scope's teardown.
    let h = Harness::new();
    h.shared.handles_states_natively.set(false);

    let mounted = mount_in_hole(&h, || view().style(stateful_sheet()).build());
    h.world.flush();

    let setter = h.state_setter(0);
    setter(StateBits::HOVERED, true);
    h.world.flush();

    mounted.shown.set(false);
    h.world.flush();

    // Must not panic. Both directions, because teardown-time focus and
    // hover events come in as `false` far more often than `true`.
    setter(StateBits::HOVERED, false);
    setter(StateBits::HOVERED, true);
    setter(StateBits::PRESSED, false);
    setter(StateBits::FOCUSED, false);
    h.world.flush();
}

#[test]
fn regression_scroll_toggle_and_input_callbacks_go_inert_after_unmount() {
    // `on_scroll` is the worst case: some backends MUST defer it to a
    // run-loop source (an inline call re-enters the reactive runtime
    // mid-allocation), so delivery after teardown is routine rather than
    // exceptional.
    let h = Harness::new();
    let hits = Rc::new(RefCell::new(Vec::<&'static str>::new()));
    let hits_for_build = hits.clone();

    let mounted = mount_in_hole(&h, move || {
        let inner = signal(0i32);
        let hs = hits_for_build.clone();
        let ht = hits_for_build.clone();
        let hi = hits_for_build.clone();
        view()
            .children(vec![
                scroll_view()
                    .on_scroll(move |_x, _y| {
                        hs.borrow_mut().push("scroll");
                        inner.set(inner.get() + 1);
                    })
                    .build(),
                toggle()
                    .on_change(move |_v| {
                        ht.borrow_mut().push("toggle");
                        inner.set(inner.get() + 1);
                    })
                    .build(),
                text_input()
                    .on_change(move |_v| {
                        hi.borrow_mut().push("input");
                        inner.set(inner.get() + 1);
                    })
                    .build(),
            ])
            .build()
    });
    h.world.flush();

    let scroll = h.scroll_handler(0).expect("scroll handler retained");
    let toggle_change = h.toggle_change(0);
    let input_change = h.text_input_change(0);

    scroll(1.0, 2.0);
    toggle_change(true);
    input_change("x".into());
    h.world.flush();
    assert_eq!(
        *hits.borrow(),
        ["scroll", "toggle", "input"],
        "callbacks must work while mounted",
    );

    mounted.shown.set(false);
    h.world.flush();
    hits.borrow_mut().clear();

    scroll(3.0, 4.0);
    toggle_change(false);
    input_change("y".into());
    h.world.flush();
    assert!(
        hits.borrow().is_empty(),
        "every retained callback must be inert once its scope is gone",
    );
}

/// The guard must not be so eager that it kills callbacks belonging to
/// nodes that are still on screen.
///
/// A keyed list's structural driver is ONE effect that re-runs on every
/// edit to the list, while keyed reconcile deliberately preserves the
/// surviving rows' subtrees. The token is anchored with
/// `runtime_world::on_owned_drop`, which binds to the row's own `Owned` —
/// dropped by reconcile if and only if the row really goes. It was written
/// that way because `on_scope_drop` used to see the driver effect from
/// inside a row's mount and defer to `on_cleanup`, so the first unrelated
/// edit silently made every live row's buttons inert. A row now renders
/// `unanchored` (no effect visible during a build), so both hooks would
/// behave here; `on_owned_drop` states the intent without depending on
/// that.
///
/// Both directions are asserted on purpose — deleting the guard outright
/// would satisfy the survival half while breaking the inert half.
#[test]
fn callbacks_survive_a_keyed_reconcile() {
    let h = Harness::new();
    // Splice support is what makes keyed reconcile PRESERVE surviving rows.
    // Without it the host takes the anchored fallback (clear_children +
    // rebuild every row), where per-row state is lost by contract and a
    // dead row-1 callback would be entirely correct.
    h.shared.splice.set(true);
    let hits = Rc::new(RefCell::new(Vec::<i32>::new()));
    let hits_for_build = hits.clone();

    let list = h.world.enter(|| signal(vec![1, 2]));
    let element = h.world.enter(move || {
        view()
            .children(vec![runtime_scene::keyed(
                move || list.get(),
                |n| *n,
                move |n: i32| {
                    let hp = hits_for_build.clone();
                    // Row-OWNED state: freed with the row, so a callback
                    // that outlived its row would read a stale slot.
                    let own = signal(n);
                    pressable(move || hp.borrow_mut().push(own.get())).build()
                },
            )])
            .build()
    });
    let _realized = h.mount(element);
    h.world.flush();

    // A backend retains row 1's handler at mount and keeps calling it.
    let row1 = h.press_handler(0);
    row1();
    h.world.flush();
    assert_eq!(*hits.borrow(), [1], "callback fires while its row is mounted");
    hits.borrow_mut().clear();

    // Append a row: the driver effect re-runs, rows 1 and 2 are preserved.
    h.clear_ops();
    list.set(vec![1, 2, 3]);
    h.world.flush();
    let ops = h.ops();
    assert!(
        !ops.iter().any(|o| o.starts_with("clear_children")),
        "precondition: the append must SPLICE, not rebuild — otherwise row 1 \
         really is gone and this test proves nothing:\n{ops:?}",
    );

    row1();
    h.world.flush();
    assert_eq!(
        *hits.borrow(),
        [1],
        "a surviving row's callback must outlive the keyed driver's re-run",
    );
    hits.borrow_mut().clear();

    // Now actually drop row 1 — the guard's real job. Its retained handler
    // must go inert rather than read `own` out of a freed slot.
    list.set(vec![2, 3]);
    h.world.flush();

    row1();
    h.world.flush();
    assert!(
        hits.borrow().is_empty(),
        "a removed row's retained callback must be inert",
    );
}
