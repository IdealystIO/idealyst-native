//! Layout — Stack, Card, Table, Divider, Center, Spacer (one page each).

use runtime_core::{ui, Element};
use idea_ui::doc_controls::DocControls;
use idea_ui::{
    tone, typography_kind, variant, Badge, Button, Card, CardProps, Center, Divider, DividerProps,
    Spacer, Stack, StackAxis, StackGap, StackProps, Table, TableCell, TableRow, Tag, Typography,
};

use crate::shell::{
    self, Callout, CodePanel, ComponentPage, Demo, DemoSurface, H2, P, Prop, PropsTable, Section,
};

// =============================================================================
// Stack
// =============================================================================

pub fn stack() -> Element {
    let state = StackProps::init_state();
    let preview = StackProps::reactive_preview(&state, |props| {
        let gap = props.gap;
        let axis = props.axis;
        let align = props.align;
        let justify = props.justify;
        let padding = props.padding;
        ui! {
            Stack(
                gap = gap,
                axis = axis,
                align = align,
                justify = justify,
                padding = padding,
                children = filler_children()
            )
        }
    });
    let controls = StackProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "Stack".to_string(),
            lead = "The workhorse flex container. Gap, axis, alignment, justification, and \
                padding are all variant axes — discrete and cacheable.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "When to use Stack vs `view`".to_string()) {
                P(content = "Use `view` when you need raw flex with custom styling. Use Stack \
                    when the layout follows the theme's spacing scale — Stack reads gap and \
                    padding through tokens, so all your layouts inherit the same rhythm \
                    automatically.".to_string())
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "gap",
                        ty: "StackGap",
                        desc: "Spacing between children. None / Xs / Sm / Md / Lg / Xl / Xxl, mapped to spacing-* tokens.",
                    },
                    Prop {
                        name: "padding",
                        ty: "StackPadding",
                        desc: "Inner padding. Same scale as `gap`. Defaults to None.",
                    },
                    Prop {
                        name: "axis",
                        ty: "StackAxis",
                        desc: "Column (default) or Row. Toolbars and button groups use Row.",
                    },
                    Prop {
                        name: "align",
                        ty: "StackAlign",
                        desc: "Cross-axis alignment: Stretch / Start / Center / End.",
                    },
                    Prop {
                        name: "justify",
                        ty: "StackJustify",
                        desc: "Main-axis alignment: Start / Center / End / SpaceBetween / SpaceAround / SpaceEvenly.",
                    },
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "Stack flattens fragment children via ChildList::append_to.",
                    },
                ])
            }

            Section(title = "Recipes".to_string()) {
                H2(content = "Vertical form layout".to_string())
                CodePanel(src = r##"Stack(gap = StackGap::Lg) {
    Field(label = Some("Email".into()), value = email, on_change = on_email)
    Field(label = Some("Password".into()), value = pwd,   on_change = on_pwd)
    Button(label = "Sign in".into(), on_click = submit, tone = tone::Primary, variant = variant::Filled)
}"##.to_string())

                H2(content = "Toolbar / button row".to_string())
                CodePanel(src = r##"Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
    Typography(content = "Title".into(), kind = typography_kind::H3)
    Spacer()
    Button(label = "Save".into(), on_click = save, tone = tone::Primary, variant = variant::Filled)
}"##.to_string())
            }
        }
    })
}

fn filler_children() -> Vec<Element> {
    vec![
        ui! { Badge(label = "one".to_string(),   tone = tone::Primary, variant = variant::Soft) },
        ui! { Badge(label = "two".to_string(),   tone = tone::Success, variant = variant::Soft) },
        ui! { Badge(label = "three".to_string(), tone = tone::Warning, variant = variant::Soft) },
    ]
}

// =============================================================================
// Card
// =============================================================================

