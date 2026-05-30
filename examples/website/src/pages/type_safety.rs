//! Absolute type safety — the concrete compile-time guarantees the
//! framework leans on. This is the "what you get" companion to Why Rust
//! (which makes the deeper language-shape argument); this page stays
//! focused on enumerable guarantees and links out for the philosophy.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{CodePanel, PageHeader, PageSection};
use crate::routes::{SERVER_FUNCTIONS_ROUTE, WHY_RUST_ROUTE};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let contract_ref: Ref<ViewHandle> = Ref::new();
    let invalid_ref: Ref<ViewHandle> = Ref::new();
    let exhaustive_ref: Ref<ViewHandle> = Ref::new();
    let refs_ref: Ref<ViewHandle> = Ref::new();
    let styles_ref: Ref<ViewHandle> = Ref::new();
    let next_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: contract_ref, label: "The signature is the contract" },
        TocEntry { handle: invalid_ref, label: "Invalid states can't compile" },
        TocEntry { handle: exhaustive_ref, label: "Exhaustiveness, codebase-wide" },
        TocEntry { handle: refs_ref, label: "Refs you can't misuse" },
        TocEntry { handle: styles_ref, label: "Styles and themes are typed" },
        TocEntry { handle: next_ref, label: "Where to go from here" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Absolute type safety",
                blurb = "The same type system that makes Rust safe makes idealyst apps hard to \
                 get wrong. The function signature is the contract \u{2014} across the \
                 network, across the component boundary, across a theme switch. Whole \
                 categories of UI bug stop being runtime surprises and start being \
                 compile errors.",
            )
            PageSection(handle = contract_ref) { contract() }
            PageSection(handle = invalid_ref) { invalid_states() }
            PageSection(handle = exhaustive_ref) { exhaustiveness() }
            PageSection(handle = refs_ref) { refs() }
            PageSection(handle = styles_ref) { styles() }
            PageSection(handle = next_ref) { where_next() }
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

fn contract() -> Element {
    let example = "// A component's props are a typed struct. The compiler checks\n\
                   // every call site against it \u{2014} no untyped prop bags.\n\
                   ui! { button(label = \"Save\".to_string(), on_click = on_save) }\n\
                   \n\
                   // A server function's signature is the wire contract. The same\n\
                   // types are checked on the client (the RPC stub) and the server\n\
                   // (the handler) \u{2014} they cannot drift out of sync.\n\
                   #[server]\n\
                   async fn save_todo(input: NewTodo) -> Result<Todo, ServerError> { ... }";
    section(
        "The signature is the contract",
        vec![
            "Everything you pass across a boundary is typed, and the compiler enforces \
             it on both sides of that boundary. Component props are a real struct, not a \
             stringly-typed bag \u{2014} pass the wrong type, misspell a field, or omit \
             a required one and the build fails with a precise message.",
            "The same idea scales up to the network. A server function's signature is \
             the wire contract: the client call site and the server handler are \
             generated from one declaration, so a request and its handler can never \
             disagree about argument or return shape. You don't maintain a client API, a \
             server API, and a DTO crate in lockstep \u{2014} there's one source of \
             truth and it's type-checked.",
        ],
        Some(example),
    )
}

fn invalid_states() -> Element {
    let example = "// In a dynamically-typed world, every combination is constructible:\n\
                   { loading: true,  data: result, error: \"oops\" }  // ...valid?!\n\
                   \n\
                   // With a sum type, the nonsense states simply don't exist:\n\
                   enum FetchState<T> {\n    \
                       Idle,\n    \
                       Loading,\n    \
                       Loaded(T),\n    \
                       Error(String),\n\
                   }";
    section(
        "Invalid states can't compile",
        vec![
            "UI state modeled as a bag of optional flags admits combinations that should \
             never happen \u{2014} loading and loaded at once, data and error together. \
             Each of those is a latent bug waiting for the wrong sequence of events.",
            "Modeled as a sum type, the state is exactly one of its variants. \"Loading \
             and loaded simultaneously\" isn't a bug you guard against; it's a value the \
             type system won't let you construct. The impossible states are gone before \
             you write a single guard.",
        ],
        Some(example),
    )
}

