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
        FullScenario {
            name: "full_style_sheet_cohort",
            about: &[
                "P3c: static SHEET applications through the token engine. Tokens",
                "installed pre-mount are delivered (install_tokens) before the",
                "first styled apply; each theme swap emits ONE backend",
                "update_tokens then re-applies every registered static node (the",
                "shared theme cohort, registration order). Hiding a styled branch",
                "removes it structurally + fires on_node_unstyled; the FOLLOWING",
                "swap re-applies only the survivor — the dead node's cohort entry",
                "is gone.",
            ],
            modes: &[Mode::Spliced],
            run: full_style_sheet_cohort,
        },
        FullScenario {
            name: "full_style_state_overlay",
            about: &[
                "P3c: a STATIC sheet application whose sheet declares",
                "`state hovered` on an event-driven backend must divert to the",
                "state machine (attach_states at mount — the static-divert",
                "regression); flipping the hover bit re-resolves WITH the overlay",
                "(one apply_style, overlay digest), flipping it off restores the",
                "base digest.",
            ],
            modes: &[Mode::Spliced],
            run: full_style_state_overlay,
        },
        FullScenario {
            name: "full_style_signal_class",
            about: &[
                "P3c: signal_class on a backend without JS class bindings — the",
                "fallback per-node binding path. Mount applies the value-0 class",
                "rules + attach_states; each signal write is exactly one",
                "apply_style with the new value's digest.",
            ],
            modes: &[Mode::Spliced],
            run: full_style_signal_class,
        },
        FullScenario {
            name: "full_style_preminted",
            about: &[
                "P3c: preminted classes stamp via attach_html_class (one per",
                "segment) with zero StyleRules work; pre-mount tokens still reach",
                "the backend (the premint host driver — no sheet ever registers),",
                "as do theme swaps. The overrides variant layers one apply_style",
                "on top of its class.",
            ],
            modes: &[Mode::Spliced],
            run: full_style_preminted,
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

// ===========================================================================
// (g) P3c: static sheets + tokens + cohort + styled unmount
// ===========================================================================

fn full_style_sheet_cohort(cx: &mut FullCx) {
    use crate::full::{surface_token, themed_sheet};
    runtime_core::install_tokens(&[surface_token("#101010")]);
    let show: Signal<bool> = signal(true);
    cx.mount(
        view(vec![
            view(vec![])
                .with_style(StyleApplication::new(themed_sheet()))
                .into_element(),
            when(
                move || show.get(),
                || {
                    view(vec![])
                        .with_style(StyleApplication::new(themed_sheet()).with("size", "large"))
                        .into_element()
                },
                || text("hidden").into_element(),
            ),
        ])
        .into_element(),
    );
    cx.step("swap theme (update_tokens + cohort re-applies both)", || {
        runtime_core::update_tokens(&[surface_token("#f0f0f0")]);
    });
    cx.step("hide the styled branch (structural + on_node_unstyled)", || {
        show.set(false);
    });
    cx.step("swap theme again (only the survivor re-applies)", || {
        runtime_core::update_tokens(&[surface_token("#202020")]);
    });
}

// ===========================================================================
// (h) P3c: static sheet with a state overlay (the divert + flip)
// ===========================================================================

fn full_style_state_overlay(cx: &mut FullCx) {
    use crate::full::hover_sheet;
    cx.mount(
        view(vec![view(vec![])
            .with_style(StyleApplication::new(hover_sheet()))
            .into_element()])
        .into_element(),
    );
    let setter = cx.state_setter(0);
    let on = setter.clone();
    cx.step("hover on (one apply_style, overlay digest)", move || {
        on(runtime_core::StateBits::HOVERED, true);
    });
    cx.step("hover off (base digest restored)", move || {
        setter(runtime_core::StateBits::HOVERED, false);
    });
}

// ===========================================================================
// (i) P3c: signal_class fallback rebind
// ===========================================================================

fn full_style_signal_class(cx: &mut FullCx) {
    use crate::full::class_app;
    let active: Signal<u32> = signal(0);
    cx.mount(
        view(vec![view(vec![])
            .with_style(runtime_core::signal_class(active, &[0, 1], class_app))
            .into_element()])
        .into_element(),
    );
    cx.step("set active = 1 (one apply_style, value-1 digest)", || {
        active.set(1)
    });
    cx.step("set active = 0 (back to the value-0 digest)", || {
        active.set(0)
    });
}

// ===========================================================================
// (j) P3c: preminted classes + premint host token delivery
// ===========================================================================

fn full_style_preminted(cx: &mut FullCx) {
    use crate::full::{surface_token, test_rules};
    use std::borrow::Cow;
    runtime_core::install_tokens(&[surface_token("#101010")]);
    cx.mount(
        view(vec![
            view(vec![])
                .with_style(runtime_core::StyleSource::Preminted {
                    class: Cow::Borrowed("iy-fixed iy-fixed-size-large"),
                    overrides: None,
                })
                .into_element(),
            view(vec![])
                .with_style(runtime_core::StyleSource::Preminted {
                    class: Cow::Borrowed("iy-over"),
                    overrides: Some(Rc::new(test_rules(50.0, "#0000ff"))),
                })
                .into_element(),
        ])
        .into_element(),
    );
    cx.step("swap theme (premint driver flushes update_tokens)", || {
        runtime_core::update_tokens(&[surface_token("#f0f0f0")]);
    });
}
