//! Tooltip — a compact, non-interactive bubble anchored to a trigger.
//!
//! Like Popover, the host owns visibility and gates mounting. The
//! framework has no cross-backend hover event (hover-to-show is a
//! web-only affordance), so the demo toggles the bubble on press —
//! which works on every backend.

use std::rc::Rc;

use runtime_core::primitives::portal::AnchorTarget;
use runtime_core::{signal, ui, Element, PressableHandle, Ref};
use idea_ui::{tone, variant, Button, Tooltip};

use crate::shell::{self, Callout, CodePanel, ComponentPage, DemoSurface, H2, P, Prop, PropsTable, Section};

// =============================================================================
// Tooltip
// =============================================================================

pub fn tooltip() -> Element {
    let open = signal!(false);
    let trigger: Ref<PressableHandle> = Ref::new();
    let toggle: Rc<dyn Fn()> = Rc::new(move || open.update(|v| *v = !*v));

    shell::layout(ui! {
        ComponentPage(
            title = "Tooltip".to_string(),
            lead = "A compact, non-interactive bubble anchored to a trigger. The host owns \
                an open-state signal and gates mounting — the same shape as Popover and \
                Menu.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            P(content = "There's no cross-backend hover event, so this demo toggles the \
                bubble on press. Tap the button to show/hide the tooltip.".to_string())
            DemoSurface {
                Button(
                    label = "Toggle tooltip".to_string(),
                    on_click = toggle,
                    tone = tone::Neutral,
                    variant = variant::Soft,
                    bind_to = Some(trigger),
                )
                if open.get() {
                    Tooltip(
                        target = Some(AnchorTarget::from(trigger)),
                        text = "Resets everything to defaults".to_string(),
                    )
                }
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "target", ty: "Option<AnchorTarget>", desc: "Element to anchor against — AnchorTarget::from(some_ref). Required." },
                    Prop { name: "text",   ty: "Reactive<String>",     desc: "Bubble text. Static or live." },
                    Prop { name: "side",   ty: "ElementSide",          desc: "Which side of the target the bubble sits on. Default: Above." },
                    Prop { name: "align",  ty: "ElementAlign",         desc: "Alignment along the anchor edge. Default: Center." },
                    Prop { name: "offset", ty: "f32",                  desc: "Gap in px between the anchor and the bubble. Default: 6." },
                ])
            }

            Section(title = "Usage".to_string()) {
                P(content = "Bind a Ref to the trigger, gate the Tooltip on an open-state \
                    signal, and pass the trigger's Ref as the anchor target.".to_string())
                CodePanel(src = r##"let trigger: Ref<PressableHandle> = Ref::new();
let open = signal!(false);

ui! {
    IconButton(
        glyph = "?".into(),
        on_click = move || open.update(|v| *v = !*v),
        bind_to = Some(trigger),
    )
    if open.get() {
        Tooltip(
            target = Some(AnchorTarget::from(trigger)),
            text = "Resets to defaults".into(),
        )
    }
}"##.to_string())
            }

            Callout(label = "Tooltip vs Popover".to_string()) {
                P(content = "Tooltip is a single styled text bubble — non-interactive, no \
                    backdrop, no focus trap. When you need clickable content (menu items, a \
                    form), reach for Popover or Menu instead.".to_string())
            }
        }
    })
}
