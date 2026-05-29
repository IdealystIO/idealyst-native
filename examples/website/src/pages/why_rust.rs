//! Why Rust — the rationale page that lives between Core concepts
//! and Demos. Argues the standard Rust benefits (memory safety, no
//! GC, native compilation) in one paragraph, then spends the rest of
//! the page on the deeper argument: the language's *shape* fits UI
//! authoring in ways most languages don't.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{CodePanel, PageHeader, PageSection};
use crate::routes::QUICKSTART_ROUTE;
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    // One `Ref<ViewHandle>` per section. The same handle is stored
    // in the `TocEntry` (so the spy can read its `absolute_frame`)
    // and passed to `PageSection` (which binds it to the
    // section's outer `View`). `Ref<H>` is `Copy`, so both reads
    // share the same slot.
    let standard: Ref<ViewHandle> = Ref::new();
    let pivot_ref: Ref<ViewHandle> = Ref::new();
    let expressions: Ref<ViewHandle> = Ref::new();
    let pattern: Ref<ViewHandle> = Ref::new();
    let enums: Ref<ViewHandle> = Ref::new();
    let macros: Ref<ViewHandle> = Ref::new();
    let closures: Ref<ViewHandle> = Ref::new();
    let refs: Ref<ViewHandle> = Ref::new();
    let traits: Ref<ViewHandle> = Ref::new();
    let zero_cost: Ref<ViewHandle> = Ref::new();
    let tradeoff: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: standard, label: "The boilerplate, briefly" },
        TocEntry { handle: pivot_ref, label: "Shape of the language fits UI" },
        TocEntry { handle: expressions, label: "Expressions, not statements" },
        TocEntry { handle: pattern, label: "Pattern matching as render switch" },
        TocEntry { handle: enums, label: "Enums and invalid states" },
        TocEntry { handle: macros, label: "Macros over explicit code" },
        TocEntry { handle: closures, label: "Closures with explicit capture" },
        TocEntry { handle: refs, label: "Ownership for refs" },
        TocEntry { handle: traits, label: "Traits, not inheritance" },
        TocEntry { handle: zero_cost, label: "The macro expansion IS the runtime" },
        TocEntry { handle: tradeoff, label: "The tradeoff" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Why Rust",
                blurb = "The standard answer is memory safety, no garbage collector, and native \
                 compilation. That's true, and reason enough. But the deeper reason \
                 idealyst is written in Rust is that the language's shape fits UI \
                 authoring in ways most languages don't.",
            )
            PageSection(handle = standard) { standard_story() }
            PageSection(handle = pivot_ref) { pivot() }
            PageSection(handle = expressions) { expressions_section() }
            PageSection(handle = pattern) { pattern_matching_section() }
            PageSection(handle = enums) { enums_section() }
            PageSection(handle = macros) { macros_section() }
            PageSection(handle = closures) { closures_section() }
            PageSection(handle = refs) { refs_section() }
            PageSection(handle = traits) { traits_section() }
            PageSection(handle = zero_cost) { zero_cost_section() }
            PageSection(handle = tradeoff) { tradeoff_section() }
        }
    };
    layout_with_toc(content, toc)
}

// =============================================================================
// Section helpers — each section is a heading + prose + optional code panel.
// =============================================================================

fn section(title: &str, paragraphs: Vec<&str>, code: Option<&str>) -> Element {
    let mut children: Vec<Element> = Vec::new();
    let title_text = title.to_string();
    children.push(ui! {
        Typography(content = title_text, kind = idea_ui::typography_kind::H2)
    });
    for p in paragraphs {
        let body = p.to_string();
        // Default kind = `Body` (14 px) — the site-wide paragraph size
        // (concepts, backends, install, server-functions, …). The page
        // lead blurb gets `BodyLg` via `PageHeader`; section prose
        // does not, or body copy reads inconsistently large.
        children.push(ui! { Typography(content = body) });
    }
    if let Some(src) = code {
        children.push(ui! { CodePanel(src = src) });
    }
    // `Lg` (16 px): comfortable gap between the H2 heading, body
    // paragraphs, and the code panel within a single section.
    ui! { Stack(gap = StackGap::Lg) { children } }
}

// =============================================================================
// Sections
// =============================================================================

fn standard_story() -> Element {
    section(
        "The boilerplate, briefly",
        vec![
            "Rust gives you memory safety without a garbage collector, ahead-of-time \
             compilation to native code on every target, and zero-cost abstractions. \
             UI frameworks are unusually sensitive to all three: GC pauses show up as \
             dropped frames, runtime overhead competes with the rendering hot path, and \
             bundle size matters on web. Picking Rust dodges all three at once.",
            "That's the elevator pitch. Most other Rust pitches stop here. The rest of \
             this page is the part that doesn't usually get said.",
        ],
        None,
    )
}

