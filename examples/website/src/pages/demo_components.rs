//! Components — tour of the idea-ui library on a live page.

use std::rc::Rc;

use runtime_core::{signal, ui, Element, Ref, ViewHandle};
use idea_ui::{
    Alert, Badge, Btn, Card, Divider, Field, Stack, Switch, Tag, Typography, StackAxis, StackGap,
};

use crate::pages::common::{page_header, page_section};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let intents_ref: Ref<ViewHandle> = Ref::new();
    let kinds_ref: Ref<ViewHandle> = Ref::new();
    let feedback_ref: Ref<ViewHandle> = Ref::new();
    let inputs_ref: Ref<ViewHandle> = Ref::new();
    let typography_ref: Ref<ViewHandle> = Ref::new();
    let footer_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: intents_ref, label: "Intents" },
        TocEntry { handle: kinds_ref, label: "Button kinds" },
        TocEntry { handle: feedback_ref, label: "Feedback" },
        TocEntry { handle: inputs_ref, label: "Inputs" },
        TocEntry { handle: typography_ref, label: "Typography" },
        TocEntry { handle: footer_ref, label: "There's more" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Components",
                "A tour of the idea-ui library \u{2014} the cross-platform component \
                 kit shipped alongside the framework. Every sample below renders the \
                 same idea-ui primitive your app would use, on the same backend."
            ) }
            { page_section(intents_ref, vec![intents()]) }
            { page_section(kinds_ref, vec![button_kinds()]) }
            { page_section(feedback_ref, vec![feedback()]) }
            { page_section(inputs_ref, vec![inputs()]) }
            { page_section(typography_ref, vec![typography_demo()]) }
            { page_section(footer_ref, vec![footer()]) }
        }
    };
    layout_with_toc(content, toc)
}

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
                shared vocabulary of seven semantic actions (Primary, Secondary, Neutral, \
                Success, Danger, Warning, Info). One row per intent: a Button (Solid), a \
                Badge (Soft), and a Tag (Outlined). The variant axis chooses the visual; \
                the tone chooses the palette.".to_string(),
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
            Typography(content = "All four visual treatments for the same intent. Solid \
                is the filled call-to-action; Soft is a tinted background; Outlined uses \
                the intent color for the border and text; Ghost is text-only.".to_string(),
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
            Typography(content = "Ten variants on the same Typography component. The \
                size scale is theme-tokenized so apps can retune without touching \
                stylesheets.".to_string(),
                muted = true)
        },
        ui! { Card { samples } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn footer() -> Element {
    let children: Vec<Element> = vec![
        ui! { Typography(content = "There's more".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Stack, Card, Divider, Center, Spacer, Modal, Popover, \
                Select, Tabs, Avatar, Skeleton, Spinner, IconButton \u{2014} the full \
                catalog (including live control panels for each) lives at \
                `examples/idea-ui-docs` in the framework repo.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}
