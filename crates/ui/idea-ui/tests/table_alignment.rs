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

// The new-core alias: same-source `runtime_core::…` paths in this test
// resolve against the glue facade (see idea-ui's lib.rs note).
#[cfg(feature = "new-core")]
extern crate runtime_facade as runtime_core;

use runtime_core::{resolve_style, StyleApplication, TextAlign};

/// The STATIC sheet application off a builder (these cell sheets are
/// all-constant). The conversion trait's method name differs per core —
/// the one fork (same shape as tests/theme_token_stylesheet.rs).
#[cfg(not(feature = "new-core"))]
fn static_app(b: impl runtime_core::IntoStyleSource) -> StyleApplication {
    match b.into_style_source() {
        runtime_core::StyleSource::Static(app) => app,
        _ => panic!("table cell stylesheets are all-constant → static"),
    }
}

#[cfg(feature = "new-core")]
fn static_app(b: impl runtime_core::IntoStyleSource) -> StyleApplication {
    match b.into_style_prop() {
        runtime_vocabulary::StyleProp::Sheet(app) => app,
        _ => panic!("table cell stylesheets are all-constant → static"),
    }
}

fn resolved_text_align(app: StyleApplication) -> Option<TextAlign> {
    resolve_style(&app).text_align
}

#[test]
fn head_and_body_cells_share_left_text_align() {
    let head = resolved_text_align(static_app(TableHeadCell()));
    let body = resolved_text_align(static_app(TableBodyCell()));

    assert_eq!(
        head,
        Some(TextAlign::Left),
        "header cell must pin text-align Left to override the UA `th` center default"
    );
    assert_eq!(body, Some(TextAlign::Left), "body cell must be left-aligned");
    assert_eq!(head, body, "header and body cell alignment must stay consistent");
}
