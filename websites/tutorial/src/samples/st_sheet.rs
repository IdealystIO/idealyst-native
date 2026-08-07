use runtime_core::{stylesheet, Color, FlexDirection, Length, Tokenized};
use idea_ui::IdeaThemeRef;

stylesheet! {
    pub Panel<IdeaThemeRef> {
        base(t) {
            background: t.color.surface(),
            border_radius: t.radius.md(),
            padding: 16.0, // bare literal — auto-wrapped
            flex_direction: FlexDirection::Column,
        }
    }
}

// The macro generates `Panel()` (a style source for a view), `Panel::sheet()`,
// and the convention-named `panel_style()` — both return the cached sheet.
fn cached() -> std::rc::Rc<runtime_core::StyleSheet> {
    panel_style()
}
