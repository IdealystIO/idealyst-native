//! `view` / `pressable` `on_focus` — the keyboard focus-within channel.
//!
//! What these pin: the author's closure is the one
//! `InputOps::install_focus_handler` receives, on the node it belongs to;
//! an absent `on_focus` installs nothing; the vocabulary forwards only
//! transitions (backends report raw focus-in / focus-out, with repeats);
//! the handler dies with its scope; and a repeat row carrying `on_focus`
//! still gets it. The platform sources on the far side (DOM
//! `focusin`/`focusout`, AppKit first-responder changes, GTK
//! `EventControllerFocus`, …) all call exactly the handler fired here.

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_scene::realize;
use runtime_vocabulary::builders::{pressable, text, view};

fn harness() -> Harness {
    let h = Harness::new();
    h.record_all();
    h
}

fn recorder() -> (Rc<RefCell<Vec<bool>>>, impl Fn(bool) + 'static) {
    let seen: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
    let s = seen.clone();
    (seen, move |f| s.borrow_mut().push(f))
}

#[test]
fn view_without_on_focus_installs_no_focus_handler() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(&h.backend, &h.registry, view().child(pressable(|| {}).build()).build())
    });
    let log = h.take_log();
    assert!(
        !log.iter().any(|l| l.starts_with("install_focus_handler")),
        "no on_focus, no subscription: {log:?}"
    );
    drop(realized);
}

#[test]
fn view_on_focus_installs_on_the_view_and_reaches_the_author() {
    let h = harness();
    let (seen, rec) = recorder();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view().on_focus(rec).child(pressable(|| {}).build()).build(),
        )
    });
    let (node, handler) = h.focus_within_handler(0);
    assert_eq!(node, 0, "installed on the wrapping view (the first node made)");
    handler(true);
    handler(false);
    assert_eq!(seen.borrow().as_slice(), [true, false]);
    drop(realized);
}

#[test]
fn pressable_on_focus_installs_on_the_pressable_node() {
    let h = harness();
    let (seen, rec) = recorder();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            pressable(|| {}).on_focus(rec).child(text().content("go")).build(),
        )
    });
    let (node, handler) = h.focus_within_handler(0);
    let pressables: Vec<_> = h.shared.press_by_node.borrow().keys().copied().collect();
    assert_eq!(pressables, vec![node], "installed on the pressable's own node");
    handler(true);
    assert_eq!(seen.borrow().as_slice(), [true]);
    drop(realized);
}

/// Backends report raw focus-in / focus-out: macOS resigns the old
/// responder (`false`) before the new one becomes (`true`) when focus moves
/// between two children of the same wrapper, and a DOM / GTK focus-in on a
/// mouse-focused child reports `false` while nothing was reported focused.
/// The author sees transitions only.
#[test]
fn on_focus_forwards_transitions_only() {
    let h = harness();
    let (seen, rec) = recorder();
    let realized = h
        .world
        .enter(|| realize(&h.backend, &h.registry, view().on_focus(rec).build()));
    let (_, handler) = h.focus_within_handler(0);
    handler(false); // not focused → not focused: nothing to say
    handler(true);
    handler(true); // focus moved between two descendants
    handler(false);
    handler(false);
    handler(true);
    assert_eq!(seen.borrow().as_slice(), [true, false, true]);
    drop(realized);
}

/// A toolkit can deliver focus-out mid-teardown (GTK fires `focus-leave`
/// while unparenting); the author's closure must not run on a dead scope.
#[test]
fn on_focus_is_inert_after_the_view_unmounts() {
    let h = harness();
    let (seen, rec) = recorder();
    let realized = h
        .world
        .enter(|| realize(&h.backend, &h.registry, view().on_focus(rec).build()));
    let (_, handler) = h.focus_within_handler(0);
    drop(realized);
    handler(true);
    assert!(seen.borrow().is_empty(), "a dead scope's on_focus must not run");
}

/// A batched repeat ships rows as a serializable op stream and installs
/// their handlers afterwards; `on_focus` is not modelled there, so a row
/// carrying one takes the per-node path — and every row gets its handler.
/// Silently dropping it would leave a keyboard user's focus unheard.
#[test]
fn repeat_rows_with_on_focus_each_get_their_handler() {
    use runtime_vocabulary::glue::__static_repeat;
    let h = harness();
    h.shared.batched_repeat.set(true); // the backend batches; the row shape bails
    let (seen, rec) = recorder();
    let rec = Rc::new(rec);
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .children(__static_repeat(3, move |i| {
                    let rec = rec.clone();
                    view()
                        .on_focus(move |f| if f { rec(i % 2 == 0) })
                        .build()
                }))
                .build(),
        )
    });
    assert_eq!(h.shared.focus_within_handlers.borrow().len(), 3, "one per row");
    for i in 0..3 {
        (h.focus_within_handler(i).1)(true);
    }
    assert_eq!(seen.borrow().as_slice(), [true, false, true]);
    drop(realized);
}
