//! The FULL-OP parity scenarios, OLD-core side (P2b gate's behavioral
//! definition). Trees build through runtime-core's PUBLIC constructors;
//! `src/scenarios_full_new.rs` rebuilds the SAME logical trees through
//! the vocabulary builders. Scenario bodies are dumb (build → mutate →
//! snapshot); prop values come from `full.rs`'s shared fixtures so both
//! sides digest identically.

use std::rc::Rc;

use runtime_core::primitives::activity_indicator::activity_indicator;
use runtime_core::primitives::graphics::graphics;
use runtime_core::primitives::slider::slider;
use runtime_core::{
    button, external_link, icon, image, pressable, scroll_view, signal, styled_text, text,
    text_area, text_input, toggle, view, virtualizer, when, Axis, IntoElement, ItemSize, Lanes,
    SafeAreaSides, Signal, StyleApplication, StyleSheet, TextRun,
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
        FullScenario {
            name: "full_virtualizer_lifecycle",
            about: &[
                "P3-set: the virtualizer's full callback contract, driven by the",
                "recorder's window sim (the recycler stand-in). Mount pins",
                "create_virtualizer(overscan, lane-model layout) -> the data",
                "effect's first virtualizer_data_changed -> apply_style. Rows",
                "realize LAZILY and DETACHED via mount_item (create + binding",
                "first-fire, no insert — placement is the backend's business);",
                "a shared-signal write updates every LIVE row exactly once;",
                "release_item frees the row's reactivity (released rows never",
                "update again); the measured-size cache is keyed by item KEY and",
                "survives release (the re-entering row mounts at its measured",
                "size, not the estimate); a count change re-fires",
                "virtualizer_data_changed; the swap-out (window cleared first)",
                "tears the node down. Two interleavings inside this golden are",
                "kernel-ordering artifacts, not contract: the live-row update",
                "order across sibling effects (README divergence #3) and the",
                "unstyle/release order inside the teardown window (divergence",
                "#5, old LIFO vs new creation order) — the new core pins its",
                "orderings via an override golden.",
            ],
            modes: &[Mode::Spliced],
            run: full_virtualizer_lifecycle,
        },
        FullScenario {
            name: "full_virtualizer_lane_swap",
            about: &[
                "P3-set: a `when` hole swapping between two virtualizer",
                "configurations (vertical list <-> 3-lane horizontal grid with",
                "gaps), pinning the VirtualLayout digest through create_* and the",
                "release_virtualizer ordering inside both dispose orderings",
                "(anchored: release then clear_children; spliced: remove_child",
                "then release). No rows are mounted — the node lifecycle itself",
                "is the contract.",
            ],
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_virtualizer_lane_swap,
        },
        FullScenario {
            name: "full_graphics_lifecycle",
            about: &[
                "P3-set: the graphics surface's lifecycle contract. Mount pins",
                "create_graphics(on_ready/on_resize/on_lost) -> apply_style; the",
                "platform sim fires ready (author reads size+scale), resize, and",
                "lost, each reaching the author's closures unwrapped ('author'",
                "lines); the swap-out releases the surface in both dispose",
                "orderings. The unstyle/release interleaving inside the teardown",
                "window is a kernel-ordering artifact (README divergence #5: old",
                "LIFO, new creation order — override-pinned); an unstyled",
                "surface still releases (independent-probe contract, pinned by",
                "the vocabulary suite).",
            ],
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_graphics_lifecycle,
        },
        FullScenario {
            name: "full_portal_toggle",
            about: &[
                "P3-set: a raw viewport portal behind a `when` toggle. Mount pins",
                "create_portal(target/on_dismiss/trap_focus) -> child insertion;",
                "the close step pins release_portal firing inside the teardown",
                "window (anchored: scope-drop [release] then clear; spliced:",
                "remove then scope-drop [release]); reopen mounts fresh.",
            ],
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_portal_toggle,
        },
        FullScenario {
            name: "full_overlay_static",
            about: &[
                "P3-set: the overlay compositions, static mount only. A modal",
                "overlay (Dismiss backdrop pressable + styled content wrapper +",
                "Bottom placement), a click-through overlay (no backdrop; portal",
                "root pinned pointer-events:none — the empty-ToastHost fix), and",
                "an anchored overlay (side/align/offset intent digested; backdrop",
                "None so no scrim child). Pins the composition lowering:",
                "backdrop-first-then-content child order and each portal's",
                "create_* argument rendering.",
            ],
            modes: &[Mode::Spliced],
            run: full_overlay_static,
        },
        FullScenario {
            name: "full_overlay_toggle",
            about: &[
                "P3-set: a modal overlay (Dismiss backdrop with a style, plain",
                "content) behind a `when` toggle. The close step pins the",
                "composed teardown window: portal release_portal + backdrop",
                "on_node_unstyled, both after the structural remove. The",
                "interleaving WITHIN the window differs across cores (README",
                "divergence #5: old LIFO scope cleanups vs new creation-order",
                "effect free) — override-pinned on the new side.",
            ],
            modes: &[Mode::Spliced],
            run: full_overlay_toggle,
        },
        FullScenario {
            name: "full_presence_cycle",
            about: &[
                "P3-set: presence with enter AND exit anims between two static",
                "siblings (pins the placeholder's in-flow position and that the",
                "handler never styles it). show: child mounts + snaps to the",
                "enter state pre-paint; the pumped frame applies rest with the",
                "enter transition. hide: the exit transform applies with the",
                "node still attached; the pumped timer runs the deferred",
                "detach + teardown. reshow mounts a fresh child. New-core",
                "override: presence is re-expressed on the scene Dyn driver +",
                "retire hook (README divergence #6).",
            ],
            modes: &[Mode::Anchored, Mode::Spliced],
            run: full_presence_cycle,
        },
        FullScenario {
            name: "full_presence_bare",
            about: &[
                "P3-set: presence with NO animations, initially present. Mount",
                "pins placeholder -> child build -> insert; hide is an immediate",
                "teardown (old: scope-drop then clear_children; new-core",
                "override: retire-owned scope-drop then remove_child — README",
                "divergence #6).",
            ],
            modes: &[Mode::Spliced],
            run: full_presence_bare,
        },
        FullScenario {
            name: "nav_swap_select",
            about: &[
                "Navigator (swap): mount = root container + fill style, the",
                "framework-mounted initial screen, then the (deferred) author",
                "layout with chrome text + outlet, and the initial screen seated",
                "into the outlet (clear_children + insert). `select about` mounts",
                "the second screen fresh and swaps the outlet; `select home`",
                "re-inserts the CACHED screen (no creates, no builder re-run);",
                "selecting the active route again is a structural no-op.",
            ],
            modes: &[Mode::Spliced],
            run: nav_swap_select,
        },
        FullScenario {
            name: "nav_swap_dispose_evict",
            about: &[
                "Navigator (swap, LazyDisposing): switching away RELEASES the",
                "evicted screen's scope inside the step — its cleanup marker +",
                "on_node_unstyled land BEFORE the incoming screen's mount ops —",
                "and returning re-mounts it fresh (creates re-appear).",
            ],
            modes: &[Mode::Spliced],
            run: nav_swap_dispose_evict,
        },
        FullScenario {
            name: "nav_stack_push_pop",
            about: &[
                "Navigator (stack, Retain): mount seats the initial screen with",
                "the flow-fill overlay folded into its root style digest. `push`",
                "mounts the detail screen and swaps the outlet (the covered",
                "screen's scope stays alive). `pop` releases the popped screen's",
                "scope FIRST (cleanup marker + on_node_unstyled before the",
                "reveal), then re-inserts the RETAINED home screen without a",
                "rebuild.",
            ],
            modes: &[Mode::Spliced],
            run: nav_stack_push_pop,
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

// ===========================================================================
// (k) P3-set: virtualizer lifecycle (window sim drives the callbacks)
// ===========================================================================

fn full_virtualizer_lifecycle(cx: &mut FullCx) {
    let present: Signal<bool> = signal(true);
    let rows: Signal<usize> = signal(3);
    let version: Signal<i32> = signal(0);
    cx.mount(
        view(vec![when(
            move || present.get(),
            move || {
                virtualizer(
                    Box::new(move || rows.get()),
                    Box::new(|i| i as u64),
                    ItemSize::Measured(Rc::new(|_| 40.0)),
                    Rc::new(move |idx| {
                        text(move || format!("item {idx} v{}", version.get())).into_element()
                    }),
                )
                .overscan(2.0)
                .with_style(static_style(120.0, "#334455"))
                .into_element()
            },
            || text("gone").into_element(),
        )])
        .into_element(),
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

fn full_virtualizer_lane_swap(cx: &mut FullCx) {
    let grid: Signal<bool> = signal(false);
    fn cells(layouted: bool) -> runtime_core::Element {
        let b = virtualizer(
            Box::new(|| 6),
            Box::new(|i| i as u64),
            ItemSize::Known(Rc::new(|_| 100.0)),
            Rc::new(|_| text("cell").into_element()),
        );
        if layouted {
            b.axis(Axis::Horizontal)
                .lanes(Lanes::Fixed(3))
                .spacing(8.0, 4.0)
                .into_element()
        } else {
            b.into_element()
        }
    }
    cx.mount(
        view(vec![when(
            move || grid.get(),
            || cells(true),
            || cells(false),
        )])
        .into_element(),
    );
    cx.step("swap to 3-lane horizontal grid (release then create)", move || {
        grid.set(true)
    });
    cx.step("swap back to vertical list", move || grid.set(false));
}

// ===========================================================================
// (m) P3-set: graphics lifecycle
// ===========================================================================

fn full_graphics_lifecycle(cx: &mut FullCx) {
    let present: Signal<bool> = signal(true);
    let rec = cx.recorder();
    cx.mount(
        view(vec![when(
            move || present.get(),
            move || {
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
                .with_style(static_style(100.0, "#202020"))
                .into_element()
            },
            || text("no canvas").into_element(),
        )])
        .into_element(),
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

// ===========================================================================
// (P3-set) portal + overlay compositions + presence
// ===========================================================================

fn full_portal_toggle(cx: &mut FullCx) {
    use runtime_core::{portal, PortalTarget, ViewportPlacement};
    let open: Signal<bool> = signal(true);
    cx.mount(
        view(vec![when(
            move || open.get(),
            || {
                portal(
                    PortalTarget::Viewport(ViewportPlacement::Center),
                    vec![text("inside").into_element()],
                )
                .on_dismiss(|| {})
                .trap_focus(true)
                .into_element()
            },
            || text("closed").into_element(),
        )])
        .into_element(),
    );
    cx.step("close (release_portal inside the teardown window)", || {
        open.set(false)
    });
    cx.step("reopen (fresh portal mount)", || open.set(true));
}

fn full_overlay_static(cx: &mut FullCx) {
    use crate::full::test_anchor_target;
    use runtime_core::{
        anchored_overlay, overlay, BackdropMode, ElementAlign, ElementSide, ViewportPlacement,
    };
    cx.mount(
        view(vec![
            overlay(vec![text("modal body").into_element()])
                .placement(ViewportPlacement::Bottom)
                .on_dismiss(|| {})
                .backdrop_style(static_style(40.0, "#00000080"))
                .with_style(static_style(320.0, "#ffffff"))
                .into_element(),
            overlay(vec![text("toast").into_element()])
                .backdrop(BackdropMode::None)
                .click_through(true)
                .into_element(),
            anchored_overlay(test_anchor_target(), vec![text("tip").into_element()])
                .side(ElementSide::Above)
                .align(ElementAlign::Center)
                .offset(4.0)
                .into_element(),
        ])
        .into_element(),
    );
}

fn full_overlay_toggle(cx: &mut FullCx) {
    use runtime_core::overlay;
    let open: Signal<bool> = signal(true);
    cx.mount(
        view(vec![when(
            move || open.get(),
            || {
                overlay(vec![text("modal body").into_element()])
                    .backdrop_style(static_style(40.0, "#00000080"))
                    .on_dismiss(|| {})
                    .into_element()
            },
            || text("closed").into_element(),
        )])
        .into_element(),
    );
    cx.step("close (backdrop unstyle + release_portal in one window)", || {
        open.set(false)
    });
    cx.step("reopen", || open.set(true));
}

fn full_presence_cycle(cx: &mut FullCx) {
    use crate::full::{presence_enter, presence_exit, pump_frames, pump_timers};
    use runtime_core::presence;
    let open: Signal<bool> = signal(false);
    cx.mount(
        view(vec![
            text("before").into_element(),
            presence(|| text("card").into_element())
                .present(move || open.get())
                .enter(presence_enter())
                .exit(presence_exit())
                .into_element(),
            text("after").into_element(),
        ])
        .into_element(),
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

fn full_presence_bare(cx: &mut FullCx) {
    use runtime_core::presence;
    let open: Signal<bool> = signal(true);
    cx.mount(
        view(vec![
            text("before").into_element(),
            presence(|| text("card").into_element())
                .present(move || open.get())
                .into_element(),
            text("after").into_element(),
        ])
        .into_element(),
    );
    cx.step("hide (no exit: immediate teardown)", || open.set(false));
}

// ===========================================================================
// Navigator scenarios (outlet-model swap/stack via the REAL SDK handlers)
// ===========================================================================

use runtime_core::primitives::navigator::Route;

const NAV_HOME: Route<()> = Route::new("home", "/");
const NAV_ABOUT: Route<()> = Route::new("about", "/about");
const NAV_DETAIL: Route<()> = Route::new("detail", "/detail");

fn nav_swap_select(cx: &mut FullCx) {
    use swap_navigator::{SwapBuilder, SwapNavigator};
    cx.register_navigators();
    let on_select: Rc<std::cell::RefCell<Option<Rc<dyn Fn(&'static str)>>>> =
        Rc::new(std::cell::RefCell::new(None));
    let nav = SwapNavigator::new(&NAV_HOME)
        .screen(NAV_HOME, |_| {
            view(vec![text("home").into_element()])
                .with_style(static_style(10.0, "#111111"))
                .into_element()
        })
        .screen(NAV_ABOUT, |_| {
            view(vec![text("about").into_element()])
                .with_style(static_style(20.0, "#222222"))
                .into_element()
        })
        .layout({
            let slot = on_select.clone();
            move |ctx| {
                *slot.borrow_mut() = Some(ctx.on_select.clone());
                view(vec![text("chrome").into_element(), ctx.outlet]).into_element()
            }
        });
    cx.mount(nav.into_element());
    let select = move |name| {
        (on_select.borrow().clone().expect("layout ran during mount"))(name)
    };
    cx.step("select about (fresh mount + outlet swap)", || select("about"));
    cx.step("select home (cached re-insert, no creates)", || select("home"));
    cx.step("select home again (no-op)", || select("home"));
}

fn nav_swap_dispose_evict(cx: &mut FullCx) {
    use swap_navigator::{MountPolicy, SwapBuilder, SwapNavigator};
    cx.register_navigators();
    let rec = cx.recorder();
    let on_select: Rc<std::cell::RefCell<Option<Rc<dyn Fn(&'static str)>>>> =
        Rc::new(std::cell::RefCell::new(None));
    let nav = SwapNavigator::new(&NAV_HOME)
        .screen(NAV_HOME, move |_| {
            let rec = rec.clone();
            runtime_core::on_cleanup(move || rec.note_cleanup("home-screen"));
            view(vec![text("home").into_element()])
                .with_style(static_style(10.0, "#111111"))
                .into_element()
        })
        .screen(NAV_ABOUT, |_| {
            view(vec![text("about").into_element()])
                .with_style(static_style(20.0, "#222222"))
                .into_element()
        })
        .mount_policy(MountPolicy::LazyDisposing)
        .layout({
            let slot = on_select.clone();
            move |ctx| {
                *slot.borrow_mut() = Some(ctx.on_select.clone());
                view(vec![text("chrome").into_element(), ctx.outlet]).into_element()
            }
        });
    cx.mount(nav.into_element());
    let select = move |name| {
        (on_select.borrow().clone().expect("layout ran during mount"))(name)
    };
    cx.step("select about (evicted home releases: cleanup + unstyle)", || {
        select("about")
    });
    cx.step("select home (re-mounts fresh)", || select("home"));
}

fn nav_stack_push_pop(cx: &mut FullCx) {
    use stack_navigator::{StackBuilder, StackHandle, StackNavigator, StackRetention};
    cx.register_navigators();
    let rec = cx.recorder();
    let nav_ref: runtime_core::Ref<StackHandle> = runtime_core::Ref::new();
    let nav = StackNavigator::new(&NAV_HOME)
        .screen(NAV_HOME, |_| {
            view(vec![text("home").into_element()])
                .with_style(static_style(10.0, "#111111"))
                .into_element()
        })
        .screen(NAV_DETAIL, move |_| {
            let rec = rec.clone();
            runtime_core::on_cleanup(move || rec.note_cleanup("detail-screen"));
            view(vec![text("detail").into_element()])
                .with_style(static_style(20.0, "#222222"))
                .into_element()
        })
        .retention(StackRetention::Retain)
        .layout(|ctx| view(vec![text("header").into_element(), ctx.outlet]).into_element())
        .bind(nav_ref.clone());
    cx.mount(nav.into_element());
    // Clone the handle OUT of the Ref before dispatching: `Ref::with`
    // holds the arena borrow, and dispatch sets nav-state signals
    // (the documented Ref::with-borrow abort class).
    let handle = nav_ref.with(|h| h.clone()).expect("handle bound");
    let push_handle = handle.clone();
    cx.step("push detail (mount + outlet swap, home retained)", move || {
        push_handle.push(&NAV_DETAIL, ());
    });
    cx.step("pop (release detail FIRST, reveal retained home)", move || {
        handle.pop();
    });
}
