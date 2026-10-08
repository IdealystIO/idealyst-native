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
    let nav_media = nav.clone();
    runtime_vocabulary::builders::stack_navigator(&crate::ROOT)
        .screen(crate::ROOT, move |_| root_page(state, nav_root.clone()))
        .screen(crate::DETAIL, move |_| detail_page(nav_detail.clone()))
        .screen(crate::COMPONENTS, move |_| components_page(nav_components.clone()))
        .screen(crate::MEDIA, move |_| media_page(nav_media.clone()))
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
    let nav_media = nav.clone();
    let goto_media = move || {
        if let Some(h) = nav_media.borrow().as_ref() {
            h.push(&crate::MEDIA, ());
        }
    };
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
        button("Media", goto_media).test_id("goto-media").into_element(),
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

    // Builder-form tail like the old file; the `#[component]` macro's
    // registration links this root view to the instance.
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
        shadow_parity_row(),
    ];
    view(children).into_element()
}

/// Cross-backend **box-shadow parity** fixture.
///
/// Three swatches that pin the shadow contract every backend has to meet:
///
/// - `shadow-plain`   — shadow, square corners.
/// - `shadow-rounded` — shadow + `border-radius`; the shadow must hug the curve.
/// - `shadow-clipped` — shadow + `border-radius` + `overflow: hidden`. **This is
///   the interesting one.** CSS clips descendants but does NOT clip the
///   element's own `box-shadow`, so web renders a shadow here. A single
///   CALayer can't do both — `masksToBounds` clips the shadow away — which is
///   why the Apple backends historically dropped it and idea-ui's `Card` docs
///   carry an "iOS caveat" telling authors to nest views by hand. Rule #7 says
///   one author tree renders the same everywhere, so this fixture exists to
///   hold the backends to that.
///
/// Shadows are deliberately heavy (opaque, wide blur) so the difference is
/// unambiguous in a screenshot rather than a subtle gradient.
fn shadow_parity_row() -> Element {
    use runtime_vocabulary::glue::{Color, Length, Overflow, Shadow, StyleRules, StyleSheet, Tokenized};

    fn swatch(radius: f32, clip: bool) -> Rc<StyleSheet> {
        let r = |v: f32| Some(Tokenized::Literal(Length::Px(v)));
        Rc::new(StyleSheet::r#static(StyleRules {
            background: Some(Tokenized::Literal(Color("#ffffff".into()))),
            width: r(120.0),
            height: r(60.0),
            border_top_left_radius: r(radius),
            border_top_right_radius: r(radius),
            border_bottom_left_radius: r(radius),
            border_bottom_right_radius: r(radius),
            // Space the swatches apart by more than the blur, or each shadow
            // lands under the next swatch's opaque white box and the fixture
            // proves nothing.
            margin_bottom: r(40.0),
            overflow: clip.then_some(Overflow::Hidden),
            shadow: Some(Shadow {
                x: 0.0,
                y: 10.0,
                blur: 24.0,
                color: Color("rgba(0, 0, 0, 0.85)".into()),
            }),
            ..Default::default()
        }))
    }

    ui! {
        view {
            text { "shadow parity" }
            view(style = swatch(0.0, false), test_id = "shadow-plain") {}
            view(style = swatch(16.0, false), test_id = "shadow-rounded") {}
            view(style = swatch(16.0, true), test_id = "shadow-clipped") {}
        }
    }
}

// ---------------------------------------------------------------------------
// Media screen — every `image()` source kind + an anchored menu pinned to
// the bottom edge.
// ---------------------------------------------------------------------------

/// 48×24 PNG: left half red, right half blue. Natural size 48×24 dp.
const PNG_DATA_URI: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAADAAAAAYCAIAAAAzn+mLAAAAMklEQVR4nO3OMQ0AMAgAMIzgh2v69+IEE3w0qYBGZ63I91eEkJCQkJCQkJCQkJDQrdAAHNC0jJaU5dgAAAAASUVORK5CYII=";

/// Percent-encoded SVG data URI (the CrewForge `BrandLockup` shape): a
/// 32×16 red/blue plate with a diagonal stroke, so a blurry upscale is
/// visible at 5×.
const SVG_DATA_URI: &str = "data:image/svg+xml,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2232%22%20height%3D%2216%22%3E%3Crect%20width%3D%2216%22%20height%3D%2216%22%20fill%3D%22%23e11d48%22%2F%3E%3Crect%20x%3D%2216%22%20width%3D%2216%22%20height%3D%2216%22%20fill%3D%22%231d4ed8%22%2F%3E%3Cpath%20d%3D%22M0%2016L32%200%22%20stroke%3D%22%23fff%22%20stroke-width%3D%221%22%2F%3E%3C%2Fsvg%3E";

/// Remote raster (PNG) and remote SVG — fetched off the UI thread.
const REMOTE_PNG: &str = "https://httpbin.org/image/png";
const REMOTE_SVG: &str = "https://www.rust-lang.org/logos/rust-logo-blk.svg";

/// Unresolvable host: the fetch fails → `on_error`.
const BROKEN_URL: &str = "https://nonexistent.invalid/missing.png";

/// The embedded SVG asset (`image_asset`), registered before mount.
fn badge() -> runtime_shared::assets::Asset<runtime_shared::assets::kinds::Image> {
    runtime_shared::embed_asset!("../assets/badge.svg")
}

