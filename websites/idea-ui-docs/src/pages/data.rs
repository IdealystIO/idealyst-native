//! Data — Card, Table (body-only pages).

use std::rc::Rc;

use runtime_core::{rx, signal, ui, Element, Signal};
use icons_lucide::{PENCIL, TRASH_2};
use idea_ui::{
    tone, typography_kind, variant, Button, Card, IconButton, Stack, StackAxis, StackGap, Tag,
    Typography,
};
use idea_ui::{ColumnPin, Table, TableCell, TableRow};

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
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Lg) {
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
            Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
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
            Section(title = "Clickable rows with buttons".to_string()) {
                P(content = "Set `on_row_click` on a `TableRow` to make the whole row a tap \
                    target, and still put `Button`s / `IconButton`s in its cells. Clicking a \
                    control fires ONLY that control — the row click does not also fire. Clicking \
                    anywhere else in the row fires the row. Try it: the counters below update \
                    independently.".to_string())
                clickable_row_table()
                CodePanel(src = r##"TableRow(on_row_click = Some(select_row)) {
    TableCell(text = Some("Build".to_string()))
    TableCell { Tag(label = "Passing".into(), tone = tone::Success, variant = variant::Soft) }
    TableCell {
        // These eat their own click — the row's on_row_click does NOT fire.
        IconButton(icon = Some(PENCIL), on_click = edit_row, ..)
        Button(label = "Delete".into(), on_click = delete_row, ..)
    }
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Status table".to_string()) {
                P(content = "On web, Table lowers to a real HTML `<table>` via the `table` SDK, so \
                    the browser's table-layout algorithm sizes columns to widest content for free. \
                    On native, the same shape lowers to a CSS-grid whose column tracks are shared \
                    across rows, so columns line up identically on every platform.".to_string())
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
            Section(title = "Horizontal scroll + frozen columns".to_string()) {
                P(content = "Set `scroll_x = true` and columns lay out at their natural width \
                    inside a horizontal scroller — wide tables overflow sideways instead of \
                    squeezing and wrapping, and narrow ones still fill. Freeze a column against \
                    either edge with `pinned` on its cells (pin the SAME cell in every row, header \
                    included): the frozen column stays put while the rest scrolls beneath it, on \
                    every backend.".to_string())
                wide_table()
                CodePanel(src = r##"Table(scroll_x = true) {
    TableRow {
        TableCell(header = true, text = Some("Service".to_string()), pinned = Some(ColumnPin::Left))
        TableCell(header = true, text = Some("Region".to_string()))
        // …more columns…
        TableCell(header = true, text = Some("Status".to_string()), pinned = Some(ColumnPin::Right))
    }
    TableRow {
        TableCell(text = Some("auth".to_string()), pinned = Some(ColumnPin::Left))
        TableCell(text = Some("eu-west-1".to_string()))
        // …
        TableCell(pinned = Some(ColumnPin::Right)) {
            Tag(label = "Healthy".into(), tone = tone::Success, variant = variant::Soft)
        }
    }
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Row drag & drop — bring your own".to_string()) {
                P(content = "idea-ui ships no drag-and-drop behavior; the `table` SDK exposes the \
                    handles a custom implementation needs, and you own the interaction. Per row: \
                    fan your drag recognizer across the cells with `visit_row_cells` + \
                    `set_cell_touch` (row touch must live per-cell), bind drop-target geometry to \
                    the row's proxy surface with `bind_row`, anchor animated drag offsets per cell \
                    with `bind_cell`, and select the themed `dragging` / `drop_target` feedback \
                    axes with `map_cell_style`, which composes them over whatever style a cell already \
                    carries. The demo below wires the \
                    `dnd` SDK through exactly those seams — long-press a row to pick it up.".to_string())
                reorder_table()
                CodePanel(src = r##"// Userland wiring — everything here is public `table` SDK + `dnd` SDK surface.
let drag = Draggable::new(&ctx, move || i)
    .activation(Activation::LongPress { threshold_ms: 250, slop_px: 8.0 });
let handler = drag.handler();          // ONE recognizer per row, cloned per cell
let (_, offset_y) = drag.offset();
let dragging = drag.is_dragging();

let drop = Droppable::new(&ctx)
    .accepts(move |from: &usize| *from != i)
    .on_drop(move |from| reorder(from, i));
let over = drop.is_over();
let row_ref: Ref<ViewHandle> = Ref::new();
drop.bind(row_ref);
table::bind_row(&row, move |h| row_ref.fill(h));   // row proxy = drop geometry

table::visit_row_cells(&row, |cell| {
    table::map_cell_style(cell, Rc::new(move |app| app
        .with("dragging", if dragging.get() { "on" } else { "off" })
        .with("drop_target", if over.get() { "on" } else { "off" })));
    table::set_cell_touch(cell, handler.clone());
    let cell_ref: Ref<ViewHandle> = Ref::new();
    table::bind_cell(cell, move |h| cell_ref.fill(h));
    offset_y.bind(cell_ref, AnimProp::TranslateY);  // row follows the finger
});"##.to_string())
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
                    Prop {
                        name: "scroll_x",
                        ty: "bool",
                        desc: "Horizontal-scroll mode: columns at natural width inside a horizontal scroller. Required for pinned columns. Default false.",
                    },
                ])
            }
        },
        ui! {
            Section(title = "TableRow props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "TableCells for this row.",
                    },
                    Prop {
                        name: "on_row_click",
                        ty: "Option<Rc<dyn Fn()>>",
                        desc: "Whole-row tap target + hover highlight. Buttons inside cells still eat their own clicks. Default None.",
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
                    Prop {
                        name: "pinned",
                        ty: "Option<ColumnPin>",
                        desc: "Freeze this column against the Left or Right scroller edge (requires Table(scroll_x = true)). Pin the same cell in every row. Default None.",
                    },
                ])
            }
        },
    ])
}

