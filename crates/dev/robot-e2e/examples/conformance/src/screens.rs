//! The conformance screens — a deliberately weird torture layout
//! (reactive labels, conditional mount/unmount, a portal modal whose card
//! wraps interactive content, nested scroll, a keyed list), every asserted
//! element carrying a stable `test_id`.
//!
//! Notable shapes the suites depend on:
//!
//! - The `Modal` is the primitive composition idea-ui's Modal wraps: an
//!   `overlay` (Center placement, dismissable backdrop) whose card is a
//!   `pressable` WRAPPING the confirm button — the exact
//!   nested-pressability regression the modal suite exists to catch.
//!   Primitives on purpose: this suite pins the PRIMITIVE composition,
//!   and idea-ui coverage lives on the COMPONENTS screen.
//! - The COMPONENTS screen drives the real idea-ui Switch / Checkbox /
//!   Button; the back affordance pops via the vocabulary `NavHandle`.
//! - `MethodCounter` exercises `#[method]` in the inline-props form (the
//!   shape `#[method]` supports) with a REACTIVE label, which is what the
//!   `component methods` suite asserts against.
//!
//! Elements the suite asserts against use the *builder* `.test_id(...)`
//! form where they're built by builders and the `ui!` attribute form
//! inside components — both lower to the same registry slot.

use std::cell::RefCell;
use std::rc::Rc;

use icons_lucide::HOME;
use runtime_macros::{component, ui};
use runtime_vocabulary::glue::primitives::activity_indicator::activity_indicator;
use runtime_vocabulary::glue::primitives::overlay::{overlay, BackdropMode, ViewportPlacement};
use runtime_vocabulary::glue::primitives::scroll_view::scroll_view;
use runtime_vocabulary::glue::primitives::slider::slider;
use runtime_vocabulary::glue::primitives::text_input::text_input;
use runtime_vocabulary::glue::primitives::toggle::toggle;
use runtime_vocabulary::glue::{
    button, icon, memo, signal, text, view, when, Element, IntoElement, Signal,
};
use runtime_vocabulary::prims::NavHandle;

/// The stack handle arrives at mount via `.on_handle` — screens share it
/// through this cell.
pub(crate) type NavCell = Rc<RefCell<Option<NavHandle>>>;

/// App-wide reactive state. Lives in the root scope so it survives
/// navigation — the suite asserts against it across push/pop.
#[derive(Clone, Copy)]
pub struct State {
    pub count: Signal<i32>,
    pub show_extra: Signal<bool>,
    pub slider: Signal<f32>,
    pub name: Signal<String>,
    pub modal_open: Signal<bool>,
    pub confirmed: Signal<i32>,
}

/// Build the app root: the vocabulary stack navigator over the same
/// The root screen: the primitives torture layout.
/// Must run with the owning world ambient (`newcore::start` wraps the
/// build in `World::enter`).
pub fn app() -> Element {
    // The idea-ui screens style through the installed theme sheets. This
    // runs inside `newcore::start`'s `World::enter`, so it lands in this
    // world's ThemeCtx.
    idea_ui::install_idea_theme(idea_ui::light_theme());

    let state = State {
        count: signal(0),
        show_extra: signal(false),
        slider: signal(0.0_f32),
        name: signal(String::new()),
        modal_open: signal(false),
        confirmed: signal(0),
    };

    let nav: NavCell = Rc::new(RefCell::new(None));

    #[cfg(feature = "robot")]
    runtime_vocabulary::glue::after_ms_detached(crate::INITIAL_RUN_DELAY_MS, crate::suites::run_all);

    let nav_root = nav.clone();
    let nav_detail = nav.clone();
    let nav_fill = nav.clone();
    let nav_components = nav.clone();
    runtime_vocabulary::builders::stack_navigator(&crate::ROOT)
        .screen(crate::ROOT, move |_| root_page(state, nav_root.clone()))
        .screen(crate::DETAIL, move |_| detail_page(nav_detail.clone()))
        .screen(crate::COMPONENTS, move |_| components_page(nav_components.clone()))
        .on_handle(move |h| *nav_fill.borrow_mut() = Some(h))
        .build()
}

