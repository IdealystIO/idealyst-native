//! Overview — landing page. What Idealyst is and why it exists.

use framework_core::{ui, Primitive};
use idea_ui::{body, card, heading, stack, BodyTone, HeadingKind, StackGap};

// The PageHeader / Section / etc. invocation macros live at crate
// root (via `#[macro_use] mod shell;`). Their expansions reference
// the function + props struct unqualified, so we import both here.
use crate::shell::{pageheader, PageHeaderProps};

pub fn page() -> Primitive {
    ui! {
        ScrollView {
            Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Idealyst".to_string(),
                description = "A Rust framework for building cross-platform native applications \
                               from a single, platform-agnostic UI tree.".to_string(),
            )

            Card {
                Heading(content = "One app, every surface".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "An Idealyst app is a Rust crate whose `app()` function returns \
                               a `Primitive` tree. The same tree mounts on the web (via WASM \
                               and the DOM), on iOS (via UIKit), on Android (via native Views), \
                               on Roku (via a BrightScript runtime), and into AAS — the dev \
                               server's app-as-server mode. Platform glue lives in tiny \
                               wrapper crates the CLI materializes for you; the cross-platform \
                               tree never touches them.".to_string(),
                    tone = BodyTone::Muted,
                )
            }

            Card {
                Heading(content = "Fine-grained reactivity".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "State lives in Signals. Reads tracked at fine granularity drive \
                               updates only where the signal is actually consumed — no virtual \
                               DOM diff, no top-down render. Signals are arena-allocated and \
                               `Copy`, so plumbing them through closures and child components \
                               is cheap.".to_string(),
                    tone = BodyTone::Muted,
                )
            }

            Card {
                Heading(content = "Macros that read like markup".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "The `ui!` macro is a JSX-shaped DSL with Rust semantics. \
                               Component invocations, prop lists, children, and Rust-native \
                               control flow (`if`, `for`, `match`) all compose in one \
                               surface. Use `#[component]` to define your own; reach for \
                               `stylesheet!` for typed, themed styles.".to_string(),
                    tone = BodyTone::Muted,
                )
            }

            Card {
                Heading(content = "Where to next".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "Head to the Quickstart for a hello-world skeleton, then \
                               work through Core Concepts to learn the primitives that \
                               every Idealyst app composes from.".to_string(),
                    tone = BodyTone::Muted,
                )
            }
        }
        }
    }
}
