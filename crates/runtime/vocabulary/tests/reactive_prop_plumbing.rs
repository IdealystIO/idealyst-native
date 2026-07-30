//! Per-primitive reactive-prop plumbing — the **new-core successor** of the
//! old core's seven single-primitive suites
//! (`runtime-core/tests/{icon_reactive_data, image_alt_reactive,
//! link_url_reactive, activity_indicator_size_reactive,
//! text_input_secure_reactive, preserves_focus, text_input_blur}.rs`).
//!
//! Each of those pinned the same **triple** for one prop:
//!
//! 1. a **live** source pushes the in-place `update_*` on change, and does
//!    NOT rebuild the node;
//! 2. a **static** source installs **no effect at all** — mount emits the
//!    create and nothing else;
//! 3. the binding effect is **freed with the scope** — after teardown the
//!    source can be written without reaching the (released) backend, and
//!    the closure itself is dropped.
//!
//! Before this file the new core had that triple for exactly ONE prop
//! (`vocab.rs::{const,dyn}_button_label_*`); `caps_conformance.rs` only
//! proves the caps are *callable*. Halves 2 and 3 are the ones that
//! regress silently: a spurious effect install costs a wasted subscription
//! and an extra mount-time backend call that no op-log golden covers (the
//! goldens record what the handlers DO emit, not what they must not), and a
//! leaked effect keeps firing into a released node — neither turns any
//! other test red.
//!
//! Mechanism note (why half 2 is a real contract, not an accident):
//! `handlers::bind_dyn` installs nothing for `Value::Const` because
//! `create_*` already consumed the initial value, while
//! `handlers::bind_value` applies once even for `Const` (the props whose
//! `create_*` does NOT take them — image `src`, controlled `value`
//! write-backs). The old walker drew exactly that line; these tests pin
//! which prop is on which side.
//!
//! Half 3 is asserted two ways: behaviourally (no further backend op) and
//! structurally (a `Weak` probe on an `Rc` moved into the source closure
//! upgrades to `None`, so the closure — and therefore the effect slot —
//! really was freed, not merely left inert). The old suites used the
//! legacy arena's `arena_stats().effects_in_use` balance for that half;
//! the world kernel has no slot-census API, and the `Weak` probe is a
//! strictly stronger statement anyway (it proves the captured state died,
//! not just that a counter moved).

use std::rc::{Rc, Weak};

use host_mock::Harness;
// Paths are spelled `runtime_shared::…` (the permanent substrate the
// vocabulary depends on directly), not through any re-export.
use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;
use runtime_shared::primitives::icon::IconData;
use runtime_shared::primitives::text_input::BlurOutcome;
use runtime_shared::FillRule;
use runtime_scene::realize;
use runtime_vocabulary::builders::{
    activity_indicator, icon, image, link, pressable, text_input, view,
};
use runtime_world::signal;

// ===========================================================================
// Harness helpers
// ===========================================================================

/// host-mock records the in-place `update_*` family at the VERBOSE tier
/// (they are not part of the core structural op set), so every test here
/// opts in.
fn harness() -> Harness {
    let h = Harness::new();
    h.record_all();
    h
}

/// Ops in the log whose name is `op` (the log lines start with the op
/// name, so a prefix match is exact for these).
fn ops_named(log: &[String], op: &str) -> Vec<String> {
    log.iter()
        .filter(|l| l.starts_with(op))
        .cloned()
        .collect::<Vec<_>>()
}

/// A liveness probe for a value moved into a reactive source closure:
/// while the binding effect lives, `upgrade()` is `Some`; once the effect
/// slot is freed the closure — and this `Rc` with it — is dropped.
fn probe() -> (Rc<()>, Weak<()>) {
    let keep = Rc::new(());
    let weak = Rc::downgrade(&keep);
    (keep, weak)
}