/// Horizontal-scroll demo: enough columns to overflow the demo surface,
/// with the identifying column frozen left and the status column frozen
/// right — the two edges stay put while the middle scrolls beneath them.
fn wide_table() -> Element {
    fn status_tone(status: &str) -> idea_ui::ToneRef {
        if status == "Healthy" { tone::Success.into() } else { tone::Danger.into() }
    }
    let services = [
        ("auth", "eu-west-1", "v2.14.0", "3", "12ms", "0.02%", "Healthy"),
        ("billing", "us-east-1", "v2.13.7", "5", "48ms", "0.10%", "Healthy"),
        ("search", "ap-south-1", "v2.14.0", "9", "220ms", "1.90%", "Degraded"),
        ("media", "us-west-2", "v2.12.9", "4", "95ms", "0.00%", "Healthy"),
    ];
    ui! {
        Table(scroll_x = true) {
            TableRow {
                TableCell(header = true, text = Some("Service".to_string()), pinned = Some(ColumnPin::Left))
                TableCell(header = true, text = Some("Region".to_string()))
                TableCell(header = true, text = Some("Version".to_string()))
                TableCell(header = true, text = Some("Replicas".to_string()))
                TableCell(header = true, text = Some("p99 latency".to_string()))
                TableCell(header = true, text = Some("Error rate".to_string()))
                TableCell(header = true, text = Some("Status".to_string()), pinned = Some(ColumnPin::Right))
            }
            for (name, region, version, replicas, p99, errs, status) in services {
                TableRow {
                    TableCell(text = Some(name.to_string()), pinned = Some(ColumnPin::Left))
                    TableCell(text = Some(region.to_string()))
                    TableCell(text = Some(version.to_string()))
                    TableCell(text = Some(replicas.to_string()))
                    TableCell(text = Some(p99.to_string()))
                    TableCell(text = Some(errs.to_string()))
                    TableCell(pinned = Some(ColumnPin::Right)) {
                        Tag(label = status.to_string(), tone = status_tone(status), variant = variant::Soft)
                    }
                }
            }
        }
    }
}

