//! Components — tour of the idea-ui library on a live page.

use std::rc::Rc;

use runtime_core::{signal, ui, Primitive};
use idea_ui::{
    alert, badge, btn, card, divider, field, stack, switch, tag, typography, BadgeKind,
    ButtonKind, IntentTag, StackAxis, StackGap, TypographyKind, TypographyTone,
};

use crate::pages::common::{page_header, page_section};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Primitive {
    const INTENTS: &str = "intents";
    const KINDS: &str = "button-kinds";
    const FEEDBACK: &str = "feedback";
    const INPUTS: &str = "inputs";
    const TYPOGRAPHY: &str = "typography";
    const FOOTER: &str = "theres-more";

    let toc = vec![
        TocEntry { id: INTENTS, label: "Intents" },
        TocEntry { id: KINDS, label: "Button kinds" },
        TocEntry { id: FEEDBACK, label: "Feedback" },
        TocEntry { id: INPUTS, label: "Inputs" },
        TocEntry { id: TYPOGRAPHY, label: "Typography" },
        TocEntry { id: FOOTER, label: "There's more" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Components",
                "A tour of the idea-ui library \u{2014} the cross-platform component \
                 kit shipped alongside the framework. Every sample below renders the \
                 same idea-ui primitive your app would use, on the same backend."
            ) }
            { page_section(INTENTS, vec![intents()]) }
            { page_section(KINDS, vec![button_kinds()]) }
            { page_section(FEEDBACK, vec![feedback()]) }
            { page_section(INPUTS, vec![inputs()]) }
            { page_section(TYPOGRAPHY, vec![typography_demo()]) }
            { page_section(FOOTER, vec![footer()]) }
        }
    };
    layout_with_toc(content, toc)
}

fn intents() -> Primitive {
    let intent_list = [
        IntentTag::Primary,
        IntentTag::Secondary,
        IntentTag::Neutral,
        IntentTag::Success,
        IntentTag::Danger,
        IntentTag::Warning,
        IntentTag::Info,
    ];
    let mut rows: Vec<Primitive> = Vec::with_capacity(intent_list.len());
    for intent in intent_list {
        let label = format!("{:?}", intent);
        let noop: Rc<dyn Fn()> = Rc::new(|| {});
        let row: Vec<Primitive> = vec![
            ui! { Btn(label = label.clone(), on_click = noop.clone(), intent = intent, kind = ButtonKind::Solid) },
            ui! { Badge(label = label.clone(), intent = intent, kind = BadgeKind::Soft) },
            ui! { Tag(label = label, intent = intent, kind = BadgeKind::Outlined) },
        ];
        rows.push(ui! { Stack(gap = StackGap::Md, axis = StackAxis::Row) { row } });
    }
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Intents".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Every themed component takes an `IntentTag` \u{2014} a \
                shared vocabulary of seven semantic actions (Primary, Secondary, Neutral, \
                Success, Danger, Warning, Info). One row per intent: a Button (Solid), a \
                Badge (Soft), and a Tag (Outlined). The kind axis chooses the visual; \
                the intent chooses the palette.".to_string(),
                tone = TypographyTone::Muted)
        },
        ui! { Card { rows } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn button_kinds() -> Primitive {
    let kinds = [ButtonKind::Solid, ButtonKind::Soft, ButtonKind::Outlined, ButtonKind::Ghost];
    let mut buttons: Vec<Primitive> = Vec::with_capacity(kinds.len());
    for kind in kinds {
        let label = format!("{:?}", kind);
        let noop: Rc<dyn Fn()> = Rc::new(|| {});
        buttons.push(ui! { Btn(label = label, on_click = noop, intent = IntentTag::Primary, kind = kind) });
    }
    let card_children: Vec<Primitive> = vec![
        ui! { Stack(gap = StackGap::Md, axis = StackAxis::Row) { buttons } },
    ];
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Button kinds".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "All four visual treatments for the same intent. Solid \
                is the filled call-to-action; Soft is a tinted background; Outlined uses \
                the intent color for the border and text; Ghost is text-only.".to_string(),
                tone = TypographyTone::Muted)
        },
        ui! { Card { card_children } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn feedback() -> Primitive {
    let alerts: Vec<Primitive> = vec![
        ui! { Alert(title = "Heads up".to_string(), body = Some("This is the Info intent.".to_string()), intent = IntentTag::Info) },
        ui! { Alert(title = "All set".to_string(), body = Some("Your changes have been saved.".to_string()), intent = IntentTag::Success) },
        ui! { Alert(title = "Careful".to_string(), body = Some("This action can't be undone.".to_string()), intent = IntentTag::Warning) },
        ui! { Alert(title = "Something went wrong".to_string(), body = Some("Couldn't reach the server.".to_string()), intent = IntentTag::Danger) },
    ];
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Feedback".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Alerts use the same intent vocabulary as buttons \u{2014} \
                Info / Success / Warning / Danger drive the surface color and the matching \
                icon.".to_string(),
                tone = TypographyTone::Muted)
        },
        ui! { Stack(gap = StackGap::Sm) { alerts } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn inputs() -> Primitive {
    let value = signal!("hello".to_string());
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |s| value.set(s));
    let switch_value = signal!(false);
    let on_toggle: Rc<dyn Fn(bool)> = Rc::new(move |b| switch_value.set(b));

    let card_children: Vec<Primitive> = vec![
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
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Inputs".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "All controlled. `Field` and `Switch` take a `Signal<T>` \
                value plus an `on_change` callback \u{2014} the host owns the source of \
                truth.".to_string(),
                tone = TypographyTone::Muted)
        },
        ui! { Card { card_children } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn typography_demo() -> Primitive {
    let samples: Vec<Primitive> = vec![
        ui! { Typography(content = "Display".to_string(), kind = TypographyKind::Display) },
        ui! { Typography(content = "Heading 1".to_string(), kind = TypographyKind::H1) },
        ui! { Typography(content = "Heading 2".to_string(), kind = TypographyKind::H2) },
        ui! { Typography(content = "Heading 3".to_string(), kind = TypographyKind::H3) },
        ui! { Typography(content = "Body extra-large \u{2014} for hero subheads.".to_string(), kind = TypographyKind::BodyXl) },
        ui! { Typography(content = "Body large.".to_string(), kind = TypographyKind::BodyLg) },
        ui! { Typography(content = "Body \u{2014} the default for paragraphs.".to_string()) },
        ui! { Typography(content = "Body small.".to_string(), kind = TypographyKind::BodySm) },
        ui! { Typography(content = "Caption for helper rows".to_string(), kind = TypographyKind::Caption) },
        ui! { Typography(content = "overline section label".to_string(), kind = TypographyKind::Overline) },
    ];
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Typography".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Ten variants on the same Typography component. The \
                size scale is theme-tokenized so apps can retune without touching \
                stylesheets.".to_string(),
                tone = TypographyTone::Muted)
        },
        ui! { Card { samples } },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn footer() -> Primitive {
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "There's more".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Stack, Card, Divider, Center, Spacer, Modal, Popover, \
                Select, Tabs, Avatar, Skeleton, Spinner, IconButton \u{2014} the full \
                catalog (including live control panels for each) lives at \
                `examples/idea-ui-docs` in the framework repo.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}
