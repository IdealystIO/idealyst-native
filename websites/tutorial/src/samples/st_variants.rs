use runtime_core::{stylesheet, Color, Tokenized};
use idea_ui::IdeaThemeRef;

stylesheet! {
    pub Btn<IdeaThemeRef> {
        base(_t) { padding: 12.0, border_radius: 8.0 }
        variant tone {
            #[default]
            neutral(t) {
                background: t.color.surface_alt(),
            }
            primary(t) {
                background: t.intent.primary.solid_bg(),
            }
        }
        state hovered(t) {
            background: t.color.surface(),
        }
    }
}

// Pick a variant at the call site. `BtnTone` is generated from the axis.
fn primary() -> Btn {
    Btn().tone(BtnTone::Primary)
}
