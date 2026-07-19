//! Navigation — the two navigator SDKs (swap / stack) on the outlet
//! model, routes & params, chrome as author layout, and the native
//! machinery each backend drives. This very site is a swap navigator
//! wrapped in an `idea_ui_nav::AppShell`, so the page documents the
//! API it's built on.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{CodePanel, PageHeader, PageSection};
use crate::routes::{BACKENDS_ROUTE, CONCEPTS_ROUTE, CROSS_PLATFORM_ROUTE};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let pitch_ref: Ref<ViewHandle> = Ref::new();
    let navigators_ref: Ref<ViewHandle> = Ref::new();
    let routes_ref: Ref<ViewHandle> = Ref::new();
    let example_ref: Ref<ViewHandle> = Ref::new();
    let options_ref: Ref<ViewHandle> = Ref::new();
    let chrome_ref: Ref<ViewHandle> = Ref::new();
    let next_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: pitch_ref, label: "Two behaviors, native feel" },
        TocEntry { handle: navigators_ref, label: "Chrome is author layout" },
        TocEntry { handle: routes_ref, label: "Routes & typed params" },
        TocEntry { handle: example_ref, label: "Building a navigator" },
        TocEntry { handle: options_ref, label: "Stacks & screen options" },
        TocEntry { handle: chrome_ref, label: "How it maps per platform" },
        TocEntry { handle: next_ref, label: "Where to go from here" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Navigation",
                blurb = "Two navigator behaviors — swap (co-equal screens) and stack \
                 (push/pop) — cover every navigation shape, with chrome as ordinary \
                 author layout around the navigator's outlet. Navigators are SDK \
                 crates, not core primitives, and each backend drives its own native \
                 machinery: `UINavigationController` on iOS, the Fragment back-stack \
                 on Android, the browser History API on web. This site itself is a \
                 swap navigator inside an `AppShell`.",
            )
            PageSection(handle = pitch_ref) { pitch() }
            PageSection(handle = navigators_ref) { navigators() }
            PageSection(handle = routes_ref) { routes() }
            PageSection(handle = example_ref) { example() }
            PageSection(handle = options_ref) { options() }
            PageSection(handle = chrome_ref) { chrome() }
            PageSection(handle = next_ref) { where_next() }
        }
    };
    layout_with_toc(content, toc)
}

// ============================================================================
// Sections
// ============================================================================

fn pitch() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Two behaviors, native feel".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Navigation is the place cross-platform frameworks usually \
                leak. A webview app fakes a navigation bar in HTML; a lowest-common-\
                denominator toolkit gives you one generic stack on every platform. Idealyst \
                reduces navigation to its two real behaviors and keeps the native machinery \
                underneath: a swap navigator switches between co-equal screens (the substance \
                of every tab bar, drawer, sidebar, and wizard), and a stack navigator pushes \
                and pops with depth — with a true `UINavigationController` swipe-back on iOS \
                and the Fragment back-stack on Android inside its outlet.".to_string())
            Typography(content = "Navigators live in SDK crates (`swap-navigator`, \
                `stack-navigator`) rather than the framework core, in keeping with the rule \
                that core stays minimal. You depend on the ones you use; their handlers are \
                backend-neutral and self-register, and the framework routes navigation \
                commands to the right machinery on every target.".to_string())
        }
    }
}