const GLYPH_A: IconData = IconData {
    view_box: (24, 24),
    paths: &["M1 1"],
    fill_rule: FillRule::NonZero,
    filled: false,
};
const GLYPH_B: IconData = IconData {
    view_box: (24, 24),
    paths: &["M9 9"],
    fill_rule: FillRule::NonZero,
    filled: false,
};

// ===========================================================================
// icon `data`
// ===========================================================================

#[test]
fn reactive_icon_data_swaps_glyph_in_place_without_rebuild() {
    let h = harness();
    let world = h.world.clone();
    let (realized, toggled) = world.enter(|| {
        let toggled = signal(false);
        let realized = realize(
            &h.backend,
            &h.registry,
            icon()
                .data_dyn(move || if toggled.get() { GLYPH_B } else { GLYPH_A })
                .build(),
        );
        (realized, toggled)
    });
    h.take_log();

    toggled.set(true);
    world.flush();
    let log = h.take_log();
    assert_eq!(
        ops_named(&log, "update_icon_data"),
        vec![format!("update_icon_data n0 {:?}", GLYPH_B.paths)],
        "flipping the data source pushes the new glyph in place: {log:?}"
    );
    assert!(
        ops_named(&log, "create").is_empty(),
        "the icon node must NOT be rebuilt: {log:?}"
    );
    drop(realized);
}

#[test]
fn static_icon_data_installs_no_update_effect() {
    let h = harness();
    let realized = h
        .world
        .enter(|| realize(&h.backend, &h.registry, icon().data(GLYPH_A).build()));
    let log = h.take_log();
    assert!(
        ops_named(&log, "update_icon_data").is_empty(),
        "a static glyph is consumed by create_icon — no update effect: {log:?}"
    );
    // And nothing can make it fire later, because nothing subscribed.
    h.world.flush();
    assert!(h.take_log().is_empty(), "no post-mount work for a static glyph");
    drop(realized);
}

#[test]
fn reactive_icon_data_effect_is_freed_on_teardown() {
    let h = harness();
    let world = h.world.clone();
    let (keep, weak) = probe();
    let (realized, toggled) = world.enter(|| {
        let toggled = signal(false);
        let realized = realize(
            &h.backend,
            &h.registry,
            icon()
                .data_dyn(move || {
                    let _hold = &keep;
                    if toggled.get() {
                        GLYPH_B
                    } else {
                        GLYPH_A
                    }
                })
                .build(),
        );
        (realized, toggled)
    });
    assert!(weak.upgrade().is_some(), "the source closure is alive while mounted");
    h.take_log();

    drop(realized);
    assert!(
        weak.upgrade().is_none(),
        "dropping the realized subtree must free the binding effect (and its closure)"
    );

    toggled.set(true);
    world.flush();
    assert!(
        ops_named(&h.take_log(), "update_icon_data").is_empty(),
        "after teardown the freed effect must not reach the released node"
    );
}

// ===========================================================================
// image `alt`
// ===========================================================================

#[test]
fn reactive_image_alt_updates_in_place_without_rebuild() {
    let h = harness();
    let world = h.world.clone();
    let (realized, label) = world.enter(|| {
        let label = signal(String::from("first"));
        let realized = realize(
            &h.backend,
            &h.registry,
            image()
                .src("a.png")
                .alt_dyn(move || Some(label.get()))
                .build(),
        );
        (realized, label)
    });
    h.take_log();

    label.set("second".to_string());
    world.flush();
    let log = h.take_log();
    assert_eq!(
        ops_named(&log, "update_image_alt"),
        vec!["update_image_alt n0 Some(\"second\")".to_string()],
        "a live alt swaps the label in place: {log:?}"
    );
    assert!(
        ops_named(&log, "create").is_empty(),
        "the image node must NOT be rebuilt: {log:?}"
    );
    drop(realized);
}

