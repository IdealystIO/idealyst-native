use idea_ui::{
    install_idea_theme, light_theme, Button, ButtonShape, ButtonSlotStyles, ButtonStyles,
    ButtonVariant, Stack, StackGap, StackPadding,
};
use runtime_core::{axis_values, ui, Element, FontWeight, Shadow, StyleRules};

// A new value on idea-ui's own `variant` axis. `ButtonVariant` belongs to
// another crate, so the value arrives on a trait; with the trait in scope,
// `ButtonVariant::Raised` reads like a built-in.
axis_values! {
    pub trait AppButtonVariants for ButtonVariant { Raised }
}

/// Install restyles once, at boot, before anything renders.
fn restyle_buttons() {
    // Every Button that doesn't pick a shape is now a pill.
    ButtonStyles::set_default(ButtonShape::Pill);

    ButtonStyles::restyle(|s| {
        // A restyle MERGES into the slot: only what it sets changes.
        s.label().base(|_| StyleRules { font_weight: Some(FontWeight::Bold), ..Default::default() });

        // Give the new value a look: one arm per slot it should change.
        s.root().arm(ButtonVariant::Raised, |t| StyleRules {
            background: Some(t.color.surface()),
            shadow: Some(Shadow { x: 0.0, y: 4.0, blur: 12.0, color: t.color.border_strong() }),
            ..Default::default()
        });
        s.label().arm(ButtonVariant::Raised, |t| StyleRules {
            color: Some(t.color.text()),
            ..Default::default()
        });
    });
}

pub fn app() -> Element {
    install_idea_theme(light_theme());
    restyle_buttons();

    // A one-off change for a single instance: the `styles` prop.
    let square = ButtonSlotStyles::default().root(StyleRules {
        border_top_left_radius: Some(0.0.into()),
        border_bottom_left_radius: Some(0.0.into()),
        ..Default::default()
    });

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
            Button(label = "Built-in filled")
            Button(label = "Built-in outlined", variant = ButtonVariant::Outlined)
            Button(label = "App value: Raised", variant = ButtonVariant::Raised)
            Button(label = "Squared left edge", styles = square)
        }
    }
}
