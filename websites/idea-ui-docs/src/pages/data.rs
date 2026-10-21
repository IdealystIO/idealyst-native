//! Data — Card, Table (body-only pages).

use std::collections::BTreeSet;
use std::rc::Rc;

use runtime_core::{current_breakpoint, rx, signal, ui, BorderStyle, Breakpoint, Element, Signal, StyleRules, StyleSheet};
use icons_lucide::{ELLIPSIS_VERTICAL, PENCIL, TRASH_2};
use idea_ui::{
    tone, typography_kind, variant, Button, Card, CardPadding, CardSlotStyles, IconButton, Stack, StackAxis,
    StackGap, Tag, Typography,
};
use idea_ui::{
    move_in_place, CardSlot, CellAlign, CellTone, ColSpan, ColumnId, ColumnPin, ColumnWidths,
    RowId, RowTone, SortState, Table, TableCards, TableCell, TableColumn, TableDensity,
    TableFooter, TableInset, TablePager, TableRow, TableVariant,
};

use crate::shell::{Callout, CodePanel, DemoSurface, Prop, PropsTable, Section, P};
use idea_ui::components::card::variant as card_variant;
use crate::restyles::DocsCardVariants;
use idea_ui::CardVariant;

// =============================================================================
// Card
// =============================================================================