#[test]
fn static_image_alt_installs_no_update_effect() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            image().src("a.png").alt("fixed").build(),
        )
    });
    let log = h.take_log();
    assert!(
        ops_named(&log, "update_image_alt").is_empty(),
        "a static alt is consumed by create_image — no update effect: {log:?}"
    );
    // `src` is on the other side of the bind_value/bind_dyn line: its
    // initial value is NOT consumed by create_image, so mount applies it
    // once even when static (old walker shape).
    assert_eq!(
        ops_named(&log, "update_image_src"),
        vec!["update_image_src n0 \"a.png\"".to_string()],
        "static src still applies once at mount: {log:?}"
    );
    drop(realized);
}

#[test]
fn reactive_image_alt_effect_is_freed_on_teardown() {
    let h = harness();
    let world = h.world.clone();
    let (keep, weak) = probe();
    let (realized, label) = world.enter(|| {
        let label = signal(String::from("first"));
        let realized = realize(
            &h.backend,
            &h.registry,
            image()
                .src("a.png")
                .alt_dyn(move || {
                    let _hold = &keep;
                    Some(label.get())
                })
                .build(),
        );
        (realized, label)
    });
    h.take_log();

    drop(realized);
    assert!(weak.upgrade().is_none(), "alt binding closure freed with the scope");
    label.set("second".to_string());
    world.flush();
    assert!(
        ops_named(&h.take_log(), "update_image_alt").is_empty(),
        "after teardown the alt effect must not fire"
    );
}

// ===========================================================================
// link `url`
// ===========================================================================

#[test]
fn reactive_link_url_updates_in_place_without_rebuild() {
    let h = harness();
    let world = h.world.clone();
    let (realized, url) = world.enter(|| {
        let url = signal(String::from("/one"));
        let realized = realize(
            &h.backend,
            &h.registry,
            // `on_activate` is required for a non-external link (the
            // handler refuses a silently-dead link); the assertion is on
            // the url binding.
            link().url(move || url.get()).on_activate(|| {}).build(),
        );
        (realized, url)
    });
    // The Dyn binding's first fire lands at mount (walker shape).
    assert_eq!(
        ops_named(&h.take_log(), "update_link_url"),
        vec!["update_link_url n0 \"/one\"".to_string()],
        "the Dyn url binding re-applies at mount"
    );

    url.set("/two".to_string());
    world.flush();
    let log = h.take_log();
    assert_eq!(
        ops_named(&log, "update_link_url"),
        vec!["update_link_url n0 \"/two\"".to_string()],
        "a live url swaps the href in place: {log:?}"
    );
    assert!(
        ops_named(&log, "create").is_empty(),
        "the link node must NOT be rebuilt: {log:?}"
    );
    drop(realized);
}

#[test]
fn static_link_url_installs_no_update_effect() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            link().url("/fixed").on_activate(|| {}).build(),
        )
    });
    let log = h.take_log();
    assert!(
        ops_named(&log, "update_link_url").is_empty(),
        "a static url is consumed by create_link — no update effect: {log:?}"
    );
    drop(realized);
}

#[test]
fn reactive_link_url_effect_is_freed_on_teardown() {
    let h = harness();
    let world = h.world.clone();
    let (keep, weak) = probe();
    let (realized, url) = world.enter(|| {
        let url = signal(String::from("/one"));
        let realized = realize(
            &h.backend,
            &h.registry,
            link()
                .url(move || {
                    let _hold = &keep;
                    url.get()
                })
                .on_activate(|| {})
                .build(),
        );
        (realized, url)
    });
    h.take_log();

    drop(realized);
    assert!(weak.upgrade().is_none(), "url binding closure freed with the scope");
    url.set("/two".to_string());
    world.flush();
    assert!(
        ops_named(&h.take_log(), "update_link_url").is_empty(),
        "after teardown the url effect must not fire"
    );
}

// ===========================================================================
// activity_indicator `size`
// ===========================================================================