fn pivot() -> Element {
    section(
        "The shape of the language fits UI",
        vec![
            "Beyond the runtime properties, Rust has a set of language-design choices \
             that line up unusually well with how UI authoring actually works. Most of \
             these aren't unique to Rust on their own, but the combination \u{2014} \
             expressions, pattern matching, enums, ownership, traits, and a real macro \
             system \u{2014} compose into a language that doesn't fight you when you're \
             writing components.",
        ],
        None,
    )
}

fn expressions_section() -> Element {
    let example = "// Every block in Rust evaluates to a value.\n\
                   let label = if count > 9 { \"9+\".to_string() } else { count.to_string() };\n\
                   ui! { Text { label } }\n\
                   \n\
                   // Compare to a language where `if` is a statement:\n\
                   let label;\n\
                   if (count > 9) {\n    label = \"9+\";\n} else {\n    label = String(count);\n}\n\
                   return <span>{label}</span>;";
    section(
        "Expressions, not statements",
        vec![
            "UI is a function of state. State branches; the view branches with it. In a \
             language where `if` is a statement, every branch forces you to introduce a \
             mutable variable, conditionally assign it, then reference it downstream \u{2014} \
             three steps for what's fundamentally one transformation.",
            "In Rust, every block is an expression. `if`, `match`, `loop`, and braced \
             blocks all evaluate to a value, and that value can flow directly into a \
             component prop, a `let` binding, or a child slot.",
        ],
        Some(example),
    )
}

fn pattern_matching_section() -> Element {
    let example = "let view = match fetch_state.get() {\n    \
                       FetchState::Idle => idle_view(),\n    \
                       FetchState::Loading => spinner(),\n    \
                       FetchState::Loaded(data) => results_view(data),\n    \
                       FetchState::Error(msg) => error_view(msg),\n\
                   };";
    section(
        "Pattern matching as a render switch",
        vec![
            "The same expression-orientation extends to enumerated states. Loading? \
             Loaded? Error? A `match` over your state enum is the natural shape for \
             picking what to render, and the matched data is destructured inline so the \
             render branch sees the typed payload immediately.",
            "The compiler enforces exhaustiveness. Add a new variant later \u{2014} \
             `FetchState::Cached(stale_data)` \u{2014} and every `match` site in the \
             codebase is a compile error until you handle it. The class of bugs where \
             you add a new state and forget to render it doesn't survive `cargo build`.",
        ],
        Some(example),
    )
}

fn enums_section() -> Element {
    let example = "// In JavaScript, every combination is reachable:\n\
                   { loading: true,  data: null,    error: null    }  // valid\n\
                   { loading: false, data: result,  error: null    }  // valid\n\
                   { loading: false, data: null,    error: \"...\"   }  // valid\n\
                   { loading: true,  data: result,  error: \"...\"   }  // also valid?!\n\
                   \n\
                   // In Rust, the type system forbids the nonsense:\n\
                   enum FetchState<T> {\n    \
                       Idle,\n    \
                       Loading,\n    \
                       Loaded(T),\n    \
                       Error(String),\n\
                   }";
    section(
        "Enums make invalid states unrepresentable",
        vec![
            "Pair `match` with Rust's sum types and a whole class of state bugs \
             disappears. The typical JavaScript loading state is a bag of optional flags, \
             and every combination of `loading`, `data`, and `error` is a constructible \
             value \u{2014} including the nonsensical ones.",
            "Rust's enum says the state is exactly one of four shapes. You cannot \
             construct \"loading and loaded simultaneously\" because the type doesn't \
             admit it. UI state stops drifting into impossible territory.",
        ],
        Some(example),
    )
}

fn macros_section() -> Element {
    let example = "// What you write:\n\
                   ui! {\n    Button(label = \"Hi\".to_string(), on_click = on_press)\n}\n\
                   \n\
                   // What the macro expands to:\n\
                   button(&ButtonProps {\n    \
                       label: \"Hi\".to_string(),\n    \
                       on_click: on_press,\n    \
                       ..Default::default()\n\
                   })";
    section(
        "Macros over explicit code",
        vec![
            "`ui!` is a macro. So are `stylesheet!`, `#[component]`, `signal!`, and the \
             rest of idealyst's authoring surface. Each is syntactic sugar that desugars \
             to plain function calls against props structs.",
            "The macro is opt-in. You can drop down to the explicit form any time and \
             the framework is identical \u{2014} same primitives, same signals, same \
             output. JSX in React started as the obvious way and quietly became the \
             only way. Rust's macros are sugar by construction; the explicit form is \
             always legible and writable.",
            "This matters for tooling and inspection. When a build error mentions a \
             type mismatch, you can read the desugared call directly without learning a \
             second mental model.",
        ],
        Some(example),
    )
}

