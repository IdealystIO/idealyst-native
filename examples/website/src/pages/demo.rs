//! Demo — the single live showcase. Everything on this page is real:
//! reactive signals, the animation system, and the idea-ui component
//! kit, all rendering on the same backend an app would ship with.
//!
//! Each section is anchored to a `Ref<ViewHandle>` shared with the
//! "On this page" table of contents, so the long page stays
//! navigable. The live `AnimatedValue`s bind to `Ref<ViewHandle>`
//! stages whose lifetimes are owned by this page's reactive scope —
//! navigating away drops the scope and unsubscribes the bindings.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use runtime_core::animation::{AnimProp, AnimatedValue, SpringTo, TweenTo};
use runtime_core::{node_ref, signal, text_fmt, ui, Element, Ref, ViewHandle};
use idea_ui::{
    Alert, Badge, Btn, Card, Divider, Field, Stack, Switch, Tag, Typography, StackAxis, StackGap,
};

use crate::pages::common::{DemoShowcase, PageHeader, PageSection};
use crate::shell::{layout_with_toc, TocEntry};
use crate::styles::{DemoStage, DemoStageRow};

pub fn page() -> Element {
    // Reactivity
    let counter_ref: Ref<ViewHandle> = Ref::new();
    // Animation
    let fade_ref: Ref<ViewHandle> = Ref::new();
    let spring_ref: Ref<ViewHandle> = Ref::new();
    let entrance_ref: Ref<ViewHandle> = Ref::new();
    let color_ref: Ref<ViewHandle> = Ref::new();
    // Components
    let intents_ref: Ref<ViewHandle> = Ref::new();
    let kinds_ref: Ref<ViewHandle> = Ref::new();
    let feedback_ref: Ref<ViewHandle> = Ref::new();
    let inputs_ref: Ref<ViewHandle> = Ref::new();
    let typography_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: counter_ref, label: "Reactive state" },
        TocEntry { handle: fade_ref, label: "Fade toggle" },
        TocEntry { handle: spring_ref, label: "Spring vs tween" },
        TocEntry { handle: entrance_ref, label: "Multi-property entrance" },
        TocEntry { handle: color_ref, label: "Color tween" },
        TocEntry { handle: intents_ref, label: "Intents" },
        TocEntry { handle: kinds_ref, label: "Button kinds" },
        TocEntry { handle: feedback_ref, label: "Feedback" },
        TocEntry { handle: inputs_ref, label: "Inputs" },
        TocEntry { handle: typography_ref, label: "Typography" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Demo",
                blurb = "Everything on this page is live \u{2014} the same framework, the same \
                 components, the same animation system your app ships with, rendered by \
                 this backend right now. Click around; use \u{201c}On this page\u{201d} \
                 to jump.",
            )
            PageSection(handle = counter_ref) { reactive_state() }
            // PageSection(handle = fade_ref) { fade_demo() }
            // PageSection(handle = spring_ref) { spring_vs_tween_demo() }
            // PageSection(handle = entrance_ref) { entrance_demo() }
            // PageSection(handle = color_ref) { color_demo() }
            PageSection(handle = intents_ref) { intents() }
            PageSection(handle = kinds_ref) { button_kinds() }
            PageSection(handle = feedback_ref) { feedback() }
            PageSection(handle = inputs_ref) { inputs() }
            PageSection(handle = typography_ref) { typography_demo() }
        }
    };
    layout_with_toc(content, toc)
}

// =============================================================================
// Reactivity — the canonical counter, beside its source
// =============================================================================

