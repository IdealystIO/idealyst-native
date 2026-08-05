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

use runtime_shared::{SafeAreaSides, TextRun};
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
        FullNewScenario {
            name: "full_repeat_fallback",
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_repeat_fallback,
        },
        FullNewScenario {
            name: "full_style_sheet_cohort",
            modes: &[Mode::Spliced],
            run: full_style_sheet_cohort,
        },
        FullNewScenario {
            name: "full_style_state_overlay",
            modes: &[Mode::Spliced],
            run: full_style_state_overlay,
        },
        FullNewScenario {
            name: "full_style_signal_class",
            modes: &[Mode::Spliced],
            run: full_style_signal_class,
        },
        FullNewScenario {
            name: "full_style_preminted",
            modes: &[Mode::Spliced],
            run: full_style_preminted,
        },
        FullNewScenario {
            name: "full_virtualizer_lifecycle",
            modes: &[Mode::Spliced],
            run: full_virtualizer_lifecycle,
        },
        FullNewScenario {
            name: "full_virtualizer_lane_swap",
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_virtualizer_lane_swap,
        },
        FullNewScenario {
            name: "full_graphics_lifecycle",
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_graphics_lifecycle,
        },
        FullNewScenario {
            name: "full_portal_toggle",
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_portal_toggle,
        },
        FullNewScenario {
            name: "full_overlay_static",
            modes: &[Mode::Spliced],
            run: full_overlay_static,
        },
        FullNewScenario {
            name: "full_overlay_toggle",
            modes: &[Mode::Spliced],
            run: full_overlay_toggle,
        },
        FullNewScenario {
            name: "full_presence_cycle",
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_presence_cycle,
        },
        FullNewScenario {
            name: "full_presence_bare",
            modes: &[Mode::Spliced],
            run: full_presence_bare,
        },
        FullNewScenario {
            name: "nav_swap_select",
            modes: &[Mode::Spliced],
            run: nav_swap_select,
        },
        FullNewScenario {
            name: "nav_swap_dispose_evict",
            modes: &[Mode::Spliced],
            run: nav_swap_dispose_evict,
        },
        FullNewScenario {
            name: "nav_stack_push_pop",
            modes: &[Mode::Spliced],
            run: nav_stack_push_pop,
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

// ===========================================================================
// (f2) static-range Repeat — the shared non-batching fallback
// ===========================================================================

fn full_repeat_fallback(cx: &mut FullNewCx) {
    use crate::full::themed_sheet;
    use runtime_shared::StyleApplication;
    use runtime_vocabulary::glue::__static_repeat;
    let present: Signal<bool> = signal(true);
    let sheet = themed_sheet();
    cx.mount(
        view()
            .child(dyn_keyed(
                move || present.get(),
                {
                    let sheet = sheet.clone();
                    move |&on| {
                        if on {
                            let sheet = sheet.clone();
                            let mut children = __static_repeat(3, move |i| {
                                view()
                                    .style(StyleApplication::new(sheet.clone()))
                                    .child(text().content(format!("row {i}")))
                                    .build()
                            });
                            children.push(text().content("after-rows").build());
                            view().children(children).build()
                        } else {
                            text().content("empty").build()
                        }
                    }
                },
            ))
            .build(),
    );
    cx.step("swap out (rows release per row, inside the step)", || {
        present.set(false)
    });
    cx.step("swap back in (rows rebuild)", || present.set(true));
}

// ===========================================================================
// (g) P3c: static sheets + tokens + cohort + styled unmount
// ===========================================================================

fn full_style_sheet_cohort(cx: &mut FullNewCx) {
    use crate::full::{surface_token, themed_sheet};
    use runtime_shared::StyleApplication;
    use runtime_vocabulary::theme;
    theme::install_tokens(&[surface_token("#101010")]);
    let show: Signal<bool> = signal(true);
    cx.mount(
        view()
            .child(view().style(StyleApplication::new(themed_sheet())))
            .child(dyn_keyed(
                move || show.get(),
                move |&on| {
                    if on {
                        view()
                            .style(StyleApplication::new(themed_sheet()).with("size", "large"))
                            .build()
                    } else {
                        text().content("hidden").build()
                    }
                },
            ))
            .build(),
    );
    cx.step("swap theme (update_tokens + cohort re-applies both)", || {
        theme::update_tokens(&[surface_token("#f0f0f0")]);
    });
    cx.step("hide the styled branch (structural + on_node_unstyled)", || {
        show.set(false);
    });
    cx.step("swap theme again (only the survivor re-applies)", || {
        theme::update_tokens(&[surface_token("#202020")]);
    });
}

// ===========================================================================
// (h) P3c: static sheet with a state overlay (the divert + flip)
// ===========================================================================

fn full_style_state_overlay(cx: &mut FullNewCx) {
    use crate::full::hover_sheet;
    use runtime_shared::StyleApplication;
    cx.mount(
        view()
            .child(view().style(StyleApplication::new(hover_sheet())))
            .build(),
    );
    let setter = cx.state_setter(0);
    let on = setter.clone();
    cx.step("hover on (one apply_style, overlay digest)", move || {
        on(runtime_shared::StateBits::HOVERED, true);
    });
    cx.step("hover off (base digest restored)", move || {
        setter(runtime_shared::StateBits::HOVERED, false);
    });
}

// ===========================================================================
// (i) P3c: signal_class fallback rebind
// ===========================================================================

fn full_style_signal_class(cx: &mut FullNewCx) {
    use crate::full::class_app;
    let active: Signal<u32> = signal(0);
    cx.mount(
        view()
            .child(view().style(runtime_vocabulary::signal_class(active, &[0, 1], class_app)))
            .build(),
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

fn full_style_preminted(cx: &mut FullNewCx) {
    use crate::full::{surface_token, test_rules};
    use runtime_vocabulary::theme;
    use runtime_vocabulary::StyleProp;
    use std::borrow::Cow;
    use std::rc::Rc;
    theme::install_tokens(&[surface_token("#101010")]);
    cx.mount(
        view()
            .child(view().style(StyleProp::Preminted {
                class: Cow::Borrowed("iy-fixed iy-fixed-size-large"),
                overrides: None,
                inline: None,
            }))
            .child(view().style(StyleProp::Preminted {
                class: Cow::Borrowed("iy-over"),
                overrides: Some(Rc::new(test_rules(50.0, "#0000ff"))),
                inline: None,
            }))
            .build(),
    );
    cx.step("swap theme (premint driver flushes update_tokens)", || {
        theme::update_tokens(&[surface_token("#f0f0f0")]);
    });
}

// ===========================================================================
// (k) P3-set: virtualizer lifecycle (window sim drives the callbacks)
// ===========================================================================

fn full_virtualizer_lifecycle(cx: &mut FullNewCx) {
    use runtime_shared::ItemSize;
    use runtime_vocabulary::builders::virtualizer;
    use std::rc::Rc;

    let present: Signal<bool> = signal(true);
    let rows: Signal<usize> = signal(3);
    let version: Signal<i32> = signal(0);
    cx.mount(
        view()
            .child(dyn_keyed(
                move || present.get(),
                move |&on| {
                    if on {
                        virtualizer(
                            move || rows.get(),
                            |i| i as u64,
                            ItemSize::Measured(Rc::new(|_| 40.0)),
                            move |idx| {
                                text()
                                    .content(move || format!("item {idx} v{}", version.get()))
                                    .build()
                            },
                        )
                        .overscan(2.0)
                        .style(test_rules(120.0, "#334455"))
                        .build()
                    } else {
                        text().content("gone").build()
                    }
                },
            ))
            .build(),
    );
    let sim = cx.virt_sim(0);
    cx.step("fill window 0..3 (lazy detached row mounts)", {
        let sim = sim.clone();
        move || sim.set_window(0..3)
    });
    cx.step("bump shared version (live rows update in creation order)", move || {
        version.set(1)
    });
    cx.step("measure row 0 (Measured-mode feedback)", {
        let sim = sim.clone();
        move || sim.report_measured(0, 55.0)
    });
    cx.step("scroll to 1..3 (row 0 released, its reactivity freed)", {
        let sim = sim.clone();
        move || sim.set_window(1..3)
    });
    cx.step("bump version again (released row must NOT update)", move || {
        version.set(2)
    });
    cx.step("scroll back to 0..3 (row 0 remounts at measured size 55)", {
        let sim = sim.clone();
        move || sim.set_window(0..3)
    });
    cx.step("shrink data to 2 rows (one data-changed notification)", move || {
        rows.set(2)
    });
    cx.step("re-window after shrink (row key=2 released)", {
        let sim = sim.clone();
        move || sim.set_window(0..3)
    });
    cx.step("clear window (remaining rows released)", {
        let sim = sim.clone();
        move || sim.set_window(0..0)
    });
    cx.step("swap out (unstyle then release_virtualizer)", move || {
        present.set(false)
    });
}

// ===========================================================================
// (l) P3-set: virtualizer lane-config change (node rebuild via a hole)
// ===========================================================================

fn full_virtualizer_lane_swap(cx: &mut FullNewCx) {
    use runtime_shared::{Axis, ItemSize, Lanes};
    use runtime_scene::Element;
    use runtime_vocabulary::builders::virtualizer;
    use std::rc::Rc;

    let grid: Signal<bool> = signal(false);
    fn cells(layouted: bool) -> Element {
        let b = virtualizer(
            || 6,
            |i| i as u64,
            ItemSize::Known(Rc::new(|_| 100.0)),
            |_| text().content("cell").build(),
        );
        if layouted {
            b.axis(Axis::Horizontal)
                .lanes(Lanes::Fixed(3))
                .spacing(8.0, 4.0)
                .build()
        } else {
            b.build()
        }
    }
    cx.mount(
        view()
            .child(dyn_keyed(
                move || grid.get(),
                move |&on| cells(on),
            ))
            .build(),
    );
    cx.step("swap to 3-lane horizontal grid (release then create)", move || {
        grid.set(true)
    });
    cx.step("swap back to vertical list", move || grid.set(false));
}

// ===========================================================================
// (m) P3-set: graphics lifecycle
// ===========================================================================

fn full_graphics_lifecycle(cx: &mut FullNewCx) {
    use runtime_vocabulary::builders::graphics;

    let present: Signal<bool> = signal(true);
    let rec = cx.recorder();
    cx.mount(
        view()
            .child(dyn_keyed(
                move || present.get(),
                move |&on| {
                    if on {
                        let (r1, r2, r3) = (rec.clone(), rec.clone(), rec.clone());
                        graphics(move |e| {
                            r1.push(format!(
                                "author on_ready {}x{} @{}",
                                e.size.0, e.size.1, e.scale
                            ))
                        })
                        .on_resize(move |e| {
                            r2.push(format!(
                                "author on_resize {}x{} @{}",
                                e.size.0, e.size.1, e.scale
                            ))
                        })
                        .on_lost(move || r3.push("author on_lost".to_string()))
                        .style(test_rules(100.0, "#202020"))
                        .build()
                    } else {
                        text().content("no canvas").build()
                    }
                },
            ))
            .build(),
    );
    let sim = cx.gfx_sim(0);
    cx.step("surface ready 640x480 @2", {
        let sim = sim.clone();
        move || sim.fire_ready((640, 480), 2.0)
    });
    cx.step("resize 800x600 @2", {
        let sim = sim.clone();
        move || sim.fire_resize((800, 600), 2.0)
    });
    cx.step("surface lost", {
        let sim = sim.clone();
        move || sim.fire_lost()
    });
    cx.step("swap out (unstyle then release_graphics)", move || {
        present.set(false)
    });
}

// ===========================================================================
// (P3-set) portal + overlay compositions + presence
// ===========================================================================

fn full_portal_toggle(cx: &mut FullNewCx) {
    use runtime_shared::primitives::portal::{PortalTarget, ViewportPlacement};
    use runtime_vocabulary::builders::portal;
    let open: Signal<bool> = signal(true);
    cx.mount(
        view()
            .child(dyn_keyed(
                move || open.get(),
                |&on| {
                    if on {
                        portal(PortalTarget::Viewport(ViewportPlacement::Center))
                            .child(text().content("inside"))
                            .on_dismiss(|| {})
                            .trap_focus(true)
                            .build()
                    } else {
                        text().content("closed").build()
                    }
                },
            ))
            .build(),
    );
    cx.step("close (release_portal inside the teardown window)", || {
        open.set(false)
    });
    cx.step("reopen (fresh portal mount)", || open.set(true));
}

fn full_overlay_static(cx: &mut FullNewCx) {
    use crate::full::test_anchor_target;
    use runtime_shared::primitives::overlay::BackdropMode;
    use runtime_shared::primitives::portal::{ElementAlign, ElementSide, ViewportPlacement};
    use runtime_vocabulary::builders::{anchored_overlay, overlay};
    cx.mount(
        view()
            .child(
                overlay()
                    .placement(ViewportPlacement::Bottom)
                    .on_dismiss(|| {})
                    .backdrop_style(test_rules(40.0, "#00000080"))
                    .style(test_rules(320.0, "#ffffff"))
                    .child(text().content("modal body")),
            )
            .child(
                overlay()
                    .backdrop(BackdropMode::None)
                    .click_through(true)
                    .child(text().content("toast")),
            )
            .child(
                anchored_overlay(test_anchor_target())
                    .side(ElementSide::Above)
                    .align(ElementAlign::Center)
                    .offset(4.0)
                    .child(text().content("tip")),
            )
            .build(),
    );
}

fn full_overlay_toggle(cx: &mut FullNewCx) {
    use runtime_vocabulary::builders::overlay;
    let open: Signal<bool> = signal(true);
    cx.mount(
        view()
            .child(dyn_keyed(
                move || open.get(),
                |&on| {
                    if on {
                        overlay()
                            .backdrop_style(test_rules(40.0, "#00000080"))
                            .on_dismiss(|| {})
                            .child(text().content("modal body"))
                            .build()
                    } else {
                        text().content("closed").build()
                    }
                },
            ))
            .build(),
    );
    cx.step("close (backdrop unstyle + release_portal in one window)", || {
        open.set(false)
    });
    cx.step("reopen", || open.set(true));
}

fn full_presence_cycle(cx: &mut FullNewCx) {
    use crate::full::{presence_enter, presence_exit, pump_frames, pump_timers};
    use runtime_vocabulary::builders::presence;
    let open: Signal<bool> = signal(false);
    cx.mount(
        view()
            .child(text().content("before"))
            .child(
                presence(|| text().content("card").build())
                    .present(open)
                    .enter(presence_enter())
                    .exit(presence_exit()),
            )
            .child(text().content("after"))
            .build(),
    );
    cx.step("show (child mounts, snaps to the enter state pre-paint)", || {
        open.set(true)
    });
    cx.step("frame (animate to rest with the enter transition)", pump_frames);
    cx.step("hide (exit transform, node still attached)", || {
        open.set(false)
    });
    cx.step("exit elapses (deferred detach + teardown)", pump_timers);
    cx.step("reshow (fresh child mounts)", || open.set(true));
}

fn full_presence_bare(cx: &mut FullNewCx) {
    use runtime_vocabulary::builders::presence;
    let open: Signal<bool> = signal(true);
    cx.mount(
        view()
            .child(text().content("before"))
            .child(presence(|| text().content("card").build()).present(open))
            .child(text().content("after"))
            .build(),
    );
    cx.step("hide (no exit: immediate teardown)", || open.set(false));
}

// ===========================================================================
// Navigator scenarios — the vocabulary swap/stack handlers, mirroring
// the old side's REAL SDK-handler runs. Dispatches stage a command +
// tick; the step's `world.flush()` runs the driver effect, so the ops
// land in the same step snapshot as the old side's synchronous dispatch.
// ===========================================================================

use std::rc::Rc;

use runtime_shared::primitives::navigator::Route;
use runtime_vocabulary::builders::{navigator_outlet, stack_navigator, swap_navigator};
use runtime_vocabulary::prims::{MountPolicy, NavHandle, StackRetention, SwapNav};
use runtime_vocabulary::on_teardown;
use runtime_world::inject;

const NAV_HOME: Route<()> = Route::new("home", "/");
const NAV_ABOUT: Route<()> = Route::new("about", "/about");
const NAV_DETAIL: Route<()> = Route::new("detail", "/detail");

fn nav_screen(label: &'static str, width: f32, background: &'static str) -> runtime_scene::Element {
    // `test_rules` as a Static prop digests identically to the old
    // side's static sheet (literal rules resolve to themselves) — the
    // same equivalence the kitchen-sink scenario relies on.
    view()
        .style(test_rules(width, background))
        .child(text().content(label))
        .build()
}

fn nav_swap_select(cx: &mut FullNewCx) {
    let on_select: Rc<std::cell::RefCell<Option<Rc<dyn Fn(&'static str)>>>> =
        Rc::new(std::cell::RefCell::new(None));
    let element = {
        let slot = on_select.clone();
        swap_navigator(&NAV_HOME)
            .screen(NAV_HOME, |_| nav_screen("home", 10.0, "#111111"))
            .screen(NAV_ABOUT, |_| nav_screen("about", 20.0, "#222222"))
            .layout(move || {
                let ctx = inject::<SwapNav>().expect("SwapNav provided during layout build");
                *slot.borrow_mut() = Some(ctx.on_select.clone());
                view()
                    .child(text().content("chrome"))
                    .child(navigator_outlet())
                    .build()
            })
            .build()
    };
    cx.mount(element);
    let select = move |name| {
        (on_select.borrow().clone().expect("layout ran during mount"))(name)
    };
    cx.step("select about (fresh mount + outlet swap)", || select("about"));
    cx.step("select home (cached re-insert, no creates)", || select("home"));
    cx.step("select home again (no-op)", || select("home"));
}

fn nav_swap_dispose_evict(cx: &mut FullNewCx) {
    let rec = cx.recorder();
    let on_select: Rc<std::cell::RefCell<Option<Rc<dyn Fn(&'static str)>>>> =
        Rc::new(std::cell::RefCell::new(None));
    let element = {
        let slot = on_select.clone();
        swap_navigator(&NAV_HOME)
            .screen(NAV_HOME, move |_| {
                // Cleanup marker via the probe-effect mapping (README:
                // the kernel's on_cleanup needs a running effect).
                let rec = rec.clone();
                on_teardown(move || rec.note_cleanup("home-screen"));
                nav_screen("home", 10.0, "#111111")
            })
            .screen(NAV_ABOUT, |_| nav_screen("about", 20.0, "#222222"))
            .mount_policy(MountPolicy::LazyDisposing)
            .layout(move || {
                let ctx = inject::<SwapNav>().expect("SwapNav provided during layout build");
                *slot.borrow_mut() = Some(ctx.on_select.clone());
                view()
                    .child(text().content("chrome"))
                    .child(navigator_outlet())
                    .build()
            })
            .build()
    };
    cx.mount(element);
    let select = move |name| {
        (on_select.borrow().clone().expect("layout ran during mount"))(name)
    };
    cx.step("select about (evicted home releases: cleanup + unstyle)", || {
        select("about")
    });
    cx.step("select home (re-mounts fresh)", || select("home"));
}

fn nav_stack_push_pop(cx: &mut FullNewCx) {
    let rec = cx.recorder();
    let handle_slot: Rc<std::cell::RefCell<Option<NavHandle>>> =
        Rc::new(std::cell::RefCell::new(None));
    let element = {
        let slot = handle_slot.clone();
        stack_navigator(&NAV_HOME)
            .screen(NAV_HOME, |_| nav_screen("home", 10.0, "#111111"))
            .screen(NAV_DETAIL, move |_| {
                let rec = rec.clone();
                on_teardown(move || rec.note_cleanup("detail-screen"));
                nav_screen("detail", 20.0, "#222222")
            })
            .retention(StackRetention::Retain)
            .layout(|| {
                view()
                    .child(text().content("header"))
                    .child(navigator_outlet())
                    .build()
            })
            .on_handle(move |handle| *slot.borrow_mut() = Some(handle))
            .build()
    };
    cx.mount(element);
    let handle = handle_slot.borrow().clone().expect("handle bound at mount");
    let push_handle = handle.clone();
    cx.step("push detail (mount + outlet swap, home retained)", move || {
        push_handle.push(&NAV_DETAIL, ());
    });
    cx.step("pop (release detail FIRST, reveal retained home)", move || {
        handle.pop();
    });
}
