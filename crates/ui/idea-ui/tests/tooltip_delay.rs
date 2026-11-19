//! `Tooltip`'s show delay and warm window, mounted on the recording host:
//! the first tooltip waits `delay_ms` with the pointer on its trigger; once
//! one has shown, the next shows the moment the pointer enters its trigger;
//! the window closes `skip_delay_ms` after the pointer leaves a shown
//! tooltip without entering another.
//!
//! Timers run through host-mock's pumped scheduler, which queues every
//! `after_ms` and fires the queue on `pump_timers()` with the delays
//! collapsed. So "after the delay" is "after a pump", and "before the
//! delay" is "with the timer still queued". The delay VALUES are asserted
//! against the props' defaults in `defaults_are_the_named_constants`.

use idea_ui::{
    install_idea_theme, light_theme, Tooltip, TooltipProps, TOOLTIP_DELAY_MS, TOOLTIP_DISMISS_MS,
    TOOLTIP_SKIP_DELAY_MS,
};
use runtime_core::{signal, ui, Element, Signal};
use runtime_shared::{TouchEvent, TouchId, TouchPhase, TouchPoint};

struct Mounted {
    h: host_mock::Harness,
    _realized: runtime_scene::Realized<u32>,
}

impl Mounted {
    fn mount(build: impl FnOnce() -> Element) -> Mounted {
        host_mock::pump::install_scheduler();
        let h = host_mock::Harness::new();
        let tree = h.world.enter(|| {
            install_idea_theme(light_theme());
            build()
        });
        let realized = h.mount(tree);
        h.flush();
        Mounted { h, _realized: realized }
    }

    /// The `nth` trigger wrapper's hover handler, in mount order (the
    /// bubbles' portals install none).
    fn hover(&self, nth: usize, entering: bool) {
        let handler = self.h.shared.hover_handlers.borrow()[nth].1.clone();
        handler(entering);
        self.h.flush();
    }

    fn long_press(&self, nth: usize) {
        let handler = self.h.shared.touch_handlers.borrow()[nth].1.clone();
        handler(&TouchEvent {
            id: TouchId(1),
            phase: TouchPhase::Began,
            position: TouchPoint::new(4.0, 4.0),
            window_position: TouchPoint::new(4.0, 4.0),
            timestamp_ns: 0,
            force: None,
        });
    }

    /// Fire every queued timer, then commit what they staged.
    fn elapse(&self) {
        host_mock::pump::pump_timers();
        self.h.flush();
    }

    /// Bubbles on screen: each one is an anchored portal.
    fn bubbles(&self) -> usize {
        fn walk(h: &host_mock::Harness, node: u32, n: &mut usize) {
            if h.kind_of(node).is_some_and(|k| k.starts_with("portal")) {
                *n += 1;
            }
            for c in h.children_of(node) {
                walk(h, c, n);
            }
        }
        let mut n = 0;
        for root in self.h.live_roots() {
            walk(&self.h, root, &mut n);
        }
        n
    }
}

fn pair(skip_delay_ms: u32) -> Element {
    ui! {
        view {
            Tooltip(text = "Cut the selection", skip_delay_ms = skip_delay_ms) { text { "Cut" } }
            Tooltip(text = "Copy the selection", skip_delay_ms = skip_delay_ms) { text { "Copy" } }
        }
    }
}

#[test]
fn defaults_are_the_named_constants() {
    let p = TooltipProps::default();
    assert_eq!(p.delay_ms.get_untracked(), TOOLTIP_DELAY_MS);
    assert_eq!(p.skip_delay_ms.get_untracked(), TOOLTIP_SKIP_DELAY_MS);
    assert_eq!(p.dismiss_ms.get_untracked(), TOOLTIP_DISMISS_MS);
    assert!(TOOLTIP_DELAY_MS > 0, "the first tooltip waits by default");
    assert!(TOOLTIP_SKIP_DELAY_MS > 0, "the warm window is open by default");
}

#[test]
fn first_tooltip_waits_its_delay() {
    let m = Mounted::mount(|| pair(TOOLTIP_SKIP_DELAY_MS));
    m.hover(0, true);
    assert_eq!(m.bubbles(), 0, "nothing shows before the delay");
    m.elapse();
    assert_eq!(m.bubbles(), 1, "the bubble shows after the delay");
    m.hover(0, false);
    assert_eq!(m.bubbles(), 0, "leaving hides it at once");
}

#[test]
fn a_pass_shorter_than_the_delay_shows_nothing() {
    let m = Mounted::mount(|| pair(TOOLTIP_SKIP_DELAY_MS));
    m.hover(0, true);
    m.hover(0, false);
    m.elapse();
    assert_eq!(m.bubbles(), 0, "the pending show went stale on leave");
    m.hover(1, true);
    assert_eq!(m.bubbles(), 0, "a tooltip that never showed does not warm the next");
}

