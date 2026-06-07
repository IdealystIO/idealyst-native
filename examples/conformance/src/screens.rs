//! The conformance screens. The **root** screen is a single scrollable
//! column packing every covered primitive in a weird-but-legal
//! configuration; the **detail** screen exists purely to exercise stack
//! push/pop.
//!
//! Every element the suite asserts against carries a `.test_id(...)` set via
//! the *builder* form (not `ui!`-attribute form): the builder method is
//! gated only on `runtime-core/robot` (always on under `idealyst dev`),
//! whereas the macro-attribute form needs this crate's own `robot` feature.
//! An E2E screen lives or dies by its `test_id`s resolving, so the builder
//! form is the reliable choice.

use std::rc::Rc;

use idea_ui::{Modal, Stack, StackGap, StackPadding};
use icons_lucide::HOME;
use runtime_core::presence;
use runtime_core::primitives::activity_indicator::activity_indicator;
use runtime_core::primitives::scroll_view::scroll_view;
use runtime_core::primitives::slider::slider;
use runtime_core::{
    button, component, icon, pressable, signal, text, text_input, toggle, ui, view, when, Element,
    IntoElement, Ref, Signal,
};
use stack_navigator::StackHandle;

use crate::State;

/// The root torture screen.
pub(crate) fn root_page(state: State, nav: Ref<StackHandle>) -> Element {
    // — Counter, driven by a button, a decrement button, and a pressable
    //   container (three distinct click paths into one signal). —
    let inc = move || state.count.update(|n| *n += 1);
    let dec = move || state.count.update(|n| *n -= 1);
    let press5 = move || state.count.update(|n| *n += 5);

    let counter = text(move || format!("Counter: {}", state.count.get()))
        .test_id("counter")
        .into_element();

    // — Toggle reveals a `when` branch (mount/unmount of the slider + an
    //   extra marker). The hidden branch is fully disposed, so the suite can
    //   assert count 0 → 1 as it toggles. —
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

    // — Modal: a portal whose card is a Pressable WRAPPING an interactive
    //   button. This is the iOS/macOS modal-pressability regression — the
    //   confirm button must still fire while nested inside the card's tap
    //   recognizer. The suite opens it, clicks confirm, and asserts the
    //   `confirmed` counter ticked. —
    let open_modal = move || state.modal_open.set(true);
    let confirmed = text(move || format!("Confirmed: {}", state.confirmed.get()))
        .test_id("confirmed")
        .into_element();

    let modal_branch = when(
        move || state.modal_open.get(),
        move || {
            let dismiss: Rc<dyn Fn()> = Rc::new(move || state.modal_open.set(false));
            let confirm = move || {
                state.confirmed.update(|n| *n += 1);
                state.modal_open.set(false);
            };
            let modal_children: Vec<Element> = vec![
                text("Confirm action?").test_id("modal-title").into_element(),
                button("Confirm", confirm)
                    .test_id("modal-confirm")
                    .into_element(),
            ];
            ui! { Modal(on_dismiss = Some(dismiss)) { modal_children } }
        },
        || view(vec![]).into_element(),
    );

    // — Stack push. Native back chrome exists on iOS/Android/web; the
    //   detail screen also carries an in-content Back for terminal + the
    //   suite. —
    let push = move || {
        nav.get().map(|h| h.push(&crate::DETAIL, ())).unwrap_or_default();
    };
    let goto_components = move || {
        nav.get()
            .map(|h| h.push(&crate::COMPONENTS, ()))
            .unwrap_or_default();
    };

    let children: Vec<Element> = vec![
        text("Conformance").test_id("title").into_element(),
        counter,
        button("Increment", inc).test_id("inc").into_element(),
        button("Decrement", dec).test_id("dec").into_element(),
        pressable(vec![text("Press me (+5)").into_element()], press5)
            .test_id("press5")
            .into_element(),
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
        button("Components", goto_components)
            .test_id("goto-components")
            .into_element(),
    ];

    // Wrap in a scroll view (weird condition: scrollable content) around a
    // spaced Stack.
    let column = ui! { Stack(gap = StackGap::Md, padding = StackPadding::Lg) { children } };
    scroll_view(vec![column]).into_element()
}

