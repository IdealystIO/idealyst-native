//! Robot & MCP — first-class automation and agentic control.
//!
//! Accuracy notes: the Robot bridge speaks newline-delimited JSON over
//! TCP (the snippet mirrors real verbs from `runtime-core`'s robot
//! module); the MCP server is `idealyst mcp` (see
//! `crates/tools/cli/src/cmd/mcp.rs`), not a separate binary.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{CodePanel, PageHeader, PageSection};
use crate::routes::CLI_ROUTE;
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let registry_ref: Ref<ViewHandle> = Ref::new();
    let e2e_ref: Ref<ViewHandle> = Ref::new();
    let methods_ref: Ref<ViewHandle> = Ref::new();
    let mcp_ref: Ref<ViewHandle> = Ref::new();
    let build_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: registry_ref, label: "The introspection registry" },
        TocEntry { handle: e2e_ref, label: "The Robot bridge" },
        TocEntry { handle: methods_ref, label: "#[method] fns" },
        TocEntry { handle: mcp_ref, label: "MCP server" },
        TocEntry { handle: build_ref, label: "Gated on a Cargo feature" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Robot & MCP",
                blurb = "First-class automation and agentic control: one introspection \
                 registry drives E2E test harnesses, IDE tooling, and an MCP server an \
                 LLM can use as a tool surface.",
            )
            PageSection(handle = registry_ref) { registry() }
            PageSection(handle = e2e_ref) { robot_bridge() }
            PageSection(handle = methods_ref) { methods_macro() }
            PageSection(handle = mcp_ref) { mcp_server() }
            PageSection(handle = build_ref) { build_profile() }
        }
    };
    layout_with_toc(content, toc)
}

fn registry() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "The introspection registry".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Every mounted primitive registers itself with a shared \
                registry. Each entry carries a stable handle, a `test_id`, a label, and a \
                primitive kind. The registry is platform-agnostic \u{2014} the same shape \
                is populated on web, iOS, Android, and any backend you add.".to_string())
            Typography(content = "Three consumers read from the same registry \u{2014} \
                the test harness, the IDE inspector, and the MCP server. They don't \
                know about each other, but they read identical data.".to_string())
        }
    }
}

fn robot_bridge() -> Element {
    let snippet = "# The bridge is newline-delimited JSON over TCP. Every consumer \u{2014}\n\
                   # tests, the inspector, MCP \u{2014} speaks the same verbs:\n\
                   {\"id\":1,\"cmd\":\"find_element\",\"args\":{\"label_contains\":\"Dark mode\"}}\n\
                   {\"id\":2,\"cmd\":\"click\",\"args\":{\"element_id\":154}}\n\
                   {\"id\":3,\"cmd\":\"screenshot\",\"args\":{}}\n\
                   \n\
                   # Run a Rust test suite over that bridge, on any platform:\n\
                   idealyst test --macos";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "The Robot bridge".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Query elements by label / kind / `test_id`, click, \
                type, set toggles, scroll, read signals, take real-surface screenshots, \
                and snapshot the tree \u{2014} over one wire protocol that every backend \
                serves. A test suite written once drives the app on web, iOS, Android, \
                and macOS.".to_string())
            CodePanel(src = snippet)
            Typography(content = "`idealyst test` wraps the bridge in a Rust harness: \
                `#[robot_test]` fns expand to ordinary `#[test]`s that launch (or attach \
                to) the app, locate elements, act, and assert on signals and labels \
                \u{2014} pass/fail lands in your normal test output.".to_string())
        }
    }
}

fn methods_macro() -> Element {
    let snippet = "#[component]\npub fn Cart() -> Element {\n    \
                       let items = signal(Vec::<Item>::new());\n    \
                       \n    \
                       #[method]\n    \
                       fn add(item: Item) {\n        \
                           items.update(|v| v.push(item));\n    \
                       }\n    \
                       \n    \
                       #[method]\n    \
                       fn clear() {\n        \
                           items.set(Vec::new());\n    \
                       }\n    \
                       \n    \
                       // ...the rest of the component\n\
                   }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "`#[method]` fns".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Inside a `#[component]` body, nested fns marked `#[method]` \
                become the component's imperative handle, registered as JSON-callable. \
                Methods are commands (they return `()`); reads stay signals. \
                External automation invokes them by name with no per-app glue.".to_string())
            CodePanel(src = snippet)
            Typography(content = "The component's `cart.add(...)` and `cart.clear()` \
                are now callable from a Robot test, an IDE inspector, \
                or an LLM tool call \u{2014} same surface, three consumers.".to_string())
        }
    }
}

fn mcp_server() -> Element {
    let snippet = "// claude_desktop_config.json \u{2014} the CLI itself is the server:\n\
                   {\n  \"mcpServers\": {\n    \
                       \"idealyst\": { \"command\": \"idealyst\", \"args\": [\"mcp\"] }\n  \
                   }\n\
                   }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "MCP server".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "`idealyst mcp` serves the framework catalog over the \
                Model Context Protocol on stdio, with the Robot tools included by \
                default (`--no-robot` to omit them). Point Claude Desktop or Claude \
                Code at it and an LLM can read every component's props schema and drive \
                a running app: fill out forms, navigate, assert state, call exposed \
                component methods.".to_string())
            CodePanel(src = snippet)
            Typography(content = "The catalog behind it covers every `#[component]`, \
                props struct, primitive, guide, and theme token in the project and its \
                component-library dependencies \u{2014} `idealyst catalog-json` prints \
                the same data for editor tooling. Recipes keep it honest: a recipe is a \
                compiled usage example, so a props change that breaks one fails the \
                build before the docs or the LLM context can drift.".to_string())
            link(route = &CLI_ROUTE, params = ()) {
                Typography(content = "The rest of the tooling \u{2192} CLI".to_string())
            }
        }
    }
}

fn build_profile() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Gated on a Cargo feature".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "The Robot bridge + registry compile in only when the \
                `robot` feature is on. Production release builds leave it off; there's \
                no runtime overhead and no exposed surface in shipped binaries. Dev \
                builds auto-enable it via the `runtime-core/dev` feature.".to_string())
        }
    }
}
