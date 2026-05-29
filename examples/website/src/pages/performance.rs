//! High performance — why the framework is fast (architecture) and how
//! that claim is kept honest (a reproducible head-to-head benchmark
//! harness). We deliberately do NOT hard-code headline millisecond
//! numbers into marketing copy: the numbers depend on hardware,
//! browser, and build, and they drift as every framework releases. The
//! harness in `benchmark/` is the source of truth \u{2014} this page
//! explains the mechanism and points you at it.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{CodePanel, PageHeader, PageSection};
use crate::routes::CONCEPTS_ROUTE;
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let why_ref: Ref<ViewHandle> = Ref::new();
    let grain_ref: Ref<ViewHandle> = Ref::new();
    let measure_ref: Ref<ViewHandle> = Ref::new();
    let against_ref: Ref<ViewHandle> = Ref::new();
    let reproduce_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: why_ref, label: "Why it's fast" },
        TocEntry { handle: grain_ref, label: "Fine-grained, not coarse" },
        TocEntry { handle: measure_ref, label: "How we measure" },
        TocEntry { handle: against_ref, label: "What it's compared against" },
        TocEntry { handle: reproduce_ref, label: "Reproduce it yourself" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "High performance",
                blurb = "Native-class speed isn't a tuning pass bolted on at the end \u{2014} it \
                 falls out of the architecture. No virtual DOM, no diffing, no bundled \
                 runtime interpreting your component tree. And we keep the claim honest \
                 with a reproducible head-to-head benchmark against the frameworks you'd \
                 actually compare us to.",
            )
            PageSection(handle = why_ref) { why_fast() }
            PageSection(handle = grain_ref) { fine_grained() }
            PageSection(handle = measure_ref) { how_measured() }
            PageSection(handle = against_ref) { compared_against() }
            PageSection(handle = reproduce_ref) { reproduce() }
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

fn why_fast() -> Element {
    section(
        "Why it's fast",
        vec![
            "Most UI frameworks pay an abstraction tax at render time: a virtual DOM is \
             built, diffed against the previous one, and the difference is reconciled \
             into the real tree. That work scales with the size of the view, runs on \
             every update, and competes with your app for the main thread.",
            "Idealyst doesn't have that layer. A `ui!` block expands to direct \
             constructor calls against primitives \u{2014} the macro expansion IS the \
             runtime. There's no diff pass, no reconciler that owns your tree, no \
             scheduler interpreting it at render time. Every abstraction collapses to a \
             direct call.",
            "Two more multipliers compound it. Ahead-of-time compilation to native code \
             (or WASM on web) means no JIT warmup and no interpreter. And there's no \
             bundled runtime: no JavaScript engine, no platform VM, nothing to ship \
             alongside your app and nothing to initialize before the first frame.",
        ],
        None,
    )
}

fn fine_grained() -> Element {
    let example = "let count = signal!(0);\n\
                   \n\
                   ui! {\n    \
                       View {\n        \
                           Text { \"This never re-runs when count changes\" }\n        \
                           Text { format!(\"Count: {}\", count.get()) }  // only THIS leaf updates\n    \
                       }\n\
                   }\n\
                   \n\
                   // count.set(1) writes one text node. No parent re-render,\n\
                   // no sibling re-evaluation, no subtree diff.";
    section(
        "Fine-grained, not coarse",
        vec![
            "Reactivity is built on signals, and the dependency graph is fine-grained. \
             A signal write updates exactly the primitives that read it \u{2014} not the \
             component, not its siblings, not a subtree. The cost of a state change is \
             proportional to what actually depends on that state, not to the size of the \
             screen it lives on.",
            "This is the difference that shows up under load: a list of ten thousand \
             rows where one cell changes touches one cell. There's no \"re-render the \
             component and let the diff figure out what moved\" step, because there's no \
             diff.",
        ],
        Some(example),
    )
}