/// One labelled image row: the image and a live `on_load`/`on_error`
/// status (`<id>-status`), which the media suite asserts against.
fn media_row(
    label: &'static str,
    img: runtime_vocabulary::glue::primitives::image::GlueImage,
    status_id: &'static str,
    size: Option<(f32, f32)>,
) -> Element {
    use runtime_vocabulary::glue::{AlignItems, FlexDirection, Length, StyleRules, StyleSheet, Tokenized};
    let status = signal(format!("{label}: pending"));
    let r = |v: f32| Some(Tokenized::Literal(Length::Px(v)));
    let img = img
        .on_load(move |ev| status.set(format!("{label}: load {}x{}", ev.width, ev.height)))
        .on_error(move || status.set(format!("{label}: error")));
    let img = match size {
        Some((w, h)) => img.with_style(Rc::new(StyleSheet::r#static(StyleRules {
            width: r(w),
            height: r(h),
            ..Default::default()
        }))),
        None => img,
    };
    let row = Rc::new(StyleSheet::r#static(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: r(12.0),
        margin_bottom: r(8.0),
        ..Default::default()
    }));
    view(vec![
        img.into_element(),
        text(move || status.get()).test_id(status_id).into_element(),
    ])
    .with_style(row)
    .into_element()
}

/// Every `image()` source kind, plus a `Below` menu anchored to a trigger
/// pinned to the screen's bottom edge: it must flip ABOVE the trigger,
/// stay inside the viewport, and keep its bottom edge on the trigger when
/// "Shrink" drops rows (the Android `AnchoredPlacer` regressions).
pub(crate) fn media_page(nav: NavCell) -> Element {
    use runtime_vocabulary::glue::primitives::image::{image, image_asset};
    use runtime_vocabulary::glue::primitives::overlay::{anchored_overlay, AnchorTarget, ElementSide};
    use runtime_vocabulary::glue::{Color, Length, Ref, StyleRules, StyleSheet, Tokenized};

    let r = |v: f32| Some(Tokenized::Literal(Length::Px(v)));
    let back = move || {
        if let Some(h) = nav.borrow().as_ref() {
            h.pop();
        }
    };

    let menu_open = signal(false);
    let menu_rows = signal(6_i32);
    let trigger: Ref<runtime_shared::ButtonHandle> = Ref::new();
    let panel_style = Rc::new(StyleSheet::r#static(StyleRules {
        background: Some(Tokenized::Literal(Color("#fef3c7".into()))),
        padding_top: r(8.0),
        padding_bottom: r(8.0),
        padding_left: r(12.0),
        padding_right: r(12.0),
        width: r(220.0),
        ..Default::default()
    }));
    let menu = when(
        move || menu_open.get(),
        move || {
            let extra_rows = when(
                move || menu_rows.get() > 2,
                || {
                    view(vec![
                        text("Item 3").into_element(),
                        text("Item 4").into_element(),
                        text("Item 5").into_element(),
                        text("Item 6").into_element(),
                    ])
                    .into_element()
                },
                || view(vec![]).into_element(),
            );
            let panel = view(vec![
                text("Item 1").test_id("menu-item-1").into_element(),
                text("Item 2").into_element(),
                extra_rows,
                button("Shrink", move || menu_rows.set(2)).test_id("menu-shrink").into_element(),
            ])
            .with_style(panel_style.clone())
            .test_id("menu-panel")
            .into_element();
            anchored_overlay(AnchorTarget::from(trigger), vec![panel])
                .side(ElementSide::Below)
                .offset(4.0)
                .into_element()
        },
        || view(vec![]).into_element(),
    );

    // Spacer pushes the trigger to the bottom edge of the screen.
    let spacer = Rc::new(StyleSheet::r#static(StyleRules {
        flex_grow: Some(Tokenized::Literal(1.0)),
        ..Default::default()
    }));
    let page = Rc::new(StyleSheet::r#static(StyleRules {
        flex_grow: Some(Tokenized::Literal(1.0)),
        padding_top: r(12.0),
        padding_bottom: r(12.0),
        padding_left: r(12.0),
        padding_right: r(12.0),
        ..Default::default()
    }));

    let children: Vec<Element> = vec![
        text("Media").test_id("media-marker").into_element(),
        media_row("png", image(PNG_DATA_URI), "img-png-status", None),
        media_row("svg", image(SVG_DATA_URI), "img-svg-status", None),
        media_row("svg 5x", image(SVG_DATA_URI), "img-svg5-status", Some((160.0, 80.0))),
        media_row("asset", image_asset(badge()), "img-asset-status", Some((72.0, 72.0))),
        media_row("remote", image(REMOTE_PNG), "img-remote-status", Some((64.0, 64.0))),
        media_row("remote svg", image(REMOTE_SVG), "img-remote-svg-status", Some((64.0, 64.0))),
        media_row("broken", image(BROKEN_URL), "img-broken-status", Some((32.0, 32.0))),
        media_row("bad data", image("data:image/png;base64,@@@@"), "img-bad-status", Some((32.0, 32.0))),
        view(vec![]).with_style(spacer).into_element(),
        text(move || format!("menu rows: {}", if menu_open.get() { menu_rows.get() } else { 0 }))
            .test_id("menu-state")
            .into_element(),
        button("Menu", move || menu_open.update(|o| !o))
            .bind(trigger)
            .test_id("menu-trigger")
            .into_element(),
        menu,
        button("Back", back).test_id("media-back").into_element(),
    ];
    view(children).with_style(page).into_element()
}
