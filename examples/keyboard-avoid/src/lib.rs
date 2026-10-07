//! `keyboard-avoid` — `keyboard_avoiding_view` at the app root.
//!
//! A chat screen: a scrolling message list over a composer pinned to the
//! bottom with `safe_area = BOTTOM`, all inside one `keyboard_avoiding_view`
//! (default: `Padding`, animated). Focus the field and the content area
//! shrinks to end at the keyboard's top, moving in step with the platform's
//! own keyboard animation (iOS: inside UIKit's keyboard animation block;
//! Android: the system IME animation's frames; mobile web: a CSS transition
//! estimating it). The composer's home-indicator inset collapses under the
//! keyboard, so it sits flush on the keyboard's top edge.
//!
//! The header reads `keyboard_inset()` — the reactive value app code uses
//! to react to the keyboard itself.

use runtime_core::{
    AlignItems, FlexDirection, JustifyContent,
    keyboard_inset, signal, stylesheet, ui, Color, Element, Ref, SafeAreaSides, TextInputHandle,
};

stylesheet! {
    pub Screen<()> {
        base(_t) {
            flex_grow: 1.0,
            background: Color("#f7f8fb".into()),
        }
    }
}

stylesheet! {
    pub Header<()> {
        base(_t) {
            padding: 12,
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            background: Color("#e5e7eb".into()),
        }
    }
}

stylesheet! {
    pub Messages<()> {
        base(_t) {
            flex_grow: 1.0,
            padding: 12,
            gap: 8,
        }
    }
}

stylesheet! {
    pub Composer<()> {
        base(_t) {
            padding: 8,
            background: Color("#ffffff".into()),
            border_top_width: 1.0,
            border_color: Color("#d1d5db".into()),
        }
    }
}

stylesheet! {
    pub Field<()> {
        base(_t) {
            padding: 10,
            border_width: 1.0,
            border_color: Color("#9ca3af".into()),
            border_radius: 8,
        }
    }
}

pub fn app() -> Element {
    let draft = signal(String::new());
    let field: Ref<TextInputHandle> = Ref::new();
    let kb = keyboard_inset();
    // "Done" drops focus, which dismisses the keyboard — the close
    // animation runs the same keyboard-synced path as the open.
    let dismiss = move || {
        if let Some(h) = field.get() {
            h.blur();
        }
    };
    ui! {
        // The only keyboard-specific line: this subtree avoids the keyboard,
        // moving in step with the platform's own keyboard animation.
        keyboard_avoiding_view(style = Screen(), safe_area = SafeAreaSides::TOP) {
            view(style = Header()) {
                text {
                    move || {
                        let k = kb.get();
                        if k.is_visible() {
                            format!("Keyboard: {:.0} pt over {} ms", k.height, k.transition.duration_ms)
                        } else {
                            "Keyboard hidden".to_string()
                        }
                    }
                }
                button(label = "Done".to_string(), on_click = dismiss)
            }
            scroll_view {
                view(style = Messages()) {
                    for i in 1..=30 {
                        text { format!("Message {i}") }
                    }
                }
            }
            view(style = Composer(), safe_area = SafeAreaSides::BOTTOM) {
                text_input(
                    value = draft,
                    on_change = move |s| draft.set(s),
                    placeholder = "Message",
                    style = Field(),
                ).bind(field)
            }
        }
    }
}

/// The Android wrapper's entry (see `examples/README.md`).
pub fn scene_app() -> Element {
    app()
}

/// SDK-handler registration seam. This app registers nothing.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}
