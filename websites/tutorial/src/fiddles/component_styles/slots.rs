use idea_ui::{install_idea_theme, light_theme, IdeaThemeRef, Stack, StackGap, StackPadding};
use runtime_core::{component, component_styles, ui, Element, FlexDirection, FontWeight, StyleRules};

// A component library declares each component's styled sections once, as
// named slots. Every slot body is a stylesheet.
component_styles! {
    /// The styled sections of `Banner`.
    pub Banner<IdeaThemeRef> {
        /// How loud the banner is. Declared once, so every slot that
        /// styles it agrees on the values and the default.
        axis emphasis { #[default] quiet, loud }

        slot root {
            base(t) {
                flex_direction: FlexDirection::Row,
                padding: t.spacing.md(),
                background: t.color.surface_alt(),
                border_radius: t.radius.md(),
            }
            variant emphasis {
                quiet(_t) {}
                loud(t) { background: t.intent.primary.solid_bg() }
            }
        }

        slot message {
            base(t) { color: t.color.text(), font_weight: FontWeight::Medium }
            variant emphasis {
                quiet(_t) {}
                loud(t) { color: t.intent.primary.solid_text() }
            }
        }
    }
}

// `styles = BannerStyles` adds a `styles` prop: one optional override per
// slot, for the caller who needs this one banner to look different.
#[runtime_core::props(styles = BannerStyles)]
#[derive(Default)]
pub struct BannerProps {
    pub message: String,
    pub emphasis: BannerEmphasis,
}

#[component]
pub fn Banner(props: &BannerProps) -> Element {
    let root = BannerStyles::root()
        .set(props.emphasis.clone())
        .overridden(props.styles.root.as_ref());
    let message = BannerStyles::message()
        .set(props.emphasis.clone())
        .overridden(props.styles.message.as_ref());
    ui! {
        view(style = root) {
            text(style = message) { props.message.clone() }
        }
    }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());
    let calm = BannerSlotStyles::default().root(StyleRules {
        background: Some(idea_ui::tokens().intent.success.soft_bg()),
        ..Default::default()
    });

    ui! {
        Stack(gap = StackGap::Sm, padding = StackPadding::Md) {
            Banner(message = "Saved a draft.")
            Banner(message = "Your trial ends today.", emphasis = BannerEmphasis::Loud)
            Banner(message = "Restyled for this one instance.", styles = calm)
        }
    }
}
