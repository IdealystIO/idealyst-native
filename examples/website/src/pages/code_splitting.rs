//! Code splitting — the `lazy!` macro carves a subtree out of the
//! main wasm bundle and loads it on demand. Web target only on
//! wasm32; native compiles the block inline.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{CodePanel, PageHeader, PageSection};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let wip_ref: Ref<ViewHandle> = Ref::new();
    let macro_ref: Ref<ViewHandle> = Ref::new();
    let expansion_ref: Ref<ViewHandle> = Ref::new();
    let placeholder_ref: Ref<ViewHandle> = Ref::new();
    let constraints_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: wip_ref, label: "Status" },
        TocEntry { handle: macro_ref, label: "The lazy! macro" },
        TocEntry { handle: expansion_ref, label: "What it expands to" },
        TocEntry { handle: placeholder_ref, label: "Placeholder and lifecycle" },
        TocEntry { handle: constraints_ref, label: "v1 constraints" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Code splitting",
                blurb = "Carve a subtree out of the main wasm bundle and load it on demand. \
                 The `lazy!` macro wraps a `ui!` block in a build-time split point; \
                 the chunk fetches the first time the boundary mounts, and native \
                 targets compile the block inline.",
            )
            PageSection(handle = wip_ref) { status() }
            PageSection(handle = macro_ref) { macro_syntax() }
            PageSection(handle = expansion_ref) { expansion() }
            PageSection(handle = placeholder_ref) { placeholder_and_lifecycle() }
            PageSection(handle = constraints_ref) { v1_constraints() }
        }
    };
    layout_with_toc(content, toc)
}

// =============================================================================
// Section helpers
// =============================================================================

fn section(title: &str, paragraphs: Vec<&str>, code: Option<&str>) -> Element {
    let mut children: Vec<Element> = Vec::new();
    let title_text = title.to_string();
    children.push(ui! {
        Typography(content = title_text, kind = idea_ui::typography_kind::H2)
    });
    for p in paragraphs {
        let body = p.to_string();
        // Default kind = `Body` (14 px) — the site-wide paragraph size.
        // The page lead blurb gets `BodyLg` via `PageHeader`; section
        // prose does not, or body copy reads inconsistently large.
        children.push(ui! { Typography(content = body) });
    }
    if let Some(src) = code {
        children.push(ui! { CodePanel(src = src) });
    }
    ui! { Stack(gap = StackGap::Lg) { children } }
}

// =============================================================================
// Sections
// =============================================================================

fn status() -> Element {
    section(
        "Status",
        vec![
            "Code splitting is a work in progress. The `lazy!` macro and the \
             `Element::Lazy` runtime are wired end-to-end, but the underlying \
             wasm-split toolchain is still settling \u{2014} expect rough edges \
             around chunk naming, dead-code elimination on the main bundle, and \
             cold-load timing.",
            "The author surface below is the shape that will ship. Internals may \
             move; the macro syntax is the part to learn first.",
        ],
        None,
    )
}

fn macro_syntax() -> Element {
    let example = "use runtime_core::{lazy, ui};\n\
                   \n\
                   ui! {\n    \
                       text { \"always loaded\" }\n    \
                       { lazy! {\n        \
                           text { \"loaded on demand from a separate chunk\" }\n    \
                       } }\n\
                   }";
    section(
        "The `lazy!` macro",
        vec![
            "`lazy!` wraps a block of UI. The block is interpreted exactly like a \
             `ui!` body \u{2014} its tail expression must implement `IntoElement`, \
             so the same primitives, components, and helpers compose inside.",
            "Use it as a child expression inside a parent `ui!` block. The braces \
             around `lazy! { ... }` are the standard `ui!` escape-to-Rust syntax; \
             the macro returns a `LazyBuilder` that coerces into a `Element` \
             through the surrounding `ui!`.",
        ],
        Some(example),
    )
}

fn expansion() -> Element {
    let example = "// What you write:\n\
                   lazy! { text { \"loaded on demand\" } }\n\
                   \n\
                   // What the macro expands to (roughly):\n\
                   {\n    \
                       // Alias runtime-core's re-export so the attribute's\n    \
                       // wasm_split::... expansion resolves \u{2014} no direct\n    \
                       // wasm-split dependency needed in your crate.\n    \
                       use ::runtime_core::__wasm_split as wasm_split;\n    \
                       #[::runtime_core::__wasm_split::wasm_split(__idealyst_lazy_<hash>)]\n    \
                       async fn __idealyst_lazy_body_<hash>(_: ()) -> Element {\n        \
                           use ::runtime_core::IntoElement as _;\n        \
                           { ui! { text { \"loaded on demand\" } } }.into_element()\n    \
                       }\n    \
                       ::runtime_core::primitives::lazy::lazy_split(|| {\n        \
                           Box::pin(__idealyst_lazy_body_<hash>(()))\n    \
                       })\n\
                   }";
    section(
        "What it expands to",
        vec![
            "The macro hoists the block into a `#[wasm_split]`-annotated async \
             function. The build's wasm-split pass pulls that function (and its \
             reachable callees) into a separate `.wasm` chunk; the main bundle \
             keeps only a stub that fetches the chunk on first call.",
            "The hash is derived from the block's tokens plus the call-site span, \
             so two identical-shaped `lazy!` blocks at different sites get \
             distinct chunks. Names are stable across rebuilds when the source \
             doesn't change.",
            "On non-wasm targets the `#[wasm_split]` attribute is transparent \
             \u{2014} the async fn compiles in, the loader resolves \
             synchronously, and the subtree mounts inline.",
        ],
        Some(example),
    )
}

fn placeholder_and_lifecycle() -> Element {
    let example = "lazy! { text { \"heavy subtree\" } }\n    \
                       .placeholder(|| ui! { text { \"loading\u{2026}\" } })\n    \
                       .on_state(|state| match state {\n        \
                           LazyState::Loading => log::debug!(\"chunk fetch in flight\"),\n        \
                           LazyState::Loaded   => log::debug!(\"chunk fetched\"),\n        \
                           LazyState::Rendered => log::debug!(\"subtree mounted\"),\n        \
                           LazyState::Error(e) => log::warn!(\"lazy failed: {e}\"),\n    \
                       })";
    section(
        "Placeholder and lifecycle",
        vec![
            "The `LazyBuilder` returned from `lazy!` exposes `.placeholder(...)` \
             and `.on_state(...)` for the load window. The placeholder mounts \
             immediately and is replaced when the chunk's `Element` is ready; \
             `on_state` fires synchronously on each lifecycle transition so you \
             can drive a spinner or error UI elsewhere in the tree.",
            "On native, the callback fires once with `LazyState::Rendered` and \
             you never observe `Loading` or `Loaded` \u{2014} the chunk is \
             compiled in.",
        ],
        Some(example),
    )
}

fn v1_constraints() -> Element {
    section(
        "v1 constraints",
        vec![
            "The block cannot reference enclosing variables. The hoisted \
             function is a plain `fn`, not a closure \u{2014} it can't carry \
             captured state. If you need to pass data in, hoist it to a signal \
             or a route param the chunk reads itself. Capture forwarding via a \
             typed `Args` struct is the v2 plan.",
            "The tail expression must coerce to `Element` via `IntoElement`. \
             A `ui! { ... }` block satisfies this; so does a bare \
             `Element::*` constructor, a `#[component]`-built builder, or \
             another `LazyBuilder` (lazy boundaries nest).",
        ],
        None,
    )
}
