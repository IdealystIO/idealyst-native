//! `text_entry_focused` — whether a field that takes typing holds keyboard
//! focus, for app-wide shortcuts to stand down while keys are typing.
//!
//! What these pin: every `text_input` and `text_area` feeds it with no
//! opt-in (the bug it fixes: a bare-key shortcut fired while typing into
//! any field whose screen hadn't wired its own focus tracking); an
//! author's own focus handler still hears every change; a field unmounted
//! while focused releases its claim even though no blur arrives; and a
//! keyed list edit that keeps a focused row does not release it. The
//! platform sources (DOM focus events, AppKit first responder, …) call
//! exactly the handlers fired here.

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_scene::realize;
use runtime_vocabulary::builders::{text_area, text_input, view};
use runtime_vocabulary::glue::text_entry_focused;
use runtime_world::signal;

fn harness() -> Harness {
    let h = Harness::new();
    h.record_all();
    h
}

#[test]
fn regression_a_text_input_without_on_focus_still_reports_typing() {
    let h = harness();
    let realized = h.world.enter(|| realize(&h.backend, &h.registry, text_input().build()));
    let focus = h.text_input_focus_handler(0);
    assert!(!text_entry_focused());
    focus(true);
    assert!(text_entry_focused(), "a focused field means keys are typing");
    focus(false);
    assert!(!text_entry_focused());
    drop(realized);
}

#[test]
fn the_authors_on_focus_still_hears_every_change() {
    let h = harness();
    let seen: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
    let s = seen.clone();
    let realized = h.world.enter(|| {
        realize(&h.backend, &h.registry, text_input().on_focus(move |f| s.borrow_mut().push(f)).build())
    });
    let focus = h.text_input_focus_handler(0);
    focus(true);
    assert!(text_entry_focused());
    focus(false);
    assert_eq!(seen.borrow().as_slice(), [true, false]);
    drop(realized);
}

#[test]
fn regression_a_text_area_reports_typing_through_its_own_node() {
    let h = harness();
    let realized = h.world.enter(|| realize(&h.backend, &h.registry, view().child(text_area().build()).build()));
    // The area is the second node made, after the wrapping view.
    let (node, focus) = h.focus_within_handler(0);
    assert_eq!(node, 1, "installed on the text area itself");
    focus(true);
    assert!(text_entry_focused());
    focus(false);
    assert!(!text_entry_focused());
    drop(realized);
}

#[test]
fn regression_a_field_unmounted_while_focused_releases_typing() {
    let h = harness();
    let realized = h.world.enter(|| realize(&h.backend, &h.registry, text_input().build()));
    h.text_input_focus_handler(0)(true);
    assert!(text_entry_focused());
    // No blur arrives: a toolkit need not blur a field it removes.
    drop(realized);
    assert!(!text_entry_focused(), "a gone field must not keep shortcuts off");
}

#[test]
fn a_keyed_edit_keeps_a_surviving_rows_focus() {
    let h = Harness::new();
    // Splicing is what makes keyed reconcile keep surviving rows.
    h.shared.splice.set(true);
    let list = h.world.enter(|| signal(vec![1, 2]));
    let element = h.world.enter(move || {
        view()
            .children(vec![runtime_scene::keyed(move || list.get(), |n| *n, |_n: i32| text_input().build())])
            .build()
    });
    let _realized = h.mount(element);
    h.world.flush();
    h.text_input_focus_handler(0)(true);
    assert!(text_entry_focused());

    // The list's driver re-runs; row 1 and its focused field survive.
    list.set(vec![1, 2, 3]);
    h.world.flush();
    assert!(text_entry_focused(), "an edit beside a focused field must not release it");

    // Its row removed: released.
    list.set(vec![2, 3]);
    h.world.flush();
    assert!(!text_entry_focused());
}
