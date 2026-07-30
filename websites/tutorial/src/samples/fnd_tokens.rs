use idea_ui::{dark_theme, set_idea_theme};
use runtime_core::{stylesheet, Color, FlexDirection, Tokenized};

// A rule stores a token REFERENCE, not a concrete value. Resolving it is
// a signal read, so this style subscribes to exactly `color-surface`.
stylesheet! {
    pub Card<()> {
        base(_t) {
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            flex_direction: FlexDirection::Column,
            padding: 16.0,
        }
    }
}

fn go_dark() {
    // Rewrites the token signals. At the flush, every node that resolved a
    // changed token re-applies its style; nodes that read none stay asleep.
    set_idea_theme(dark_theme());
}
