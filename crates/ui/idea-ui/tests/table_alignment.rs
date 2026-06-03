//! Regression: a `Table`'s header (`<th>`) cells must left-align like its
//! body (`<td>`) cells.
//!
//! The browser UA stylesheet defaults `th { text-align: center }`. Each
//! idea-ui cell wraps its text in a shrink-wrapped inline span, so that
//! span's own `text_align: Left` can't win — the *cell's* alignment is
//! what positions the inline span. The fix pins `text_align: Left` on the
//! cell stylesheet (`TableHeadCell`), not just the inner text node. This
//! test asserts both head and body cells resolve to the same Left
//! alignment so they can't drift apart again.
//!
//! Bug it guards: catalog-docs props tables rendered centered headers
//! over left-aligned body cells.

use idea_ui::stylesheets::{TableBodyCell, TableHeadCell};
use runtime_core::{resolve_style, IntoStyleSource, StyleSource, TextAlign};

/// Resolve a stylesheet builder to its `text_align`, expecting the
/// constant (non-reactive) `Static` source these cell sheets produce.
fn resolved_text_align(src: StyleSource) -> Option<TextAlign> {
    match src {
        StyleSource::Static(app) => resolve_style(&app).text_align,
        _ => panic!("table cell stylesheets are all-constant → StyleSource::Static"),
    }
}

#[test]
fn head_and_body_cells_share_left_text_align() {
    let head = resolved_text_align(TableHeadCell().into_style_source());
    let body = resolved_text_align(TableBodyCell().into_style_source());

    assert_eq!(
        head,
        Some(TextAlign::Left),
        "header cell must pin text-align Left to override the UA `th` center default"
    );
    assert_eq!(body, Some(TextAlign::Left), "body cell must be left-aligned");
    assert_eq!(head, body, "header and body cell alignment must stay consistent");
}
