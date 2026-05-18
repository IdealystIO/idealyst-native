//! Components — the `#[component]` attribute and what it gives you.

use framework_core::{ui, Primitive};
use idea_ui::{body, card, heading, stack, BodyTone, HeadingKind, StackGap};

use crate::shell::{
    codeblock, pageheader, section, sectionwithcode, CodeBlockProps, PageHeaderProps,
    SectionProps, SectionWithCodeProps,
};

pub fn page() -> Primitive {
    ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Components".to_string(),
                description = "Reusable UI building blocks declared as Rust functions.".to_string(),
            )

            Section(
                title = "What is a component?".to_string(),
                body = "An Idealyst component is a Rust function annotated with `#[component]` that \
                        returns a `Primitive`. The attribute rewrites the function so it can be \
                        called from inside `ui!` using a JSX-style invocation: a `counter` function \
                        becomes invocable as `Counter(...)` inside another component's UI tree.".to_string(),
            )

            Card {
                Heading(content = "A minimal component".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "Props are declared as a single Props struct parameter. The macro \
                               generates a per-component invocation macro that parses \
                               named-argument syntax and constructs the call.".to_string(),
                    tone = BodyTone::Muted,
                )
                CodeBlock(
                    code = "use framework_core::{component, ui, Primitive};\n\
                            use idea_ui::{body, heading, stack, HeadingKind, StackGap};\n\
                            \n\
                            #[derive(Default)]\n\
                            pub struct GreetingProps { pub name: String }\n\
                            \n\
                            #[component]\n\
                            pub fn greeting(props: &GreetingProps) -> Primitive {\n    \
                                let name = props.name.clone();\n    \
                                ui! {\n        \
                                    Stack(gap = StackGap::Sm) {\n            \
                                        Heading(content = format!(\"Hello, {name}\"), kind = HeadingKind::H1)\n            \
                                        Body(content = \"Welcome aboard.\".to_string())\n        \
                                    }\n    \
                                }\n\
                            }\n\
                            \n\
                            // Call it from another component:\n\
                            // ui! { Greeting(name = \"Ada\".to_string()) }".to_string(),
                )
            }

            SectionWithCode(
                title = "Default values".to_string(),
                body = "Mark optional props with `#[component(default(field = expr))]` on the \
                        function. Callers can omit the prop; the macro fills the default in.".to_string(),
                code = "#[component(default(intent = IntentTag::Primary))]\n\
                        pub fn badge(props: &BadgeProps) -> Primitive { /* ... */ }".to_string(),
            )

            Section(
                title = "Children".to_string(),
                body = "A component can accept a `children: Vec<Primitive>` field on its Props. \
                        When called from `ui!`, the brace block becomes the children list \
                        automatically — `Card { Heading(...) Body(...) }` flows the heading and \
                        body into the card's `children` slot.".to_string(),
            )

            Section(
                title = "Reactive scopes".to_string(),
                body = "Each component body runs inside its own reactive scope. Signal reads \
                        inside the function are tracked, and updates to those signals cause the \
                        relevant effects (and the bound UI nodes) to re-fire — without re-running \
                        the component body wholesale. Scope cleanup happens automatically when \
                        the component unmounts.".to_string(),
            )

            Section(
                title = "Imperative handles".to_string(),
                body = "Some components expose an imperative API (e.g. `Navigator`'s `push`/`pop` \
                        or a modal's `open`/`close`). Bind a `Ref<Handle>` via `.bind(ref)` on \
                        the call site, and the framework fills the ref once the component mounts. \
                        From then on the parent can drive the child imperatively.".to_string(),
            )
        }
    }
}
