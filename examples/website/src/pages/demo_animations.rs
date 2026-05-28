//! Animations — explainer + four live demos exercising the animation
//! system. Each demo creates its own `AnimatedValue` instances, binds
//! them to a stage View via `Ref<ViewHandle>`, and triggers
//! `SpringTo` / `TweenTo` from button clicks. The reactive scope of
//! the page hosts the binding lifetimes — when the user navigates
//! away the scope drops and the bindings unsubscribe.

use std::rc::Rc;
use std::time::Duration;

use runtime_core::animation::{AnimProp, AnimatedValue, SpringTo, TweenTo};
use runtime_core::{node_ref, ui, Element, Ref, ViewHandle};
use idea_ui::{Btn, Card, Stack, Typography, StackAxis, StackGap};

use crate::pages::common::{code_panel, page_header, page_section};
use crate::shell::{layout_with_toc, TocEntry};
use crate::styles::{DemoStage, DemoStageRow};

pub fn page() -> Element {
    let model_ref: Ref<ViewHandle> = Ref::new();
    let fade_ref: Ref<ViewHandle> = Ref::new();
    let spring_tween_ref: Ref<ViewHandle> = Ref::new();
    let entrance_ref: Ref<ViewHandle> = Ref::new();
    let color_ref: Ref<ViewHandle> = Ref::new();
    let when_ref: Ref<ViewHandle> = Ref::new();
    let welcome_ref: Ref<ViewHandle> = Ref::new();
    let perf_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: model_ref, label: "The model" },
        TocEntry { handle: fade_ref, label: "Fade toggle" },
        TocEntry { handle: spring_tween_ref, label: "Spring vs tween" },
        TocEntry { handle: entrance_ref, label: "Multi-property entrance" },
        TocEntry { handle: color_ref, label: "Color tween" },
        TocEntry { handle: when_ref, label: "When to pick which" },
        TocEntry { handle: welcome_ref, label: "The welcome scene" },
        TocEntry { handle: perf_ref, label: "What you don't pay for" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Animations",
                "Springs, tweens, and per-frame writes that dispatch native motion \
                 primitives on every backend. The four demos below are real \
                 `AnimatedValue`s bound to real `Ref<ViewHandle>`s \u{2014} click \
                 the buttons and watch them move."
            ) }
            { page_section(model_ref, vec![model()]) }
            { page_section(fade_ref, vec![fade_demo()]) }
            { page_section(spring_tween_ref, vec![spring_vs_tween_demo()]) }
            { page_section(entrance_ref, vec![entrance_demo()]) }
            { page_section(color_ref, vec![color_demo()]) }
            { page_section(when_ref, vec![springs_vs_tweens_note()]) }
            { page_section(welcome_ref, vec![welcome_breakdown()]) }
            { page_section(perf_ref, vec![performance()]) }
        }
    };
    layout_with_toc(content, toc)
}

// =============================================================================
// Explainer (kept tight; the demos do the heavy lifting now)
// =============================================================================

