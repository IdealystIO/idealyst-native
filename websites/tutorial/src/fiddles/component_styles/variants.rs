use idea_ui::{install_idea_theme, light_theme, IdeaThemeRef};
use runtime_core::{component, derived, signal, stylesheet, ui, Element, FlexDirection};

// One stylesheet, two variant axes. `#[default]` marks the value used
// when the call site doesn't pick one.
stylesheet! {
    pub Badge<IdeaThemeRef> {
        base(t) {
            padding_vertical: t.spacing.xs(),
            padding_horizontal: t.spacing.sm(),
            border_radius: t.radius.pill(),
        }
        variant tone {
            #[default]
            neutral(t) { background: t.intent.neutral.soft_bg(), color: t.intent.neutral.soft_text() }
            primary(t) { background: t.intent.primary.soft_bg(), color: t.intent.primary.soft_text() }
            danger(t) { background: t.intent.danger.solid_bg(), color: t.intent.danger.solid_text() }
        }
        variant size {
            #[default]
            regular(_t) { font_size: 14.0 }
            large(_t) { font_size: 20.0 }
        }
    }
}

stylesheet! {
    pub Row<IdeaThemeRef> {
        base(t) { flex_direction: FlexDirection::Row, gap: t.spacing.sm(), padding: t.spacing.md() }
    }
}

// A component whose prop picks the variant. `BadgeTone` is the enum the
// macro generated from `variant tone`; like any prop, `tone` takes a
// plain value or a signal.
#[component]
fn Pill(label: String, tone: BadgeTone) -> Element {
    ui! { text(style = Badge().tone(tone)) { label } }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());
    let count = signal(0);

    ui! {
        view {
            view(style = Row()) {
                Pill(label = "Draft", tone = BadgeTone::Neutral)
                Pill(label = "New", tone = BadgeTone::Primary)
                Pill(label = "Failed", tone = BadgeTone::Danger)
            }
            view(style = Row()) {
                // A variant can follow state: `derived` re-picks it
                // whenever the signals it reads change.
                text(style = Badge()
                    .size(BadgeSize::Large)
                    .tone(derived(move || if count.get() >= 3 { BadgeTone::Danger } else { BadgeTone::Primary }))
                ) { "{count} unread" }
                button(label = "Add one", on_click = move || count.update(|n| n + 1))
            }
        }
    }
}
