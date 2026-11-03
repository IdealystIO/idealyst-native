//! `HoverCard` mounted on the recording host: the pointer path (open and
//! close delays, bridging trigger → card), the touch path (long-press,
//! click-away), Escape, and the test ids.
//!
//! Timers run through host-mock's pumped scheduler, which queues every
//! `after_ms` and fires the queue on `pump_timers()` with the delays
//! collapsed. So "after the open delay" is "after a pump", and "before the
//! delay" is "with the timer still queued". The delay VALUES are asserted
//! against the props' defaults in `defaults_are_the_named_constants`.
//!
//! The regression this file exists for (CrewForge F1): a `Popover` driven
//! by hover lays a transparent fullscreen click-away catcher over the page,
//! which covers the trigger, so the pointer "leaves" the trigger the moment
//! the card opens and it flickers shut. A hover-opened HoverCard must mount
//! exactly one portal — the anchored card — and no catcher.

use idea_ui::{
    install_idea_theme, light_theme, HoverCard, HoverCardProps, HOVER_CARD_CLOSE_DELAY_MS,
    HOVER_CARD_DEFAULT_OFFSET, HOVER_CARD_OPEN_DELAY_MS,
};
use runtime_core::{ui, Element, Reactive};
use runtime_shared::{TouchEvent, TouchId, TouchPhase, TouchPoint};
use runtime_vocabulary::robot::{Query, Robot};

const TRIGGER: &str = "progress-trigger";
const CARD: &str = "progress-card";

struct Mounted {
    h: host_mock::Harness,
    _realized: runtime_scene::Realized<u32>,
}

impl Mounted {
    fn new() -> Mounted {
        host_mock::pump::install_scheduler();
        Robot::new().reset();
        let h = host_mock::Harness::new();
        let tree = h.world.enter(|| {
            install_idea_theme(light_theme());
            hover_card()
        });
        let realized = h.mount(tree);
        h.flush();
        Mounted {
            h,
            _realized: realized,
        }
    }

    /// The trigger wrapper's hover handler — the first one installed (the
    /// card is not mounted yet when the trigger is).
    fn hover_trigger(&self, entering: bool) {
        let handler = self.h.shared.hover_handlers.borrow()[0].1.clone();
        handler(entering);
        self.h.flush();
    }

    /// The open card's hover handler — the latest one installed.
    fn hover_card(&self, entering: bool) {
        let handler = self
            .h
            .shared
            .hover_handlers
            .borrow()
            .last()
            .expect("a hover handler")
            .1
            .clone();
        handler(entering);
        self.h.flush();
    }

    /// Fire every queued timer, then commit what they staged.
    fn elapse(&self) {
        host_mock::pump::pump_timers();
        self.h.flush();
    }

    fn card_open(&self) -> bool {
        Robot::new().find(Query::test_id(CARD)).is_some()
    }

    /// Live nodes of `kind` anywhere in the tree.
    fn live(&self, kind: &str) -> usize {
        fn walk(h: &host_mock::Harness, kind: &str, node: u32, n: &mut usize) {
            if h.kind_of(node).is_some_and(|k| k.starts_with(kind)) {
                *n += 1;
            }
            for c in h.children_of(node) {
                walk(h, kind, c, n);
            }
        }
        let mut n = 0;
        for root in self.h.live_roots() {
            walk(&self.h, kind, root, &mut n);
        }
        n
    }

