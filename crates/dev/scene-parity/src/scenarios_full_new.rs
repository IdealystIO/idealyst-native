//! The FULL-OP scenarios re-targeted at the NEW core: the SAME logical
//! trees as `scenarios_full.rs` (same names, modes, step labels,
//! mutations, prop values from `full.rs`'s shared fixtures), built
//! through the vocabulary builders and driven by `runtime-world`
//! signals.
//!
//! Lowering map (old public constructor → vocabulary builder):
//!
//! | old                              | new                                     |
//! |----------------------------------|-----------------------------------------|
//! | `view(children).with_style(app)` | `view().style(rules).child(...)`        |
//! | `text("s")` / `text(closure)`    | `text().content(...)` (TextContent)     |
//! | `styled_text(runs)`              | `text().runs(runs)`                     |
//! | `button(label, f)`               | `button().label(...).on_press(f)`       |
//! | `pressable(children, f)`         | `pressable(f).child(...)`               |
//! | `image(src).alt(s)`              | `image().src(...).alt(s)`               |
//! | `icon(DATA)`                     | `icon().data(DATA)`                     |
//! | `toggle(sig, f)`                 | `toggle().value(sig).on_change(f)`      |
//! | `slider(sig, f).range().step()`  | `slider().value(sig)...`                |
//! | `activity_indicator()`           | `activity_indicator()`                  |
//! | `external_link(url, ch)`         | `link().url(url).external(true)...`     |
//! | `scroll_view(ch).safe_area(s)`   | `scroll_view().safe_area(s).child(...)` |
//! | `text_input(sig, f)`             | `text_input().value(sig).on_change(f)`  |
//! | `text_area(sig, f)`              | `text_area().value(sig).on_change(f)`   |
//! | `when(cond, a, b)`               | `dyn_keyed(cond, ...)` (guarded hole)   |
//! | `.with_style(closure)`           | `.style(closure)` (Dynamic StyleProp)   |

use runtime_core::{SafeAreaSides, TextRun};
use runtime_scene::dyn_keyed;
use runtime_vocabulary::builders::{
    activity_indicator, button, icon, image, link, pressable, scroll_view, slider, text,
    text_area, text_input, toggle, view,
};
use runtime_world::{signal, Signal};

use crate::full::{test_rules, TEST_ICON};
use crate::full_new::{FullNewCx, FullNewScenario};
use crate::Mode;

/// The re-targeted registry — one entry per old-side scenario.
pub fn full_new_scenarios() -> Vec<FullNewScenario> {
    vec![
        FullNewScenario {
            name: "full_static_kitchen_sink",
            modes: &[Mode::Spliced],
            run: full_static_kitchen_sink,
        },
        FullNewScenario {
            name: "full_reactive_text",
            modes: &[Mode::Spliced],
            run: full_reactive_text,
        },
        FullNewScenario {
            name: "full_reactive_style",
            modes: &[Mode::Spliced],
            run: full_reactive_style,
        },
        FullNewScenario {
            name: "full_prop_updates",
            modes: &[Mode::Spliced],
            run: full_prop_updates,
        },
        FullNewScenario {
            name: "full_dyn_swap_primitives",
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_dyn_swap_primitives,
        },
        FullNewScenario {
            name: "full_release_on_swap",
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_release_on_swap,
        },
    ]
}

// ===========================================================================
// (a) kitchen sink
// ===========================================================================

