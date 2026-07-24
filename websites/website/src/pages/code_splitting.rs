//! Code splitting — `#[component(lazy)]` compiles a component's body
//! into a separate wasm chunk loaded on demand. Web target only on
//! wasm32; native compiles the body inline.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{PageHeader, PageSection, Section};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let attr_ref: Ref<ViewHandle> = Ref::new();
    let props_ref: Ref<ViewHandle> = Ref::new();
    let states_ref: Ref<ViewHandle> = Ref::new();
    let split_ref: Ref<ViewHandle> = Ref::new();
    let native_ref: Ref<ViewHandle> = Ref::new();
    let sdk_ref: Ref<ViewHandle> = Ref::new();
    let legacy_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: attr_ref, label: "Lazy components" },
        TocEntry { handle: props_ref, label: "Props cross the boundary" },
        TocEntry { handle: states_ref, label: "Loading and error UI" },
        TocEntry { handle: split_ref, label: "How the split works" },
        TocEntry { handle: native_ref, label: "Native and SSR" },
        TocEntry { handle: sdk_ref, label: "Heavy SDKs" },
        TocEntry { handle: legacy_ref, label: "The lazy! block (deprecated)" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Code splitting",
                blurb = "Ship a heavy component as a separate wasm chunk fetched the \
                 first time it mounts. Mark the component with `#[component(lazy)]`; \
                 its props cross the chunk boundary, its `loading` and `error` props \
                 render the in-between states, and native targets compile the body \
                 inline.",
            )
            PageSection(handle = attr_ref) { lazy_components() }
            PageSection(handle = props_ref) { props_boundary() }
            PageSection(handle = states_ref) { loading_and_error() }
            PageSection(handle = split_ref) { how_the_split_works() }
            PageSection(handle = native_ref) { native_and_ssr() }
            PageSection(handle = sdk_ref) { heavy_sdks() }
            PageSection(handle = legacy_ref) { legacy_lazy_block() }
        }
    };
    layout_with_toc(content, toc)
}

// =============================================================================
// Sections — no-param file-local helpers (allowed per CLAUDE.md §9.5).
// Each body invokes the shared PascalCase `Section` component.
// =============================================================================

fn lazy_components() -> Element {
    let example = "use runtime_core::{component, ui, Element};\n\
                   \n\
                   /// The whole subtree \u{2014} and every dependency only it\n\
                   /// reaches \u{2014} ships in a separate chunk.\n\
                   #[component(lazy)]\n\
                   fn Editor(document_id: u32) -> Element {\n    \
                       ui! { view { /* heavy editor UI */ } }\n\
                   }\n\
                   \n\
                   // The call site is a normal component tag:\n\
                   ui! {\n    \
                       Editor(document_id = 42)\n\
                   }";
    ui! {
        Section(
            title = "Lazy components".to_string(),
            paragraphs = vec![
                "Add `lazy` to `#[component]` (or use the `#[lazy_component]` \
                 shorthand). The component's body compiles into a separate wasm \
                 chunk; the main bundle keeps a loader stub that fetches the chunk \
                 the first time the component mounts. Chunks are fetched once per \
                 session and cached \u{2014} later mounts resolve before the next \
                 paint.".to_string(),
                "The chunk file is named after the component \
                 (`\u{2026}_lazy_Editor.wasm`), so it's recognizable in the network \
                 tab and in bundle-size reports.".to_string(),
            ],
            code = Some(example.to_string()),
        )
    }
}

fn props_boundary() -> Element {
    ui! {
        Section(
            title = "Props cross the boundary".to_string(),
            paragraphs = vec![
                "The component's props are the data that crosses the split: the \
                 loader carries them into the chunk and the body receives them as \
                 ordinary parameters. Runtime input \u{2014} a document id, a route \
                 param, a config struct \u{2014} is just a prop, typed and \
                 compile-checked at the call site.".to_string(),
                "Signals and callbacks created inside the body work as usual: the \
                 chunk's construction runs under its own reactive scope, owned and \
                 torn down with the component.".to_string(),
            ],
        )
    }
}

