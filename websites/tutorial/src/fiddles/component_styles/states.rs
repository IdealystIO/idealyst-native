use idea_ui::{install_idea_theme, light_theme, IdeaThemeRef};
use runtime_core::{
    signal, stylesheet, ui, AlignItems, Color, Element, FlexDirection, FontWeight, Transform,
};

stylesheet! {
    pub Action<IdeaThemeRef> {
        base(t) {
            padding_vertical: t.spacing.sm(),
            padding_horizontal: t.spacing.lg(),
            border_radius: t.radius.md(),
            background: t.intent.primary.solid_bg(),
            color: t.intent.primary.solid_text(),
            font_weight: FontWeight::SemiBold,
            // A transparent border at rest, so the focus ring below
            // doesn't change the button's size.
            border_width: 2.0,
            border_color: Color("transparent".into()),
        }
        // Each block merges over the base while its state is on.
        state hovered(t) { background: t.intent.primary.fg() }
        state pressed(_t) { transform: vec![Transform::Scale(0.96)] }
        state focused(t) { border_color: t.color.focus_ring() }
        // `disabled` switches the other three off.
        state disabled(t) { background: t.color.border(), color: t.color.text_muted() }
        // Animate the change instead of snapping to it.
        transitions {
            background: 150ms EaseOut,
            transform: 100ms EaseOut,
        }
    }
}

stylesheet! {
    pub Column<IdeaThemeRef> {
        base(t) { gap: t.spacing.md(), padding: t.spacing.lg(), align_items: AlignItems::FlexStart }
    }
}

stylesheet! {
    pub Row<IdeaThemeRef> {
        base(t) { flex_direction: FlexDirection::Row, gap: t.spacing.sm(), align_items: AlignItems::Center }
    }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());
    let presses = signal(0);
    let off = signal(false);

    ui! {
        view(style = Column()) {
            // Hover it, press it, and Tab to it from the keyboard.
            button(
                label = "Save changes",
                on_click = move || presses.update(|n| n + 1),
                disabled = off,
                style = Action(),
            )
            text { "Pressed {presses} times" }
            view(style = Row()) {
                toggle(value = off, on_change = move |v| off.set(v))
                text { "Disabled" }
            }
        }
    }
}