fn full_static_kitchen_sink(cx: &mut FullNewCx) {
    let toggle_on: Signal<bool> = signal(true);
    let slider_val: Signal<f32> = signal(2.5);
    let input_val: Signal<String> = signal(String::from("hi"));
    let area_val: Signal<String> = signal(String::from("hello\nworld"));

    cx.mount(
        view()
            .style(test_rules(120.0, "#112233"))
            .child(text().content("hello"))
            .child(text().runs(vec![TextRun::plain("styled"), TextRun::plain(" runs")]))
            .child(text().content("leaf").style(test_rules(80.0, "#445566")))
            .child(button().label("Go").on_press(|| {}))
            .child(pressable(|| {}).child(text().content("press me")))
            .child(image().src("logo.png").alt("Logo"))
            .child(icon().data(TEST_ICON))
            .child(toggle().value(toggle_on).on_change(|_| {}))
            .child(
                slider()
                    .value(slider_val)
                    .on_change(|_| {})
                    .range(0.0, 10.0)
                    .step(0.5),
            )
            .child(activity_indicator())
            .child(
                link()
                    .url("https://example.com")
                    .external(true)
                    .child(text().content("docs")),
            )
            .child(
                scroll_view()
                    .safe_area(SafeAreaSides::TOP)
                    .child(text().content("scrollable")),
            )
            .child(
                text_input()
                    .value(input_val)
                    .on_change(|_| {})
                    .placeholder("Type here"),
            )
            .child(
                text_area()
                    .value(area_val)
                    .on_change(|_| {})
                    .placeholder("notes")
                    .min_rows(2)
                    .max_rows(6),
            )
            .build(),
    );
}

// ===========================================================================
// (b) reactive text
// ===========================================================================

fn full_reactive_text(cx: &mut FullNewCx) {
    let count: Signal<i32> = signal(0);
    cx.mount(
        view()
            .child(text().content(move || format!("count: {}", count.get())))
            .build(),
    );
    cx.step("set count = 1", || count.set(1));
    cx.step("set count = 2", || count.set(2));
}

// ===========================================================================
// (c) reactive style
// ===========================================================================

fn full_reactive_style(cx: &mut FullNewCx) {
    let wide: Signal<bool> = signal(false);
    cx.mount(
        view()
            .style(move || {
                if wide.get() {
                    test_rules(300.0, "#112233")
                } else {
                    test_rules(100.0, "#112233")
                }
            })
            .child(text().content("styled by signal"))
            .build(),
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

fn full_prop_updates(cx: &mut FullNewCx) {
    let checked: Signal<bool> = signal(false);
    let level: Signal<f32> = signal(0.0);
    let src: Signal<String> = signal(String::from("a.png"));
    let typed: Signal<String> = signal(String::new());
    let masked: Signal<bool> = signal(false);
    let label: Signal<String> = signal(String::from("Start"));

    cx.mount(
        view()
            .child(toggle().value(checked).on_change(|_| {}))
            .child(slider().value(level).on_change(|_| {}))
            .child(image().src(move || src.get()))
            .child(text_input().value(typed).on_change(|_| {}).secure(masked))
            .child(button().label(move || label.get()).on_press(|| {}))
            .build(),
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

fn full_dyn_swap_primitives(cx: &mut FullNewCx) {
    let show_button: Signal<bool> = signal(true);
    let toggle_val: Signal<bool> = signal(true);
    cx.mount(
        view()
            .child(dyn_keyed(
                move || show_button.get(),
                move |&on| {
                    if on {
                        button().label("Act").on_press(|| {}).build()
                    } else {
                        toggle().value(toggle_val).on_change(|_| {}).build()
                    }
                },
            ))
            .build(),
    );
    cx.step("swap to toggle (create+initial write-back inside rebuild)", || {
        show_button.set(false)
    });
    cx.step("swap back to button", || show_button.set(true));
}

// ===========================================================================
// (f) resource release on swap-out
// ===========================================================================

fn full_release_on_swap(cx: &mut FullNewCx) {
    let present: Signal<bool> = signal(true);
    let a: Signal<i32> = signal(1);
    let b: Signal<i32> = signal(2);
    let styled: Signal<bool> = signal(false);
    cx.mount(
        view()
            .child(dyn_keyed(
                move || present.get(),
                move |&on| {
                    if on {
                        view()
                            .style(move || {
                                if styled.get() {
                                    test_rules(200.0, "#606060")
                                } else {
                                    test_rules(60.0, "#606060")
                                }
                            })
                            .child(text().content(move || format!("a={}", a.get())))
                            .child(text().content(move || format!("b={}", b.get())))
                            .build()
                    } else {
                        text().content("empty").build()
                    }
                },
            ))
            .build(),
    );
    cx.step("swap out (ids released + on_node_unstyled, inside the step)", || {
        present.set(false)
    });
    cx.step("swap back in (fresh ids minted)", || present.set(true));
}
