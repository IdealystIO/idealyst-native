//! The FULL-OP parity scenarios, OLD-core side (P2b gate's behavioral
//! definition). Trees build through runtime-core's PUBLIC constructors;
//! `src/scenarios_full_new.rs` rebuilds the SAME logical trees through
//! the vocabulary builders. Scenario bodies are dumb (build → mutate →
//! snapshot); prop values come from `full.rs`'s shared fixtures so both
//! sides digest identically.

use std::rc::Rc;

use runtime_core::primitives::activity_indicator::activity_indicator;
use runtime_core::primitives::slider::slider;
use runtime_core::{
    button, external_link, icon, image, pressable, scroll_view, signal, styled_text, text,
    text_area, text_input, toggle, view, when, IntoElement, SafeAreaSides, Signal,
    StyleApplication, StyleSheet, TextRun,
};

use crate::full::{test_rules, FullCx, FullScenario, TEST_ICON};
use crate::Mode;

/// Wrap shared test rules in a static sheet (the old core's route to a
/// resolved-rules apply).
fn static_style(width: f32, background: &str) -> StyleApplication {
    StyleApplication::new(Rc::new(StyleSheet::r#static(test_rules(width, background))))
}

/// The full-op scenario registry.
pub fn full_scenarios() -> Vec<FullScenario> {
    vec![
        FullScenario {
            name: "full_static_kitchen_sink",
            about: &[
                "One static tree containing all 13 P2 primitives with static",
                "styles on the root + one leaf. Pins every create_* signature's",
                "argument rendering, the per-primitive mount sequences (create ->",
                "style -> handlers -> initial write-backs), and child insertion",
                "order. Controlled widgets (toggle/slider/input/area) emit one",
                "initial update_* at mount (the controlled write-back's first",
                "fire); static text/button/link/image-alt emit none.",
            ],
            modes: &[Mode::Spliced],
            run: full_static_kitchen_sink,
        },
        FullScenario {
            name: "full_reactive_text",
            about: &[
                "A bound text (closure over a signal). Mount takes the",
                "batched-id fast path (create_text_with_id + one",
                "update_text_by_id); each signal write is exactly one",
                "update_text_by_id — no structural ops.",
            ],
            modes: &[Mode::Spliced],
            run: full_reactive_text,
        },
        FullScenario {
            name: "full_reactive_style",
            about: &[
                "A view with a DYNAMIC style closure over a signal. Mount:",
                "apply_style with the initial resolved rules + attach_states",
                "(the event-driven state hookup). Each signal write re-applies",
                "via ONE apply_style with the new digest — no structural ops,",
                "no re-attach.",
            ],
            modes: &[Mode::Spliced],
            run: full_reactive_style,
        },
        FullScenario {
            name: "full_prop_updates",
            about: &[
                "Per-primitive reactive prop updates: toggle checked, slider",
                "value, image src, input value + secure, button label. Each",
                "step writes ONE signal and must produce exactly the matching",
                "update_* call — nothing else.",
            ],
            modes: &[Mode::Spliced],
            run: full_prop_updates,
        },
        FullScenario {
            name: "full_dyn_swap_primitives",
            about: &[
                "A `when` hole swapping between two DIFFERENT primitives",
                "(button <-> toggle), proving primitive mount sequences run",
                "inside driver rebuilds: the incoming branch's create + initial",
                "write-backs land between the driver's structural ops, in the",
                "P1-pinned dispose orderings (anchored: scope-drop then clear;",
                "spliced: remove then scope-drop).",
            ],
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_dyn_swap_primitives,
        },
        FullScenario {
            name: "full_release_on_swap",
            about: &[
                "Resource release on subtree teardown, WITHIN a step (a `when`",
                "swap to an empty branch, so it's inside the goldens unlike the",
                "final Owner drop): two bound texts release their backend ids",
                "(release_text_id) and a dynamically-styled view fires",
                "on_node_unstyled. The old core releases scope cleanups LIFO;",
                "the new core releases in creation order — a sanctioned",
                "ordering-artifact divergence (see README), same op set.",
            ],
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_release_on_swap,
        },
    ]
}

// ===========================================================================
// (a) kitchen sink
// ===========================================================================

fn full_static_kitchen_sink(cx: &mut FullCx) {
    let toggle_on: Signal<bool> = signal(true);
    let slider_val: Signal<f32> = signal(2.5);
    let input_val: Signal<String> = signal(String::from("hi"));
    let area_val: Signal<String> = signal(String::from("hello\nworld"));

    cx.mount(
        view(vec![
            text("hello").into_element(),
            styled_text(vec![TextRun::plain("styled"), TextRun::plain(" runs")]).into_element(),
            text("leaf")
                .with_style(static_style(80.0, "#445566"))
                .into_element(),
            button("Go", || {}).into_element(),
            pressable(vec![text("press me").into_element()], || {}).into_element(),
            image("logo.png").alt("Logo".to_string()).into_element(),
            icon(TEST_ICON).into_element(),
            toggle(toggle_on, |_| {}).into_element(),
            slider(slider_val, |_| {})
                .range(0.0, 10.0)
                .step(0.5)
                .into_element(),
            activity_indicator().into_element(),
            external_link(
                "https://example.com",
                vec![text("docs").into_element()],
            )
            .into_element(),
            scroll_view(vec![text("scrollable").into_element()])
                .safe_area(SafeAreaSides::TOP)
                .into_element(),
            text_input(input_val, |_| {})
                .placeholder("Type here".to_string())
                .into_element(),
            text_area(area_val, |_| {})
                .placeholder("notes".to_string())
                .min_rows(2)
                .max_rows(6)
                .into_element(),
        ])
        .with_style(static_style(120.0, "#112233"))
        .into_element(),
    );
}

// ===========================================================================
// (b) reactive text
// ===========================================================================

fn full_reactive_text(cx: &mut FullCx) {
    let count: Signal<i32> = signal(0);
    cx.mount(
        view(vec![text(move || format!("count: {}", count.get())).into_element()]).into_element(),
    );
    cx.step("set count = 1", || count.set(1));
    cx.step("set count = 2", || count.set(2));
}

// ===========================================================================
// (c) reactive style
// ===========================================================================

fn full_reactive_style(cx: &mut FullCx) {
    let wide: Signal<bool> = signal(false);
    cx.mount(
        view(vec![text("styled by signal").into_element()])
            .with_style(move || {
                if wide.get() {
                    static_style(300.0, "#112233")
                } else {
                    static_style(100.0, "#112233")
                }
            })
            .into_element(),
    );
    cx.step("set wide = true (one apply_style, new digest)", || {
        wide.set(true)
    });
    cx.step("set wide = false (back to the narrow digest)", || {
        wide.set(false)
    });
}

// ===========================================================================
// (d) per-primitive prop updates
// ===========================================================================

fn full_prop_updates(cx: &mut FullCx) {
    let checked: Signal<bool> = signal(false);
    let level: Signal<f32> = signal(0.0);
    let src: Signal<String> = signal(String::from("a.png"));
    let typed: Signal<String> = signal(String::new());
    let masked: Signal<bool> = signal(false);
    let label: Signal<String> = signal(String::from("Start"));

    cx.mount(
        view(vec![
            toggle(checked, |_| {}).into_element(),
            slider(level, |_| {}).into_element(),
            image(move || src.get()).into_element(),
            text_input(typed, |_| {}).secure(masked).into_element(),
            button(move || label.get(), || {}).into_element(),
        ])
        .into_element(),
    );
    cx.step("toggle checked = true", || checked.set(true));
    cx.step("slider value = 0.7", || level.set(0.7));
    cx.step("image src = b.png", || src.set("b.png".to_string()));
    cx.step("input value = 'abc'", || typed.set("abc".to_string()));
    cx.step("input secure = true", || masked.set(true));
    cx.step("button label = 'Stop'", || label.set("Stop".to_string()));
}

// ===========================================================================
// (e) dyn hole swapping between two primitives
// ===========================================================================

fn full_dyn_swap_primitives(cx: &mut FullCx) {
    let show_button: Signal<bool> = signal(true);
    let toggle_val: Signal<bool> = signal(true);
    cx.mount(
        view(vec![when(
            move || show_button.get(),
            || button("Act", || {}).into_element(),
            move || toggle(toggle_val, |_| {}).into_element(),
        )])
        .into_element(),
    );
    cx.step("swap to toggle (create+initial write-back inside rebuild)", || {
        show_button.set(false)
    });
    cx.step("swap back to button", || show_button.set(true));
}

// ===========================================================================
// (f) resource release on swap-out
// ===========================================================================

fn full_release_on_swap(cx: &mut FullCx) {
    let present: Signal<bool> = signal(true);
    let a: Signal<i32> = signal(1);
    let b: Signal<i32> = signal(2);
    let styled: Signal<bool> = signal(false);
    cx.mount(
        view(vec![when(
            move || present.get(),
            move || {
                view(vec![
                    text(move || format!("a={}", a.get())).into_element(),
                    text(move || format!("b={}", b.get())).into_element(),
                ])
                .with_style(move || {
                    if styled.get() {
                        static_style(200.0, "#606060")
                    } else {
                        static_style(60.0, "#606060")
                    }
                })
                .into_element()
            },
            || text("empty").into_element(),
        )])
        .into_element(),
    );
    cx.step("swap out (ids released + on_node_unstyled, inside the step)", || {
        present.set(false)
    });
    cx.step("swap back in (fresh ids minted)", || present.set(true));
}
