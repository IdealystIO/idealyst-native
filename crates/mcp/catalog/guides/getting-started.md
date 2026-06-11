+++
title = "Getting Started"
order = 10
tags = ["intro", "core"]
+++

# Getting Started with Idealyst

Idealyst is a cross-platform UI framework: one author tree, native rendering on iOS, Android, web, and macOS. This guide walks you through a minimal app.

## Creating a project

```bash
idealyst new my-app
cd my-app
idealyst dev
```

`idealyst new` scaffolds a library crate plus a `.claude/mcp.json` that wires Claude Code into the project's MCP server. `idealyst dev` builds the runtime-server host binary, launches it, and serves the wasm bundle so you can see the app in a browser while the native targets stream the same wire log.

## The minimum app

Every Idealyst app exposes an `app()` function that returns a [[View]] (or any [[Element]]). The framework's host calls this once at startup, and reactive signals inside it drive incremental updates.

```rust
use runtime_core::*;

pub fn app() -> Element {
    ui! {
        View(style = view_style()) {
            Text("Hello, Idealyst!")
        }
    }
}
```

That's it. No `main` function in your crate — the per-platform host (web/iOS/Android/macOS) wraps `app()` and runs the framework's reactive loop.

## Three concepts to learn next

1. **[[primitives|Primitives]]** — the leaf nodes of `ui!`: [[View]], [[Text]], [[Button]], [[ScrollView]], etc. These map to native widgets on every backend.
2. **[[components|Components]]** — your own reusable units, declared with `#[component]`. Compose them inside `ui!` just like primitives.
3. **[[reactivity|Reactivity]]** — `Signal<T>` values, `bind!`, and the closure-form reactive bindings inside `ui!`.

Beyond the UI core, capabilities like networking, persistence, and a full component library live in **opt-in SDK crates** you add as you need them — see **[[sdks|SDKs & opt-in crates]]** for the index (and **[[server-functions]]** for the `#[server]` RPC layer). `net` makes HTTP requests, `storage`/`credentials` persist data, `idea-ui` provides ready-made components.

## Where to look in the catalog

The MCP server (running automatically when you launch the project in Claude Code) exposes the entire framework surface:

- `list_primitives` → every framework primitive
- `list_utilities` → free helpers like [[platform]], [[parse]], [[now_micros]]
- `list_states` → interaction state names you can use in `stylesheet!`
- `list_guides` → these documents
- `describe_*` for any individual entry

Pair these with `list_components` for your project's own components, and you have the complete authoring vocabulary in one place.
