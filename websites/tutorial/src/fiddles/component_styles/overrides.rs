use idea_ui::{install_idea_theme, light_theme, IdeaThemeRef};
use runtime_core::{derived, signal, stylesheet, ui, Element};

stylesheet! {
    pub Card<IdeaThemeRef> {
        base(t) {
            background: t.color.surface_alt(),
            border_width: 2.0,
            border_color: t.color.border(),
        }
        variant selected {
            #[default]
            off(_t) {}
            on(t) { border_color: t.intent.primary.solid_bg() }
        }
        variant urgent {
            #[default]
            no(_t) {}
            yes(t) { background: t.intent.danger.soft_bg() }
        }
        // Applies only when BOTH hold, after every single-axis arm.
        compound (selected: on, urgent: yes)(t) {
            border_color: t.intent.danger.solid_bg()
        }
        // Values no variant could list in advance. Each line generates a
        // setter on the builder: `Card().padding(12.0)`.
        override padding: f32
        override border_radius: f32
    }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());
    let padding = signal(16.0_f32);
    let radius = signal(8.0_f32);
    let selected = signal(false);
    let urgent = signal(false);

    // Setters take signals as well as values. Each one the style reads
    // re-applies it when it changes — every slider drag and toggle flip.
    let card = Card()
        .selected(derived(move || if selected.get() { CardSelected::On } else { CardSelected::Off }))
        .urgent(derived(move || if urgent.get() { CardUrgent::Yes } else { CardUrgent::No }))
        .padding(padding)
        .border_radius(radius);

    ui! {
        view {
            view(style = card) {
                text { "Padding {padding}, radius {radius}" }
            }
            text { "Padding" }
            slider(value = padding, min = 0.0, max = 48.0, on_change = move |v| padding.set(v))
            text { "Corner radius" }
            slider(value = radius, min = 0.0, max = 32.0, on_change = move |v| radius.set(v))
            text { "Selected" }
            toggle(value = selected, on_change = move |v| selected.set(v))
            text { "Urgent" }
            toggle(value = urgent, on_change = move |v| urgent.set(v))
        }
    }
}
