//! UI DSL — the `ui!` macro: syntax, control flow, reactive forms.

use runtime_core::{ui, Primitive};
use idea_ui::{Typography, Card};

use crate::shell::{
    CodeBlock, PageBody, PageHeader, Section, CodeBlockProps, PageTypographyProps, PageHeaderProps,
    SectionProps,
};

pub fn page() -> Primitive {
    ui! {
        PageBody {
            PageHeader(
                title = "UI DSL".to_string(),
                description = "The `ui!` macro: declarative UI trees that lower to plain Rust calls.".to_string(),
            )

            Section(
                title = "Grammar at a glance".to_string(),
                body = "A `ui!` block is a sequence of nodes. Each node is a component \
                        invocation, a control-flow form (`if`, `for`, `match`), or a raw Rust \
                        expression that flattens into the current children list.".to_string(),
            )

            Card {
                Typography(content = "Component invocation".to_string(), kind = idea_ui::typography_kind::H2.into())
                Typography(
                    content = "Components are called as `Name(prop = expr, ...) { children }`. \
                               Identifiers starting with an uppercase ASCII letter are treated \
                               as component invocations; anything lowercase falls through as a \
                               plain Rust expression. The parens and the children block are \
                               both optional individually.".to_string(),
                    muted = true,
                )
                CodeBlock(
                    code = "ui! {\n    \
                                Stack(gap = StackGap::Md) {\n        \
                                    Typography(content = \"Hello\".to_string(), kind = TypographyKind::H1)\n        \
                                    Typography(content = \"World\".to_string())\n    \
                                }\n\
                            }".to_string(),
                )
            }

            Card {
                Typography(content = "Reactive `if`".to_string(), kind = idea_ui::typography_kind::H2.into())
                Typography(
                    content = "An `if` whose condition reads a signal (contains `.get()`) is \
                               lowered to a reactive `when(...)` call — the branch \
                               re-evaluates whenever the signal changes. An `if` with a plain \
                               boolean is evaluated once at build time and emits the selected \
                               branch.".to_string(),
                    muted = true,
                )
                CodeBlock(
                    code = "let open = signal!(false);\n\
                            ui! {\n    \
                                if open.get() {\n        \
                                    Typography(content = \"It's open!\".to_string())\n    \
                                } else {\n        \
                                    Typography(content = \"It's closed.\".to_string())\n    \
                                }\n\
                            }".to_string(),
                )
            }

            Card {
                Typography(content = "Reactive `match`".to_string(), kind = idea_ui::typography_kind::H2.into())
                Typography(
                    content = "Same rule as `if`: a `match` whose scrutinee reads a signal \
                               lowers to a `switch(...)` call. Each arm's body is a UI block \
                               (always brace-delimited) so the parser stays unambiguous.".to_string(),
                    muted = true,
                )
                CodeBlock(
                    code = "match status.get() {\n    \
                                Status::Loading => { Spinner() }\n    \
                                Status::Ready(value) => { Typography(content = value) }\n    \
                                Status::Failed(err) => { Alert(title = err, intent = IntentTag::Danger) }\n\
                            }".to_string(),
                )
            }

            Card {
                Typography(content = "`for` lists".to_string(), kind = idea_ui::typography_kind::H2.into())
                Typography(
                    content = "Iterate over any `IntoIterator`. The macro expands the loop into \
                               a `Vec<Primitive>` that flows into the surrounding children. A \
                               trailing `.style(...)` chain pins the row container's style for \
                               virtualized lists.".to_string(),
                    muted = true,
                )
                CodeBlock(
                    code = "ui! {\n    \
                                Stack(gap = StackGap::Sm) {\n        \
                                    for entry in items.iter() {\n            \
                                        Card { Typography(content = entry.title.clone()) }\n        \
                                    }\n    \
                                }\n\
                            }".to_string(),
                )
            }

            Section(
                title = "Trailing method chains".to_string(),
                body = "Components and `for`-blocks accept a trailing `.method(args)` chain. \
                        The most common use is `.bind(ref)` to hand back an imperative handle \
                        for the mounted component.".to_string(),
            )

            Section(
                title = "Pass-through expressions".to_string(),
                body = "Any plain Rust expression inside `ui!` is dropped into the surrounding \
                        children list via the `ChildList` trait. The trait flattens \
                        `Primitive`, `Option<Primitive>` (skipping `None`), and \
                        `Vec<Primitive>` (inlined) — so you can mix precomputed nodes, \
                        conditional `Some` blocks, and helper-built lists in one place.".to_string(),
            )
        }
    }
}
