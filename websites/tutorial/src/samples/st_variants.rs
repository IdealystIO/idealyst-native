use runtime_core::{stylesheet, Color, Tokenized};

stylesheet! {
    pub Btn<()> {
        base(_t) { padding: 12.0, border_radius: 8.0 }
        variant tone {
            #[default]
            neutral(_t) {
                background: Tokenized::token("color-surface-alt", Color("#eeeeee".into())),
            }
            primary(_t) {
                background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
            }
        }
        state hovered(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
        }
    }
}

// Pick a variant at the call site. `BtnTone` is generated from the axis.
fn primary() -> Btn {
    Btn().tone(BtnTone::Primary)
}