/// Reorder demo — a USERLAND drag-and-drop implementation, wired
/// entirely through public seams: the `table` SDK's row/cell handles
/// plus the `dnd` SDK. idea-ui contributes only the inert themed
/// feedback axes (`dragging` / `drop_target`) on its cell sheets.
///
/// Shape: the row order lives in a signal; each row gets a
/// `Draggable<usize>` (long-press pickup — vertical drag IS the page's
/// scroll direction, so the scroll-aware activation is wrong here)
/// whose ONE recognizer fans out to the cells, a `Droppable` bound to
/// the row's proxy surface, and the drag's vertical offset bound to
/// every cell so the row follows the finger. Dropping reorders the
/// signal and the keyed loop rebuilds.
fn reorder_table() -> Element {
    use dnd::{Activation, DragContext, Draggable, Droppable};
    use runtime_core::animation::AnimProp;
    use runtime_core::{memo, Ref, ViewHandle};

    let items: Signal<Vec<&'static str>> =
        signal(vec!["Ship hotfix", "Review PR #412", "Update changelog", "Cut release"]);
    let ctx: DragContext<usize> = DragContext::new();

    let wire = move |ctx: &DragContext<usize>, i: usize, row: &Element| {
        let drag = Draggable::new(ctx, move || i)
            .activation(Activation::LongPress { threshold_ms: 250, slop_px: 8.0 });
        let dragging = drag.is_dragging();
        let (_, offset_y) = drag.offset();
        // One recognizer per row, cloned per cell — `handler()` builds
        // fresh recognizer state on every call.
        let handler = drag.handler();

        let drop = Droppable::new(ctx)
            .accepts(move |from: &usize| *from != i)
            .on_drop(move |from| {
                let mut v = items.get();
                if from < v.len() && i <= v.len() {
                    let moved = v.remove(from);
                    v.insert(i.min(v.len()), moved);
                    items.set(v);
                }
            });
        let over = drop.is_over();
        let row_ref: Ref<ViewHandle> = Ref::new();
        drop.bind(row_ref);
        table::bind_row(row, move |h| row_ref.fill(h));

        table::visit_row_cells(row, |cell| {
            table::map_cell_style(
                cell,
                Rc::new(move |app: runtime_core::StyleApplication| {
                    app.with("dragging", if dragging.get() { "on" } else { "off" })
                        .with("drop_target", if over.get() { "on" } else { "off" })
                }),
            );
            table::set_cell_touch(cell, handler.clone());
            let cell_ref: Ref<ViewHandle> = Ref::new();
            table::bind_cell(cell, move |h| cell_ref.fill(h));
            offset_y.bind(cell_ref, AnimProp::TranslateY);
        });
    };

    // Indexed view of the list so each row knows its ordinal.
    let indexed = memo(move || {
        items
            .get()
            .into_iter()
            .enumerate()
            .collect::<Vec<(usize, &'static str)>>()
    });

    ui! {
        Table {
            TableRow {
                TableCell(header = true, text = Some("".to_string()))
                TableCell(header = true, text = Some("Task (long-press to reorder)".to_string()))
            }
            for (i, name) in indexed, key = name {
                {
                    let row = ui! {
                        TableRow {
                            TableCell(text = Some("\u{283f}".to_string()))
                            TableCell(text = Some(name.to_string()))
                        }
                    };
                    wire(&ctx, i, &row);
                    row
                }
            }
        }
    }
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

/// Interactive demo: rows with `on_row_click` whose cells ALSO hold buttons.
/// Two reactive readouts make the fix observable — `on_row_click` sets
/// "Selected row", the cell buttons set "Last button action". Clicking a
/// button changes ONLY the button line; the selected row is untouched. If the
/// row swallowed the button (the old bug), a button press would ALSO change
/// the selected row. The status text is wrapped in `rx!` so it re-renders when
/// either signal changes (a bare `.get()` would read once and never update).
fn clickable_row_table() -> Element {
    let selected: Signal<String> = signal("(none — click a row's text/background)".to_string());
    let last_action: Signal<String> = signal("(none — click a button or the edit icon)".to_string());

    // Closure factories: each returns an `Rc<dyn Fn()>`. Signals are `Copy`
    // handles, so the factories capture them by value and can be called once
    // per row.
    let on_row = move |name: &'static str| -> Rc<dyn Fn()> {
        Rc::new(move || selected.set(name.to_string()))
    };
    let on_edit = move |name: &'static str| -> Rc<dyn Fn()> {
        Rc::new(move || last_action.set(format!("edit icon → {name}")))
    };
    let on_delete = move |name: &'static str| -> Rc<dyn Fn()> {
        Rc::new(move || last_action.set(format!("delete button → {name}")))
    };

    ui! {
        Stack(axis = StackAxis::Column, gap = StackGap::Sm) {
            Typography(
                kind = typography_kind::Body,
                content = rx!(format!("Selected row: {}", selected.get())),
            )
            Typography(
                kind = typography_kind::Body,
                content = rx!(format!("Last button action: {}", last_action.get())),
            )
            Table {
                TableRow {
                    TableCell(header = true, text = Some("Job".to_string()))
                    TableCell(header = true, text = Some("Status".to_string()))
                    TableCell(header = true, text = Some("Actions".to_string()))
                }
                TableRow(on_row_click = Some(on_row("Build"))) {
                    TableCell(text = Some("Build".to_string()))
                    TableCell {
                        Tag(label = "Passing".to_string(), tone = tone::Success, variant = variant::Soft)
                    }
                    TableCell {
                        Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                            IconButton(icon = Some(PENCIL), on_click = on_edit("Build"), tone = tone::Neutral, variant = variant::Soft)
                            Button(label = "Delete".to_string(), on_click = on_delete("Build"), tone = tone::Danger, variant = variant::Soft)
                        }
                    }
                }
                TableRow(on_row_click = Some(on_row("Unit tests"))) {
                    TableCell(text = Some("Unit tests".to_string()))
                    TableCell {
                        Tag(label = "Passing".to_string(), tone = tone::Success, variant = variant::Soft)
                    }
                    TableCell {
                        Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                            IconButton(icon = Some(PENCIL), on_click = on_edit("Unit tests"), tone = tone::Neutral, variant = variant::Soft)
                            Button(label = "Delete".to_string(), on_click = on_delete("Unit tests"), tone = tone::Danger, variant = variant::Soft)
                        }
                    }
                }
                TableRow(on_row_click = Some(on_row("Deploy"))) {
                    TableCell(text = Some("Deploy".to_string()))
                    TableCell {
                        Tag(label = "Blocked".to_string(), tone = tone::Danger, variant = variant::Soft)
                    }
                    TableCell {
                        Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                            IconButton(icon = Some(TRASH_2), on_click = on_edit("Deploy"), tone = tone::Neutral, variant = variant::Soft)
                            Button(label = "Delete".to_string(), on_click = on_delete("Deploy"), tone = tone::Danger, variant = variant::Soft)
                        }
                    }
                }
            }
        }
    }
}