    fn screen(&self) -> String {
        self.h
            .live_roots()
            .into_iter()
            .map(|r| self.h.live_tree(r))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The anchored card portal's `on_dismiss` — the latest portal's
    /// (the catcher, when there is one, mounts before the card).
    fn escape(&self) {
        let dismiss = self
            .h
            .shared
            .portal_dismissals
            .borrow()
            .last()
            .cloned()
            .flatten()
            .expect("the card portal has an on_dismiss (Escape)");
        dismiss();
        self.h.flush();
    }

    fn long_press_trigger(&self) {
        let handler = self.h.shared.touch_handlers.borrow()[0].1.clone();
        handler(&TouchEvent {
            id: TouchId(1),
            phase: TouchPhase::Began,
            position: TouchPoint::new(4.0, 4.0),
            window_position: TouchPoint::new(4.0, 4.0),
            timestamp_ns: 0,
            force: None,
        });
        // The recognizer's hold timer.
        self.elapse();
    }
}

fn hover_card() -> Element {
    ui! {
        view {
            HoverCard(
                content = || ui! {
                    view {
                        text { "Drilling 64%" }
                        text { "Bolting 40%" }
                    }
                },
                test_id = Some(TRIGGER),
                card_test_id = Some(CARD),
            ) {
                text { "52%" }
            }
        }
    }
}

#[test]
fn defaults_are_the_named_constants() {
    let p = HoverCardProps::default();
    let get = |r: &Reactive<u32>| r.get();
    assert_eq!(get(&p.open_delay_ms), HOVER_CARD_OPEN_DELAY_MS);
    assert_eq!(get(&p.close_delay_ms), HOVER_CARD_CLOSE_DELAY_MS);
    assert_eq!(p.offset.get(), HOVER_CARD_DEFAULT_OFFSET);
    assert!((300..=500).contains(&HOVER_CARD_OPEN_DELAY_MS));
    assert!((150..=300).contains(&HOVER_CARD_CLOSE_DELAY_MS));
}

#[test]
fn trigger_test_id_lands_on_the_wrapper() {
    let m = Mounted::new();
    assert!(
        Robot::new().find(Query::test_id(TRIGGER)).is_some(),
        "{}",
        m.screen()
    );
    assert!(!m.card_open(), "closed until hovered");
}

#[test]
fn hover_opens_only_after_the_open_delay() {
    let m = Mounted::new();
    m.hover_trigger(true);
    assert!(!m.card_open(), "the card waits for the open delay");
    m.elapse();
    assert!(
        m.card_open(),
        "the card opens once the open delay elapses:\n{}",
        m.screen()
    );
}

#[test]
fn a_pass_shorter_than_the_open_delay_opens_nothing() {
    let m = Mounted::new();
    m.hover_trigger(true);
    m.hover_trigger(false);
    m.elapse();
    assert!(
        !m.card_open(),
        "leaving before the open delay cancels the open"
    );
}

/// CrewForge F1: a hover-opened card has no backdrop — exactly one portal
/// (the anchored card), no fullscreen click-away catcher over the trigger.
#[test]
fn regression_hover_opened_card_has_no_backdrop() {
    let m = Mounted::new();
    m.hover_trigger(true);
    m.elapse();
    assert!(m.card_open());
    assert_eq!(
        m.live("portal"),
        1,
        "only the anchored card portal, no catcher:\n{}",
        m.screen()
    );
    assert_eq!(
        m.live("pressable"),
        0,
        "no backdrop pressable mounted with the card:\n{}",
        m.screen()
    );
}

#[test]
fn moving_from_trigger_into_card_keeps_it_open() {
    let m = Mounted::new();
    m.hover_trigger(true);
    m.elapse();
    assert!(m.card_open());
    // Leave the trigger: the close timer is armed…
    m.hover_trigger(false);
    // …and entering the card before it fires cancels it.
    m.hover_card(true);
    m.elapse();
    assert!(
        m.card_open(),
        "the pointer reached the card inside the close delay"
    );
    // Back from the card to the trigger: still open.
    m.hover_card(false);
    m.hover_trigger(true);
    m.elapse();
    assert!(
        m.card_open(),
        "the pointer went back to the trigger inside the close delay"
    );
}

#[test]
fn leaving_both_closes_after_the_close_delay() {
    let m = Mounted::new();
    m.hover_trigger(true);
    m.elapse();
    m.hover_trigger(false);
    m.hover_card(true);
    m.hover_card(false);
    assert!(m.card_open(), "the card waits for the close delay");
    m.elapse();
    assert!(
        !m.card_open(),
        "the card closes once the close delay elapses:\n{}",
        m.screen()
    );
    assert_eq!(m.live("portal"), 0, "{}", m.screen());
}

#[test]
fn escape_closes_and_the_next_hover_reopens() {
    let m = Mounted::new();
    m.hover_trigger(true);
    m.elapse();
    assert!(m.card_open());
    m.escape();
    assert!(!m.card_open(), "Escape closes the card at once");
    m.elapse();
    assert!(
        !m.card_open(),
        "no timer reopens it while the pointer sits still"
    );
    m.hover_trigger(false);
    m.hover_trigger(true);
    m.elapse();
    assert!(m.card_open(), "a fresh hover opens it again");
}

#[test]
fn long_press_opens_with_a_click_away_catcher() {
    let m = Mounted::new();
    m.long_press_trigger();
    assert!(
        m.card_open(),
        "a long-press opens the card:\n{}",
        m.screen()
    );
    // The catcher is a fullscreen `overlay` (its own backdrop portal +
    // content portal) whose backdrop is the one pressable.
    assert_eq!(
        m.live("pressable"),
        1,
        "touch-opened: a click-away catcher:\n{}",
        m.screen()
    );
    // A tap outside lands on the catcher, the first portal mounted.
    let catcher =
        m.h.shared
            .portal_dismissals
            .borrow()
            .iter()
            .rev()
            .nth(1)
            .cloned()
            .flatten();
    let catcher = catcher.expect("the catcher dismisses");
    catcher();
    m.h.flush();
    assert!(!m.card_open(), "a tap outside closes a touch-opened card");
    assert_eq!(m.live("portal"), 0, "{}", m.screen());
}

#[test]
fn card_test_id_is_gone_after_close() {
    let m = Mounted::new();
    m.hover_trigger(true);
    m.elapse();
    let all = Robot::new().find_all(Query::test_id(CARD));
    assert_eq!(all.len(), 1, "exactly one live card: {all:?}");
    m.escape();
    assert!(Robot::new().find_all(Query::test_id(CARD)).is_empty());
}