#[test]
fn reactive_activity_indicator_size_resizes_in_place_without_rebuild() {
    let h = harness();
    let world = h.world.clone();
    let (realized, big) = world.enter(|| {
        let big = signal(false);
        let realized = realize(
            &h.backend,
            &h.registry,
            activity_indicator()
                .size_dyn(move || {
                    if big.get() {
                        ActivityIndicatorSize::Large
                    } else {
                        ActivityIndicatorSize::Small
                    }
                })
                .build(),
        );
        (realized, big)
    });
    h.take_log();

    big.set(true);
    world.flush();
    let log = h.take_log();
    assert_eq!(
        ops_named(&log, "update_activity_indicator_size"),
        vec!["update_activity_indicator_size n0 Large".to_string()],
        "a live size resizes the spinner in place: {log:?}"
    );
    assert!(
        ops_named(&log, "create").is_empty(),
        "the spinner must NOT be rebuilt: {log:?}"
    );
    drop(realized);
}

#[test]
fn static_activity_indicator_size_installs_no_update_effect() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            activity_indicator()
                .size(ActivityIndicatorSize::Large)
                .build(),
        )
    });
    let log = h.take_log();
    assert!(
        ops_named(&log, "update_activity_indicator_size").is_empty(),
        "a static size is consumed by create_activity_indicator: {log:?}"
    );
    drop(realized);
}

#[test]
fn reactive_activity_indicator_size_effect_is_freed_on_teardown() {
    let h = harness();
    let world = h.world.clone();
    let (keep, weak) = probe();
    let (realized, big) = world.enter(|| {
        let big = signal(false);
        let realized = realize(
            &h.backend,
            &h.registry,
            activity_indicator()
                .size_dyn(move || {
                    let _hold = &keep;
                    if big.get() {
                        ActivityIndicatorSize::Large
                    } else {
                        ActivityIndicatorSize::Small
                    }
                })
                .build(),
        );
        (realized, big)
    });
    h.take_log();

    drop(realized);
    assert!(weak.upgrade().is_none(), "size binding closure freed with the scope");
    big.set(true);
    world.flush();
    assert!(
        ops_named(&h.take_log(), "update_activity_indicator_size").is_empty(),
        "after teardown the size effect must not fire"
    );
}

// ===========================================================================
// text_input `secure` + `placeholder`
// ===========================================================================

#[test]
fn reactive_text_input_secure_toggles_in_place_without_rebuild() {
    let h = harness();
    let world = h.world.clone();
    let (realized, masked) = world.enter(|| {
        let masked = signal(true);
        let realized = realize(
            &h.backend,
            &h.registry,
            text_input()
                .value("hunter2")
                .secure(move || masked.get())
                .build(),
        );
        (realized, masked)
    });
    // Dyn secure re-applies at mount.
    assert_eq!(
        ops_named(&h.take_log(), "update_text_input_secure"),
        vec!["update_text_input_secure n0 true".to_string()],
    );

    masked.set(false);
    world.flush();
    let log = h.take_log();
    assert_eq!(
        ops_named(&log, "update_text_input_secure"),
        vec!["update_text_input_secure n0 false".to_string()],
        "the mask toggles in place: {log:?}"
    );
    // The whole point of in-place: the typed text survives a mask toggle
    // because the input is never recreated.
    assert!(
        ops_named(&log, "create").is_empty(),
        "the input must NOT be rebuilt (that would drop the typed value): {log:?}"
    );
    drop(realized);
}

#[test]
fn static_text_input_secure_installs_no_toggle_effect() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            text_input().value("hunter2").secure(true).build(),
        )
    });
    let log = h.take_log();
    assert!(
        ops_named(&log, "update_text_input_secure").is_empty(),
        "a static mask is consumed by create_text_input: {log:?}"
    );
    drop(realized);
}