pub fn card() -> Element {
    let state = CardProps::init_state();
    let preview = CardProps::reactive_preview(&state, |props| {
        let variant = props.variant;
        let padding = props.padding;
        ui! {
            Card(variant = variant, padding = padding) {
                Typography(content = "Card heading".to_string(), kind = typography_kind::H3)
                Typography(content = "Cards group related content. Variant picks the surface; \
                                  padding controls the inner spacing.".to_string())
            }
        }
    });
    let controls = CardProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "Card".to_string(),
            lead = "Surface container for grouping related content. Two built-in variants: \
                Flat (page surface) and Elevated (raised surface with drop shadow).".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Anatomy".to_string()) {
                P(content = "A Card is a themed `view` with rounded corners, hairline border, \
                    and one of two background tokens. The variant determines which background — \
                    Flat reads `color-surface`, Elevated reads `color-surface-alt` and adds a \
                    drop shadow.".to_string())
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "variant",
                        ty: "VariantRef",
                        desc: "card::variant::Flat (default) or card::variant::Elevated.",
                    },
                    Prop {
                        name: "padding",
                        ty: "CardPadding",
                        desc: "None / Sm / Md (default) / Lg — mapped to spacing tokens.",
                    },
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "Card flattens fragment children via ChildList::append_to.",
                    },
                ])
            }

            Section(title = "Custom Card variants".to_string()) {
                P(content = "Card's variant trait is open. Implement Variant on a marker type \
                    and install a sheet that includes the new arm:".to_string())
                CodePanel(src = r##"use idea_ui::components::card::{build_card_sheet, install_card_sheet, variant};
use idea_theme::extensible::Variant;

#[derive(Copy, Clone, Default)]
struct Tinted;
impl Variant for Tinted {
    fn key(&self) -> &'static str { "tinted" }
    fn render(&self, ctx: &idea_theme::extensible::ResolutionCtx) -> runtime_core::StyleRules {
        runtime_core::StyleRules {
            background: Some(ctx.theme.intents().primary.soft_bg.clone()),
            ..Default::default()
        }
    }
}

// Install once, before any Card mounts.
install_card_sheet(build_card_sheet(vec![
    variant::Flat.into(),
    variant::Elevated.into(),
    Tinted.into(),
]));

// Then use it anywhere:
Card(variant = Tinted.into()) { Typography(content = "Tinted card".into()) }"##.to_string())
            }
        }
    })
}

// =============================================================================
// Table
// =============================================================================

pub fn table() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "Table".to_string(),
            lead = "Themed data table. On web, lowers to real HTML \
                `<table>`/`<tr>`/`<td>` via the `table` SDK so the browser's \
                table-layout algorithm sizes columns to widest content for \
                free. On native, the same shape falls through to a flex \
                passthrough.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            simple_table_demo()

            Section(title = "Rich cells with children".to_string()) {
                P(content = "Pass `children` instead of `text` to compose richer cell \
                    contents — Tags, Buttons, Typography, anything. The cell-level \
                    padding/border still applies; the inner default text styling is \
                    bypassed.".to_string())
                rich_table_demo()
                CodePanel(src = r##"Table {
    TableRow {
        TableCell(header = true, text = Some("Name".into()))
        TableCell(header = true, text = Some("Status".into()))
        TableCell(header = true, text = Some("Action".into()))
    }
    TableRow {
        TableCell(text = Some("Build".into()))
        TableCell {
            Tag(label = "Passing".into(), tone = tone::Success, variant = variant::Soft)
        }
        TableCell {
            Button(label = "Re-run".into(), on_click = run, tone = tone::Primary, variant = variant::Soft)
        }
    }
}"##.to_string())
            }

            Section(title = "Table props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "TableRows. Flattened via ChildList::append_to so `for` loops splat cleanly.",
                    },
                ])
            }

            Section(title = "TableRow props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "TableCells. Thin passthrough today; row-level affordances (hover, zebra) will land here without changing call sites.",
                    },
                ])
            }

            Section(title = "TableCell props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "header",
                        ty: "bool",
                        desc: "When true, renders as `<th>` with the head-cell surface + uppercase muted text style. Default: false.",
                    },
                    Prop {
                        name: "text",
                        ty: "Reactive<Option<String>>",
                        desc: "Shorthand for cell content — wraps the value in a themed text node using head/body typography. Ignored when `children` is non-empty.",
                    },
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "Custom cell contents (links, badges, multiple inline pieces). Replaces the default text rendering when provided.",
                    },
                ])
            }

            Section(title = "Why two layers".to_string()) {
                P(content = "The cross-platform `table` SDK (in `crates/sdk/table/`) emits real \
                    HTML table elements on web and a flex passthrough on native — that's the \
                    primitive layer. idea-ui's `Table` / `TableRow` / `TableCell` wrap those with \
                    theme tokens for surface, border, header background, and head/body \
                    typography. Same pattern as `Spinner` over `activity_indicator` and \
                    `Switch` over `toggle`: SDK does the mechanism, idea-ui does the look.".to_string())
            }

            Callout(label = "Column widths".to_string()) {
                P(content = "Columns auto-size to the widest content in each column thanks to the \
                    browser's `table-layout: auto` algorithm. To pin a column to a specific width, \
                    pass a `children` block in that column's header cell containing a fixed-width \
                    `view`. (Native targets currently use flex passthrough — explicit column-width \
                    controls land when there's a use case.)".to_string())
            }
        }
    })
}