/// Reactive list with a PER-ROW conditional affordance — the exact shape of
/// the whiteboard Layers popover's `CanvasRow`: a `for` over a `Signal<Vec>`
/// where each KEPT row renders `if rows.len() > 1 { DelMarker }`. When the list
/// shrinks to a single row, every surviving row's `when` must re-evaluate to
/// false and REMOVE its marker. The suite asserts the marker count drops to 0.
/// Guards the iOS bug where a kept row's conditional didn't drop on list
/// shrink (the whiteboard "delete button won't disappear on the last canvas").
#[component]
fn ReflowBox() -> Element {
    let rows: Signal<Vec<i32>> = signal!(vec![0, 1, 2]);
    let active: Signal<usize> = signal!(0);
    let remove = move || {
        // Mimic `delete_canvas`: CHANGE `active` to a different value FIRST
        // (re-running the row's sibling effect that reads active + the list),
        // THEN shrink the list.
        active.set(active.get().wrapping_add(1));
        rows.update(|v| {
            if v.len() > 1 {
                v.remove(0);
            }
        });
    };

    let remove_btn = button("Remove row", remove)
        .test_id("remove-row")
        .into_element();

    // Wrap the list in TWO NESTED presences — the whiteboard popover is
    // `focus_gate(presence(... Each ...))`, and `focus_gate` is itself a
    // `presence`. Tests whether a `when` inside an `Each` inside nested
    // presences still drops on list shrink.
    let list_presence = presence(move || {
        presence(move || {
            ui! {
                view {
                    for r in rows, key = *r {
                        ReflowRow(rows = rows, active = active, id = r)
                    }
                }
            }
        })
        .into_element()
    })
    .into_element();

    ui! {
        view {
            list_presence
            remove_btn
        }
    }
}

/// One row — a `#[component]` (like the whiteboard's `CanvasRow`) that holds the
/// `if rows.len() > 1 { DelMarker }` conditional and reads the LIST signal via a
/// prop. This is the faithful shape: a `when` INSIDE a kept component INSIDE an
/// `Each`, gated on the same list that drives the `Each`.
#[component]
fn ReflowRow(props: &ReflowRowProps) -> Element {
    let rows = props.rows;
    let active = props.active;
    let id = props.id;
    // This row's POSITION in the (reactive) list — exactly the whiteboard's
    // `index_of = canvas_ids.position(id)`. Read it in a SIBLING reactive effect
    // (like `row_style`/`label_style` read `active == index_of()`), so multiple
    // per-row effects subscribe to the list signal via `position`, and a delete
    // SHIFTS those positions.
    let index_of = move || rows.get().iter().position(|x| *x == id).unwrap_or(0);
    // EXACTLY like the whiteboard's `del_visible`: a `memo` (a `Signal<bool>`),
    // branched on as a BARE `if del_visible`. `ui!` is type-driven, so this is
    // reactive because the condition's *type* is a reactive signal — and it
    // must re-evaluate to false (dropping the marker) when the list shrinks to
    // one. A plain `move || …` closure here would be an opaque `fn() -> bool`,
    // which the macro treats as STATIC — the original "won't disappear" bug.
    let del_visible = runtime_core::memo(move || rows.get().len() > 1);
    ui! {
        view {
            text(move || format!("i{} a{}", index_of(), active.get()))
            if del_visible {
                DelMarker()
            }
        }
    }
}

pub struct ReflowRowProps {
    pub rows: Signal<Vec<i32>>,
    pub active: Signal<usize>,
    pub id: i32,
}

impl Default for ReflowRowProps {
    fn default() -> Self {
        Self {
            rows: Signal::new(Vec::new()),
            active: Signal::new(0),
            id: 0,
        }
    }
}

/// The per-row conditional affordance. A `#[component]` (like the whiteboard's
/// `DeleteCanvasButton`) so the `if` branch holds no captures. All rows share
/// the `del-marker` test_id; the suite counts them.
#[component]
fn DelMarker() -> Element {
    text("del").test_id("del-marker").into_element()
}

/// The pushed detail screen — proves stack push/pop. Its `detail-marker`
/// test_id is unique, so its presence/absence is an unambiguous proxy for
/// "is the detail screen on top".
pub(crate) fn detail_page(nav: Ref<StackHandle>) -> Element {
    let back = move || {
        nav.get().map(|h| h.pop()).unwrap_or_default();
    };
    let children: Vec<Element> = vec![
        text("Detail screen").test_id("detail-marker").into_element(),
        button("Back", back).test_id("back").into_element(),
    ];
    ui! { Stack(gap = StackGap::Lg, padding = StackPadding::Lg) { children } }
}

/// idea-ui component coverage — `Switch`, `Checkbox`, `Button` (idea-ui "as a
/// key implementor"). Each carries a forwarded `test_id` so the robot can
/// drive it; each is paired with a primitive status `text` (whose own
/// `test_id` the suite asserts on) reflecting the component's state.
pub(crate) fn components_page(nav: Ref<StackHandle>) -> Element {
    use idea_ui::{Button, Checkbox, Switch};

    let sw = runtime_core::signal!(false);
    let cb = runtime_core::signal!(false);
    let clicks = runtime_core::signal!(0_i32);

    let on_sw: Rc<dyn Fn(bool)> = Rc::new(move |v| sw.set(v));
    let on_cb: Rc<dyn Fn(bool)> = Rc::new(move |v| cb.set(v));
    let on_btn: Rc<dyn Fn()> = Rc::new(move || clicks.update(|n| *n += 1));
    let on_back: Rc<dyn Fn()> = Rc::new(move || {
        nav.get().map(|h| h.pop()).unwrap_or_default();
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
    ui! { Stack(gap = StackGap::Md, padding = StackPadding::Lg) { children } }
}
