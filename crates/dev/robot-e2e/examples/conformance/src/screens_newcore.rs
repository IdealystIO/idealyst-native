//! The conformance screens on the NEW core — the same torture layout as
//! `screens.rs` (same `test_id`s, same reactive shapes, same nested
//! presence/keyed-list/conditional structure), authored against the
//! vocabulary glue + `ui!`'s new-core lowering.
//!
//! Deliberate deltas from the old-core file, each a named blocked seam
//! (documented in suites.rs; never a silent weakening):
//!
//! - **idea-ui is old-core-only** (its own `ui!` sites lower to the old
//!   core; SDK retarget P6): the `Stack` wrapper becomes a plain
//!   `view`, and the `Modal` becomes the primitive composition it wraps
//!   — an `overlay` (Center placement, dismissable backdrop) whose card
//!   is a `pressable` WRAPPING the confirm button, which is the exact
//!   nested-pressability regression the modal suite exists to catch.
//!   The COMPONENTS screen (idea-ui Switch/Checkbox/Button) has no
//!   new-core counterpart yet.
//! - **`#[method]` is P5-deferred** on the new core (loud macro error):
//!   the MethodCounter component is omitted; its suite is gated out.
//!
//! Like the old file, elements the suite asserts against use the
//! *builder* `.test_id(...)` form where they're built by builders, and
//! the `ui!` attribute form inside components — both lower to the same
//! vocabulary registry slot on this core.

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
/// through this cell (the new-core stand-in for `Ref<StackHandle>`).
pub(crate) type NavCell = Rc<RefCell<Option<NavHandle>>>;

/// App-wide reactive state — same fields as the old-core `State`.
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
/// ROOT/DETAIL routes (COMPONENTS is old-core-only — module docs).
/// Must run with the owning world ambient (`newcore::start` wraps the
/// build in `World::enter`).
pub fn app_newcore() -> Element {
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
    runtime_core::after_ms_detached(crate::INITIAL_RUN_DELAY_MS, crate::suites::run_all);

    let nav_root = nav.clone();
    let nav_detail = nav.clone();
    let nav_fill = nav.clone();
    runtime_vocabulary::builders::stack_navigator(&crate::ROOT)
        .screen(crate::ROOT, move |_| root_page(state, nav_root.clone()))
        .screen(crate::DETAIL, move |_| detail_page(nav_detail.clone()))
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
        button("Push detail", push).test_id("push-detail").into_element(),
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