fn reactive_state() -> Element {
    let count = signal!(0);
    // `Signal<T>` is `Copy`, so each closure owns its own handle to the
    // same reactive cell \u{2014} no shadowing dance, no `.clone()`.
    let increment: Rc<dyn Fn()> = Rc::new(move || count.update(|n| *n += 1));
    let decrement: Rc<dyn Fn()> = Rc::new(move || count.update(|n| *n -= 1));
    let reset: Rc<dyn Fn()> = Rc::new(move || count.set(0));

    let buttons: Vec<Element> = vec![
        ui! { Btn(label = "\u{2212}".to_string(), on_click = decrement, tone = idea_ui::tone::Neutral, variant = idea_ui::variant::Soft) },
        ui! { Btn(label = "Reset".to_string(), on_click = reset, tone = idea_ui::tone::Neutral, variant = idea_ui::variant::Ghost) },
        ui! { Btn(label = "+".to_string(), on_click = increment, tone = idea_ui::tone::Primary, variant = idea_ui::variant::Filled) },
    ];
    // `text_fmt!` builds a reactive `TextSource`; `bind!(signal)` marks
    // the arg the framework subscribes to. The text node re-resolves on
    // every write \u{2014} no surrounding tree rebuild.
    let preview: Vec<Element> = vec![
        ui! { Text { text_fmt!("Count: {}", bind!(count)) } },
        ui! { Stack(gap = StackGap::Md, axis = StackAxis::Row) { buttons } },
    ];

    let snippet = "let count = signal!(0);\n\
                   ui! {\n    \
                       Text { text_fmt!(\"Count: {}\", bind!(count)) }\n    \
                       Button(label = \"+\", on_click = move || count.update(|n| *n += 1))\n\
                   }";

    let children: Vec<Element> = vec![
        ui! { Typography(content = "Reactive state".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "The canonical reactive counter \u{2014} no virtual DOM, no \
                re-render pass. Clicking mutates exactly the text node bound to the count \
                signal.".to_string(),
                muted = true)
        },
        ui! { DemoShowcase(source = snippet) { preview } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

// =============================================================================
// Animation — live AnimatedValues bound to real view stages
// =============================================================================

fn fade_demo() -> Element {
    let opacity = AnimatedValue::new(1.0_f32);
    let box_ref: Ref<ViewHandle> = node_ref!(ViewHandle);
    opacity.bind(box_ref, AnimProp::Opacity);

    // Drive each click by reading the current value: ~1.0 \u{2192} 0.0, ~0.0 \u{2192} 1.0.
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
                   opacity.bind(box_ref, AnimProp::Opacity);\n\
                   \n\
                   // On click:\n\
                   opacity.animate(\n    \
                       TweenTo::new(target, Duration::from_millis(300)).ease_out()\n\
                   );";

    let preview: Vec<Element> = vec![
        ui! { View(style = row_style) { stage_children } },
        ui! {
            Stack(gap = StackGap::Md, axis = StackAxis::Row) {
                Btn(label = "Toggle".to_string(), on_click = on_toggle, tone = idea_ui::tone::Primary, variant = idea_ui::variant::Filled)
            }
        },
    ];

    let children: Vec<Element> = vec![
        ui! { Typography(content = "Fade toggle".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Tween a scalar between 0.0 and 1.0. The framework writes \
                the value to `AnimProp::Opacity` on the bound view every frame.".to_string(),
                muted = true)
        },
        ui! { DemoShowcase(source = snippet) { preview } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

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

    let preview: Vec<Element> = vec![
        ui! { Stack(gap = StackGap::Sm) { tween_row } },
        ui! { Stack(gap = StackGap::Sm) { spring_row } },
        ui! {
            Stack(gap = StackGap::Md, axis = StackAxis::Row) {
                Btn(label = "Move".to_string(), on_click = on_move, tone = idea_ui::tone::Primary, variant = idea_ui::variant::Filled)
            }
        },
    ];

    let children: Vec<Element> = vec![
        ui! { Typography(content = "Spring vs tween".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Click \"Move\" repeatedly. The tween always takes exactly \
                600 ms with its ease-in-out curve, even if you click again mid-flight. The \
                spring blends velocity into the new target, so rapid clicks produce smooth \
                handoffs.".to_string(),
                muted = true)
        },
        ui! { DemoShowcase(source = snippet) { preview } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

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
    let preview: Vec<Element> = vec![
        ui! { View(style = row_style) { View(style = stage_style) {}.bind(stage_ref) } },
        ui! { Stack(gap = StackGap::Md, axis = StackAxis::Row) { buttons } },
    ];

    let children: Vec<Element> = vec![
        ui! { Typography(content = "Multi-property entrance".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Opacity tweens while scale and translate-y spring in \
                parallel. Three animated values, three properties on one view, one click \
                to choreograph the lot.".to_string(),
                muted = true)
        },
        ui! { DemoShowcase(source = snippet) { preview } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

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
    let cursor: Rc<Cell<usize>> = Rc::new(Cell::new(0));
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

    let preview: Vec<Element> = vec![
        ui! { View(style = row_style) { View(style = stage_style) {}.bind(stage_ref) } },
        ui! {
            Stack(gap = StackGap::Md, axis = StackAxis::Row) {
                Btn(label = "Next color".to_string(), on_click = on_next, tone = idea_ui::tone::Primary, variant = idea_ui::variant::Filled)
            }
        },
    ];

    let children: Vec<Element> = vec![
        ui! { Typography(content = "Color tween".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Animated values handle color too. `bind_color` writes an \
                `(r, g, b, a)` 4-tuple to the bound view's background each frame; tweens \
                interpolate channel-wise.".to_string(),
                muted = true)
        },
        ui! { DemoShowcase(source = snippet) { preview } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

// =============================================================================
// Components — a tour of the idea-ui kit on a live page
// =============================================================================

fn intents() -> Element {
    let intent_list: Vec<(&str, fn() -> idea_ui::ToneRef)> = vec![
        ("Primary", || idea_ui::tone::Primary.into()),
        ("Secondary", || idea_ui::tone::Secondary.into()),
        ("Neutral", || idea_ui::tone::Neutral.into()),
        ("Success", || idea_ui::tone::Success.into()),
        ("Danger", || idea_ui::tone::Danger.into()),
        ("Warning", || idea_ui::tone::Warning.into()),
        ("Info", || idea_ui::tone::Info.into()),
    ];
    let mut rows: Vec<Element> = Vec::with_capacity(intent_list.len());
    for (name, make_tone) in intent_list {
        let label = name.to_string();
        let noop: Rc<dyn Fn()> = Rc::new(|| {});
        let row: Vec<Element> = vec![
            ui! { Btn(label = label.clone(), on_click = noop.clone(), tone = make_tone(), variant = idea_ui::variant::Filled) },
            ui! { Badge(label = label.clone(), tone = make_tone(), variant = idea_ui::variant::Soft) },
            ui! { Tag(label = label, tone = make_tone(), variant = idea_ui::variant::Outlined) },
        ];
        rows.push(ui! { Stack(gap = StackGap::Md, axis = StackAxis::Row) { row } });
    }
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Intents".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Every themed component takes a `tone` handle \u{2014} a \
                shared vocabulary of seven semantic actions. One row per intent: a Button \
                (Solid), a Badge (Soft), and a Tag (Outlined). The variant axis chooses the \
                visual; the tone chooses the palette.".to_string(),
                muted = true)
        },
        ui! { Card { rows } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn button_kinds() -> Element {
    let kinds: Vec<(&str, fn() -> idea_ui::VariantRef)> = vec![
        ("Solid", || idea_ui::variant::Filled.into()),
        ("Soft", || idea_ui::variant::Soft.into()),
        ("Outlined", || idea_ui::variant::Outlined.into()),
        ("Ghost", || idea_ui::variant::Ghost.into()),
    ];
    let mut buttons: Vec<Element> = Vec::with_capacity(kinds.len());
    for (name, make_variant) in kinds {
        let label = name.to_string();
        let noop: Rc<dyn Fn()> = Rc::new(|| {});
        buttons.push(ui! { Btn(label = label, on_click = noop, tone = idea_ui::tone::Primary, variant = make_variant()) });
    }
    let card_children: Vec<Element> = vec![
        ui! { Stack(gap = StackGap::Md, axis = StackAxis::Row) { buttons } },
    ];
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Button kinds".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "All four visual treatments for the same intent. Solid is \
                the filled call-to-action; Soft is a tinted background; Outlined uses the \
                intent color for the border and text; Ghost is text-only.".to_string(),
                muted = true)
        },
        ui! { Card { card_children } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn feedback() -> Element {
    let alerts: Vec<Element> = vec![
        ui! { Alert(title = "Heads up".to_string(), body = Some("This is the Info intent.".to_string()), tone = idea_ui::tone::Info) },
        ui! { Alert(title = "All set".to_string(), body = Some("Your changes have been saved.".to_string()), tone = idea_ui::tone::Success) },
        ui! { Alert(title = "Careful".to_string(), body = Some("This action can't be undone.".to_string()), tone = idea_ui::tone::Warning) },
        ui! { Alert(title = "Something went wrong".to_string(), body = Some("Couldn't reach the server.".to_string()), tone = idea_ui::tone::Danger) },
    ];
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Feedback".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Alerts use the same intent vocabulary as buttons \u{2014} \
                Info / Success / Warning / Danger drive the surface color and the matching \
                icon.".to_string(),
                muted = true)
        },
        ui! { Stack(gap = StackGap::Sm) { alerts } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn inputs() -> Element {
    let value = signal!("hello".to_string());
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |s| value.set(s));
    let switch_value = signal!(false);
    let on_toggle: Rc<dyn Fn(bool)> = Rc::new(move |b| switch_value.set(b));

    let card_children: Vec<Element> = vec![
        ui! {
            Field(
                label = Some("Name".to_string()),
                value = value,
                on_change = on_change,
                placeholder = Some("Your name".to_string()),
                help = Some("This shows up on your profile.".to_string()),
            )
        },
        ui! { Divider() },
        ui! {
            Switch(
                label = Some("Send me updates".to_string()),
                value = switch_value,
                on_change = on_toggle,
            )
        },
    ];
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Inputs".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "All controlled. `Field` and `Switch` take a `Signal<T>` \
                value plus an `on_change` callback \u{2014} the host owns the source of \
                truth.".to_string(),
                muted = true)
        },
        ui! { Card { card_children } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn typography_demo() -> Element {
    let samples: Vec<Element> = vec![
        ui! { Typography(content = "Display".to_string(), kind = idea_ui::typography_kind::Display) },
        ui! { Typography(content = "Heading 1".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! { Typography(content = "Heading 2".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! { Typography(content = "Heading 3".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! { Typography(content = "Body extra-large \u{2014} for hero subheads.".to_string(), kind = idea_ui::typography_kind::BodyXl) },
        ui! { Typography(content = "Body large.".to_string(), kind = idea_ui::typography_kind::BodyLg) },
        ui! { Typography(content = "Body \u{2014} the default for paragraphs.".to_string()) },
        ui! { Typography(content = "Body small.".to_string(), kind = idea_ui::typography_kind::BodySm) },
        ui! { Typography(content = "Caption for helper rows".to_string(), kind = idea_ui::typography_kind::Caption) },
        ui! { Typography(content = "overline section label".to_string(), kind = idea_ui::typography_kind::Overline) },
    ];
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Typography".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Ten variants on the same Typography component. The size \
                scale is theme-tokenized so apps can retune without touching \
                stylesheets.".to_string(),
                muted = true)
        },
        ui! { Card { samples } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}
