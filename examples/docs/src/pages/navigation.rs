//! Navigation — Navigator, DrawerNavigator, TabNavigator, Routes.

use framework_core::{ui, Primitive};
use idea_ui::{body, card, heading, stack, BodyTone, HeadingKind, StackGap};

use crate::shell::{
    codeblock, pageheader, section, CodeBlockProps, PageHeaderProps, SectionProps,
};

pub fn page() -> Primitive {
    ui! {
        ScrollView {
            Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Navigation".to_string(),
                description = "Stack, drawer, and tab navigators backed by native platform chrome.".to_string(),
            )

            Section(
                title = "Routes".to_string(),
                body = "A `Route<P>` is a typed handle to a screen — a stable name (used \
                        internally by the navigator) plus a URL path pattern (used on the web \
                        for the address bar, and for deep links elsewhere). Declare routes \
                        once as `const`s and reference them everywhere — no string typos at \
                        call sites.".to_string(),
            )

            Card {
                Heading(content = "Stack navigator".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "The default. `push` adds a screen on top; `pop` peels back. \
                               Wraps `UINavigationController` on iOS, `FragmentManager` on \
                               Android, and a stack of mounted scopes on the web with \
                               History API integration so the browser's back button works.".to_string(),
                    tone = BodyTone::Muted,
                )
                CodeBlock(
                    code = "const HOME: Route<()> = Route::<()>::new(\"home\", \"/\");\n\
                            const DETAIL: Route<(u64,)> = Route::<(u64,)>::new(\"detail\", \"/detail/:id\");\n\
                            \n\
                            let nav: Ref<NavigatorHandle> = Ref::new();\n\
                            let navigator = Navigator::new(&HOME)\n    \
                                .screen(HOME, move |_| pages::home())\n    \
                                .screen(DETAIL, move |(id,)| pages::detail(id))\n    \
                                .bind(nav);\n\
                            ui! { { navigator } }".to_string(),
                )
            }

            Card {
                Heading(content = "Drawer navigator".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "A slide-in side panel (mobile) or pinned sidebar (tablet / \
                               desktop) plus a body region that swaps to the selected entry's \
                               screen. Each route is registered via `.screen(...)` and returns \
                               a `Screen::new(...).title(...).header_left(...)` value. The \
                               drawer panel's contents come from `.content(closure)`. Phone vs. \
                               tablet adaptation is the backend's responsibility — no \
                               app-side knob.".to_string(),
                    tone = BodyTone::Muted,
                )
                CodeBlock(
                    code = "let drawer = DrawerNavigator::new(&HOME)\n    \
                                .screen(HOME, |_| Screen::new(home_page()).title(\"Home\"))\n    \
                                .screen(SETTINGS, |_| Screen::new(settings_page()).title(\"Settings\"))\n    \
                                .content(|props| build_drawer_panel(props))\n    \
                                .layout(|layout| build_web_layout(layout));".to_string(),
                )
            }

            Card {
                Heading(content = "Tab navigator".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "Bottom or top tabs (configurable via `TabPlacement`). Each tab \
                               maps to a screen; tapping a tab switches the active body. Like \
                               the drawer, screens can be kept mounted across switches \
                               (`MountPolicy::LazyPersistent`) or torn down on every \
                               change.".to_string(),
                    tone = BodyTone::Muted,
                )
            }

            Section(
                title = "Link".to_string(),
                body = "The `Link` primitive emits an anchor-shaped node that captures the \
                        ambient navigator and dispatches a `Select` (or `Push`, configurable) \
                        on activation. On the web it renders an actual `<a href>`, so \
                        middle-click 'open in new tab' and right-click 'copy link' work for \
                        free. Use it for entries in a drawer or sidebar; the navigator handles \
                        the dispatch.".to_string(),
            )

            Section(
                title = "Layout closures".to_string(),
                body = "Both stack and drawer navigators accept a `.layout(closure)`. The \
                        closure receives `LayoutProps` — outlet, sidebar (pre-built), \
                        active-route signal, depth signal, back callback — and returns the \
                        chrome that wraps the active screen. Use this for app bars, sidebars, \
                        breadcrumbs, anything persistent across navigation.".to_string(),
            )

            Section(
                title = "Imperative control".to_string(),
                body = "Bind a `Ref<NavigatorHandle>` (or `Ref<DrawerHandle>`) via \
                        `.bind(ref)`. After mount, parents call `handle.push(&ROUTE, params)`, \
                        `handle.pop()`, `handle.replace(...)`, `handle.toggle()` and friends — \
                        the navigator forwards the command to the native dispatcher.".to_string(),
            )
        }
        }
    }
}
