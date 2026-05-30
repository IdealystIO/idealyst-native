//! Themes & Intents — swatches per intent, dark-mode reflected.

use std::rc::Rc;

use runtime_core::{ui, Element, Signal};
use idea_ui::{Badge, Typography, Btn, Card, Stack, StackAxis, StackGap, ToneRef};

use crate::shell::page_header;

pub fn page(_is_dark: Signal<bool>) -> Element {
    ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Themes & Intents",
                "Intent is idea-ui's global semantic action vocabulary (Primary / Secondary / \
                 Neutral / Success / Danger / Warning / Info). It pairs with a per-component \
                 `kind` axis — Solid, Soft, Outlined, Ghost on Button — to produce the visual."
            ) }

            { intent_grid() }
            { extension_section() }
        }
    }
}

/// A grid of every built-in intent rendered as a Button + a Badge.
fn intent_grid() -> Element {
    let intents: Vec<(&'static str, ToneRef)> = vec![
        ("Primary", idea_ui::tone::Primary.into()),
        ("Secondary", idea_ui::tone::Secondary.into()),
        ("Neutral", idea_ui::tone::Neutral.into()),
        ("Success", idea_ui::tone::Success.into()),
        ("Danger", idea_ui::tone::Danger.into()),
        ("Warning", idea_ui::tone::Warning.into()),
        ("Info", idea_ui::tone::Info.into()),
    ];
    let mut rows: Vec<Element> = Vec::with_capacity(intents.len());
    for (name_str, tone) in intents {
        let name = name_str.to_string();
        let on_click: Rc<dyn Fn()> = Rc::new(|| {});
        rows.push(ui! {
            Stack(axis = StackAxis::Row, gap = StackGap::Md) {
                Btn(
                    label = name.clone(),
                    on_click = on_click.clone(),
                    tone = tone.clone(),
                    variant = idea_ui::variant::Filled,
                )
                Badge(label = name, tone = tone, variant = idea_ui::variant::Soft)
            }
        });
    }
    ui! {
        Card {
            Typography(content = "Built-in intents".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Each row pairs a button (Solid) and a Badge (Soft) for the same \
                              intent. The intent is shared vocabulary; the kind picks the visual.".to_string(),
                 muted = true)
            Stack(gap = StackGap::Sm) { rows }
        }
    }
}

fn extension_section() -> Element {
    ui! {
        Card {
            Typography(content = "Adding a custom intent".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "v1's component props take a built-in `IntentTag` enum directly. \
                              Custom intents (a `Hype` brand color, a `Beta` flag color) plug in \
                              by implementing `Intent` and `IntentTag::Custom(\"hype\")` — \
                              support for that is a follow-up; for v1 use the seven built-ins.".to_string(),
                 muted = true)
        }
    }
}
