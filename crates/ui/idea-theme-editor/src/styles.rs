//! The editor's own sheets. Themed like everything else, so the panel
//! re-tints along with the app it is editing.

use idea_theme::theme::IdeaThemeRef;
use runtime_core::stylesheet;

// `Swatch` — the color preview beside a color row.
//
// The tint itself rides the INLINE layer (a `Tokenized::token`
// reference to the row's own token), not this sheet: one class serves
// every swatch, and the color still comes from the live registry. See
// `editor::ColorSwatch`.
stylesheet! {
    pub Swatch<IdeaThemeRef> {
        base(t) {
            width: 28.0,
            height: 28.0,
            // Swatches sit in a row beside a field that can grow; without
            // this the preview is the first thing the row squeezes.
            flex_shrink: 0.0,
            border_radius: t.radius.sm(),
            border_width: 1.0,
            border_color: t.color.border(),
        }
        // Marks a row whose text doesn't parse. A border swap rather
        // than an extra element: the panel is a long column of rows, and
        // an error marker that changes a row's box reflows every row
        // below it on each keystroke.
        variant invalid {
            #[default]
            off(_t) {}
            on(t) {
                border_width: 2.0,
                border_color: t.intent.danger.fg(),
            }
        }
    }
}