pub fn card() -> Element {
    crate::pages::body(vec![
        ui! {
            Section(title = "Composition".to_string()) {
                P(content = "A Card is a themed surface with rounded corners and one of four \
                    variants: Regular, Outline, Flat and Elevated. Compose the inner anatomy yourself — header, \
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
            Section(title = "Variants".to_string()) {
                P(content = "Outline is the base for a dashed or dotted frame: the fill is gone, so \
                    a `border_style` override is the whole look.".to_string())
                DemoSurface {
                    variant_gallery()
                }
                CodePanel(src = r##"let dashed = Rc::new(StyleSheet::r#static(StyleRules {
    border_style: Some(BorderStyle::Dashed),
    ..Default::default()
}));
Card(variant = card::variant::Outline, style = Some(dashed)) { … }"##.to_string())
            }
        },
        ui! {
            Section(title = "Anatomy".to_string()) {
                P(content = "The variant determines the surface. Regular (the default) reads \
                    `color-surface` inside a 1px border. Outline keeps the border and drops the fill, \
                    so it is the one to start from for a dashed or dotted frame — set `border_style` \
                    in the card's `style` override. Flat reads `color-surface-alt` with no border. \
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
                        ty: "CardVariant",
                        desc: "card::variant::Regular (default, page surface + border), Outline (border only, no fill), Flat (secondary surface, no border), Elevated (raised surface + shadow), or an app value (`CardVariant::define`).",
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
            Section(title = "Your own variant".to_string()) {
                P(content = "Card's variant axis is open: `axis_values!` names a new value, \
                    `CardVariant::Tinted` (its key derived from the name), and one restyle arm at boot gives it a look. The built-in variants stay; an arm \
                    keyed `regular` (or another built-in's key) changes just that one. A Card variant \
                    is Card's own type, so it does not fit a Button.".to_string())
                DemoSurface {
                    Card(variant = CardVariant::Tinted) {
                        Typography(content = "Tinted".to_string(), kind = typography_kind::H3)
                        Typography(content = "An app-defined variant.".to_string(), muted = true)
                    }
                }
                CodePanel(src = r##"axis_values! { pub trait AppCardVariants for CardVariant { Tinted } }

// At boot, before the first render:
CardStyles::restyle(|s| {
    s.root().arm(CardVariant::Tinted, |t| StyleRules {
        background: Some(t.intent.primary.soft_bg()),
        border_top_color: Some(t.intent.primary.solid_bg()),
        // …the other three edges…
        ..Default::default()
    });
});

// Anywhere:
Card(variant = CardVariant::Tinted) { … }"##.to_string())
            }
        },
    ])
}

fn media_card() -> Element {
    let on_act: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Card(variant = card_variant::Elevated) {
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

fn variant_gallery() -> Element {
    let dashed = CardSlotStyles::default().root(StyleRules {
        border_style: Some(BorderStyle::Dashed),
        ..Default::default()
    });
    ui! {
        Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Lg) {
            Card(variant = card_variant::Regular) {
                Typography(content = "Regular".to_string(), kind = typography_kind::H3)
                Typography(content = "Surface fill, hairline border.".to_string(), muted = true)
            }
            Card(variant = card_variant::Outline) {
                Typography(content = "Outline".to_string(), kind = typography_kind::H3)
                Typography(content = "Border only.".to_string(), muted = true)
            }
            Card(variant = card_variant::Outline, styles = dashed) {
                Typography(content = "Outline, dashed".to_string(), kind = typography_kind::H3)
                Typography(content = "border_style: Dashed.".to_string(), muted = true)
            }
            Card(variant = card_variant::Flat) {
                Typography(content = "Flat".to_string(), kind = typography_kind::H3)
                Typography(content = "Secondary fill, no border.".to_string(), muted = true)
            }
            Card(variant = card_variant::Elevated) {
                Typography(content = "Elevated".to_string(), kind = typography_kind::H3)
                Typography(content = "Raised with a shadow.".to_string(), muted = true)
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
            Section(title = "A page table: column model, resize, selection, sort, pager".to_string()) {
                P(content = "Everything a data table needs, from Table, TableRow and TableCell alone. \
                    `columns` puts each column's settings on the TABLE — width, floor, cap, fill, \
                    resizable, frozen edge, alignment — so the cells carry none of it. Hover a \
                    header and drag its trailing edge to resize the whole column live (double-click \
                    the edge to reset); the widths live in a signal the screen owns, so a resized \
                    column survives the table being rebuilt for the next page. `selection` adds the \
                    tri-state checkbox column, the name column and the checkboxes freeze together, \
                    the trailing `TableColumn::actions` column is frozen right, headers with `sort` \
                    toggle the order, and the pager sits in the footer slot under the scrolling \
                    rows. Resize the window: the edge inset steps with the breakpoint without the \
                    table rebuilding — and below `sm` the same column specs lay the rows out as \
                    cards.".to_string())
                page_table()
                CodePanel(src = r##"let widths = signal(saved_widths);            // the SCREEN owns these
let selected = signal(BTreeSet::<RowId>::new());
let columns = vec![
    TableColumn::new("name").label("Project").width(220.0).min_width(120.0)
        .resizable().pinned(ColumnPin::Left).card(CardSlot::Title),
    TableColumn::new("client").label("Client").width(150.0).resizable(),
    TableColumn::new("status").label("Status").card(CardSlot::Opposite),
    TableColumn::new("due").label("Due"),
    TableColumn::new("hours").label("Hours").width(90.0).align(CellAlign::Right),
    TableColumn::new("notes").label("Notes").fill().min_width(160.0).truncate(),
    TableColumn::actions("menu"),
];
ui! {
    if current_breakpoint().get().is_at_least(Breakpoint::Sm) {
        Table(
            scroll_x = true, scroll_y = true, max_height = Some(360.0),
            variant = TableVariant::Plain,
            inset_x = TableInset::by_breakpoint(TableInset::Px(16.0), TableInset::Px(24.0), TableInset::Px(28.0)),
            columns = columns.clone(),
            column_widths = Some(widths),
            on_column_resize = Some(save_width),
            selection = Some(selected),
            footer_slot = Some(ui! {
                TablePager(offset = offset, page_size = size, total = total,
                           noun = "projects".to_string(), on_offset = go,
                           page_sizes = vec![10, 25, 50], on_page_size = Some(resize_page))
            }),
        ) {
            TableRow {
                TableCell(header = true, text = Some("Project".into()),
                          sort = rx!(sort_of("name")), on_sort = Some(sort_by("name")))
                // …one header cell per column, the menu column's empty…
            }
            for p in page {
                TableRow(id = Some(p.id.into()), tone = landed(p.id), on_row_click = Some(open(p.id))) {
                    TableCell(text = Some(p.name))
                    // …
                    TableCell(text = Some(p.due), tone = if p.overdue { CellTone::Danger } else { CellTone::None })
                    TableCell { IconButton(icon = Some(ELLIPSIS_VERTICAL), …) }
                }
            }
        }
    } else {
        TableCards(columns = columns.clone(), row_count = rx!(rows.get().len()),
                   row_id = row_id, cell = cell, on_end_reached = Some(load_more))
    }
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Drag to reorder".to_string()) {
                P(content = "`on_reorder` adds a leading grip column. Grab a row by its grip and drag: \
                    the row lifts above its neighbours while they slide aside to open the gap, and \
                    dropping calls `on_reorder(from, to)` — ordinals over the draggable rows; \
                    `move_in_place` is the matching list move. Rows only land within their \
                    `reorder_group`, and a row with `draggable = false` (the group titles here) has \
                    no grip. The lifted row keeps its frozen column frozen: scroll sideways mid-drag \
                    and the name stays put.".to_string())
                reorder_table()
                CodePanel(src = r##"Table(scroll_x = true, on_reorder = Some(Rc::new(move |from, to| {
    let mut v = items.get();
    if move_in_place(&mut v, from, to) { items.set(v) }   // then persist
}))) {
    TableRow { TableCell(header = true, text = Some("Task".into()), pinned = Some(ColumnPin::Left)) … }
    for task in tasks {
        TableRow(id = Some(task.id.into()), reorder_group = Some(task.group.into())) { … }
    }
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Empty state".to_string()) {
                P(content = "`empty` keeps the header of a table with no rows and spans one row saying \
                    why across every column — past the selection and grip columns, so the sentence \
                    starts where the first column does. No column counting.".to_string())
                empty_table()
                CodePanel(src = r##"Table(selection = Some(selected), empty = Some("No one is signed in yet.".into())) {
    TableRow { TableCell(header = true, text = Some("Name".into())) … }
    for person in signed_in { TableRow(id = Some(person.id.into())) { … } }
}"##.to_string())
            }
        },
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
                wide_table(TableVariant::Framed)
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
            Section(title = "Pinned header (vertical scroll)".to_string()) {
                P(content = "Set `scroll_y = true` and the rows scroll inside the table's own \
                    height while the header row stays pinned to the top. Bound it with \
                    `max_height`, or let it fill a bounded parent. With `scroll_x` as well the \
                    table owns ONE two-axis scroller: the header pins to the top, frozen \
                    columns pin to the sides, and a frozen header cell — the corner — pins \
                    both ways, painted above everything sliding beneath it. (In a `scroll_x` \
                    table that scrolls with the page the header cannot stick: a sticky cell \
                    pins against its nearest scroller, which is the table's own.)".to_string())
                pinned_header_table()
                CodePanel(src = r##"Table(scroll_x = true, scroll_y = true, max_height = Some(240.0)) {
    TableRow {
        TableCell(header = true, text = Some("Service".to_string()), pinned = Some(ColumnPin::Left))
        TableCell(header = true, text = Some("Region".to_string()))
        // …more columns…
    }
    for row in rows {
        TableRow {
            TableCell(text = Some(row.name.clone()), pinned = Some(ColumnPin::Left))
            TableCell(text = Some(row.region.clone()))
            // …
        }
    }
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Edge inset".to_string()) {
                P(content = "`inset_x` sets the outer padding of every row's first and last \
                    cell (and of the frame slots) and leaves the cells between alone, so the \
                    row dividers run the table's full width while the text lines up with \
                    the page margin. Token sizes follow the theme's spacing scale; \
                    `TableInset::Px(28.0)` takes an exact margin. Pair it with the plain \
                    variant for an edge-to-edge list.".to_string())
                inset_table()
                CodePanel(src = r##"Table(variant = TableVariant::Plain, inset_x = TableInset::Xl) {
    TableRow {
        TableCell(header = true, text = Some("Crew".to_string()))
        TableCell(header = true, text = Some("Shift".to_string()), align = CellAlign::Right)
    }
    // …rows…
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Plain variant".to_string()) {
                P(content = "`variant = TableVariant::Plain` drops the card: no frame, no \
                    rounded corners, no header band and no fill of its own — it sits on the \
                    surface around it, and the row dividers are its only lines (give the \
                    container a background if you want one). In a `scroll_x` table a \
                    frozen column draws its inner edge only while columns are scrolled \
                    beneath it: at rest the right edge shows (there is more to the right), \
                    and the left edge appears once you scroll. Frozen cells must stay \
                    opaque, so they paint `color-surface`: sit a plain table on that \
                    surface.".to_string())
                // On a surface, as a plain table belongs: its frozen cells
                // paint `color-surface`, which the demo ground is not.
                Card(padding = CardPadding::Md) { wide_table(TableVariant::Plain) }
                CodePanel(src = r##"Table(scroll_x = true, variant = TableVariant::Plain) {
    TableRow {
        TableCell(header = true, text = Some("Service".to_string()), pinned = Some(ColumnPin::Left))
        // …more columns…
        TableCell(header = true, text = Some("Status".to_string()), pinned = Some(ColumnPin::Right))
    }
    // …rows, pinned the same way…
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Column widths, alignment, truncation".to_string()) {
                P(content = "Column settings go on EVERY cell of the column, header included. \
                    `width` holds a column at exactly that width; the spare width goes to the \
                    columns without one — so size every column but one and that one fills the \
                    rest (the browser works the same way, which is why there is no separate \
                    \"fill\" setting). `min_width` floors a column without fixing it. `align` \
                    sets header and body alignment together. `truncate` keeps a sized column \
                    to one line ending in \"…\". `width` and `min_width` are reactive, so a \
                    column can follow a resize drag.".to_string())
                columns_table()
                CodePanel(src = r##"TableRow {
    TableCell(header = true, width = Some(140.0), truncate = true, text = Some("Crew".into()))
    TableCell(header = true, text = Some("Notes".into()))            // no width → fills
    TableCell(header = true, width = Some(96.0), align = CellAlign::Center, text = Some("Status".into()))
    TableCell(header = true, width = Some(80.0), align = CellAlign::Right, text = Some("Hours".into()))
}
TableRow {
    TableCell(width = Some(140.0), truncate = true, text = Some(name))
    TableCell(text = Some(notes))
    TableCell(width = Some(96.0), align = CellAlign::Center) { Tag(..) }
    TableCell(width = Some(80.0), align = CellAlign::Right, text = Some(hours))
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Fill the rest: flex columns".to_string()) {
                P(content = "`flex = Some(1.0)` gives a column the width the other columns leave, \
                    and lets it shrink to nothing — pair it with `truncate` and a long name ends \
                    in \"…\" instead of pushing the short columns off the edge. `flex` or \
                    `max_width` on any column lays the table out as grid columns: `width` columns \
                    are fixed, `flex` columns share the leftover, a `max_width` column is its \
                    content's width up to the cap, and every other column keeps its content \
                    width. Narrow the window to watch Name give way while Type and Page hold.".to_string())
                flex_columns_table()
                CodePanel(src = r##"Table {
    TableRow {
        TableCell(header = true, flex = Some(1.0), truncate = true, text = Some("Name".into()))
        TableCell(header = true, text = Some("Type".into()))
        TableCell(header = true, align = CellAlign::Right, text = Some("Page".into()))
    }
    for (name, kind, page) in fields {
        TableRow {
            TableCell(flex = Some(1.0), truncate = true, text = Some(name))
            TableCell(text = Some(kind))
            TableCell(align = CellAlign::Right, text = Some(page))
        }
    }
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Spanning cells, row tones, footer".to_string()) {
                P(content = "`span = ColSpan::Columns(n)` covers n columns; `ColSpan::Rest` covers \
                    every column to the end of the row — the blank row, an add row, a group \
                    title. `TableRow(tone = …)` tints a whole row (`Highlight`, `Warning`, \
                    `Danger`); the tint keeps the clickable-row hover (it deepens) and stays \
                    opaque on frozen columns. `TableFooter` holds a totals row: a real \
                    `<tfoot>` on web, the header band's tint, and always after the body rows. \
                    The demo above uses all three.".to_string())
                CodePanel(src = r##"TableRow(tone = RowTone::Warning, on_row_click = Some(open)) { … }
if crew.is_empty() {
    TableRow { TableCell(span = ColSpan::Rest, text = Some("Nobody yet".into())) }
}
TableFooter {
    TableRow {
        TableCell(span = ColSpan::Columns(3), text = Some("Crew totals · 4".into()))
        TableCell(width = Some(80.0), align = CellAlign::Right, text = Some("38.5".into()))
    }
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Frame slots, stacked frozen columns, density".to_string()) {
                P(content = "`header_slot` and `footer_slot` draw inside the table's frame but \
                    outside its horizontal scroller, so a notice strip or an \"Add entry\" \
                    action keeps still while the columns scroll and shares the table's border. \
                    To freeze two leading columns, size them and give the second \
                    `pin_offset = Some(<first width>)`. `density` sets row padding for the whole \
                    table — `Comfortable` puts a one-line row in the 44–52pt touch band.".to_string())
                slots_table()
                CodePanel(src = r##"Table(
    scroll_x = true,
    density = TableDensity::Comfortable,
    header_slot = Some(ui! { Typography(content = "Mon 13 is a public holiday".into()) }),
    footer_slot = Some(ui! { Button(label = "Add entry".into(), on_click = add, ..) }),
) {
    TableRow {
        TableCell(header = true, width = Some(48.0), pinned = Some(ColumnPin::Left))
        TableCell(header = true, width = Some(160.0), pinned = Some(ColumnPin::Left),
                  pin_offset = Some(48.0), text = Some("Name".into()))
        // …hour columns scroll beneath both…
    }
}"##.to_string())
            }
        },
        ui! {
            Section(title = "Building your own row interaction".to_string()) {
                P(content = "For an interaction the table does not ship, the `table` SDK exposes the \
                    handles its own are built from: `bind_row` for a row's proxy surface (drop \
                    geometry, anchoring), `visit_row_cells` + `set_cell_touch` to fan a recognizer \
                    across a row's cells, `bind_cell` for animated per-cell offsets, and \
                    `map_cell_style` to select themed axes (the cell sheets ship `dragging` / \
                    `drop_target` arms) over whatever a cell already carries.".to_string())
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
                    Prop {
                        name: "scroll_y",
                        ty: "bool",
                        desc: "Vertical-scroll mode: rows scroll inside the table's own height and the header row (the leading rows of header cells) pins to the top. With scroll_x the table owns one two-axis scroller, so the header and frozen columns pin together. Default false.",
                    },
                    Prop {
                        name: "max_height",
                        ty: "Option<f32>",
                        desc: "The most a scroll_y table grows to, in px, before its rows scroll. Without it a scroll_y table fills the height its parent gives it. Ignored without scroll_y. Default None.",
                    },
                    Prop {
                        name: "inset_x",
                        ty: "Reactive<TableInset>",
                        desc: "Outer padding of each row's first and last cell (and the frame slots): None (default), Xs–Xxl spacing tokens, or Px(f32). Dividers keep running the full width while text aligns to the page margin. Reactive — TableInset::by_breakpoint(base, md, lg) re-pads in place as the page margin steps, without rebuilding the table.",
                    },
                    Prop {
                        name: "variant",
                        ty: "TableVariant",
                        desc: "Framed (default: the card — frame, rounded corners, surface fill, header band) or Plain (no frame or fill of its own — it sits on the surface around it; row dividers only; a frozen column's edge shows only while columns are scrolled beneath it).",
                    },
                    Prop {
                        name: "density",
                        ty: "TableDensity",
                        desc: "Row padding for every cell: Compact, Standard (default) or Comfortable (a one-line row lands in the 44–52pt touch band).",
                    },
                    Prop {
                        name: "header_slot",
                        ty: "Option<Element>",
                        desc: "Content inside the frame above the rows, outside the horizontal scroller (stays put while columns scroll). Gets cell padding and a divider. Default None.",
                    },
                    Prop {
                        name: "footer_slot",
                        ty: "Option<Element>",
                        desc: "Content inside the frame below the rows, outside the scroller — a TablePager, an Add action. Pinned under a scroll_y table's rows. Default None.",
                    },
                    Prop {
                        name: "columns",
                        ty: "Vec<TableColumn>",
                        desc: "The column model: column i describes the i-th column you wrote (the selection and grip columns are not counted). Each spec — width, min/max, fill, resizable, pinned, align, truncate, padding — applies to every cell of its column, header and body. Default empty (cells size their own columns).",
                    },
                    Prop {
                        name: "column_widths",
                        ty: "Option<Signal<ColumnWidths>>",
                        desc: "Live column widths by id: a resize drag writes here, and an entry overrides the spec width. Create it in the screen and seed it with the saved layout so widths survive a rebuild. Default None (the table keeps its own).",
                    },
                    Prop {
                        name: "on_column_resize",
                        ty: "Option<Rc<dyn Fn(ColumnId, Option<f32>)>>",
                        desc: "Called when a resize drag is released (Some(width)) or a column is reset by double-clicking its handle (None). Save the layout here.",
                    },
                    Prop {
                        name: "selection",
                        ty: "Option<Signal<BTreeSet<RowId>>>",
                        desc: "Adds a leading checkbox column: a tri-state select-all over the visible selectable rows, and a box per row with an id. Frozen with a pinned first column. Default None.",
                    },
                    Prop {
                        name: "on_reorder",
                        ty: "Option<Rc<dyn Fn(usize, usize)>>",
                        desc: "Adds a leading grip column and drag-to-reorder: (from, to) are ordinals over the draggable body rows; move_in_place applies them. Default None.",
                    },
                    Prop {
                        name: "empty",
                        ty: "Option<TableEmpty>",
                        desc: "What a table with no body rows says: one row under the header spanning every column past the added ones. Some(\"…\".into()) or any element. Default None.",
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
                    Prop {
                        name: "tone",
                        ty: "Reactive<RowTone>",
                        desc: "Whole-row tint: None (default), Highlight, Warning or Danger. Keeps the clickable-row hover and stays opaque on frozen columns. Applies to body cells.",
                    },
                    Prop {
                        name: "bind_to",
                        ty: "Option<Ref<ViewHandle>>",
                        desc: "Filled with the row's handle on mount (the <tr> on web, a row-spanning backdrop on native) — anchor a hover card or read the row frame. Default None.",
                    },
                    Prop {
                        name: "id",
                        ty: "Option<RowId>",
                        desc: "The row's identity — what Table(selection) holds. A row without one gets no checkbox. Default None.",
                    },
                    Prop {
                        name: "selectable",
                        ty: "bool",
                        desc: "Whether the row takes part in the selection column. Default true.",
                    },
                    Prop {
                        name: "draggable",
                        ty: "bool",
                        desc: "Whether the row can be dragged in a reorderable table (false for a group title). Default true.",
                    },
                    Prop {
                        name: "reorder_group",
                        ty: "Option<RowId>",
                        desc: "A draggable row lands only among rows of the same group. Default None (one group).",
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
                    Prop {
                        name: "pin_offset",
                        ty: "Option<f32>",
                        desc: "Inset from the pinned edge, px — set a second frozen column's to the summed widths of the pinned columns before it. Default None (at the edge).",
                    },
                    Prop {
                        name: "width",
                        ty: "Reactive<Option<f32>>",
                        desc: "Hold the column at exactly this width (px); spare width goes to unsized columns. Same value on every cell of the column. Default None.",
                    },
                    Prop {
                        name: "min_width",
                        ty: "Reactive<Option<f32>>",
                        desc: "Floor for the column's width (px). Default None.",
                    },
                    Prop {
                        name: "max_width",
                        ty: "Option<f32>",
                        desc: "Cap the column (px): content width up to the cap, or with flex, grow up to it. Switches the table to grid columns. Default None.",
                    },
                    Prop {
                        name: "flex",
                        ty: "Option<f32>",
                        desc: "Share of the width the other columns leave; may shrink to min_width (default 0) — pair with truncate. Switches the table to grid columns. Default None.",
                    },
                    Prop {
                        name: "align",
                        ty: "CellAlign",
                        desc: "Left (default), Center or Right — on the cell, its text, and rich children. Same value on every cell of the column.",
                    },
                    Prop {
                        name: "truncate",
                        ty: "bool",
                        desc: "One line ending in \"…\". Pair with `width` or `flex`. Default false.",
                    },
                    Prop {
                        name: "span",
                        ty: "ColSpan",
                        desc: "Columns covered: ColSpan::Columns(n), or ColSpan::Rest for every column to the end of the row. Default one column.",
                    },
                    Prop {
                        name: "padding",
                        ty: "CellPadding",
                        desc: "Standard (default) or Icon — the compact padding of a column of icons or icon buttons, such as a row menu.",
                    },
                    Prop {
                        name: "tone",
                        ty: "Reactive<CellTone>",
                        desc: "One value's text colour — None (default), Muted, Success, Warning, Danger (an overdue date). A whole row takes TableRow(tone).",
                    },
                    Prop {
                        name: "sort",
                        ty: "Reactive<Option<SortState>>",
                        desc: "Makes a header cell a sort control: Some(SortState::None) sortable, Some(Asc)/Some(Desc) sorted. Draws the direction; a press calls on_sort. Default None.",
                    },
                    Prop {
                        name: "on_sort",
                        ty: "Option<Rc<dyn Fn(SortState)>>",
                        desc: "Called with the sort a press asks for (SortState::next of the current). The sorting is yours.",
                    },
                ])
            }
        },
        ui! {
            Section(title = "TableColumn".to_string()) {
                P(content = "A column spec for `Table(columns = …)` and `TableCards(columns = …)`, built \
                    with `TableColumn::new(id)` and chained setters. `TableColumn::actions(id)` is the \
                    trailing row-menu column: fixed width, frozen right, compact, centred.".to_string())
                PropsTable(rows = vec![
                    Prop { name: "label", ty: "Option<String>", desc: "The column's name where the header row is not — card labels and the row detail sheet." },
                    Prop { name: "width", ty: "Option<f32>", desc: "Exact width unless the width map has an entry. None sizes to content." },
                    Prop { name: "min_width / max_width", ty: "Option<f32>", desc: "Floor and cap — also a resize drag's bounds. max_width lays the table out as grid columns." },
                    Prop { name: "fill", ty: "bool", desc: "Take the width the other columns leave (a dragged width becomes its floor). Lays the table out as grid columns." },
                    Prop { name: "resizable", ty: "bool", desc: "A header drag handle, revealed on hover; live resize of the whole column; double-click resets." },
                    Prop { name: "pinned", ty: "Option<ColumnPin>", desc: "Freeze against an edge; later frozen columns inset by the widths before them." },
                    Prop { name: "align / truncate / padding", ty: "CellAlign / bool / CellPadding", desc: "Applied to every cell of the column, header and body, and to the text inside." },
                    Prop { name: "card", ty: "CardSlot", desc: "Where the column goes in TableCards: Title, Opposite, Actions, Field (default), Detail or Hidden." },
                ])
            }
        },
        ui! {
            Section(title = "TablePager and TableCards".to_string()) {
                P(content = "`TablePager(offset, page_size, total, noun, on_offset, page_sizes, on_page_size)` — \
                    the paged table's footer: the range, a Rows picker and the page numbers. \
                    `TableCards(columns, row_count, row_id, cell, max_cells, on_row_click, \
                    on_end_reached, selection, empty)` — the narrow form: a virtualized list of \
                    cards with measured heights, laid out from the same column specs, loading the \
                    next page as the reader nears the end.".to_string())
            }
        },
    ])
}

/// Horizontal-scroll demo: enough columns to overflow the demo surface,
/// with the identifying column frozen left and the status column frozen
/// right — the two edges stay put while the middle scrolls beneath them.
/// Shown in both [`TableVariant`]s.
fn wide_table(frame: TableVariant) -> Element {
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
        Table(scroll_x = true, variant = frame) {
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

/// Pinned-header demo: a two-axis table bounded at 240px, its header row
/// pinned to the top and its first column frozen, so the corner cell pins
/// both ways.
fn pinned_header_table() -> Element {
    let regions = ["eu-west-1", "us-east-1", "ap-south-1", "us-west-2", "sa-east-1", "af-south-1"];
    ui! {
        Table(scroll_x = true, scroll_y = true, max_height = Some(240.0)) {
            TableRow {
                TableCell(header = true, text = Some("Service".to_string()), pinned = Some(ColumnPin::Left))
                TableCell(header = true, text = Some("Region".to_string()))
                TableCell(header = true, text = Some("Version".to_string()))
                TableCell(header = true, text = Some("Replicas".to_string()))
                TableCell(header = true, text = Some("p99 latency".to_string()))
                TableCell(header = true, text = Some("Error rate".to_string()))
            }
            for (i, region) in regions.into_iter().enumerate() {
                TableRow {
                    TableCell(text = Some(format!("service-{i}")), pinned = Some(ColumnPin::Left))
                    TableCell(text = Some(region.to_string()))
                    TableCell(text = Some(format!("v2.{i}.0")))
                    TableCell(text = Some(format!("{}", i + 2)))
                    TableCell(text = Some(format!("{}ms", 12 + i * 31)))
                    TableCell(text = Some(format!("0.0{i}%")))
                }
            }
        }
    }
}

/// Edge-inset demo: a plain table whose first and last cells take the
/// `xl` page margin while the dividers run edge to edge.
fn inset_table() -> Element {
    let crew = [("Ana Ruiz", "Day"), ("Bo Chen", "Night"), ("Cy Okafor", "Day")];
    ui! {
        Table(variant = TableVariant::Plain, inset_x = TableInset::Xl) {
            TableRow {
                TableCell(header = true, text = Some("Crew".to_string()))
                TableCell(header = true, text = Some("Shift".to_string()), align = CellAlign::Right)
            }
            for (name, shift) in crew {
                TableRow {
                    TableCell(text = Some(name.to_string()))
                    TableCell(text = Some(shift.to_string()), align = CellAlign::Right)
                }
            }
        }
    }
}

/// Column layout demo: sized + aligned + truncated columns around one
/// fill column, a toned clickable row, a full-row blank message, and a
/// footer totals row.
/// Flex-column demo: a long Name column fills the rest and truncates
/// while Type and Page keep their content width — the document-editor
/// field finder shape.
fn flex_columns_table() -> Element {
    let fields = [
        ("MG-05 Non-Routine Hazardous Task Authorisation — supervisor signature block", "Signature", "12"),
        ("Permit number", "Text", "1"),
        ("Isolation points verified by the competent person before work starts", "Checkbox", "4"),
        ("Gas test reading (ppm)", "Number", "4"),
    ];
    ui! {
        Table {
            TableRow {
                TableCell(header = true, flex = Some(1.0), truncate = true, text = Some("Name".to_string()))
                TableCell(header = true, text = Some("Type".to_string()))
                TableCell(header = true, align = CellAlign::Right, text = Some("Page".to_string()))
            }
            for (name, kind, page) in fields {
                TableRow {
                    TableCell(flex = Some(1.0), truncate = true, text = Some(name.to_string()))
                    TableCell(text = Some(kind.to_string()))
                    TableCell(align = CellAlign::Right, text = Some(page.to_string()))
                }
            }
        }
    }
}

fn columns_table() -> Element {
    let crew = [
        ("Ana Lucía Fernández-Whitmore", "Night shift lead", "On site", "11.5", RowTone::None),
        ("Bo Chen", "Flagged: missing sign-out", "Review", "9.0", RowTone::Warning),
        ("Chidi Okafor", "", "On site", "10.0", RowTone::None),
        ("Dana Ruiz", "Gas alarm, level 3", "Stopped", "6.5", RowTone::Danger),
    ];
    let status_tone = |s: &str| -> idea_ui::ToneRef {
        match s {
            "On site" => tone::Success.into(),
            "Review" => tone::Warning.into(),
            _ => tone::Danger.into(),
        }
    };
    ui! {
        Table {
            TableRow {
                TableCell(header = true, width = Some(150.0), truncate = true, text = Some("Crew".to_string()))
                TableCell(header = true, text = Some("Notes".to_string()))
                TableCell(header = true, width = Some(110.0), align = CellAlign::Center, text = Some("Status".to_string()))
                TableCell(header = true, width = Some(80.0), align = CellAlign::Right, text = Some("Hours".to_string()))
            }
            for (name, notes, status, hours, row_tone) in crew {
                TableRow(tone = row_tone, on_row_click = Some(Rc::new(|| {}) as Rc<dyn Fn()>)) {
                    TableCell(width = Some(150.0), truncate = true, text = Some(name.to_string()))
                    TableCell(text = Some(notes.to_string()))
                    TableCell(width = Some(110.0), align = CellAlign::Center) {
                        Tag(label = status.to_string(), tone = status_tone(status), variant = variant::Soft)
                    }
                    TableCell(width = Some(80.0), align = CellAlign::Right, text = Some(hours.to_string()))
                }
            }
            TableRow {
                TableCell(span = ColSpan::Rest, text = Some("No visitors signed in today.".to_string()))
            }
            TableFooter {
                TableRow {
                    TableCell(span = ColSpan::Columns(3), text = Some("Crew totals · 4".to_string()))
                    TableCell(width = Some(80.0), align = CellAlign::Right, text = Some("37.0".to_string()))
                }
            }
        }
    }
}

/// Frame slots + two stacked frozen columns + comfortable density in a
/// scroll-x table.
fn slots_table() -> Element {
    let days = ["Mon 13", "Tue 14", "Wed 15", "Thu 16", "Fri 17", "Sat 18", "Sun 19"];
    let people = [("Ana Fernández", "8 · 0 · 10 · 10 · 9 · 0 · 0"), ("Bo Chen", "10 · 0 · 8 · 8 · 8 · 6 · 0")];
    let header_slot = ui! {
        Typography(content = "Mon 13 is a public holiday — hours count double.".to_string(), kind = typography_kind::Caption, muted = true)
    };
    let add: Rc<dyn Fn()> = Rc::new(|| {});
    let footer_slot = ui! {
        Button(label = "Add entry".to_string(), on_click = add, tone = tone::Primary, variant = variant::Soft)
    };
    ui! {
        Table(
            scroll_x = true,
            density = TableDensity::Comfortable,
            header_slot = Some(header_slot),
            footer_slot = Some(footer_slot),
        ) {
            TableRow {
                TableCell(header = true, width = Some(48.0), pinned = Some(ColumnPin::Left), text = Some("#".to_string()))
                TableCell(header = true, width = Some(160.0), pinned = Some(ColumnPin::Left), pin_offset = Some(48.0), text = Some("Name".to_string()))
                for day in days {
                    TableCell(header = true, width = Some(96.0), align = CellAlign::Right, text = Some(day.to_string()))
                }
            }
            for (i, (name, hours)) in people.into_iter().enumerate() {
                TableRow {
                    TableCell(width = Some(48.0), pinned = Some(ColumnPin::Left), text = Some((i + 1).to_string()))
                    TableCell(width = Some(160.0), pinned = Some(ColumnPin::Left), pin_offset = Some(48.0), text = Some(name.to_string()))
                    for h in hours.split(" · ") {
                        TableCell(width = Some(96.0), align = CellAlign::Right, text = Some(h.to_string()))
                    }
                }
            }
        }
    }
}

/// One project row of the page-table demo.
#[derive(Clone, PartialEq)]
struct Project {
    id: String,
    name: String,
    client: &'static str,
    status: &'static str,
    due: String,
    overdue: bool,
    hours: f32,
    notes: &'static str,
}

/// Sixty generated projects — enough to page and to scroll.
fn projects() -> Vec<Project> {
    const NAMES: [&str; 12] = [
        "North portal", "Decline extension", "Vent raise 3", "Crusher reline", "Shaft collar",
        "Haul road", "Pump station", "Stope 14B", "Rehab drive", "Escapeway", "Magazine", "Workshop",
    ];
    const CLIENTS: [&str; 4] = ["Ridgeway Gold", "Kestrel Nickel", "Saltbush Coal", "Mount Iron"];
    const STATUSES: [&str; 3] = ["Active", "On hold", "Closing"];
    const NOTES: [&str; 3] = [
        "Ground support review booked for the night shift",
        "Awaiting the client's survey pickup before the next blast",
        "",
    ];
    (0..60)
        .map(|i| Project {
            id: format!("p{i}"),
            name: format!("{} {}", NAMES[i % NAMES.len()], i / NAMES.len() + 1),
            client: CLIENTS[i % CLIENTS.len()],
            status: STATUSES[i % STATUSES.len()],
            due: format!("{:02} Nov", 1 + (i * 7) % 28),
            overdue: i % 9 == 4,
            hours: 40.0 + ((i * 37) % 160) as f32 / 2.0,
            notes: NOTES[i % NOTES.len()],
        })
        .collect()
}

/// The page-table demo's columns — one list for both forms.
fn project_columns() -> Vec<TableColumn> {
    vec![
        TableColumn::new("name")
            .label("Project")
            .width(200.0)
            .min_width(120.0)
            .max_width(360.0)
            .resizable()
            .pinned(ColumnPin::Left)
            .card(CardSlot::Title),
        TableColumn::new("client").label("Client").width(150.0).resizable(),
        TableColumn::new("status").label("Status").card(CardSlot::Opposite),
        TableColumn::new("due").label("Due"),
        TableColumn::new("hours").label("Hours").width(90.0).align(CellAlign::Right),
        TableColumn::new("notes").label("Notes").fill().min_width(160.0).truncate(),
        TableColumn::actions("menu"),
    ]
}

/// One project cell, by column — the renderer both forms share.
fn project_cell(p: &Project, col: &ColumnId) -> Element {
    let status_tone = |s: &str| -> idea_ui::ToneRef {
        match s {
            "Active" => tone::Success.into(),
            "On hold" => tone::Warning.into(),
            _ => tone::Neutral.into(),
        }
    };
    match col.as_str() {
        "name" => ui! { Typography(content = p.name.clone(), kind = typography_kind::Body) },
        "client" => ui! { Typography(content = p.client.to_string(), kind = typography_kind::Body) },
        "status" => ui! { Tag(label = p.status.to_string(), tone = status_tone(p.status), variant = variant::Soft) },
        "due" => ui! { Typography(content = p.due.clone(), kind = typography_kind::Body, tone = p.overdue.then(|| tone::Danger.into())) },
        "hours" => ui! { Typography(content = format!("{:.1}", p.hours), kind = typography_kind::Body) },
        "notes" => ui! { Typography(content = p.notes.to_string(), kind = typography_kind::Body, muted = true) },
        _ => {
            let noop: Rc<dyn Fn()> = Rc::new(|| {});
            ui! { IconButton(icon = Some(ELLIPSIS_VERTICAL), on_click = noop, tone = tone::Neutral, variant = variant::Ghost) }
        }
    }
}

/// The page-table demo's state — owned above the table, so a page change,
/// a sort or a breakpoint crossing rebuilds the table without losing the
/// selection, the column widths or the landing.
#[derive(Clone, Copy)]
struct ProjectsPage {
    all: Signal<Vec<Project>>,
    widths: Signal<ColumnWidths>,
    last_resize: Signal<String>,
    selected: Signal<BTreeSet<RowId>>,
    opened: Signal<Option<String>>,
    sort: Signal<(&'static str, SortState)>,
    offset: Signal<usize>,
    page_size: Signal<usize>,
    /// The card form pages by growing: twenty more each time the reader
    /// nears the end.
    shown: Signal<usize>,
}

impl ProjectsPage {
    fn sorted(&self) -> Vec<Project> {
        let mut v = self.all.get();
        let (key, dir) = self.sort.get();
        match (key, dir) {
            (_, SortState::None) => {}
            ("hours", _) => v.sort_by(|a, b| a.hours.total_cmp(&b.hours)),
            _ => v.sort_by(|a, b| a.name.cmp(&b.name)),
        }
        if dir == SortState::Desc {
            v.reverse();
        }
        v
    }

    fn sort_of(&self, key: &'static str) -> runtime_core::Reactive<Option<SortState>> {
        let sort = self.sort;
        runtime_core::Reactive::Dynamic(Rc::new(move || {
            let (k, d) = sort.get();
            Some(if k == key { d } else { SortState::None })
        }))
    }

    fn sort_by(&self, key: &'static str) -> Option<Rc<dyn Fn(SortState)>> {
        let (sort, offset) = (self.sort, self.offset);
        Some(Rc::new(move |next| {
            sort.set((key, next));
            offset.set(0);
        }))
    }
}

/// The page-table demo: the wide form at `sm` and up, cards below.
fn page_table() -> Element {
    let page = ProjectsPage {
        all: signal(projects()),
        widths: signal(ColumnWidths::new()),
        last_resize: signal("(drag a header edge)".to_string()),
        selected: signal(BTreeSet::new()),
        opened: signal(Some("p3".to_string())),
        sort: signal(("name", SortState::None)),
        offset: signal(0),
        page_size: signal(10),
        shown: signal(20),
    };
    let (selected, last_resize, opened) = (page.selected, page.last_resize, page.opened);
    ui! {
        Stack(axis = StackAxis::Column, gap = StackGap::Sm) {
            Typography(
                kind = typography_kind::Caption,
                muted = true,
                content = rx!(format!(
                    "Selected: {} · last resize: {} · opened: {}",
                    selected.get().len(),
                    last_resize.get(),
                    opened.get().unwrap_or_default(),
                )),
            )
            // On a surface, as a plain table belongs: its frozen cells
            // paint `color-surface`.
            if current_breakpoint().get().is_at_least(Breakpoint::Sm) {
                Card(padding = CardPadding::None) { wide_projects(page.clone()) }
            } else {
                narrow_projects(page.clone())
            }
        }
    }
}

/// The wide form: one table per (sort, page) — the rows are rebuilt for
/// each page, the widths and the selection are not.
fn wide_projects(page: ProjectsPage) -> Element {
    let p = page.clone();
    ui! {
        match (page.sorted(), page.offset.get(), page.page_size.get()) {
            (rows, from, size) => {
                projects_table(p.clone(), rows.iter().skip(*from).take(*size).cloned().collect())
            }
        }
    }
}

/// One page of the wide form.
fn projects_table(page: ProjectsPage, rows: Vec<Project>) -> Element {
    let (offset, page_size, all, opened, last_resize) =
        (page.offset, page.page_size, page.all, page.opened, page.last_resize);
    let on_resize: Rc<dyn Fn(ColumnId, Option<f32>)> = Rc::new(move |id, w| {
        last_resize.set(match w {
            Some(w) => format!("{id} → {w}px"),
            None => format!("{id} reset"),
        })
    });
    let pager = ui! {
        TablePager(
            offset = offset,
            page_size = page_size,
            total = rx!(all.get().len()),
            noun = "projects".to_string(),
            on_offset = Rc::new(move |o| offset.set(o)) as Rc<dyn Fn(usize)>,
            page_sizes = vec![10, 25, 50],
            on_page_size = Some(Rc::new(move |n| { page_size.set(n); offset.set(0) }) as Rc<dyn Fn(usize)>),
        )
    };
    ui! {
        Table(
            scroll_x = true,
            scroll_y = true,
            max_height = Some(360.0),
            variant = TableVariant::Plain,
            inset_x = TableInset::by_breakpoint(TableInset::Px(16.0), TableInset::Px(24.0), TableInset::Px(28.0)),
            columns = project_columns(),
            column_widths = Some(page.widths),
            on_column_resize = Some(on_resize),
            selection = Some(page.selected),
            footer_slot = Some(pager),
        ) {
            TableRow {
                TableCell(header = true, text = Some("Project".to_string()), sort = page.sort_of("name"), on_sort = page.sort_by("name"))
                TableCell(header = true, text = Some("Client".to_string()))
                TableCell(header = true, text = Some("Status".to_string()))
                TableCell(header = true, text = Some("Due".to_string()))
                TableCell(header = true, text = Some("Hours".to_string()), sort = page.sort_of("hours"), on_sort = page.sort_by("hours"))
                TableCell(header = true, text = Some("Notes".to_string()))
                TableCell(header = true)
            }
            for p in rows {
                TableRow(
                    id = Some(RowId::from(p.id.as_str())),
                    tone = {
                        let id = p.id.clone();
                        rx!(if opened.get().as_deref() == Some(id.as_str()) { RowTone::Highlight } else { RowTone::None })
                    },
                    on_row_click = {
                        let id = p.id.clone();
                        Some(Rc::new(move || opened.set(Some(id.clone()))) as Rc<dyn Fn()>)
                    },
                ) {
                    TableCell(text = Some(p.name.clone()))
                    TableCell(text = Some(p.client.to_string()))
                    TableCell { project_cell(&p, &ColumnId::from("status")) }
                    TableCell(text = Some(p.due.clone()), tone = if p.overdue { CellTone::Danger } else { CellTone::None })
                    TableCell(text = Some(format!("{:.1}", p.hours)))
                    TableCell(text = Some(p.notes.to_string()), tone = CellTone::Muted)
                    TableCell { project_cell(&p, &ColumnId::from("menu")) }
                }
            }
        }
    }
}

/// The narrow form: the same column specs and cell renderer, as cards.
fn narrow_projects(page: ProjectsPage) -> Element {
    let (all, shown, opened) = (page.all, page.shown, page.opened);
    let rows = page.clone();
    let cell: Rc<dyn Fn(usize, &ColumnId) -> Element> = Rc::new(move |i, col| {
        match rows.sorted().get(i) {
            Some(p) => project_cell(p, col),
            None => ui! { view {} },
        }
    });
    let ids = page.clone();
    let row_id: Rc<dyn Fn(usize) -> RowId> = Rc::new(move |i| {
        ids.sorted().get(i).map(|p| RowId::from(p.id.as_str())).unwrap_or_else(|| RowId::from(i))
    });
    let open = page.clone();
    let on_open: Rc<dyn Fn(usize)> = Rc::new(move |i| opened.set(open.sorted().get(i).map(|p| p.id.clone())));
    let more: Rc<dyn Fn()> = Rc::new(move || shown.set((shown.get() + 20).min(all.get().len())));
    ui! {
        view(style = phone_frame()) {
            TableCards(
                columns = project_columns(),
                row_count = rx!(shown.get().min(all.get().len())),
                row_id = row_id,
                cell = cell,
                selection = Some(page.selected),
                on_row_click = Some(on_open),
                on_end_reached = Some(more),
            )
        }
    }
}

/// The card form's bounded box in the demo — the list virtualizes inside
/// it, as it would inside a phone screen's body.
fn phone_frame() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_| StyleRules {
        height: Some(runtime_core::Tokenized::Literal(runtime_core::Length::Px(520.0))),
        ..Default::default()
    }))
}

/// Built-in reorder demo: two groups of tasks in a scroll-x table with a
/// frozen task column. Group titles do not drag; tasks land only within
/// their own group.
fn reorder_table() -> Element {
    let tasks: Signal<Vec<(&'static str, &'static str, &'static str)>> = signal(vec![
        ("t1", "Today", "Ship hotfix"),
        ("t2", "Today", "Review PR #412"),
        ("t3", "Today", "Update changelog"),
        ("t4", "This week", "Cut release"),
        ("t5", "This week", "Plan the retro"),
        ("t6", "This week", "Rotate on-call"),
    ]);
    let reorder: Rc<dyn Fn(usize, usize)> = Rc::new(move |from, to| {
        let mut v = tasks.get();
        if move_in_place(&mut v, from, to) {
            tasks.set(v);
        }
    });
    ui! {
        match tasks.get() {
            list => {
                Table(scroll_x = true, on_reorder = Some(reorder.clone())) {
                    TableRow {
                        TableCell(header = true, text = Some("Task".to_string()), width = Some(200.0), pinned = Some(ColumnPin::Left))
                        TableCell(header = true, text = Some("Owner".to_string()))
                        TableCell(header = true, text = Some("Estimate".to_string()))
                        TableCell(header = true, text = Some("Board".to_string()))
                        TableCell(header = true, text = Some("Notes".to_string()))
                    }
                    for group in ["Today", "This week"] {
                        TableRow(draggable = false) {
                            TableCell(span = ColSpan::Rest, text = Some(group.to_string()), tone = CellTone::Muted)
                        }
                        for (id, _, name) in list.iter().filter(|t| t.1 == group).cloned() {
                            TableRow(id = Some(RowId::from(id)), reorder_group = Some(RowId::from(group))) {
                                TableCell(text = Some(name.to_string()), width = Some(200.0), pinned = Some(ColumnPin::Left))
                                TableCell(text = Some("Nicho".to_string()))
                                TableCell(text = Some("2h".to_string()))
                                TableCell(text = Some("Release train".to_string()))
                                TableCell(text = Some("Drag the grip — the task column stays frozen while you scroll sideways".to_string()))
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Empty-state demo: a selectable table whose rows a button clears.
fn empty_table() -> Element {
    let people = signal(vec!["Ana Ruiz", "Bo Chen"]);
    let selected: Signal<BTreeSet<RowId>> = signal(BTreeSet::new());
    let clear: Rc<dyn Fn()> = Rc::new(move || people.set(Vec::new()));
    let refill: Rc<dyn Fn()> = Rc::new(move || people.set(vec!["Ana Ruiz", "Bo Chen"]));
    ui! {
        Stack(axis = StackAxis::Column, gap = StackGap::Sm) {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                Button(label = "Sign everyone out".to_string(), on_click = clear, tone = tone::Neutral, variant = variant::Soft)
                Button(label = "Sign them back in".to_string(), on_click = refill, tone = tone::Neutral, variant = variant::Soft)
            }
            match people.get() {
                list => {
                    Table(selection = Some(selected), empty = Some("No one is signed in yet.".into())) {
                        TableRow {
                            TableCell(header = true, text = Some("Name".to_string()))
                            TableCell(header = true, text = Some("Signed in".to_string()))
                            TableCell(header = true, text = Some("Site".to_string()))
                        }
                        for name in list.iter().copied() {
                            TableRow(id = Some(RowId::from(name))) {
                                TableCell(text = Some(name.to_string()))
                                TableCell(text = Some("06:12".to_string()))
                                TableCell(text = Some("North portal".to_string()))
                            }
                        }
                    }
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