fn how_measured() -> Element {
    section(
        "How we measure",
        vec![
            "Performance claims are easy to make and easy to fudge, so the benchmark \
             harness is built to be hard to cheat. Every framework renders the same \
             screen, does the same work, and is timed with the same instrumentation. \
             The rules that keep it honest:",
            "Production builds everywhere. React runs the production esm.sh bundle \
             calling the JSX runtime directly; Vue uses the runtime-only production \
             build with hand-written render functions; Svelte is AOT-compiled with \
             `dev: false`; idealyst builds release WASM with profiling features off. \
             Dev-mode equivalents are 2\u{2013}5\u{00d7} slower and are never reported.",
            "A strict resolution contract. The measured `setRows(n)` call must resolve \
             by microtask, never `requestAnimationFrame` \u{2014} a rAF wait would bake \
             ~16 ms of paint delay into the number. React's update is wrapped in \
             `flushSync` so its commit happens inside the measured window instead of \
             being batched away afterward.",
            "Honest baselines bracket the result. Three hand-written vanilla variants \
             mark the floor and ceiling: a static-className/CSS-variables version (what \
             the cascade can do with no per-row JS), an honest per-element \
             `createElement` mount, and a single bulk `innerHTML` write (the physical \
             DOM ceiling no component framework can beat). Every framework should land \
             somewhere on that spectrum, and you can see exactly where.",
        ],
        None,
    )
}

fn compared_against() -> Element {
    let rows: Vec<(&str, &str)> = vec![
        ("vanilla-css-vars", "Cascade ceiling \u{2014} static classNames referencing :root variables, no per-row JS-side style work."),
        ("vanilla-classes", "Honest per-element mount \u{2014} createElement + setAttribute per row, batched through a DocumentFragment."),
        ("vanilla-classes-bulk", "Physical DOM ceiling \u{2014} one innerHTML write hands the whole subtree to the parser. The \"no JS overhead can beat this\" line."),
        ("react-naive", "React with inline style={...} props."),
        ("react-cssvars", "React with CSS variables + static classNames."),
        ("vue", "Vue 3, :style bindings, runtime-only production build."),
        ("svelte", "Svelte 5 with $state runes, AOT-compiled."),
        ("idealyst-native", "The framework's own web backend, release WASM."),
    ];

    let mut row_children: Vec<Element> = Vec::with_capacity(rows.len() * 2);
    for (name, notes) in rows {
        row_children.push(ui! {
            Typography(content = name.to_string(), kind = idea_ui::typography_kind::H3)
        });
        row_children.push(ui! {
            Typography(content = notes.to_string(), muted = true)
        });
    }

    let mut children: Vec<Element> = vec![
        ui! { Typography(content = "What it's compared against".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "The suite ships eight variants spanning the realistic \
                competitive set plus the vanilla baselines that bracket it. Two test \
                suites run against them: a rebuild suite that alternates between row \
                counts to stress mount + teardown, and a theme-toggle suite that stresses \
                per-element style re-apply.".to_string())
        },
    ];
    children.extend(row_children);
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn reproduce() -> Element {
    let snippet = "# Builds every variant's bundle, then serves the runner.\n\
                   benchmark/serve\n\
                   \n\
                   # Open http://localhost:8080/ , pick a suite, click Run.\n\
                   # Run with DevTools CLOSED for headline numbers;\n\
                   # open the Performance tab for per-function attribution.";
    let title = ui! {
        Typography(content = "Reproduce it yourself".to_string(), kind = idea_ui::typography_kind::H2)
    };
    let para_1 = ui! {
        Typography(content = "We publish the harness, not just a headline. Numbers depend \
            on your hardware, your browser, and the day's framework releases \u{2014} so \
            rather than freeze a cherry-picked figure into this page, the whole rig is in \
            the repo under `benchmark/`. Clone it, run one command, and measure on your \
            own machine.".to_string())
    };
    let code = ui! { CodePanel(src = snippet) };
    let para_2 = ui! {
        Typography(content = "If you want to add your own framework to the comparison, the \
            contract is small: build the same screen, expose `setRows(n)` honoring the \
            microtask-resolution rule, and register the variant. The methodology, the \
            honesty rules, and the full instructions live alongside the code.".to_string())
    };
    let concepts_cta = ui! {
        Link(route = &CONCEPTS_ROUTE, params = ()) {
            Typography(content = "How the reactive core works \u{2192}".to_string())
        }
    };
    let children: Vec<Element> = vec![title, para_1, code, para_2, concepts_cta];
    ui! { Stack(gap = StackGap::Md) { children } }
}
