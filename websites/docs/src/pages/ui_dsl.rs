//! UI DSL — the `ui!` macro: syntax, control flow, reactive forms.

use runtime_core::{ui, Element};
use idea_ui::{Typography, Card};

use crate::shell::{
    CodeBlock, PageBody, PageHeader, Section, CodeBlockProps, PageTypographyProps, PageHeaderProps,
    SectionProps,
};

pub fn page() -> Element {
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
                Typography(content = "Component invocation".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "Nodes are written `name(prop = expr, ...) { children }`; the \
                               parens and the children block are each optional. An identifier \
                               followed by `(` or `{` is a node. The snake_case primitive names \
                               (`view`, `text`, `button`, `image`, `icon`, `text_input`, \
                               `scroll_view`, `flat_list`, `link`, …) are primitives; every other \
                               name is a `#[component]` tag, so components are PascalCase. A bare \
                               identifier with no parens or braces (`children`) is a Rust \
                               expression.".to_string(),
                    muted = true,
                )
                CodeBlock(
                    code = "ui! {\n    \
                                Stack(gap = StackGap::Md) {\n        \
                                    Typography(content = \"Hello\".to_string(), kind = typography_kind::H1)\n        \
                                    Typography(content = \"World\".to_string())\n    \
                                }\n\
                            }".to_string(),
                )
            }

            Card {
                Typography(content = "Reactive `if`".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "An `if` whose condition might read a signal (a `.get()`, a \
                               method call, or a function call anywhere in it) is lowered to a \
                               reactive `when(...)` call — the branch re-evaluates whenever the \
                               signal changes. Only a condition that provably reads no signal \
                               (a plain `bool` variable, a literal) is evaluated once at build \
                               time.".to_string(),
                    muted = true,
                )
                CodeBlock(
                    code = "let open = signal(false);\n\
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
                Typography(content = "Reactive `match`".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "Same rule as `if`: a `match` whose scrutinee might read a \
                               signal lowers to a `switch(...)` call, which rebuilds only when \
                               the value changes (so the scrutinee type must be `PartialEq`). \
                               Each arm's body is a UI block (always brace-delimited) so the \
                               parser stays unambiguous.".to_string(),
                    muted = true,
                )
                CodeBlock(
                    code = "match status.get() {\n    \
                                Status::Loading => { activity_indicator() }\n    \
                                Status::Ready(value) => { Typography(content = value) }\n    \
                                Status::Failed(err) => { Alert(title = err, tone = tone::Danger) }\n\
                            }".to_string(),
                )
            }

            Card {
                Typography(content = "`for` lists".to_string(), kind = idea_ui::typography_kind::H2)
                Typography(
                    content = "Iterate over any `IntoIterator` — a `Vec`, a slice, a map — and \
                               the rows are built once and spliced into the surrounding \
                               children. Iterate a `Signal<Vec<T>>` instead and the list is \
                               reactive; that form needs a key (`for item in items, key = \
                               item.id { ... }`) so rows keep their state across updates. For \
                               long lists use `flat_list` (see Lists).".to_string(),
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
                body = "A node or `for`-block can end in a `.method(args)` chain, applied to \
                        what the node builds — for builder setters that have no inline prop \
                        (`view(...).container()`). To get a handle, use the props instead: \
                        primitives take `bind = my_ref`, and a component with `#[method]` fns \
                        takes `bind_to = my_ref`.".to_string(),
            )

            Section(
                title = "Pass-through expressions".to_string(),
                body = "A plain Rust expression inside `ui!` — in braces (`{ expr }`) or a \
                        bare identifier — is dropped into the surrounding children list via \
                        the `ChildList` trait. The trait flattens \
                        `Element`, `Option<Element>` (skipping `None`), and \
                        `Vec<Element>` (inlined) — so you can mix precomputed nodes, \
                        conditional `Some` blocks, and helper-built lists in one place.".to_string(),
            )
        }
    }
}
