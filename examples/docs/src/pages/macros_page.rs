//! Macros — the proc-macro surface area: `ui!`, `#[component]`,
//! `signal!`, `stylesheet!`, `jsx!`.

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
                title = "Macros".to_string(),
                description = "Reference for the proc-macros that make up Idealyst's authoring surface.".to_string(),
            )

            Card {
                Heading(content = "`ui!`".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "JSX-shaped DSL for declaring UI trees. Components, control flow \
                               (`if`, `for`, `match`), and pass-through Rust expressions all \
                               compose in a single block. See the UI DSL page for full \
                               syntax.".to_string(),
                    tone = BodyTone::Muted,
                )
            }

            Card {
                Heading(content = "`#[component]`".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "Function attribute that turns `fn foo(props: &FooProps) -> \
                               Primitive` into a callable component. Generates a per-component \
                               invocation macro (`foo!(...)`), wires default values via \
                               `#[component(default(field = expr))]`, and registers the \
                               function in the hot-reload table when that feature is on.".to_string(),
                    tone = BodyTone::Muted,
                )
                CodeBlock(
                    code = "#[derive(Default)]\n\
                            pub struct BadgeProps { pub label: String, pub intent: IntentTag }\n\
                            \n\
                            #[component(default(intent = IntentTag::Primary))]\n\
                            pub fn badge(props: &BadgeProps) -> Primitive { /* ... */ }".to_string(),
                )
            }

            Card {
                Heading(content = "`signal!`".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "Shorthand for `Signal::new(value)`. Identical in every way; \
                               just less typing.".to_string(),
                    tone = BodyTone::Muted,
                )
                CodeBlock(
                    code = "let count = signal!(0);\n\
                            let username = signal!(String::new());".to_string(),
                )
            }

            Card {
                Heading(content = "`stylesheet!`".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "Declares a typed stylesheet over a theme. Generates a builder, \
                               variant enums, state overlays, and an `IntoStyleSource` impl so \
                               the result drops into any `style = ...` slot. See Styles & \
                               Themes for a worked example.".to_string(),
                    tone = BodyTone::Muted,
                )
            }

            Card {
                Heading(content = "`jsx!`".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "A sibling of `ui!` that accepts JSX-style angle-bracket syntax \
                               (`<Stack gap={StackGap::Md}>...</Stack>`). Same primitives, \
                               same reactivity, same control-flow rules — authors pick \
                               whichever feels more natural.".to_string(),
                    tone = BodyTone::Muted,
                )
            }

            Section(
                title = "`children!`".to_string(),
                body = "Builds a `Vec<Primitive>` from a mixed list of children expressions. \
                        Flattens `Option<Primitive>` (skipping `None`) and `Vec<Primitive>` \
                        (inlined) so call sites can mix conditionals and pre-built lists in \
                        one place. Useful outside `ui!` blocks where you need to assemble a \
                        children list imperatively.".to_string(),
            )
        }
        }
    }
}
