//! Real layout of the table's column model on the native grid lowering
//! (`tests/common`: host-mock tree mirrored into Taffy).
//!
//! What the unit tests in `components/table/tests.rs` pin as styles, this
//! pins as geometry: one column spec puts the header and the body cells
//! of a column in one track; a saved width moves the whole column; the
//! empty-state row starts past the columns the table added and spans the
//! author's.

mod common;

use std::collections::BTreeSet;

use common::lay_out;
use idea_ui::{ColumnId, ColumnWidths, Table, TableCell, TableColumn, TableRow, SELECT_COLUMN_WIDTH_PX};
use runtime_core::{signal, ui, Element};

fn crew(widths: Option<runtime_core::Signal<ColumnWidths>>) -> Element {
    let columns = vec![TableColumn::new("name").width(200.0).resizable(), TableColumn::new("notes").fill()];
    ui! {
        Table(columns = columns, column_widths = widths) {
            TableRow {
                TableCell(header = true, text = Some("Name".to_string()))
                TableCell(header = true, text = Some("Notes".to_string()))
            }
            TableRow {
                TableCell(text = Some("Ana".to_string()))
                TableCell(text = Some("Night shift".to_string()))
            }
        }
    }
}

/// One spec, one track: the header and body cells of each column start
/// at the same x, and the fixed column is exactly its width (the second
/// column's text starts one column width plus the cells' padding in).
#[test]
fn one_column_spec_puts_header_and_body_in_one_track() {
    let laid = lay_out(800.0, 600.0, || crew(None));
    let (name, ana) = (laid.frame("text \"Name"), laid.frame("text \"Ana"));
    let (notes, night) = (laid.frame("text \"Notes"), laid.frame("text \"Night"));
    assert_eq!(name.x, ana.x, "column 1 lines up:\n{}", laid.h.tree(laid.root));
    assert_eq!(notes.x, night.x, "column 2 lines up");
    let pad = name.x;
    assert!((notes.x - (200.0 + pad)).abs() < 0.5, "the name column is 200 wide (notes at {})", notes.x);
}

/// A saved width (the screen's width map) moves the whole column.
#[test]
fn a_saved_width_moves_the_whole_column() {
    let laid = lay_out(800.0, 600.0, || {
        let mut saved = ColumnWidths::new();
        saved.insert(ColumnId::from("name"), 260.0);
        crew(Some(signal(saved)))
    });
    let pad = laid.frame("text \"Name").x;
    assert!((laid.frame("text \"Notes").x - (260.0 + pad)).abs() < 0.5);
    assert!((laid.frame("text \"Night").x - (260.0 + pad)).abs() < 0.5);
}

/// The empty-state row starts past the selection column — its message
/// lines up with the first author column — and runs to the table's end.
#[test]
fn empty_state_lines_up_with_the_first_author_column() {
    let laid = lay_out(800.0, 600.0, || {
        let selection = signal(BTreeSet::new());
        ui! {
            Table(selection = Some(selection), empty = Some("Nobody yet".into())) {
                TableRow {
                    TableCell(header = true, text = Some("Name".to_string()))
                    TableCell(header = true, text = Some("Role".to_string()))
                    TableCell(header = true, text = Some("Hours".to_string()))
                }
            }
        }
    });
    let name = laid.frame("text \"Name");
    let message = laid.frame("text \"Nobody");
    assert_eq!(message.x, name.x, "the message starts where the first author column does");
    assert!(name.x > SELECT_COLUMN_WIDTH_PX, "past the selection column");
}
