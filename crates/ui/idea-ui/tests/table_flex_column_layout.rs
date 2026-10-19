//! Regression (reported against idea-ui 8.0.0): a table column could not
//! "take the width the other columns leave, then truncate". The
//! document editor's input finder (Name | Type | Page) had long names
//! push Type and Page off the right edge; a fixed width on Name wasted
//! space when wide and still clipped when narrow.
//!
//! `TableCell(flex = Some(1.0), truncate = true)` on the Name column lays
//! the table out as grid columns: Name `minmax(0, 1fr)`, Type and Page
//! `auto`. Real layout (`tests/common`: host-mock tree mirrored into
//! Taffy — the native grid lowering, which runs plain CSS grid sizing
//! once a column is not `auto`), at the report's narrow and wide widths.
//!
//! The report's failure was a browser one: a one-line cell keeps a
//! `<table>` column at its full width. Native's auto-table pass already
//! shrank that column (it sizes from max-content alone), so this test
//! pins the feature on native rather than reproducing the bug; the web
//! lowering's grid markup — the half that failed — is pinned in the
//! `table` SDK's `tests/scene.rs`.

mod common;

use common::{lay_out, Laid, CHAR_ADVANCE};
use idea_ui::{Table, TableCell, TableRow};
use runtime_core::{ui, Element};

const LONG_NAME: &str =
    "MG-05 Non-Routine Hazardous Task Authorisation supervisor signature block";

fn finder() -> Element {
    let name_flex = Some(1.0);
    ui! {
        Table {
            TableRow {
                TableCell(header = true, text = Some("Name".to_string()), flex = name_flex, truncate = true)
                TableCell(header = true, text = Some("Type".to_string()))
                TableCell(header = true, text = Some("Page".to_string()))
            }
            TableRow {
                TableCell(text = Some(LONG_NAME.to_string()), flex = name_flex, truncate = true)
                TableCell(text = Some("Signature".to_string()))
                TableCell(text = Some("12".to_string()))
            }
        }
    }
}

fn text_width(s: &str) -> f32 {
    s.chars().count() as f32 * CHAR_ADVANCE
}

fn check(laid: &Laid, pane: f32) {
    let name = laid.frame("text \"MG-05");
    let kind = laid.frame("text \"Signature");
    let page = laid.frame("text \"12");
    // A content-sized column is as wide as its widest cell: the
    // "Signature" value, and the "Page" header.
    assert_eq!(
        kind.width,
        text_width("Signature"),
        "at {pane}px Type keeps its content width:\n{}",
        laid.h.tree(laid.root)
    );
    assert_eq!(page.width, text_width("Page"), "at {pane}px Page keeps its content width");
    assert!(
        page.x + page.width <= pane + 0.01,
        "at {pane}px Page stays inside the table (ends at {}):\n{}",
        page.x + page.width,
        laid.h.tree(laid.root)
    );
    assert!(
        name.width < text_width(LONG_NAME),
        "at {pane}px the long name is cut to the space left ({} of {})",
        name.width,
        text_width(LONG_NAME)
    );
    assert!(name.x + name.width <= kind.x + 0.01, "the name ends before Type");
}

#[test]
fn flex_column_fills_and_truncates_at_phone_width() {
    let laid = lay_out(390.0, 800.0, || finder());
    check(&laid, 390.0);
}

#[test]
fn flex_column_fills_and_truncates_at_desktop_width() {
    // 600px: still narrower than the long name, wide enough that a
    // fixed Name width would have been the report's "wasted space" case.
    let laid = lay_out(600.0, 800.0, || finder());
    check(&laid, 600.0);
    // And Name takes all the rest: Type sits right after it, Page ends the row.
    let page = laid.frame("text \"12");
    assert!(page.x + page.width > 600.0 - 40.0, "Page sits at the row's end, not mid-table");
}