fn exhaustiveness() -> Element {
    let example = "let view = match fetch_state.get() {\n    \
                       FetchState::Idle       => idle_view(),\n    \
                       FetchState::Loading    => spinner(),\n    \
                       FetchState::Loaded(d)  => results_view(d),\n    \
                       FetchState::Error(msg) => error_view(msg),\n\
                   };\n\
                   \n\
                   // Add `FetchState::Cached(T)` later, and EVERY match over\n\
                   // FetchState becomes a compile error until you handle it.";
    section(
        "Exhaustiveness, codebase-wide",
        vec![
            "A `match` over a state enum must cover every variant. That turns rendering \
             into a checked switch: the compiler guarantees you handled idle, loading, \
             loaded, and error before the code builds.",
            "The guarantee holds as the code evolves. Add a new variant six months \
             later and every `match` site across the entire codebase that doesn't handle \
             it is a compile error \u{2014} a worklist the compiler hands you for free. \
             The class of bug where you add a state and forget to render it somewhere \
             doesn't survive `cargo build`.",
        ],
        Some(example),
    )
}

fn refs() -> Element {
    let example = "// You never hold a raw handle. You read it through a closure,\n\
                   // and only while the node is actually mounted:\n\
                   btn_ref.with(|handle| handle.focus());\n\
                   \n\
                   // Returns Option<R> \u{2014} None when the button isn't mounted.\n\
                   // There's no way to stash a handle and call .focus() later,\n\
                   // after the component might already be gone.";
    section(
        "Refs you can't misuse",
        vec![
            "A `Ref<H>` to a node's handle doesn't expose the handle directly. The read \
             API is a closure the framework runs only if the node is mounted right now \
             \u{2014} the borrow can't escape it.",
            "That shape is enforced at the type level, so the entire family of \
             \"used a ref after the component unmounted\" crashes is unreachable. You \
             can't accidentally call a method on a handle that's been torn down, because \
             you were never able to hold onto it in the first place.",
        ],
        Some(example),
    )
}

fn styles() -> Element {
    let example = "// Variants and states are typed axes on the stylesheet,\n\
                   // not magic strings the compiler can't see.\n\
                   let style = NavLink().active(derived(move || {\n        \
                       if is_current.get() { NavLinkActive::On } else { NavLinkActive::Off }\n    \
                   }));";
    section(
        "Styles and themes are typed",
        vec![
            "Styling goes through `stylesheet!`, which gives each style a typed surface: \
             variants and states are enums, not class-name strings you hope match \
             something. Select the wrong variant and it's a compile error, not a \
             silently-missing rule at runtime.",
            "Theme tokens are typed too. A token resolves to a concrete `Color`, \
             `Length`, or scalar, and reading one subscribes the surrounding reactive \
             scope so a theme switch re-resolves it automatically \u{2014} no untyped \
             CSS-variable lookups that fail quietly when a name drifts.",
        ],
        Some(example),
    )
}

fn where_next() -> Element {
    let title = ui! {
        Typography(content = "Where to go from here".to_string(), kind = idea_ui::typography_kind::H2)
    };
    let para = ui! {
        Typography(content = "These guarantees are the practical payoff of the language's \
            shape. Why Rust makes the deeper argument \u{2014} why expressions, pattern \
            matching, ownership, and a real macro system fit UI authoring. The Server \
            functions page shows the type contract stretched across the network.".to_string())
    };
    let why_cta = ui! {
        link(route = &WHY_RUST_ROUTE, params = ()) {
            Typography(content = "Read \u{2192} Why Rust".to_string())
        }
    };
    let srv_cta = ui! {
        link(route = &SERVER_FUNCTIONS_ROUTE, params = ()) {
            Typography(content = "The signature as a wire contract \u{2192}".to_string())
        }
    };
    let children: Vec<Element> = vec![title, para, why_cta, srv_cta];
    ui! { Stack(gap = StackGap::Md) { children } }
}
