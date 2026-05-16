//! Themes & Intents — swatches per intent, dark-mode reflected.

use std::rc::Rc;

use framework_core::{ui, Primitive, Signal};
use idea_ui::{
    badge, body, card, heading, hstack, pressable, vstack, BodyTone, HeadingKind, StackGap,
};
use idea_ui::doc_controls::IntentKind;

use crate::shell::page_header;

pub fn page(_is_dark: Signal<bool>) -> Primitive {
    ui! {
        VStack(gap = StackGap::Xl) {
            { page_header(
                "Themes & Intents",
                "Intent is idea-ui's global semantic-coloring vocabulary. The sidebar's \
                 Dark mode toggle swaps `light_theme()` for `dark_theme()`; every component \
                 below re-renders against the new tokens automatically."
            ) }

            { intent_grid() }
            { extension_section() }
        }
    }
}

/// A grid of every built-in intent shown as a Pressable + a Badge.
/// Two components, same intent — that's the whole point: intent is
/// shared vocabulary, not per-component variant.
fn intent_grid() -> Primitive {
    let intents = IntentKind::all();
    let mut rows: Vec<Primitive> = Vec::with_capacity(intents.len());
    for &kind in intents {
        let name = kind.name().to_string();
        let press_intent = kind.into_rc();
        let badge_intent = kind.into_rc();
        let on_click: Rc<dyn Fn()> = Rc::new(|| {});
        rows.push(ui! {
            HStack(gap = StackGap::Md) {
                Pressable(label = name.clone(), on_click = on_click.clone(), intent = press_intent)
                Badge(label = name, intent = badge_intent)
            }
        });
    }
    ui! {
        Card {
            Heading(content = "Built-in intents".to_string(), kind = HeadingKind::H2)
            Body(content = "Each row pairs a Pressable and a Badge under the same intent.".to_string(),
                 tone = BodyTone::Muted)
            VStack(gap = StackGap::Sm) { rows }
        }
    }
}

fn extension_section() -> Primitive {
    ui! {
        Card {
            Heading(content = "Adding a custom intent".to_string(), kind = HeadingKind::H2)
            Body(content = "An intent is anything implementing `Intent` — typically a \
                              zero-sized marker type. Its `palette(theme)` method returns a \
                              `IntentPalette` (background / hover / pressed / foreground / \
                              optional border). Once implemented, the marker works in every \
                              intent-aware component without changing idea-ui itself.".to_string(),
                 tone = BodyTone::Muted)
        }
    }
}