/// The root torture screen (mirror of `screens.rs::root_page`).
pub(crate) fn root_page(state: State, nav: NavCell) -> Element {
    // — Counter, driven by a button, a decrement button, and a pressable
    //   container (three distinct click paths into one signal). —
    let inc = move || state.count.update(|n| n + 1);
    let dec = move || state.count.update(|n| n - 1);
    let press5 = move || state.count.update(|n| n + 5);

    let counter = text(move || format!("Counter: {}", state.count.get()))
        .test_id("counter")
        .into_element();

    // — Toggle reveals a `when` branch (mount/unmount of the slider + an
    //   extra marker). —
    let toggle_extra = toggle(state.show_extra, move |v| state.show_extra.set(v))
        .test_id("toggle")
        .into_element();

    let reveal = move || {
        let extra: Vec<Element> = vec![
            text("Extra revealed").test_id("extra").into_element(),
            slider(state.slider, move |v| state.slider.set(v))
                .range(0.0, 100.0)
                .test_id("slider")
                .into_element(),
            text(move || format!("Slider: {}", state.slider.get() as i32))
                .test_id("slider-val")
                .into_element(),
        ];
        view(extra).into_element()
    };
    let extra_branch = when(
        move || state.show_extra.get(),
        reveal,
        || view(vec![]).into_element(),
    );

    // — Text input echoed into a live greeting. —
    let name = state.name;
    let greeting = text(move || {
        let n = name.get();
        if n.is_empty() {
            "Hello, stranger".to_string()
        } else {
            format!("Hello, {n}")
        }
    })
    .test_id("greeting")
    .into_element();

    // — Modal: the primitive composition idea-ui's Modal wraps (module
    //   docs) — an overlay whose card is a Pressable WRAPPING an
    //   interactive button. The suite opens it, clicks confirm, and
    //   asserts the `confirmed` counter ticked. —
    let open_modal = move || state.modal_open.set(true);
    let confirmed = text(move || format!("Confirmed: {}", state.confirmed.get()))
        .test_id("confirmed")
        .into_element();

    let modal_branch = when(
        move || state.modal_open.get(),
        move || {
            let confirm = move || {
                state.confirmed.update(|n| n + 1);
                state.modal_open.set(false);
            };
            // Card = pressable wrapping the button (the nested-tap
            // regression shape); overlay = Center + dismissable
            // backdrop, the Modal defaults.
            let card = runtime_vocabulary::builders::pressable(|| {})
                .child(text("Confirm action?").test_id("modal-title").into_element())
                .child(button("Confirm", confirm).test_id("modal-confirm").into_element())
                .build();
            overlay(vec![card])
                .placement(ViewportPlacement::Center)
                .backdrop(BackdropMode::Dismiss)
                .on_dismiss(move || state.modal_open.set(false))
                .trap_focus(true)
                .into_element()
        },
        || view(vec![]).into_element(),
    );

    // — Stack push. —
    let nav_components = nav.clone();
    let goto_components = move || {
        if let Some(h) = nav_components.borrow().as_ref() {
            h.push(&crate::COMPONENTS, ());
        }
    };
    let push = move || {
        if let Some(h) = nav.borrow().as_ref() {
            h.push(&crate::DETAIL, ());
        }
    };

    let children: Vec<Element> = vec![
        text("Conformance").test_id("title").into_element(),
        counter,
        button("Increment", inc).test_id("inc").into_element(),
        button("Decrement", dec).test_id("dec").into_element(),
        runtime_vocabulary::builders::pressable(press5)
            .child(text("Press me (+5)").into_element())
            .test_id("press5")
            .build(),
        toggle_extra,
        extra_branch,
        text_input(name, move |s: String| name.set(s))
            .placeholder("Type a name".to_string())
            .test_id("name")
            .into_element(),
        greeting,
        activity_indicator().test_id("spinner").into_element(),
        icon(HOME).test_id("icon").into_element(),
        button("Open modal", open_modal)
            .test_id("open-modal")
            .into_element(),
        confirmed,
        modal_branch,
        ui! { ReflowBox() },
        // A `#[method]`-bearing component: exercises robot/inspector
        // method invocation (same placement as the old file).
        ui! { MethodCounter(initial = 10i32) },
        button("Push detail", push).test_id("push-detail").into_element(),
        button("Components", goto_components)
            .test_id("goto-components")
            .into_element(),
    ];

    // Wrap in a scroll view (weird condition: scrollable content). The
    // idea-ui Stack becomes a plain column view on this core.
    let column = view(children).into_element();
    scroll_view(vec![column]).into_element()
}

/// Reactive list with a PER-ROW conditional affordance — the whiteboard
/// `CanvasRow` shape ported 1:1 from `screens.rs::ReflowBox` (see that
/// file's docs for the bug this guards).
#[component]
fn ReflowBox() -> Element {
    let rows: Signal<Vec<i32>> = signal(vec![0, 1, 2]);
    let active: Signal<usize> = signal(0);
    let remove = move || {
        // Mimic `delete_canvas`: CHANGE `active` first, THEN shrink.
        active.update(|a| a.wrapping_add(1));
        rows.update(|v| {
            let mut v = v.clone();
            if v.len() > 1 {
                v.remove(0);
            }
            v
        });
    };

    ui! {
        view {
            // TWO NESTED presences around the keyed list, exactly like
            // the old file (focus_gate(presence(...Each...))).
            presence(present = || true) {
                presence(present = || true) {
                    view {
                        for r in rows, key = r {
                            ReflowRow(rows = rows, active = active, id = r)
                        }
                    }
                }
            }
            button(label = "Remove row", test_id = "remove-row", on_click = remove)
        }
    }
}

