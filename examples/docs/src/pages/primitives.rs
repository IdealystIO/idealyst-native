//! Primitives — the low-level building blocks each backend implements.

use framework_core::{ui, Primitive};
use idea_ui::{body, card, heading, stack, BodyTone, HeadingKind, StackGap};

use crate::shell::{
    codeblock, pageheader, section, CodeBlockProps, PageHeaderProps, SectionProps,
};

pub fn page() -> Primitive {
    ui! {
        ScrollView {
            Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Primitives".to_string(),
                description = "The framework's stable, backend-agnostic vocabulary.".to_string(),
            )

            Section(
                title = "What is a primitive?".to_string(),
                body = "A primitive is a `Primitive` enum variant the framework guarantees \
                        every backend can render. Backends implement a fixed `Backend` trait — \
                        `create_view`, `create_text`, `create_button`, and friends — and the \
                        framework's render walker calls those functions to materialize the \
                        cross-platform tree.".to_string(),
            )

            Card {
                Heading(content = "View".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "A flexible container with layout. Holds children, takes a style, \
                               and corresponds to a UIView, a native View, or a `<div>` \
                               depending on the backend. Inside `ui!`, write \
                               `View(style = ...) { children }`.".to_string(),
                    tone = BodyTone::Muted,
                )
            }

            Card {
                Heading(content = "Text".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "A text node. Pass content as a single expression in the children \
                               block (`Text { \"hello\" }`) or via a `content` prop. A `style` \
                               prop sets font, color, alignment, etc.".to_string(),
                    tone = BodyTone::Muted,
                )
                CodeBlock(
                    code = "ui! {\n    \
                                Text(style = title_style) { \"Welcome\" }\n    \
                                Text { format!(\"You have {} messages\", count.get()) }\n\
                            }".to_string(),
                )
            }

            Card {
                Heading(content = "Button".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "A native pressable with a label and a click handler. The \
                               framework also exposes `Pressable` for cases where you need an \
                               interactive surface that isn't shaped like a button — cards, \
                               list rows, custom touch targets.".to_string(),
                    tone = BodyTone::Muted,
                )
                CodeBlock(
                    code = "ui! {\n    \
                                Button(\n        \
                                    label = \"Save\",\n        \
                                    on_click = move || persist(),\n    \
                                )\n\
                            }".to_string(),
                )
            }

            Section(
                title = "Reactive primitives".to_string(),
                body = "`When` toggles between two branches based on a reactive condition; \
                        `Switch` routes to one of N branches based on a match. The `ui!` macro \
                        lowers signal-reading `if` and `match` into these automatically — you \
                        rarely construct them by hand.".to_string(),
            )

            Section(
                title = "Higher-level on top".to_string(),
                body = "Primitives are intentionally minimal. The `idea-ui` component library \
                        is built entirely on top of them: every Card, Stack, Button, Heading, \
                        Field — down to the smallest Badge — composes from the same `View` / \
                        `Text` / `Pressable` substrate. The same is true of any component \
                        library you build yourself.".to_string(),
            )

            Section(
                title = "Custom primitives".to_string(),
                body = "If you need behavior no existing primitive supports — a Canvas, a \
                        Video, an OpenGL surface — implement a new variant on the `Primitive` \
                        enum and add the matching `Backend` method. Backends that don't \
                        implement it fall back to a sensible no-op (typically an empty View) \
                        so cross-platform crates keep compiling.".to_string(),
            )
        }
        }
    }
}
