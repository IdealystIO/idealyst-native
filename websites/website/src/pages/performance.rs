//! High performance — why the framework is fast, explained through the
//! architecture: direct constructor calls instead of a retained
//! description of the screen, signal-grain updates, ahead-of-time
//! compilation. We deliberately publish no benchmark numbers or
//! head-to-head comparisons here; the substance of the mechanism is
//! the page.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{PageHeader, PageSection, Section};
use crate::routes::CONCEPTS_ROUTE;
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let why_ref: Ref<ViewHandle> = Ref::new();
    let grain_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: why_ref, label: "Why it's fast" },
        TocEntry { handle: grain_ref, label: "Fine-grained updates" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "High performance",
                blurb = "Speed falls out of the architecture: `ui!` blocks compile to direct \
                 constructor calls, signals write updates straight to the views that read \
                 them, and the whole app ships as ahead-of-time compiled code.",
            )
            PageSection(handle = why_ref) { why_fast() }
            PageSection(handle = grain_ref) { fine_grained() }
        }
    };
    layout_with_toc(content, toc)
}

// =============================================================================
// Sections — no-param file-local helpers (allowed per CLAUDE.md §9.5).
// Each body invokes the shared PascalCase `Section` component.
// =============================================================================

fn why_fast() -> Element {
    ui! {
        Section(
            title = "Why it's fast".to_string(),
            paragraphs = vec![
                "A `ui!` block expands to direct constructor calls against primitives \
                 \u{2014} the macro expansion is the runtime. When state changes, signals \
                 write the new value straight into the views that read it. That is the \
                 entire update path: a state change costs a handful of direct calls.".to_string(),
                "Frameworks built on a virtual DOM rebuild a description of the screen on \
                 every update and diff it to find what changed; that work scales with the \
                 size of the view. Here the dependency graph already knows what changed \
                 before the update starts.".to_string(),
                "Ahead-of-time compilation compounds it. The app ships as native machine \
                 code (WASM on the web), so the first frame starts rendering as soon as \
                 the process does \u{2014} the code that runs is the code you compiled.".to_string(),
            ],
        )
    }
}

fn fine_grained() -> Element {
    let example = "let count = signal(0);\n\
                   \n\
                   ui! {\n    \
                       view {\n        \
                           text { \"This never re-runs when count changes\" }\n        \
                           text { format!(\"Count: {}\", count.get()) }  // only THIS leaf updates\n    \
                       }\n\
                   }\n\
                   \n\
                   // count.set(1) writes that one text node.";
    ui! {
        Stack(gap = StackGap::Md) {
            Section(
                title = "Fine-grained updates".to_string(),
                paragraphs = vec![
                    "Reactivity is built on signals, and the dependency graph is \
                     fine-grained: a signal write updates exactly the primitives that read \
                     it. The cost of a state change is proportional to what depends on \
                     that state.".to_string(),
                    "This is the difference that shows up under load: in a list of ten \
                     thousand rows where one cell changes, the update touches one cell.".to_string(),
                ],
                code = Some(example.to_string()),
            )
            link(route = &CONCEPTS_ROUTE, params = ()) {
                Typography(content = "How the reactive core works \u{2192}".to_string())
            }
        }
    }
}