fn loading_and_error() -> Element {
    let example = "ui! {\n    \
                       Editor(\n        \
                           document_id = 42,\n        \
                           loading = || ui! { Skeleton() },\n        \
                           error = |e| ui! {\n            \
                               view {\n                \
                                   text { format!(\"Couldn't load: {}\", e.message()) }\n                \
                                   Button(label = \"Retry\", on_press = e.retry())\n            \
                               }\n        \
                           },\n    \
                       )\n\
                   }\n\
                   \n\
                   // retry() re-runs the loader with the same props \u{2014} opt in:\n\
                   #[component(lazy, retryable)]\n\
                   fn Editor(document_id: u32) -> Element { /* \u{2026} */ }";
    ui! {
        Section(
            title = "Loading and error UI".to_string(),
            paragraphs = vec![
                "Every lazy component gains two generated props. `loading` renders \
                 while the chunk fetches; `error` renders if the fetch or link \
                 fails, receiving a `LazyError` with `.message()` and a `.retry()` \
                 handle.".to_string(),
                "`retry()` re-invokes the loader, which re-passes the props \u{2014} \
                 so it requires them to be `Clone`. `#[component(lazy, retryable)]` \
                 opts in and derives the bound; the default moves the props into \
                 the loader once.".to_string(),
            ],
            code = Some(example.to_string()),
        )
    }
}

fn how_the_split_works() -> Element {
    ui! {
        Section(
            title = "How the split works".to_string(),
            paragraphs = vec![
                "The attribute wraps the body in a `#[wasm_split]` async function. \
                 A release web build (`idealyst build --web --release`) runs the \
                 wasm-split pass after wasm-bindgen: it walks the call graph, moves \
                 code reachable only through the split function into a chunk wasm, \
                 and emits the loader glue that fetches and links the chunk against \
                 the live main instance.".to_string(),
                "Chunk-only code leaves the main bundle automatically. Chunk-only \
                 static data follows under the experimental `--data-prune` flag; \
                 verify the app still renders when enabling it.".to_string(),
            ],
        )
    }
}

fn native_and_ssr() -> Element {
    ui! {
        Section(
            title = "Native and SSR".to_string(),
            paragraphs = vec![
                "On iOS, Android, macOS, and the GPU backend there is one binary and \
                 no network fetch: the attribute compiles the body inline, the \
                 loader resolves synchronously, and the `loading` UI is never \
                 visible. The author surface is identical across targets.".to_string(),
                "Server-side rendering emits the `loading` UI as its output; the \
                 client hydrates it and then loads the chunk. Content that must be \
                 prerendered belongs outside a lazy boundary.".to_string(),
            ],
        )
    }
}

fn heavy_sdks() -> Element {
    let example = "#[component(lazy)]\n\
                   fn CanvasCorner() -> Element {\n    \
                       // Registration is the main-bundle anchor \u{2014} move it\n    \
                       // into the chunk, then render.\n    \
                       canvas_sdk::register_lazy();\n    \
                       canvas_sdk::widget()\n\
                   }";
    ui! {
        Section(
            title = "Heavy SDKs".to_string(),
            paragraphs = vec![
                "An SDK that renders through `Element::External` (canvas, PDF, \
                 maps) is anchored in the main bundle by its handler registration, \
                 wherever the rendering happens. Splitting such an SDK means \
                 registering from inside the lazy component's body: the SDK exposes \
                 a `register_lazy()` built on `defer_external_registration`, and \
                 the app calls it as the body's first line.".to_string(),
                "The framework drains the queued registration right before the \
                 chunk's external dispatches, so the handler is in place when the \
                 subtree mounts. The lazy-loading guide covers the full pattern, \
                 including the inventory-registration pitfall.".to_string(),
            ],
            code = Some(example.to_string()),
        )
    }
}

fn legacy_lazy_block() -> Element {
    ui! {
        Section(
            title = "The lazy! block (deprecated)".to_string(),
            paragraphs = vec![
                "Earlier versions exposed `lazy! { \u{2026} }`, an anonymous block \
                 form of the same split. It is deprecated in favor of lazy \
                 components: the block form accepts no input across the boundary \
                 (the hoisted function is a plain `fn`, so the block can't \
                 reference enclosing variables) and names its chunks by content \
                 hash. Existing `lazy!` blocks keep compiling and splitting; the \
                 compiler warning points at the replacement.".to_string(),
                "Migrating is mechanical: hoist the block's body into a \
                 `#[component(lazy)]` fn, turn anything the block couldn't capture \
                 into props, and move `.placeholder(\u{2026})` to the `loading` \
                 prop at the call site.".to_string(),
            ],
        )
    }
}