fn model() -> Element {
    let children: Vec<Element> = vec![
        ui! { Typography(content = "The model".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "An `AnimatedValue<T>` is a reactive cell with a driver. \
                You write target values; the driver interpolates current toward target \
                each frame. Bind the value to a primitive's property (Opacity, \
                TranslateY, Scale, color stops, etc.) and the framework pushes per-frame \
                writes through the backend's animation API.".to_string())
        },
        ui! {
            Typography(content = "Two drivers ship today: `SpringTo` (physics-based, \
                interruptible) and `TweenTo` (fixed duration + easing curve). Both \
                produce the same kind of output \u{2014} a stream of T values \u{2014} \
                so binding doesn't care which is in use.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

// =============================================================================
// Demo 1 — Fade toggle
// =============================================================================

fn fade_demo() -> Element {
    let opacity = AnimatedValue::new(1.0_f32);
    let box_ref: Ref<ViewHandle> = node_ref!(ViewHandle);
    opacity.bind(box_ref, AnimProp::Opacity);

    // Drive each click by reading the current value: ~1.0 → 0.0, ~0.0 → 1.0.
    let opacity_for_click = opacity.clone();
    let on_toggle: Rc<dyn Fn()> = Rc::new(move || {
        let target = if opacity_for_click.get() > 0.5 { 0.0 } else { 1.0 };
        opacity_for_click.animate(
            TweenTo::new(target, Duration::from_millis(300)).ease_out(),
        );
    });

    let stage_style = DemoStage();
    let row_style = DemoStageRow();
    let stage_children: Vec<Element> = vec![
        ui! { View(style = stage_style) {} .bind(box_ref) },
    ];
    let snippet = "let opacity = AnimatedValue::new(1.0_f32);\n\
                   let box_ref: Ref<ViewHandle> = node_ref!(ViewHandle);\n\
                   opacity.bind(box_ref, AnimProp::Opacity);\n\
                   \n\
                   // On click:\n\
                   opacity.animate(\n    \
                       TweenTo::new(target, Duration::from_millis(300)).ease_out()\n\
                   );";

    let card_children: Vec<Element> = vec![
        ui! { Typography(content = "Fade toggle".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "Tween a scalar between 0.0 and 1.0. The framework writes \
                the value to `AnimProp::Opacity` on the bound view every frame.".to_string(),
                muted = true)
        },
        ui! { View(style = row_style) { stage_children } },
        ui! {
            Stack(gap = StackGap::Md, axis = StackAxis::Row) {
                Btn(label = "Toggle".to_string(), on_click = on_toggle, tone = idea_ui::tone::Primary, variant = idea_ui::variant::Filled)
            }
        },
        code_panel(snippet),
    ];
    ui! { Card { card_children } }
}

// =============================================================================
// Demo 2 — Spring vs tween (two boxes side by side)
// =============================================================================

fn spring_vs_tween_demo() -> Element {
    let tween_x = AnimatedValue::new(0.0_f32);
    let spring_x = AnimatedValue::new(0.0_f32);
    let tween_ref: Ref<ViewHandle> = node_ref!(ViewHandle);
    let spring_ref: Ref<ViewHandle> = node_ref!(ViewHandle);
    tween_x.bind(tween_ref, AnimProp::TranslateX);
    spring_x.bind(spring_ref, AnimProp::TranslateX);

    // Each click flips the target between 0 and 120 px.
    let tween_clone = tween_x.clone();
    let spring_clone = spring_x.clone();
    let on_move: Rc<dyn Fn()> = Rc::new(move || {
        let next_t = if tween_clone.get() > 60.0 { 0.0 } else { 120.0 };
        let next_s = if spring_clone.get() > 60.0 { 0.0 } else { 120.0 };
        tween_clone.animate(
            TweenTo::new(next_t, Duration::from_millis(600)).ease_in_out(),
        );
        spring_clone.animate(
            SpringTo::new(next_s).stiffness(140.0).damping(12.0),
        );
    });

    let stage_a = DemoStage();
    let stage_b = DemoStage();
    let row_style_a = DemoStageRow();
    let row_style_b = DemoStageRow();
    // Two stacked rows so the labels sit above the moving boxes.
    let tween_row: Vec<Element> = vec![
        ui! { Typography(content = "Tween".to_string(), kind = idea_ui::typography_kind::Overline) },
        ui! { View(style = row_style_a) { View(style = stage_a) {}.bind(tween_ref) } },
    ];
    let spring_row: Vec<Element> = vec![
        ui! { Typography(content = "Spring".to_string(), kind = idea_ui::typography_kind::Overline) },
        ui! { View(style = row_style_b) { View(style = stage_b) {}.bind(spring_ref) } },
    ];

    let snippet = "tween_x.animate(\n    \
                       TweenTo::new(120.0, Duration::from_millis(600)).ease_in_out()\n\
                   );\n\
                   spring_x.animate(\n    \
                       SpringTo::new(120.0).stiffness(140.0).damping(12.0)\n\
                   );";

    let card_children: Vec<Element> = vec![
        ui! { Typography(content = "Spring vs tween".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "Click \"Move\" repeatedly. The tween always takes \
                exactly 600 ms with its ease-in-out curve, even if you click again \
                mid-flight. The spring blends velocity into the new target, so rapid \
                clicks produce smooth handoffs.".to_string(),
                muted = true)
        },
        ui! { Stack(gap = StackGap::Sm) { tween_row } },
        ui! { Stack(gap = StackGap::Sm) { spring_row } },
        ui! {
            Stack(gap = StackGap::Md, axis = StackAxis::Row) {
                Btn(label = "Move".to_string(), on_click = on_move, tone = idea_ui::tone::Primary, variant = idea_ui::variant::Filled)
            }
        },
        code_panel(snippet),
    ];
    ui! { Card { card_children } }
}

// =============================================================================
// Demo 3 — Multi-property entrance (welcome Act 1 shape)
// =============================================================================

fn entrance_demo() -> Element {
    let opacity = AnimatedValue::new(0.0_f32);
    let scale = AnimatedValue::new(0.85_f32);
    let translate_y = AnimatedValue::new(24.0_f32);
    let stage_ref: Ref<ViewHandle> = node_ref!(ViewHandle);
    opacity.bind(stage_ref, AnimProp::Opacity);
    scale.bind(stage_ref, AnimProp::Scale);
    translate_y.bind(stage_ref, AnimProp::TranslateY);

    let opacity_in = opacity.clone();
    let scale_in = scale.clone();
    let y_in = translate_y.clone();
    let on_enter: Rc<dyn Fn()> = Rc::new(move || {
        opacity_in.animate(TweenTo::new(1.0, Duration::from_millis(700)).ease_out());
        scale_in.animate(SpringTo::new(1.0).stiffness(170.0).damping(22.0));
        y_in.animate(SpringTo::new(0.0).stiffness(170.0).damping(22.0));
    });

    let opacity_out = opacity.clone();
    let scale_out = scale.clone();
    let y_out = translate_y.clone();
    let on_reset: Rc<dyn Fn()> = Rc::new(move || {
        opacity_out.set(0.0);
        scale_out.set(0.85);
        y_out.set(24.0);
    });

    let stage_style = DemoStage();
    let row_style = DemoStageRow();
    let snippet = "// Three values, three properties, one click.\n\
                   opacity.animate(TweenTo::new(1.0, Duration::from_millis(700)).ease_out());\n\
                   scale.animate(SpringTo::new(1.0).stiffness(170.0).damping(22.0));\n\
                   translate_y.animate(SpringTo::new(0.0).stiffness(170.0).damping(22.0));";

    let buttons: Vec<Element> = vec![
        ui! { Btn(label = "Enter".to_string(), on_click = on_enter, tone = idea_ui::tone::Primary, variant = idea_ui::variant::Filled) },
        ui! { Btn(label = "Reset".to_string(), on_click = on_reset, tone = idea_ui::tone::Neutral, variant = idea_ui::variant::Ghost) },
    ];
    let card_children: Vec<Element> = vec![
        ui! { Typography(content = "Multi-property entrance".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "The welcome example's Act 1 in miniature: opacity \
                tweens while scale and translate-y spring in parallel. Three animated \
                values, three properties on one view, one click to choreograph the lot.".to_string(),
                muted = true)
        },
        ui! { View(style = row_style) { View(style = stage_style) {}.bind(stage_ref) } },
        ui! { Stack(gap = StackGap::Md, axis = StackAxis::Row) { buttons } },
        code_panel(snippet),
    ];
    ui! { Card { card_children } }
}

// =============================================================================
// Demo 4 — Color tween (background)
// =============================================================================

fn color_demo() -> Element {
    let palette: [(f32, f32, f32, f32); 4] = [
        (0.353, 0.310, 0.812, 1.0), // indigo
        (0.231, 0.647, 0.365, 1.0), // green
        (0.898, 0.282, 0.302, 1.0), // red
        (0.880, 0.659, 0.180, 1.0), // amber
    ];
    let color = AnimatedValue::new(palette[0]);
    let stage_ref: Ref<ViewHandle> = node_ref!(ViewHandle);
    color.bind_color(stage_ref, AnimProp::BackgroundColor);

    let color_for_click = color.clone();
    let cursor: Rc<std::cell::Cell<usize>> = Rc::new(std::cell::Cell::new(0));
    let on_next: Rc<dyn Fn()> = Rc::new(move || {
        let next = (cursor.get() + 1) % palette.len();
        cursor.set(next);
        color_for_click.animate(
            TweenTo::new(palette[next], Duration::from_millis(500)).ease_in_out(),
        );
    });

    let stage_style = DemoStage();
    let row_style = DemoStageRow();
    let snippet = "let color = AnimatedValue::new((0.353, 0.310, 0.812, 1.0));\n\
                   color.bind_color(stage_ref, AnimProp::BackgroundColor);\n\
                   \n\
                   color.animate(\n    \
                       TweenTo::new(next_rgba, Duration::from_millis(500)).ease_in_out()\n\
                   );";

    let card_children: Vec<Element> = vec![
        ui! { Typography(content = "Color tween".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "Animated values handle color too. `bind_color` writes \
                an `(r, g, b, a)` 4-tuple to the bound view's background each frame; \
                tweens interpolate channel-wise.".to_string(),
                muted = true)
        },
        ui! { View(style = row_style) { View(style = stage_style) {}.bind(stage_ref) } },
        ui! {
            Stack(gap = StackGap::Md, axis = StackAxis::Row) {
                Btn(label = "Next color".to_string(), on_click = on_next, tone = idea_ui::tone::Primary, variant = idea_ui::variant::Filled)
            }
        },
        code_panel(snippet),
    ];
    ui! { Card { card_children } }
}

// =============================================================================
// Explainer continued (springs vs tweens deep dive)
// =============================================================================

fn springs_vs_tweens_note() -> Element {
    let children: Vec<Element> = vec![
        ui! { Typography(content = "When to pick which".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Tweens are right for choreographed motion: act \
                transitions, page-in fades, anything where the duration is part of \
                the design. The driver picks the interpolation curve; you pick the \
                duration.".to_string())
        },
        ui! {
            Typography(content = "Springs are right for interruptible motion: \
                drag-to-snap, bounce-on-arrival, anything that responds to the user \
                mid-animation. If a new target arrives while the spring is in flight, \
                it blends velocity smoothly into the new path \u{2014} no visible \
                discontinuity, no hand-coded \"is currently animating\" state.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn welcome_breakdown() -> Element {
    let children: Vec<Element> = vec![
        ui! { Typography(content = "The welcome scene".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "`idealyst new my-app` scaffolds the welcome example: \
                a three-act cinematic intro built from the same API the demos above use.".to_string())
        },
        ui! { Typography(content = "Act 1".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "The phrase \"Welcome to Idealyst\" rises into a light \
                frame. Three animated values: opacity (tween, 700 ms ease-out), scale \
                (spring, slight bounce on settle), translate-y (spring, lifts to rest).".to_string())
        },
        ui! { Typography(content = "Act 2".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "The frame washes dark while the headline color tweens \
                to its inverse. A radial sun gradient blooms from the top-right, pulled \
                in by a separate animation tied to a corner-anchored gradient stop.".to_string())
        },
        ui! { Typography(content = "Act 3".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "The headline shuffles up to make room for the subtitle, \
                which fades in from below. A `raf_loop` then drives a steady-state pulse \
                across the sun, vignette corners, and orbiting planets \u{2014} one phase \
                clock shared by every breathing element.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn performance() -> Element {
    let children: Vec<Element> = vec![
        ui! { Typography(content = "What you don't pay for".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Animations don't go through the render walker every \
                frame. The walker mounts once; the animation system pushes per-frame \
                property writes directly to the bound primitive's native node via the \
                backend's `set_animated_*` hooks. No tree traversal, no virtual DOM \
                diff, no React-style \"re-render the whole scene at 60Hz\" pattern.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}