fn navigators() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Chrome is author layout".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "There is no tab navigator and no drawer navigator. Each \
                navigator hands its `.layout(|nav| …)` closure a single outlet — the analog \
                of react-router's `<Outlet/>` — plus reactive nav state (`active_route`, \
                `can_go_back`, …) and the commands to wire buttons to. What used to be \
                per-kind chrome is now layout you own: put a bar under `{nav.outlet}` and \
                you have tabs; wrap it in a side panel and you have a drawer.".to_string())
            Typography(content = "Swap navigator".to_string(), kind = idea_ui::typography_kind::H3)
            Typography(content = "A flat set of co-equal screens switched with `Select` — no \
                depth. Inactive screens can stay mounted (`LazyPersistent`, the tab default: \
                switching back is instant, state intact) or be disposed (`LazyDisposing`, \
                browser-style). Links inside a swap screen switch, never push.".to_string())
            Typography(content = "Stack navigator".to_string(), kind = idea_ui::typography_kind::H3)
            Typography(content = "Push/pop with a back-stack — the workhorse for drill-down \
                flows: a list that opens a detail, a settings tree, a checkout. Native \
                swipe-back on iOS, system back on Android, browser back on web all pop it. \
                Covered screens stay alive on native and rebuild from the URL on web, each \
                per its platform's semantics.".to_string())
            Typography(content = "Themed chrome, ready made".to_string(), kind = idea_ui::typography_kind::H3)
            Typography(content = "`idea_ui_nav` packages the common shapes so you rarely \
                hand-roll them: `TabBar` (wired to the swap context), `Drawer` (sliding \
                panel + scrim), `StackHeader` (title + back + header slots; self-suppresses \
                where a native bar renders), and `AppShell` — the responsive pinned-sidebar \
                ⇄ off-canvas-drawer shell whose breakpoint split compiles to real `@media` \
                rules, so the SSR first paint is viewport-correct with no JS.".to_string())
        }
    }
}

fn routes() -> Element {
    let snippet = "use runtime_core::Route;\n\
                   \n\
                   // A route with no params:\n\
                   const HOME: Route<()> = Route::<()>::new(\"home\", \"/\");\n\
                   \n\
                   // A route that carries typed params — the param type is part\n\
                   // of the route, so a push with the wrong shape doesn't compile:\n\
                   const USER: Route<UserParams> = Route::<UserParams>::new(\"user\", \"/users/:id\");\n\
                   \n\
                   // Navigate via the navigator handle — the param type is checked:\n\
                   nav.push(&USER, UserParams { id: 42 });";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Routes & typed params".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "A `Route<P>` pairs a stable name (the in-navigator key, used \
                for active-route highlighting) with a URL path and a typed parameter `P`. \
                Because the param type is baked into the route constant, navigating to a \
                route with the wrong param shape is a compile error — there's no \
                stringly-typed params bag to get out of sync. The `link` primitive and the \
                navigator handles both take a `&Route<P>` plus its params.".to_string())
            CodePanel(src = snippet)
        }
    }
}

fn example() -> Element {
    let snippet = "let nav: Ref<SwapHandle> = Ref::new();\n\
                   let drawer_open = signal(false);\n\
                   \n\
                   let builder = SwapNavigator::new(&HOME)\n    \
                       .screen(HOME,  move |_| Screen::new(home_page()))\n    \
                       .screen(ABOUT, move |_| Screen::new(about_page()))\n    \
                       .screen(USER,  move |params| Screen::new(user_page(params)))\n    \
                       // The layout owns the chrome and splats the outlet once.\n    \
                       // AppShell = pinned sidebar on desktop, drawer on mobile;\n    \
                       // the sidebar builds ONCE and survives every navigation:\n    \
                       .layout(move |nav| ui! {\n        \
                           AppShell(sidebar = vec![sidebar(nav.active_route)],\n                 \
                                    is_open = drawer_open) {\n            \
                               { nav.outlet }\n        \
                           }\n    \
                       });\n\
                   \n\
                   ui! { builder.bind(nav) }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Building a navigator".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "A navigator is built with a fluent builder: name the \
                initial route, register one `.screen(route, builder)` per destination, hand \
                `.layout(...)` the chrome that wraps the outlet, and `.bind(handle)` it so \
                the rest of your app can navigate imperatively. The screen builder closure \
                receives the route's typed params and returns a `Screen` wrapping that \
                page's `Element`. This is verbatim the shape this site's own shell \
                uses.".to_string())
            CodePanel(src = snippet)
            Typography(content = "The outlet is a one-shot value: splat `{nav.outlet}` \
                exactly once, in one stable spot, and restyle the chrome around it for \
                responsive layouts (that's what `AppShell` does — pin/unpin is pure static \
                breakpoint styling, so crossing the breakpoint never remounts the sidebar \
                or the outlet). The layout closure's nav state (`active_route`, `depth`, \
                `can_go_back`) is reactive, so chrome like an active-link highlight is an \
                ordinary signal read.".to_string(),
                muted = true)
        }
    }
}