#[test]
fn next_tooltip_shows_at_once_inside_the_warm_window() {
    let m = Mounted::mount(|| pair(TOOLTIP_SKIP_DELAY_MS));
    m.hover(0, true);
    m.elapse();
    assert_eq!(m.bubbles(), 1);

    m.hover(0, false);
    m.hover(1, true);
    assert_eq!(m.bubbles(), 1, "the neighbour shows with no delay");

    // Its show staled the first tooltip's cool-down: the window is still
    // open after that timer fires.
    m.elapse();
    m.hover(1, false);
    m.hover(0, true);
    assert_eq!(m.bubbles(), 1, "still warm: back to the first one at once");
}

#[test]
fn warm_window_closes_after_skip_delay() {
    let m = Mounted::mount(|| pair(TOOLTIP_SKIP_DELAY_MS));
    m.hover(0, true);
    m.elapse();
    m.hover(0, false);
    m.elapse(); // the skip delay runs out with the pointer on no trigger

    m.hover(1, true);
    assert_eq!(m.bubbles(), 0, "cold again: the next tooltip waits");
    m.elapse();
    assert_eq!(m.bubbles(), 1);
}

#[test]
fn zero_skip_delay_makes_every_tooltip_wait() {
    let m = Mounted::mount(|| pair(0));
    m.hover(0, true);
    m.elapse();
    m.hover(0, false);
    m.hover(1, true);
    assert_eq!(m.bubbles(), 0, "no warm window with skip_delay_ms = 0");
    m.elapse();
    assert_eq!(m.bubbles(), 1);
}

#[test]
fn zero_delay_shows_on_enter() {
    let m = Mounted::mount(|| ui! { Tooltip(text = "Now", delay_ms = 0u32) { text { "t" } } });
    m.hover(0, true);
    assert_eq!(m.bubbles(), 1);
}

#[test]
fn long_press_shows_at_once_and_does_not_warm() {
    let m = Mounted::mount(|| pair(TOOLTIP_SKIP_DELAY_MS));
    m.long_press(0);
    m.elapse(); // the recognizer's hold timer
    assert_eq!(m.bubbles(), 1, "the hold is the delay");
    m.elapse(); // the dismiss timer
    assert_eq!(m.bubbles(), 0, "auto-dismissed");
    m.hover(1, true);
    assert_eq!(m.bubbles(), 0, "a long-press opens no warm window");
}

/// A tooltip removed while showing reports no pointer leave. The window
/// must still close behind it, or every later tooltip would skip its delay
/// for the life of the app.
#[test]
fn regression_unmounting_a_shown_tooltip_closes_the_warm_window() {
    let shown: std::rc::Rc<std::cell::Cell<Option<Signal<bool>>>> = Default::default();
    let out = shown.clone();
    let m = Mounted::mount(move || {
        let first = signal(true);
        out.set(Some(first));
        ui! {
            view {
                if first.get() {
                    Tooltip(text = "Cut the selection") { text { "Cut" } }
                }
                Tooltip(text = "Copy the selection") { text { "Copy" } }
            }
        }
    });
    m.hover(0, true);
    m.elapse();
    assert_eq!(m.bubbles(), 1);

    let first = shown.get().unwrap();
    m.h.world.enter(|| first.set(false));
    m.h.flush();
    assert_eq!(m.bubbles(), 0, "the bubble went with its tooltip");
    m.elapse();

    // The removed tooltip's handler stays at index 0 in the recording.
    m.hover(1, true);
    assert_eq!(m.bubbles(), 0, "the window closed behind the removed tooltip");
}

/// A long-press whose dismiss timer outlives the tooltip must not write
/// the tooltip's freed `open` signal (a stale handle write aborts).
#[test]
fn regression_dismiss_timer_after_unmount_is_inert() {
    let shown: std::rc::Rc<std::cell::Cell<Option<Signal<bool>>>> = Default::default();
    let out = shown.clone();
    let m = Mounted::mount(move || {
        let on = signal(true);
        out.set(Some(on));
        ui! {
            view {
                if on.get() {
                    Tooltip(text = "Cut the selection") { text { "Cut" } }
                }
            }
        }
    });
    m.long_press(0);
    m.elapse();
    assert_eq!(m.bubbles(), 1);
    let on = shown.get().unwrap();
    m.h.world.enter(|| on.set(false));
    m.h.flush();
    m.elapse();
    assert_eq!(m.bubbles(), 0);
}
