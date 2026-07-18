//! Advanced track — scaffolded outlines for the deeper topics. Each will
//! grow into a full walkthrough.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Typography};

use crate::common::{Callout, DocsLink, LessonPage};
use crate::routes::{ADV_BACKENDS_ROUTE, ADV_CLI_ROUTE, ADV_EMBEDDED_ROUTE};
use crate::shell;

pub fn custom_backends() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = ADV_BACKENDS_ROUTE.name(),
            title = "Custom backends".to_string(),
            lead = "Implement the Backend trait to bring idealyst to a new platform.".to_string(),
        ) {
            Typography(
                content = "The Backend trait is the framework's only seam to a platform. \
                    Implement it once and the entire existing app surface runs on your target. \
                    This walkthrough will go function-by-function through the key methods a \
                    backend provides.".to_string()
            )
            Typography(content = "What we'll cover".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Node creation (create_view / create_text / create_button), the \
                    property-update path (update_text and friends), tree mutation (insert / \
                    remove / clear), and apply_style \u{2014} how StyleRules become native \
                    style. Plus the optional hooks: interaction states, stylesheet \
                    pre-generation, and teardown cleanup.".to_string()
            )

            Callout(label = "Outline".to_string()) {
                Typography(
                    content = "This advanced topic is scaffolded \u{2014} the walkthrough is \
                        being written. The link below points at the reference material it will \
                        build on.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "The full Backend trait surface and per-method contract.".to_string(),
                link_label = "Backend reference".to_string(),
                doc_file = "backend.md".to_string(),
            )
        }
    })
}

pub fn interactive_cli() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = ADV_CLI_ROUTE.name(),
            title = "Interactive CLIs".to_string(),
            lead = "The same tree, rendered as a terminal UI.".to_string(),
        ) {
            Typography(
                content = "The terminal backend renders the same author tree into an \
                    interactive TUI. This walkthrough will cover building text-mode tools on \
                    top of it \u{2014} and the terminal-minimalism conventions that keep them \
                    from fighting the medium (no animations, no auto-rendered chrome; pages \
                    own their own headers).".to_string()
            )

            Callout(label = "Outline".to_string()) {
                Typography(
                    content = "This advanced topic is scaffolded \u{2014} the walkthrough is \
                        being written.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Backend trait + how the terminal backend maps primitives to cells.".to_string(),
                link_label = "Backend reference".to_string(),
                doc_file = "backend.md".to_string(),
            )
        }
    })
}

pub fn embedded() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = ADV_EMBEDDED_ROUTE.name(),
            title = "Embedded rendering".to_string(),
            lead = "Painting to a framebuffer on bare metal via the CPU renderer.".to_string(),
        ) {
            Typography(
                content = "idealyst runs all the way down to microcontrollers. The CPU \
                    renderer paints into a framebuffer with no GPU and no OS \u{2014} the path \
                    that drives an ESP32 + ILI9341 display. This walkthrough will cover \
                    deploying to constrained hardware and the explicit 'X not supported' \
                    placeholders the CPU backend renders for primitives that can't fit MCU \
                    limits.".to_string()
            )

            Callout(label = "Outline".to_string()) {
                Typography(
                    content = "This advanced topic is scaffolded \u{2014} the walkthrough is \
                        being written.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Backend reference \u{2014} the seam the CPU renderer implements.".to_string(),
                link_label = "Backend reference".to_string(),
                doc_file = "backend.md".to_string(),
            )
        }
    })
}