/// One row — the `when`-inside-kept-component-inside-`Each` shape.
#[component]
fn ReflowRow(
    rows: Signal<Vec<i32>>,
    active: Signal<usize>,
    /// Row identity (static — it IS the key).
    #[prop(static)]
    id: i32,
) -> Element {
    let index_of = move || rows.get().iter().position(|x| *x == id).unwrap_or(0);
    // A memo branched on as a bare `if` — reactive because the
    // condition's TYPE is a signal (the original "won't disappear" bug).
    let del_visible = memo(move || rows.get().len() > 1);
    ui! {
        view {
            text { move || format!("i{} a{}", index_of(), active.get()) }
            if del_visible {
                DelMarker()
            }
        }
    }
}

/// The per-row conditional affordance; all rows share the `del-marker`
/// test_id (the suite counts them).
#[component]
fn DelMarker() -> Element {
    ui! {
        text(test_id = "del-marker") { "del" }
    }
}

// ---------------------------------------------------------------------------
// MethodCounter — the `#[method]`-bearing component the methods suite
// drives via `list_components` → `invoke_method` (mirror of screens.rs;
// module docs cover the two deliberate deltas: inline-props shape and
// the reactive label).
// ---------------------------------------------------------------------------

#[component]
fn MethodCounter(
    /// Mount-time starting value (static — the suite asserts it).
    #[prop(static)]
    initial: i32,
) -> Element {
    let value = signal(initial);
    // Bodies use `set(get() + n)`: the two cores' `update` closure
    // shapes differ, and this file's methods mirror the old file's
    // BEHAVIOR, not its core-specific spelling.
    /// No-arg increment — the inspector's easy manual case.
    #[method]
    fn increment() {
        value.set(value.get() + 1);
    }
    #[method]
    fn reset() {
        value.set(0);
    }
    #[method]
    fn bump_by(n: i32) {
        value.set(value.get() + n);
    }

    // Builder-form tail like the old file; the `#[component]` macro
    // wraps this root view in the instance link (`__component_root`).
    let label = text(move || format!("methods: {}", value.get()))
        .test_id("method-counter-val")
        .into_element();
    view(vec![label]).test_id("method-counter").into_element()
}

/// The pushed detail screen — proves stack push/pop.
pub(crate) fn detail_page(nav: NavCell) -> Element {
    let back = move || {
        if let Some(h) = nav.borrow().as_ref() {
            h.pop();
        }
    };
    let children: Vec<Element> = vec![
        text("Detail screen").test_id("detail-marker").into_element(),
        button("Back", back).test_id("back").into_element(),
    ];
    view(children).into_element()
}

/// idea-ui component coverage — a `Switch`/`Checkbox`/`Button` screen
/// with stable `test_id`s and status texts the idea-ui suite asserts
/// against. The back affordance pops via the vocabulary `NavHandle`.
pub(crate) fn components_page(nav: NavCell) -> Element {
    use idea_ui::{Button, Checkbox, Switch};

    let sw = signal(false);
    let cb = signal(false);
    let clicks = signal(0_i32);

    let on_sw: Rc<dyn Fn(bool)> = Rc::new(move |v| sw.set(v));
    let on_cb: Rc<dyn Fn(bool)> = Rc::new(move |v| cb.set(v));
    let on_btn: Rc<dyn Fn()> = Rc::new(move || clicks.update(|n| n + 1));
    let on_back: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(h) = nav.borrow().as_ref() {
            h.pop();
        }
    });

    let sw_status = text(move || format!("switch={}", sw.get()))
        .test_id("ui-switch-status")
        .into_element();
    let cb_status = text(move || format!("check={}", cb.get()))
        .test_id("ui-check-status")
        .into_element();
    let btn_status = text(move || format!("clicks={}", clicks.get()))
        .test_id("ui-button-status")
        .into_element();

    let children: Vec<Element> = vec![
        text("Components").test_id("components-marker").into_element(),
        ui! {
            Switch(
                value = sw,
                on_change = on_sw,
                label = Some("Notifications".to_string()),
                test_id = Some("ui-switch"),
            )
        },
        sw_status,
        ui! {
            Checkbox(
                value = cb,
                on_change = on_cb,
                label = Some("Accept terms".to_string()),
                test_id = Some("ui-check"),
            )
        },
        cb_status,
        ui! { Button(label = "Tap me".to_string(), on_click = on_btn, test_id = Some("ui-button")) },
        btn_status,
        ui! { Button(label = "Back".to_string(), on_click = on_back, test_id = Some("comp-back")) },
    ];
    view(children).into_element()
}
