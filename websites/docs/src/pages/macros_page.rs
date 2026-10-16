//! Macros — the proc-macro surface area: `ui!`, `#[component]`,
//! `#[method]`, `stylesheet!`, `jsx!`.

use runtime_core::{ui, Element};
use idea_ui::{Typography, Card};

use crate::shell::{
    CodeBlock, PageBody, PageHeader, CodeBlockProps, PageTypographyProps, PageHeaderProps,
};

pub fn page() -> Element {
    ui! {
        PageBody {
            PageHeader(
                title = "Macros".to_string(),
                description = "Reference for the proc-macros that make up Idealyst's authoring surface.".to_string(),
            )

            Card {
                Typography(content = "`ui!`".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "JSX-shaped DSL for declaring UI trees. Components, control flow \
                               (`if`, `for`, `match`), and pass-through Rust expressions all \
                               compose in a single block. See the UI DSL page for full \
                               syntax.".to_string(),
                    muted = true,
                )
            }

            Card {
                Typography(content = "`#[component]`".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "Function attribute that turns `fn Foo(...) -> Element` into a \
                               `ui!` tag. Props are declared as fn parameters (defaults via \
                               `#[prop(default = expr)]`), or as one explicit `props: &FooProps` \
                               struct (defaults via `#[component(default(field = expr))]`). \
                               Either way it emits a `pub type Foo = FooProps` alias and a \
                               `BuildElement` impl — that is what `ui! { Foo(...) }` calls; \
                               there is no per-component macro. It also registers the function \
                               in the hot-reload table when that feature is on.".to_string(),
                    muted = true,
                )
                CodeBlock(
                    code = "#[component]\n\
                            pub fn Badge(label: String, #[prop(default = 3)] count: i32) -> Element {\n    \
                                ui! { text { format!(\"{} ({})\", label.get(), count.get()) } }\n\
                            }\n\
                            \n\
                            // Call site:\n\
                            ui! { Badge(label = \"Inbox\") }".to_string(),
                )
            }

            Card {
                Typography(content = "`#[method]`".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "Marks a nested fn inside a `#[component]` body as an imperative \
                               method. The macro generates a `FooHandle` type and adds a \
                               `bind_to: Option<Ref<FooHandle>>` prop; the parent passes a \
                               `Ref` and calls methods through `.get()`. Methods return `()`. \
                               State itself needs no macro: `signal(value)` is a plain \
                               function. See Components.".to_string(),
                    muted = true,
                )
                CodeBlock(
                    code = "#[method]\n\
                            fn reset() { count.set(0); }\n\
                            \n\
                            // Parent:\n\
                            let h: Ref<CounterHandle> = Ref::new();\n\
                            ui! { Counter(bind_to = h) }\n\
                            h.get().map(|c| c.reset());".to_string(),
                )
            }

            Card {
                Typography(content = "`stylesheet!`".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "Declares a typed stylesheet over a theme. Generates a builder, \
                               variant enums, state overlays, and an `IntoStyleSource` impl so \
                               the result drops into any `style = ...` slot. See Styles & \
                               Themes for a worked example.".to_string(),
                    muted = true,
                )
            }

            Card {
                Typography(content = "`jsx!`".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "A sibling of `ui!` that accepts JSX-style angle-bracket syntax \
                               (`<Stack gap={StackGap::Md}>...</Stack>`, `<text>\"hi\"</text>`). Same primitives, \
                               same reactivity, same control-flow rules — authors pick \
                               whichever feels more natural.".to_string(),
                    muted = true,
                )
            }
        }
    }
}
