//! Robot & MCP — first-class automation and agentic control.

use runtime_core::{ui, Primitive};
use idea_ui::{stack, typography, StackGap, TypographyKind};

use crate::pages::common::{code_panel, page_header, page_section};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Primitive {
    const REGISTRY: &str = "registry";
    const E2E: &str = "e2e-tests";
    const METHODS: &str = "methods-macro";
    const MCP: &str = "mcp-server";
    const BUILD: &str = "build-profile";

    let toc = vec![
        TocEntry { id: REGISTRY, label: "The introspection registry" },
        TocEntry { id: E2E, label: "E2E test harnesses" },
        TocEntry { id: METHODS, label: "methods! { ... }" },
        TocEntry { id: MCP, label: "MCP server" },
        TocEntry { id: BUILD, label: "Gated on a Cargo feature" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Robot & MCP",
                "First-class automation and agentic control baked into the framework \
                 itself \u{2014} not bolted on after. One introspection registry \
                 drives E2E test harnesses, IDE tooling, and an MCP server an LLM \
                 can use as a tool surface."
            ) }
            { page_section(REGISTRY, vec![registry()]) }
            { page_section(E2E, vec![e2e_tests()]) }
            { page_section(METHODS, vec![methods_macro()]) }
            { page_section(MCP, vec![mcp_server()]) }
            { page_section(BUILD, vec![build_profile()]) }
        }
    };
    layout_with_toc(content, toc)
}

fn registry() -> Primitive {
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "The introspection registry".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Every mounted primitive registers itself with a shared \
                registry. Each entry carries a stable handle, a `test_id`, a label, and a \
                primitive kind. The registry is platform-agnostic \u{2014} the same shape \
                is populated on web, iOS, Android, and any backend you add.".to_string())
        },
        ui! {
            Typography(content = "Three consumers read from the same registry. They don't \
                know about each other, but they read identical data.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn e2e_tests() -> Primitive {
    let snippet = "// Robot.rs \u{2014} cross-platform E2E test harness.\n\
                   let robot = Robot::connect(\"localhost:9000\")?;\n\
                   robot.tap_by_test_id(\"submit-button\")?;\n\
                   robot.type_text_into(\"name-field\", \"Alice\")?;\n\
                   let count = robot.signal_value::<i32>(\"counter\")?;\n\
                   assert_eq!(count, 1);";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "E2E test harnesses".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Query by `test_id`, click buttons, type into inputs, \
                read signals, snapshot the tree. The same `Robot` API drives web, iOS, \
                and Android \u{2014} no separate platform runner per target, no \
                Detox-on-iOS-and-Espresso-on-Android split.".to_string())
        },
        code_panel(snippet),
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn methods_macro() -> Primitive {
    let snippet = "#[component]\npub fn cart(props: &CartProps) -> Primitive {\n    \
                       let items = signal!(Vec::<Item>::new());\n    \
                       \n    \
                       methods! {\n        \
                           fn add(item: Item) {\n            \
                               items.update(|v| v.push(item));\n        \
                           }\n        \
                           fn clear() {\n            \
                               items.set(Vec::new());\n        \
                           }\n        \
                           fn total() -> f64 {\n            \
                               items.get().iter().map(|i| i.price).sum()\n        \
                           }\n    \
                       }\n    \
                       \n    \
                       // ...the rest of the component\n\
                   }";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "`methods! { ... }`".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Inside a `#[component]` body, a `methods! { ... }` block \
                exposes named methods that the registry registers as JSON-callable. \
                External automation can invoke them by name without per-app glue.".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(content = "The component's `cart.add(...)`, `cart.clear()`, and \
                `cart.total()` are now callable from a Robot test, an IDE inspector, \
                or an LLM tool call \u{2014} same surface, three consumers.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn mcp_server() -> Primitive {
    let snippet = "// claude_desktop_config.json\n\
                   {\n  \"mcpServers\": {\n    \
                       \"idealyst\": {\n      \
                           \"command\": \"idealyst-mcp\",\n      \
                           \"args\": [\"--from-bin\", \"./target/debug/my-app\"]\n    \
                       }\n  }\n\
                   }";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "MCP server".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "`idealyst-mcp` is a stdio MCP server that turns each \
                registry capability into an MCP tool. Drop it into a Claude Desktop \
                config and an LLM can drive a running iOS / Android / web app directly: \
                fill out forms, navigate, assert state, call exposed component methods.".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(content = "The framework's component catalog (every primitive, \
                every props struct, every theme token) ships alongside the live registry, \
                so the LLM also has rich type information for the code it's manipulating.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn build_profile() -> Primitive {
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Gated on a Cargo feature".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "The Robot bridge + registry compile in only when the \
                `robot` feature is on. Production release builds leave it off; there's \
                no runtime overhead and no exposed surface in shipped binaries. Dev \
                builds auto-enable it via the `runtime-core/dev` feature.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}
