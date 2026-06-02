//! Navigation — the navigator SDKs (stack / tab / drawer), routes &
//! params, screen options, and the native chrome each one maps to.
//! Companion to the bundled `navigation.md` guide. This very site is a
//! `DrawerNavigator`, so the page documents the API it's built on.

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
        TocEntry { handle: pitch_ref, label: "One API, native chrome" },
        TocEntry { handle: navigators_ref, label: "The navigators" },
        TocEntry { handle: routes_ref, label: "Routes & typed params" },
        TocEntry { handle: example_ref, label: "Building a navigator" },
        TocEntry { handle: options_ref, label: "Screen options" },
        TocEntry { handle: chrome_ref, label: "How the chrome maps" },
        TocEntry { handle: next_ref, label: "Where to go from here" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Navigation",
                blurb = "The navigator system maps platform-native navigation chrome — \
                 `UINavigationController` on iOS, the Fragment back-stack on Android, the \
                 browser History API on web — to a single author API. Navigators are SDK \
                 crates, not core primitives, so you pull in only the ones you use. This \
                 site itself is a drawer navigator.",
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
            Typography(content = "One API, native chrome".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Navigation is the place cross-platform frameworks usually \
                leak. A webview app fakes a navigation bar in HTML; a lowest-common-\
                denominator toolkit gives you one generic stack on every platform. Idealyst \
                does neither: you describe screens and transitions once, and each backend \
                drives its own real navigation toolkit. iOS gets a true \
                `UINavigationController` with native swipe-back; Android gets the Fragment \
                back-stack and a real `Toolbar`; web gets History-API URLs and a responsive \
                sidebar.".to_string())
            Typography(content = "Navigators live in SDK crates (`stack-navigator`, \
                `tab-navigator`, `drawer-navigator`) rather than the framework core, in \
                keeping with the rule that core stays minimal. You depend on the navigators \
                you use, register their per-backend handler once, and the framework routes \
                navigation commands to the right native machinery.".to_string())
        }
    }
}

fn navigators() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "The navigators".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Stack navigator".to_string(), kind = idea_ui::typography_kind::H3)
            Typography(content = "Push/pop screens with native swipe-back on iOS. The \
                workhorse for drill-down flows — a list that opens a detail, a settings \
                tree, a checkout. On macOS it swaps the outlet on push/pop without animated \
                chrome, per the single-window desktop design.".to_string())
            Typography(content = "Tab navigator".to_string(), kind = idea_ui::typography_kind::H3)
            Typography(content = "A bottom tab bar on mobile, a side rail on desktop/web. \
                Each tab owns its own screen (often its own nested stack), so switching tabs \
                preserves each tab's place.".to_string())
            Typography(content = "Drawer navigator".to_string(), kind = idea_ui::typography_kind::H3)
            Typography(content = "A hamburger drawer that is responsive between modal \
                (mobile: slides in over the content) and pinned (desktop: a permanent \
                sidebar beside the body). The switch is keyed off the active theme \
                breakpoint, not a hardcoded pixel width. This page is rendered inside \
                one.".to_string())
            Typography(content = "Card-tabs navigator".to_string(), kind = idea_ui::typography_kind::H3)
            Typography(content = "A secondary tab layer inside a single screen — for \
                segmented sub-views that don't warrant a full navigation level.".to_string())
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
            Typography(content = "A `Route<P>` pairs a stable name (the in-stack key, used \
                for active-route highlighting) with a URL path and a typed parameter `P`. \
                Because the param type is baked into the route constant, navigating to a \
                route with the wrong param shape is a compile error — there's no \
                stringly-typed params bag to get out of sync. The `link` primitive and the \
                navigator handle both take a `&Route<P>` plus its params.".to_string())
            CodePanel(src = snippet)
        }
    }
}

fn example() -> Element {
    let snippet = "let nav: Ref<DrawerHandle> = Ref::new();\n\
                   \n\
                   let builder = DrawerNavigator::new(&HOME)\n    \
                       .screen(HOME,    move |_| Screen::new(home_page()).title(\"Home\"))\n    \
                       .screen(ABOUT,   move |_| Screen::new(about_page()).title(\"About\"))\n    \
                       .screen(USER,    move |params| Screen::new(user_page(params)))\n    \
                       // Persistent sidebar — mounts once, survives every screen swap:\n    \
                       .leading_with(move |slot| sidebar(slot));\n\
                   \n\
                   ui! { builder.bind(nav) }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Building a navigator".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "A navigator is built with a fluent builder: name the \
                initial route, register one `.screen(route, builder)` per destination, and \
                `.bind(handle)` it so the rest of your app can push routes imperatively. The \
                screen builder closure receives the route's typed params and returns a \
                `Screen` wrapping that page's `Element`. This is verbatim the shape this \
                site's own shell uses.".to_string())
            CodePanel(src = snippet)
            Typography(content = "Slots like the drawer's `leading_with` (the sidebar) mount \
                ONCE at navigator init and survive every screen change — the navigator only \
                swaps the screen body. If you wire a slot or layout reactively, keep a \
                keepalive `Effect` alive past the build call so its reactive scope isn't \
                dropped (otherwise updates stop firing).".to_string(),
                muted = true)
        }
    }
}

fn options() -> Element {
    let snippet = "// Per-screen chrome is configured through screen options,\n\
                   // NOT the style system:\n\
                   Screen::new(details_page())\n    \
                       .title(\"Details\")          // nav-bar title (iOS + Android)\n\
                   \n\
                   // Navigator-level header theming re-resolves token reads on\n\
                   // theme swaps, so the native bar re-tints with the app:\n\
                   DrawerNavigator::new(&HOME)\n    \
                       .header(|| HeaderStyle {\n        \
                           background: Some(surface_token()),\n        \
                           title:      Some(text_token()),\n    \
                       })";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Screen options".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Navigation chrome — titles, tab bar items, the drawer \
                header — is configured through typed screen options, not the `style` system. \
                That's the native-first stance: a screen's `title` drives \
                `UINavigationItem.title` on iOS and the `Toolbar` title on Android directly, \
                rather than you hand-rendering a bar that only looks right on one platform. \
                The `.layout(...)` builder remains a deliberate web-only escape hatch.".to_string())
            CodePanel(src = snippet)
        }
    }
}

fn chrome() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "How the chrome maps".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "iOS: a real `UINavigationController`. Push/pop drive the \
                native transition, swipe-back works, the title bar is a true \
                `UINavigationItem`. Android: the Fragment back-stack plus a `Toolbar` whose \
                title and hamburger are rebuilt per screen. Web: the History API — each \
                push is a real URL, back/forward work, and the drawer renders as a \
                responsive sidebar that the SSR first paint already gets right.".to_string())
            Typography(content = "macOS follows a single-window, web-style design: a \
                persistent sidebar with an outlet that swaps on navigation, rather than \
                iOS-style animated push/pop. Terminal no-ops navigation chrome entirely, in \
                keeping with the terminal-minimalism convention — pages own their own \
                headers there if they want them.".to_string())
            Typography(content = "All three navigator SDKs ship modules for web, iOS, \
                Android, and macOS; the Backends page has the per-platform status.".to_string(),
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
            Typography(content = "Why the same author code produces native chrome on every \
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
