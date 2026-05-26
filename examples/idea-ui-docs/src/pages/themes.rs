//! Themes & Intents — swatches per intent, dark-mode reflected.

use std::rc::Rc;

use runtime_core::{ui, Primitive, Signal};
use idea_ui::{badge, typography, btn, card, stack, BadgeKind, TypographyTone, ButtonKind, TypographyKind, IntentTag, StackAxis, StackGap};

use crate::shell::page_header;

pub fn page(_is_dark: Signal<bool>) -> Primitive {
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
fn intent_grid() -> Primitive {
    let intents = IntentTag::all();
    let mut rows: Vec<Primitive> = Vec::with_capacity(intents.len());
    for &intent in intents {
        let name = format!("{:?}", intent);
        let on_click: Rc<dyn Fn()> = Rc::new(|| {});
        rows.push(ui! {
            Stack(axis = StackAxis::Row, gap = StackGap::Md) {
                Btn(
                    label = name.clone(),
                    on_click = on_click.clone(),
                    intent = intent,
                    kind = ButtonKind::Solid,
                )
                Badge(label = name, intent = intent, kind = BadgeKind::Soft)
            }
        });
    }
    ui! {
        Card {
            Typography(content = "Built-in intents".to_string(), kind = TypographyKind::H2)
            Typography(content = "Each row pairs a Button (Solid) and a Badge (Soft) for the same \
                              intent. The intent is shared vocabulary; the kind picks the visual.".to_string(),
                 tone = TypographyTone::Muted)
            Stack(gap = StackGap::Sm) { rows }
        }
    }
}

fn extension_section() -> Primitive {
    ui! {
        Card {
            Typography(content = "Adding a custom intent".to_string(), kind = TypographyKind::H2)
            Typography(content = "v1's component props take a built-in `IntentTag` enum directly. \
                              Custom intents (a `Hype` brand color, a `Beta` flag color) plug in \
                              by implementing `Intent` and `IntentTag::Custom(\"hype\")` — \
                              support for that is a follow-up; for v1 use the seven built-ins.".to_string(),
                 tone = TypographyTone::Muted)
        }
    }
}
