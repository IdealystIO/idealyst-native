//! Server-side rendering — render any tree to HTML + CSS on the server,
//! match the web backend's first paint, then hand off to the live app
//! by adopting the server DOM. Honest about status: the SSR backend
//! works and is exercised by this very site; in-place hydration is a
//! working prototype, not yet a turnkey production path.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{code_panel, page_header, page_section};
use crate::routes::CODE_SPLITTING_ROUTE;
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let render_ref: Ref<ViewHandle> = Ref::new();
    let firstpaint_ref: Ref<ViewHandle> = Ref::new();
    let hydrate_ref: Ref<ViewHandle> = Ref::new();
    let serve_ref: Ref<ViewHandle> = Ref::new();
    let status_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: render_ref, label: "Render on the server" },
        TocEntry { handle: firstpaint_ref, label: "The same first paint" },
        TocEntry { handle: hydrate_ref, label: "Hydration by adoption" },
        TocEntry { handle: serve_ref, label: "Serving it" },
        TocEntry { handle: status_ref, label: "Status" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Server-side rendering",
                "Render the app to fully-styled HTML on the server for a fast, \
                 crawlable first paint \u{2014} then let the WASM bundle adopt that exact \
                 DOM and bring it to life, instead of throwing it away and re-rendering. \
                 The page you're reading is served this way."
            ) }
            { page_section(render_ref, vec![render_on_server()]) }
            { page_section(firstpaint_ref, vec![first_paint()]) }
            { page_section(hydrate_ref, vec![hydration()]) }
            { page_section(serve_ref, vec![serving()]) }
            { page_section(status_ref, vec![status()]) }
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
        children.push(code_panel(src));
    }
    ui! { Stack(gap = StackGap::Lg) { children } }
}

// =============================================================================
// Sections
// =============================================================================

fn render_on_server() -> Element {
    let snippet = "use backend_ssr::{render_path_with, render_document};\n\
                   \n\
                   // Render the SAME app() the web bundle mounts \u{2014} at a URL,\n\
                   // on the host, against the SSR backend.\n\
                   let page = render_path_with(\"/features/ssr\", register_exts, my_app::app);\n\
                   let html = render_document(&page, Some(\"/pkg/app.js\"));\n\
                   // \u{2192} a complete <html> document: markup + scoped CSS + font links.";
    section(
        "Render on the server",
        vec![
            "The SSR backend is just another `Backend`. It walks the same primitive \
             tree your app produces and emits a complete HTML document \u{2014} markup, \
             the scoped CSS for every style rule the tree used, and the font links it \
             needs \u{2014} for any route, on the host, with no browser involved.",
            "Because it renders at a path, a complex app with a navigator renders \
             correctly per URL: request `/features/ssr` and you get that screen's \
             document, request `/quickstart` and you get that one. The reactive arena, \
             token registry, and scheduler are thread-local, so each route can render in \
             full isolation with no cross-render contamination.",
        ],
        Some(snippet),
    )
}

fn first_paint() -> Element {
    section(
        "The same first paint",
        vec![
            "SSR output isn't an approximation of the app \u{2014} it's the web \
             backend's own first paint, produced by the same style system. The server \
             resolves the same theme tokens, emits the same class names, and lays text \
             out with the same fonts, so the HTML a crawler or a cold browser sees is \
             pixel-for-pixel what the live app would have painted on frame one.",
            "That buys you the things a pure client-rendered WASM app gives up: real \
             content in the initial response for search engines and link unfurlers, and \
             a meaningful first paint before a single byte of WASM has executed. The \
             user sees the page; the interactivity catches up.",
        ],
        None,
    )
}

fn hydration() -> Element {
    section(
        "Hydration by adoption",
        vec![
            "The expensive mistake most SSR setups make is re-rendering on the client: \
             the server sends HTML, then the framework boots, ignores that HTML, builds \
             its own tree, and replaces the DOM \u{2014} a flash and a pile of wasted \
             work. Idealyst hydrates by adoption instead. The booting WASM walks the \
             server-rendered DOM and binds its reactive primitives to the nodes that are \
             already there, in place.",
            "Adoption only works if the client agrees with the server about what the \
             tree should be, down to layout. The framework keeps that agreement with \
             viewport-determinism \u{2014} the client's first measurement matches the \
             server's assumptions \u{2014} so nodes line up and the bundle can claim the \
             existing DOM rather than rebuild it.",
        ],
        None,
    )
}

fn serving() -> Element {
    let snippet = "use backend_ssr::{serve, ServeConfig};\n\
                   \n\
                   serve(\n    \
                       \"127.0.0.1:8787\",\n    \
                       ServeConfig {\n        \
                           // Built web bundle to boot + hydrate the server DOM:\n        \
                           bundle_module: Some(\"/pkg/website.js\".into()),\n        \
                           static_dir: Some(\"dist/web\".into()),  // fonts + bundle\n    \
                       },\n    \
                       register_exts,   // same extensions the web build registers\n    \
                       website::app,\n\
                   )?;\n\
                   // Each request is SSR-rendered for its route; the bundle then\n\
                   // boots and hydrates by adopting that document.";
    section(
        "Serving it",
        vec![
            "`serve(...)` stands up an HTTP server that renders the right route per \
             request and returns a fully-styled document. Point `static_dir` at a built \
             web bundle (`idealyst build --web`) and the page also boots that bundle and \
             hydrates; leave it off and you get a pure server-rendered content / SEO \
             preview with no client JS at all.",
            "The one rule for matching output: register the same extensions the web \
             build registers. SSR renders identically to the client only when both sides \
             agree on the navigator chrome, the code-block renderer, and any other \
             `Element::External` leaf in the tree.",
        ],
        Some(snippet),
    )
}

fn status() -> Element {
    let title = ui! {
        Typography(content = "Status".to_string(), kind = idea_ui::typography_kind::H2)
    };
    let para_1 = ui! {
        Typography(content = "The SSR backend is real and in active use: it renders this \
            marketing site \u{2014} a full DrawerNavigator app \u{2014} per route, and \
            render-at-path is proven across every page. What you can rely on today is \
            server rendering: HTML + scoped CSS + fonts for any route, matching the web \
            first paint.".to_string())
    };
    let para_2 = ui! {
        Typography(content = "In-place hydration (the DOM-adoption path above) is a working \
            prototype rather than a turnkey production feature \u{2014} the demo lives in \
            `examples/hydration-demo` and proves DOM adoption plus viewport-determinism \
            end to end. Expect the ergonomics around wiring it into a scaffolded project \
            to keep firming up.".to_string())
    };
    let split_cta = ui! {
        Link(route = &CODE_SPLITTING_ROUTE, params = ()) {
            Typography(content = "Pairs well with code splitting \u{2192}".to_string())
        }
    };
    let children: Vec<Element> = vec![title, para_1, para_2, split_cta];
    ui! { Stack(gap = StackGap::Md) { children } }
}
