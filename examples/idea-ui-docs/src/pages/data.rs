//! Data — Card, Table (body-only pages).

use std::rc::Rc;

use runtime_core::{ui, Element};
use idea_ui::{
    tone, typography_kind, variant, Button, Card, Stack, StackAxis, StackGap, Table, TableCell,
    TableRow, Tag, Typography,
};

use crate::shell::{Callout, CodePanel, DemoSurface, Prop, PropsTable, Section, P};
use idea_ui::components::card::variant as card_variant;

// =============================================================================
// Card
// =============================================================================

pub fn card() -> Element {
    crate::pages::body(vec![
        ui! {
            Section(title = "Composition".to_string()) {
                P(content = "A Card is a themed surface with rounded corners, a hairline border, \
                    and one of two background tokens. Compose the inner anatomy yourself — header, \
                    body, and footer are just Typography and actions inside the card.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, gap = StackGap::Lg) {
                        media_card()
                        stat_card()
                    }
                }
            }
        },
        ui! {
            Section(title = "Anatomy".to_string()) {
                P(content = "The variant determines the background — Flat reads `color-surface`, \
                    Elevated reads `color-surface-alt` and adds a drop shadow so the card reads as \
                    a layer above the page even on platforms that don't render shadows.".to_string())
                CodePanel(src = r##"Card(variant = card::variant::Elevated, padding = CardPadding::Md) {
    Typography(content = "Heading".into(), kind = typography_kind::H3)
    Typography(content = "Body copy.".into())
    Button(label = "Action".into(), on_click = act, tone = tone::Primary, variant = variant::Filled)
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "variant",
                        ty: "VariantRef",
                        desc: "card::variant::Flat (default, page surface) or card::variant::Elevated (raised surface + shadow).",
                    },
                    Prop {
                        name: "padding",
                        ty: "CardPadding",
                        desc: "None / Sm / Md (default) / Lg — mapped to spacing tokens.",
                    },
                    Prop {
                        name: "tone",
                        ty: "Option<ToneRef>",
                        desc: "Optional intent tint. When Some, paints a muted tone-tinted background + border (the Soft treatment) for support/info panels. Default None.",
                    },
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "Card flattens fragment children via ChildList::append_to.",
                    },
                ])
            }
        },
        ui! {
            Callout(label = "Custom variants".to_string()) {
                P(content = "Card's variant trait is open. Implement Variant on a marker type and \
                    install an extended sheet via install_card_sheet before any Card mounts — then \
                    use Card(variant = Tinted.into()) anywhere.".to_string())
            }
        },
    ])
}

fn media_card() -> Element {
    let on_act: Rc<dyn Fn()> = Rc::new(|| {});
    let elevated: idea_ui::VariantRef = card_variant::Elevated.into();
    ui! {
        Card(variant = elevated) {
            Typography(content = "Release notes".to_string(), kind = typography_kind::Overline, muted = true)
            Typography(content = "Spring update".to_string(), kind = typography_kind::H3)
            Typography(content = "New layout primitives, a themed data table, and faster reactive re-renders across every backend.".to_string())
            Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                Button(
                    label = "Read more".to_string(),
                    on_click = on_act.clone(),
                    tone = tone::Primary,
                    variant = variant::Filled,
                )
                Button(
                    label = "Dismiss".to_string(),
                    on_click = on_act,
                    tone = tone::Neutral,
                    variant = variant::Soft,
                )
            }
        }
    }
}

fn stat_card() -> Element {
    ui! {
        Card {
            Typography(content = "Active sessions".to_string(), kind = typography_kind::Overline, muted = true)
            Typography(content = "1,284".to_string(), kind = typography_kind::H1)
            Tag(label = "+12% this week".to_string(), tone = tone::Success, variant = variant::Soft)
        }
    }
}

// =============================================================================
// Table
// =============================================================================

pub fn table() -> Element {
    crate::pages::body(vec![
        ui! {
            Section(title = "Status table".to_string()) {
                P(content = "On web, Table lowers to a real HTML `<table>` via the `table` SDK, so \
                    the browser's table-layout algorithm sizes columns to widest content for free. \
                    On native, the same shape falls through to a styled flex passthrough.".to_string())
                status_table()
            }
        },
        ui! {
            Section(title = "Rich cells with children".to_string()) {
                P(content = "Pass `children` instead of `text` to compose richer cell contents — \
                    Tags, Buttons, Typography, anything. The cell-level padding/border still \
                    applies; the default inner text styling is bypassed.".to_string())
                CodePanel(src = r##"TableCell {
    Tag(label = "Passing".into(), tone = tone::Success, variant = variant::Soft)
}
TableCell {
    Button(label = "Re-run".into(), on_click = run, tone = tone::Primary, variant = variant::Soft)
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Table props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "TableRows. Flattened via ChildList::append_to so `for` loops splat cleanly.",
                    },
                ])
            }
        },
        ui! {
            Section(title = "TableCell props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "header",
                        ty: "bool",
                        desc: "When true, renders as `<th>` with the head-cell surface + uppercase muted text style. Default false.",
                    },
                    Prop {
                        name: "text",
                        ty: "Reactive<Option<String>>",
                        desc: "Shorthand cell content — wraps the value in a themed text node using head/body typography. Ignored when `children` is non-empty.",
                    },
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "Custom cell contents (links, badges, multiple inline pieces). Replaces the default text rendering when provided.",
                    },
                ])
            }
        },
    ])
}

fn status_table() -> Element {
    let on_run: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Table {
            TableRow {
                TableCell(header = true, text = Some("Job".to_string()))
                TableCell(header = true, text = Some("Status".to_string()))
                TableCell(header = true, text = Some("Action".to_string()))
            }
            TableRow {
                TableCell(text = Some("Build".to_string()))
                TableCell {
                    Tag(label = "Passing".to_string(), tone = tone::Success, variant = variant::Soft)
                }
                TableCell {
                    Button(
                        label = "Re-run".to_string(),
                        on_click = on_run.clone(),
                        tone = tone::Primary,
                        variant = variant::Soft,
                    )
                }
            }
            TableRow {
                TableCell(text = Some("Unit tests".to_string()))
                TableCell {
                    Tag(label = "Passing".to_string(), tone = tone::Success, variant = variant::Soft)
                }
                TableCell {
                    Button(
                        label = "Re-run".to_string(),
                        on_click = on_run.clone(),
                        tone = tone::Primary,
                        variant = variant::Soft,
                    )
                }
            }
            TableRow {
                TableCell(text = Some("Deploy".to_string()))
                TableCell {
                    Tag(label = "Blocked".to_string(), tone = tone::Danger, variant = variant::Soft)
                }
                TableCell {
                    Button(
                        label = "Investigate".to_string(),
                        on_click = on_run,
                        tone = tone::Danger,
                        variant = variant::Soft,
                    )
                }
            }
        }
    }
}
