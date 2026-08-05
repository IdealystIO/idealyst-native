use runtime_core::{stylesheet, Color, FlexDirection, Length, Tokenized};

stylesheet! {
    pub Panel<()> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_radius: Tokenized::token("radius-md", Length::Px(8.0)),
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
