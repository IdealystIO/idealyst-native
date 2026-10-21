//! Regression: a `Table`'s header (`<th>`) cells must left-align like its
//! body (`<td>`) cells.
//!
//! The browser UA stylesheet defaults `th { text-align: center }`. Each
//! idea-ui cell wraps its text in a shrink-wrapped inline span, so that
//! span's own `text_align: Left` can't win — the *cell's* alignment is
//! what positions the inline span. The fix pins `text_align: Left` on the
//! cell stylesheet (`head_cell`), not just the inner text node. This
//! test asserts both head and body cells resolve to the same Left
//! alignment so they can't drift apart again.
//!
//! Bug it guards: catalog-docs props tables rendered centered headers
//! over left-aligned body cells.

use idea_ui::TableStyles;

use runtime_core::{resolve_style, StyleApplication, TextAlign};

/// The STATIC sheet application off a builder (these cell sheets are
/// all-constant).
fn static_app(b: impl runtime_core::IntoStyleSource) -> StyleApplication {
    match b.into_style_prop() {
        runtime_vocabulary::StyleProp::Sheet(app) => *app,
        _ => panic!("table cell stylesheets are all-constant → static"),
    }
}

fn resolved_text_align(app: StyleApplication) -> Option<TextAlign> {
    resolve_style(&app).text_align
}

#[test]
fn head_and_body_cells_share_left_text_align() {
    let head = resolved_text_align(static_app(TableStyles::head_cell()));
    let body = resolved_text_align(static_app(TableStyles::body_cell()));

    assert_eq!(
        head,
        Some(TextAlign::Left),
        "header cell must pin text-align Left to override the UA `th` center default"
    );
    assert_eq!(body, Some(TextAlign::Left), "body cell must be left-aligned");
    assert_eq!(head, body, "header and body cell alignment must stay consistent");
}
