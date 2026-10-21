//! A `fill` column in a `Table(scroll_x = true)` takes the width the
//! scroller leaves and truncates, with its `min_width` as its only floor:
//! the table overflows (and scrolls) only once the other columns plus
//! that floor exceed the scroller.
//!
//! CrewForge (2026-10-21) hit the opposite in the browser: the `<table>`
//! took `width: max-content`, and an `fr` track's max-content contribution
//! is its content, so one long name widened the table past the page
//! (`434.516px 180px 168px 168px 40px` on a 945px parent). That half is
//! pinned in a real browser by `backend-web`'s
//! `table_layout_tests` (the `regression_fill_column_in_a_*` tests).
//! This file pins the same geometry on the native grid lowering (`tests/common`:
//! host-mock tree mirrored into Taffy), so the two lowerings keep agreeing.

mod common;

use common::{lay_out, Laid, CHAR_ADVANCE};
use idea_ui::{ColumnPin, Table, TableCell, TableColumn, TableRow};
use runtime_core::{ui, Element};

const LONG_NAME: &str =
    "Shotcrete crew lead, night shift, north decline, ventilation raise and refuge bay";

fn crew(pin_name: bool) -> Element {
    let name = TableColumn::new("name").fill().truncate().min_width(120.0);
    let name = if pin_name { name.pinned(ColumnPin::Left) } else { name };
    let columns = vec![
        name,
        TableColumn::new("role").width(180.0),
        TableColumn::new("start").width(168.0),
        TableColumn::new("end").width(168.0),
        TableColumn::actions("menu"),
    ];
    ui! {
        Table(scroll_x = true, columns = columns) {
            TableRow {
                TableCell(header = true, text = Some("Name".to_string()))
                TableCell(header = true, text = Some("Role".to_string()))
                TableCell(header = true, text = Some("Start".to_string()))
                TableCell(header = true, text = Some("End".to_string()))
                TableCell(header = true)
            }
            TableRow {
                TableCell(text = Some(LONG_NAME.to_string()))
                TableCell(text = Some("Lead".to_string()))
                TableCell(text = Some("06:00".to_string()))
                TableCell(text = Some("18:00".to_string()))
                TableCell(text = Some("...".to_string()))
            }
        }
    }
}

/// The name cell (the body text's cell) and the row's last cell.
fn edges(laid: &Laid) -> (f32, f32) {
    let name = laid.frame("text \"Shotcrete");
    let menu = laid.frame("text \"...");
    (name.width, menu.x + menu.width)
}

/// At the reported 945px the five columns fit — 180 + 168 + 168 + 52
/// fixed plus the name's 120 floor — so nothing overflows: the name takes
/// what is left and is cut short of its full line.
#[test]
fn regression_fill_column_in_scroll_x_takes_the_rest_and_truncates() {
    let laid = lay_out(945.0, 600.0, || crew(false));
    let (name_w, row_end) = edges(&laid);
    let full = LONG_NAME.chars().count() as f32 * CHAR_ADVANCE;
    assert!(
        row_end <= 945.0 + 0.5,
        "the row ends inside the 945px scroller (at {row_end}):\n{}",
        laid.h.tree(laid.root)
    );
    assert!(name_w < full, "the long name is cut to the space left ({name_w} of {full})");
    assert!(name_w > 120.0, "and takes more than its floor at this width ({name_w})");
}

/// On a phone the fixed columns alone overflow the scroller: the frozen
/// fill column drops to its `min_width` floor (plus padding) and the rest
/// scroll beneath it — it never grows to its content.
#[test]
fn regression_frozen_fill_column_on_a_phone_shrinks_to_its_floor() {
    let laid = lay_out(390.0, 800.0, || crew(true));
    let (name_w, row_end) = edges(&laid);
    assert!(
        name_w <= 120.0 + 0.5,
        "the frozen name column sits at its floor, not its content ({name_w}):\n{}",
        laid.h.tree(laid.root)
    );
    assert!(row_end > 390.0, "the fixed columns overflow, so the table scrolls");
}