#[test]
fn reactive_text_input_secure_effect_is_freed_on_teardown() {
    let h = harness();
    let world = h.world.clone();
    let (keep, weak) = probe();
    let (realized, masked) = world.enter(|| {
        let masked = signal(true);
        let realized = realize(
            &h.backend,
            &h.registry,
            text_input()
                .value("hunter2")
                .secure(move || {
                    let _hold = &keep;
                    masked.get()
                })
                .build(),
        );
        (realized, masked)
    });
    h.take_log();

    drop(realized);
    assert!(weak.upgrade().is_none(), "secure binding closure freed with the scope");
    masked.set(false);
    world.flush();
    assert!(
        ops_named(&h.take_log(), "update_text_input_secure").is_empty(),
        "after teardown the secure effect must not fire"
    );
}

#[test]
fn reactive_text_input_placeholder_updates_in_place_without_rebuild() {
    let h = harness();
    let world = h.world.clone();
    let (realized, hint) = world.enter(|| {
        let hint = signal(String::from("email"));
        let realized = realize(
            &h.backend,
            &h.registry,
            text_input().placeholder_dyn(move || Some(hint.get())).build(),
        );
        (realized, hint)
    });
    h.take_log();

    hint.set("e-mail address".to_string());
    world.flush();
    let log = h.take_log();
    assert_eq!(
        ops_named(&log, "update_text_input_placeholder"),
        vec!["update_text_input_placeholder n0 Some(\"e-mail address\")".to_string()],
        "a live placeholder updates in place: {log:?}"
    );
    assert!(
        ops_named(&log, "create").is_empty(),
        "the input must NOT be rebuilt: {log:?}"
    );
    drop(realized);
}

#[test]
fn static_text_input_placeholder_installs_no_update_effect() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            text_input().placeholder("email").build(),
        )
    });
    let log = h.take_log();
    assert!(
        ops_named(&log, "update_text_input_placeholder").is_empty(),
        "a static placeholder is consumed by create_text_input: {log:?}"
    );
    drop(realized);
}

// ===========================================================================
// `preserves_focus` — the focus-preserving press region (Autocomplete
// mouse-selection fix). Successor of runtime-core/tests/preserves_focus.rs.
// ===========================================================================

#[test]
fn preserves_focus_marks_view_and_pressable() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .preserves_focus(true)
                .child(pressable(|| {}).preserves_focus(true).build())
                .build(),
        )
    });
    let log = h.take_log();
    let marks = ops_named(&log, "mark_preserves_focus");
    assert_eq!(
        marks.len(),
        2,
        "both the marked view and the marked pressable reach the backend: {log:?}"
    );
    drop(realized);
}

#[test]
fn unmarked_nodes_are_not_marked() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .child(pressable(|| {}).build())
                .build(),
        )
    });
    let log = h.take_log();
    assert!(
        ops_named(&log, "mark_preserves_focus").is_empty(),
        "an unmarked view/pressable must not be marked: {log:?}"
    );
    drop(realized);
}

// ===========================================================================
// text_input `on_blur` — the cancelable `BlurOutcome`. Successor of
// runtime-core/tests/text_input_blur.rs.
// ===========================================================================

#[test]
fn on_blur_keep_threads_to_backend() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            text_input().on_blur(|| BlurOutcome::Keep).build(),
        )
    });
    let handler = h
        .blur_handler(0)
        .expect("create_text_input received the author's on_blur");
    assert!(
        matches!(handler(), BlurOutcome::Keep),
        "the author's veto reaches the platform through the handler"
    );
    drop(realized);
}

#[test]
fn on_blur_allow_threads_to_backend() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            text_input().on_blur(|| BlurOutcome::Allow).build(),
        )
    });
    let handler = h.blur_handler(0).expect("on_blur threaded");
    assert!(matches!(handler(), BlurOutcome::Allow));
    drop(realized);
}

#[test]
fn no_on_blur_registers_no_handler() {
    let h = harness();
    let realized = h
        .world
        .enter(|| realize(&h.backend, &h.registry, text_input().build()));
    assert!(
        h.blur_handler(0).is_none(),
        "an input without on_blur must register no handler (the platform keeps its default)"
    );
    drop(realized);
}