fn options() -> Element {
    let snippet = "// Per-screen header chrome rides on the Screen, via StackScreenExt:\n\
                   Screen::new(details_page())\n    \
                       .title(\"Details\")            // native bar title on mobile;\n                                    \
                   // StackHeader title on web/desktop\n    \
                       .header_right(HeaderButton::text(\"Edit\").on_press(edit))\n    \
                       .back_enabled(false)         // lock swipe-back / system back\n\
                   \n\
                   // The layout renders one StackHeader for every screen; it reads\n\
                   // the active screen's slots and self-suppresses where a native\n\
                   // bar already renders them:\n\
                   .layout(|nav| ui! {\n    \
                       view {\n        \
                           StackHeader(\n            \
                               state = rx!(header_state(&nav.screen_chrome)),\n            \
                               show_back = nav.can_go_back,\n            \
                               on_back = Some(nav.pop.clone()),\n        \
                           )\n        \
                           { nav.outlet }\n    \
                       }\n\
                   })";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Stacks & screen options".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "A stack screen's chrome — its title, header slots, \
                back-lock, full-screen request — is configured through typed screen options \
                on the `Screen` itself, and surfaces in two places from the one \
                declaration: the native `UINavigationController` / back-stack machinery on \
                mobile (transitions, gestures, back-lock), and the author's `StackHeader` \
                everywhere it draws. The bar you see is the same author component on every \
                backend; the mechanics underneath are each platform's own.".to_string())
            CodePanel(src = snippet)
        }
    }
}

fn chrome() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "How it maps per platform".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "iOS and Android: the author `.layout(...)` renders exactly \
                as everywhere else, and INSIDE the stack's outlet lives the real native \
                machinery — a `UINavigationController` (interactive swipe-back, native \
                push/pop transitions) or the Fragment back-stack (system back button, \
                back-lock) — with the native bar hidden so the author's `StackHeader` is \
                the one header everywhere. Web: URL sync is substrate-provided and \
                automatic — pushes and selects write real History entries, back/forward \
                reconcile into ordinary nav commands, deep links cold-start the right \
                screen, and scroll positions restore on back.".to_string())
            Typography(content = "SSR renders the same shell per route — `AppShell`'s \
                pinned-vs-drawer split is static `@media` styling, so the server-rendered \
                first paint is already viewport-correct. Desktop targets render the same \
                author chrome as every other backend; there is no per-platform chrome \
                fork to keep in sync.".to_string())
            Typography(content = "Both navigator SDKs ship one backend-neutral handler plus \
                the native mobile surfaces; the Backends page has the per-platform \
                status.".to_string(),
                muted = true)
        }
    }
}

fn where_next() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Where to go from here".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Navigators are the reference example of the framework's \
                extension model — peripheral features live in SDK crates on top of core, not \
                inside it. The Core concepts page covers that app/host/extension split.".to_string())
            link(route = &CONCEPTS_ROUTE, params = ()) {
                Typography(content = "Core concepts \u{2192}".to_string())
            }
            Typography(content = "Why the same author code produces native mechanics on every \
                target instead of a faked-up bar:".to_string())
            link(route = &CROSS_PLATFORM_ROUTE, params = ()) {
                Typography(content = "Cross-platform \u{2192}".to_string())
            }
            link(route = &BACKENDS_ROUTE, params = ()) {
                Typography(content = "Backends \u{2192}".to_string())
            }
        }
    }
}