fn simple_table_demo() -> Element {
    ui! {
        Table {
            TableRow {
                TableCell(header = true, text = Some("Token".to_string()))
                TableCell(header = true, text = Some("Value".to_string()))
                TableCell(header = true, text = Some("Used by".to_string()))
            }
            TableRow {
                TableCell(text = Some("color-text".to_string()))
                TableCell(text = Some("#1a1a1f".to_string()))
                TableCell(text = Some("Typography default, Field labels".to_string()))
            }
            TableRow {
                TableCell(text = Some("color-surface".to_string()))
                TableCell(text = Some("#ffffff".to_string()))
                TableCell(text = Some("Card, Field background, Table".to_string()))
            }
            TableRow {
                TableCell(text = Some("intent-primary-fg".to_string()))
                TableCell(text = Some("#3947d6".to_string()))
                TableCell(text = Some("Outlined Button, Ghost actions, Tag (Outlined)".to_string()))
            }
            TableRow {
                TableCell(text = Some("radius-lg".to_string()))
                TableCell(text = Some("12px".to_string()))
                TableCell(text = Some("Card corner radius, Table outer radius".to_string()))
            }
        }
    }
}

fn rich_table_demo() -> Element {
    let on_run: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(|| {});
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

// =============================================================================
// Divider
// =============================================================================

pub fn divider() -> Element {
    let state = DividerProps::init_state();
    let preview = DividerProps::reactive_preview(&state, |props| {
        let axis = props.axis;
        ui! {
            Stack(gap = StackGap::Md) {
                Typography(content = "Above".to_string())
                Divider(axis = axis)
                Typography(content = "Below".to_string())
            }
        }
    });
    let controls = DividerProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "Divider".to_string(),
            lead = "Thin separator line, horizontal or vertical.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "axis",
                        ty: "DividerAxis",
                        desc: "Horizontal (default) renders a 1px-tall border-bottom; Vertical renders a 1px-wide border-left.",
                    },
                ])
            }

            Callout(label = "Color".to_string()) {
                P(content = "Divider reads `color-border`; the line follows the active theme \
                    automatically. To color a divider for emphasis, wrap the surrounding \
                    Stack with a token override instead of writing a per-divider stylesheet.".to_string())
            }
        }
    })
}

// =============================================================================
// Center
// =============================================================================

pub fn center() -> Element {
    let preview = ui! {
        Center {
            Badge(
                label = "Centered".to_string(),
                tone = tone::Primary,
                variant = variant::Soft,
            )
        }
    };

    shell::layout(ui! {
        ComponentPage(
            title = "Center".to_string(),
            lead = "Two-axis centering container — the shorthand for the common case \
                (empty-state, spinner, full-screen splash).".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            DemoSurface { preview }

            Section(title = "Why it exists".to_string()) {
                P(content = "Equivalent to a Stack with align: Center, justify: Center. The \
                    shorthand exists so the common case (centering a loading spinner, an \
                    empty-state illustration) doesn't need a one-off stylesheet.".to_string())
            }

            Section(title = "Recipe — empty state".to_string()) {
                CodePanel(src = r##"Center {
    Stack(gap = StackGap::Sm) {
        Icon(name = "inbox", size = 48.0)
        Typography(content = "No messages".into(), kind = typography_kind::H3)
        Typography(content = "You're all caught up.".into(), muted = true)
    }
}"##.to_string())
            }
        }
    })
}

// =============================================================================
// Spacer
// =============================================================================

pub fn spacer() -> Element {
    let noop: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(|| {});
    let preview = ui! {
        Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
            Typography(content = "Title".to_string(), kind = typography_kind::H3)
            Spacer()
            Button(
                label = "Save".to_string(),
                on_click = noop,
                tone = tone::Primary,
                variant = variant::Filled,
            )
        }
    };

    shell::layout(ui! {
        ComponentPage(
            title = "Spacer".to_string(),
            lead = "Empty flex item that grows to fill available space. Used to push \
                siblings to opposite ends of a row.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            DemoSurface { preview }

            Section(title = "How it works".to_string()) {
                P(content = "Spacer renders an empty `view` with `flex_grow: 1`. Drop one \
                    between siblings inside a row Stack to push them to opposite ends without \
                    computing margins.".to_string())
                CodePanel(src = r##"Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
    Typography(content = "Title".into(), kind = typography_kind::H3)
    Spacer()
    Button(label = "Save".into(), on_click = save, tone = tone::Primary, variant = variant::Filled)
}"##.to_string())
            }

            Section(title = "Multiple Spacers".to_string()) {
                P(content = "Two Spacers in the same Stack split the leftover space evenly. \
                    Three Spacers split it in thirds. Useful for distributing items along an \
                    axis without computing exact widths.".to_string())
            }
        }
    })
}
