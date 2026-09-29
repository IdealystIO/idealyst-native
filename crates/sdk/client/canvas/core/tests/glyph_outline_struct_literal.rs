//! Its own process: the one way around the outliner's install is a
//! `FontResource` struct literal (the fields are public). That draws no text
//! — with a one-time warning — rather than panicking or drawing garbage.

use std::sync::Arc;

use canvas_core::{expand_glyph_run, Color, FontResource, Paint, PositionedGlyph, Transform};

const INTER: &[u8] = include_bytes!("../../../../../gpu-backend/engine/assets/fonts/Inter-Regular.ttf");

#[test]
fn a_struct_literal_font_draws_no_text() {
    let font = FontResource { id: 1, index: 0, data: Arc::new(INTER.to_vec()) };
    let ops = expand_glyph_run(
        &font,
        &[PositionedGlyph::new(3, Transform::IDENTITY)],
        &Paint::solid(Color::new(0, 0, 0, 255)),
    );
    assert!(ops.is_empty());
}