fn closures_section() -> Element {
    let example = "// `move` is explicit \u{2014} you see what the closure captures.\n\
                   let on_click = move || count.update(|n| *n += 1);\n\
                   \n\
                   // Compare JavaScript, where capture is implicit:\n\
                   const onClick = () => setCount(c => c + 1);\n\
                   // — which render's `setCount` does this point to?\n\
                   // — if the parent re-renders, does this closure still work?";
    section(
        "Closures with explicit capture",
        vec![
            "Event handlers are closures. `move ||` captures by ownership; without \
             `move`, the closure borrows. The keyword is surface-visible \u{2014} you \
             know exactly which signals the handler holds onto, and the borrow checker \
             enforces it.",
            "React's stale-closure bug \u{2014} an effect or callback capturing an old \
             render's variables \u{2014} is a direct consequence of implicit capture. \
             In Rust, the moment you write `move`, you've declared the boundary, and \
             the framework can store the closure and call it later with no risk of \
             pointing at a stale binding.",
        ],
        Some(example),
    )
}

fn refs_section() -> Element {
    let example = "// You don't get the handle directly. You read it through a closure:\n\
                   btn_ref.with(|handle| handle.focus());\n\
                   \n\
                   // Returns Option<R> \u{2014} `None` when the button isn't mounted.\n\
                   // There's no way to get a raw handle out and call .focus() on it\n\
                   // later, after the button might have been torn down.";
    section(
        "Ownership for refs and handles",
        vec![
            "A `Ref<ButtonHandle>` in idealyst doesn't expose the handle directly. The \
             read API is a closure: you hand the ref a function that takes the handle, \
             and the framework runs it only if the button is mounted right now.",
            "The shape is enforced at the type level. There's no way to extract a raw \
             handle and call a method on it later \u{2014} the borrow can't outlive the \
             closure. A whole category of \"used a ref after the component \
             unmounted\" crashes is gone.",
        ],
        Some(example),
    )
}

fn traits_section() -> Element {
    section(
        "Traits, not inheritance",
        vec![
            "`Backend` is a trait. So is `IdeaTheme`, `IntoStyleSource`, `RouteParams`, \
             `IntoElement`. Adding a new platform, a new theme implementation, or a \
             new style source means implementing a trait \u{2014} no class extension, no \
             method-override resolution, no diamond inheritance, no `super` calls.",
            "Composition over inheritance isn't a moral guideline in Rust; it's the \
             only option the language gives you. The framework's contracts are surface- \
             visible because every contract is a trait you can read top to bottom in \
             one file.",
        ],
        None,
    )
}

fn zero_cost_section() -> Element {
    section(
        "The macro expansion IS the runtime",
        vec![
            "`ui! { Button(label = \"Hi\") }` becomes `button(&ButtonProps { ... })` \
             becomes a `Element::Button { ... }` constructor. No virtual DOM diff, no \
             JSX-to-element transformation, no reflection pass, no decorator metadata. \
             You read the macro as one thing; the compiler reads it as another; the \
             machine code is the second one.",
            "There's no shared runtime to interpret, no scheduler that owns your \
             component tree, no abstraction layer the framework lazily resolves at \
             render time. Every abstraction the macros expose collapses to a direct \
             call against a primitive constructor.",
        ],
        None,
    )
}

fn tradeoff_section() -> Element {
    let cta = ui! {
        Link(route = &QUICKSTART_ROUTE, params = ()) {
            Typography(
                content = "Try the Quickstart \u{2192}".to_string(),
                tone = Some(idea_ui::tone::Primary.into()),
            )
        }
    };
    let title = ui! {
        Typography(content = "The tradeoff".to_string(), kind = idea_ui::typography_kind::H2)
    };
    let para_1 = ui! {
        Typography(content = "Rust costs something. You need the toolchain installed (rustup is one command, but it's \
            not nothing). The first compile of a fresh project is slow \u{2014} cargo builds the framework \
            crates from scratch. And if you've never seen the language before, there's a learning curve: \
            ownership, lifetimes, the borrow checker.".to_string())
    };
    let para_2 = ui! {
        Typography(content = "The framework's macros hide most of the day-to-day friction, and the type system catches \
            entire categories of bugs before you run the code. We think the leverage above earns the cost. \
            The Quickstart is the fastest way to find out for yourself.".to_string())
    };
    let children: Vec<Element> = vec![title, para_1, para_2, cta];
    ui! { Stack(gap = StackGap::Md) { children } }
}
