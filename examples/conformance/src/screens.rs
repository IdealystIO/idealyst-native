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
use runtime_core::primitives::activity_indicator::activity_indicator;
use runtime_core::primitives::scroll_view::scroll_view;
use runtime_core::primitives::slider::slider;
use runtime_core::{
    button, icon, pressable, text, text_input, toggle, ui, view, when, Element, IntoElement, Ref,
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
        button("Push detail", push).test_id("push-detail").into_element(),
    ];

    // Wrap in a scroll view (weird condition: scrollable content) around a
    // spaced Stack.
    let column = ui! { Stack(gap = StackGap::Md, padding = StackPadding::Lg) { children } };
    scroll_view(vec![column]).into_element()
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
