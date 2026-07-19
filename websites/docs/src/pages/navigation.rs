//! Navigation page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "navigation",
    title = "Navigation",
    category = Reference,
    description = "Swap and stack navigators, chrome as author layout around the outlet, and the Link primitive — typed routes and ambient navigators.",
    related = ["primitives", "components", "refs", "styles"],
    concepts = [
        Route,
        RouteParams,
        Screen,
        SwapNavigator,
        StackNavigator,
        NavigatorOutlet,
        Link,
        AmbientNavigator,
        MountPolicy,
    ],

    section(heading = "Overview") {
        p("Navigation is how your app moves between screens. Idealyst ships \
           two navigator behaviors as SDK crates — ", code("swap-navigator"),
          " (a flat set of co-equal screens the user switches between) and ",
          code("stack-navigator"), " (push/pop with depth and a back-stack) \
           — plus a ", code("Link"), " primitive that declaratively \
           dispatches navigation without you wiring up a handle."),
        p("There is deliberately no tab navigator and no drawer navigator. \
           What used to be a \"tab bar\" or a \"drawer panel\" is just \
           ordinary author layout wrapped around the navigator's single \
           outlet — the analog of react-router's ",
          code("<Outlet/>"), ". A tab UI is a swap navigator whose ",
          code(".layout(|nav| …)"), " puts a bar under ", code("{nav.outlet}"),
          "; a drawer UI is the same navigator with the outlet wrapped in a \
           side panel. ", code("idea_ui_nav"), " ships themed chrome \
           components for the common shapes: ", code("TabBar"), ", ",
          code("Drawer"), ", ", code("AppShell"), " (pinned-sidebar ⇄ drawer), \
           and ", code("StackHeader"), "."),
        p("Both navigators share a substrate: typed routes, per-screen \
           reactive scopes, an ambient-navigator stack the ", code("Link"),
          " primitive reads, URL-pattern matching, and web URL sync \
           (history, deep links, scroll restore) provided by the framework. \
           What differs is the command vocabulary — ", code("Select"),
          " for swap, ", code("Push"), "/", code("Pop"), "/", code("Replace"),
          "/", code("Reset"), " for stack — and whether covered screens \
           exist below the visible one."),
        p("This page covers all of it: routes and params, declaring screens, \
           the two navigator behaviors, chrome as layout, headers, the ",
          code("Link"), " primitive, and nested navigators."),
    },

    section(heading = "Routes") {
        p("A route is a typed name plus a URL pattern. You declare each route \
           once, give it a stable identifier, and reuse it everywhere — on \
           navigators, in ", code("Link"), "s, in pushes:"),

        code(rust, r##"
            use runtime_core::Route;

            pub const HOME:    Route<()>            = Route::new("home",    "/");
            pub const PROFILE: Route<ProfileParams> = Route::new("profile", "/profile/:id");
            pub const ABOUT:   Route<()>            = Route::new("about",   "/about");
        "##),

        p("The ", code("name"), " (first arg) is the in-navigator key — what \
           the framework and chrome components use to identify the route. \
           The ", code("path"), " (second arg) is the URL pattern web and \
           SSR map ", code("window.location"), " against. Native backends \
           ignore ", code("path"), "; they work purely from ",
          code("name"), " plus boxed params."),

        p("The generic ", code("P"), " is the typed payload the route carries. \
           For no-params routes, use ", code("()"), ". For routes that take \
           data, declare a struct and implement ", code("RouteParams"), ":"),

        code(rust, r##"
            use runtime_core::RouteParams;
            use std::collections::HashMap;

            pub struct ProfileParams {
                pub id: u32,
            }

            impl RouteParams for ProfileParams {
                fn to_path(&self, _pattern: &str) -> String {
                    format!("/profile/{}", self.id)
                }

                fn from_segments(segments: &HashMap<String, String>) -> Option<Self> {
                    Some(Self {
                        id: segments.get("id")?.parse().ok()?,
                    })
                }
            }
        "##),

        p("The two methods round-trip your typed struct through a URL on web \
           and SSR. ", code("to_path"), " builds the URL when you navigate to \
           the route; ", code("from_segments"), " parses it back when the \
           browser's location changes (back/forward, deep links). Native \
           backends never call either method — they pass the boxed struct \
           through directly."),
    },

    section(heading = "Why params are typed") {
        p(code("nav.push(&PROFILE, ProfileParams { id: 42 })"), " is a \
           compile-time check that the params match the route. If you try to \
           push a mismatched payload, the compiler rejects it. Inside the \
           framework, the params get boxed into ", code("Box<dyn Any>"),
          " for storage, and each screen builder downcasts back to its \
           declared type before rendering. There's only one place that \
           downcast can fail — if you fabricate a ", code("Route<X>"),
          " at runtime with the wrong ", code("P"), " — and the framework \
           panics with a clear message rather than silently using the wrong \
           data."),
    },

    section(heading = "Screens") {
        p("A ", code("Screen"), " is what a route's render closure returns: a \
           primitive tree plus optional per-screen options."),

        code(rust, r##"
            use runtime_core::{Screen, ui};
            use stack_navigator::StackScreenExt;

            fn render_home(_params: ()) -> Screen {
                Screen::new(ui! {
                    // ...home page content
                })
                .title("Home")
            }
        "##),

        p("The ", code("Screen::new(...)"), " builder takes anything that \
           converts to an ", code("Element"), ". For stack screens, the ",
          code("StackScreenExt"), " trait (from ", code("stack-navigator"),
          ") adds chainable header options:"),

        list(
            [code(".title(\"...\")"), " — the screen title. Drives the \
              native bar title on mobile and the ", code("StackHeader"),
             " on web/desktop."],
            [code(".header_left(HeaderButton::text(\"Edit\").on_press(...))"),
             " — leading header slot."],
            [code(".header_right(...)"), " — trailing header slot."],
            [code(".hide_header(true)"), " — hide the header entirely for \
              this screen."],
            [code(".back_enabled(false)"),
             " — lock the platform back affordance (iOS swipe-back, Android \
              system back) while this screen is on top. Browser back cannot \
              be locked — a platform constraint."],
            [code(".fullscreen(true)"),
             " — request full-screen (status bar / home indicator hidden) \
              while this screen is on top. Native mobile only."],
        ),

        p("Swap screens draw no navigator chrome of their own, so they take \
           no options today. If you don't need options, return a bare ",
          code("Element"), " — the ", code("Into<Screen>"),
          " impl wraps it for you:"),

        code(rust, r##"
            fn render_home(_: ()) -> Element {
                ui! { /* ... */ }
            }
        "##),

        p("…and either navigator builder accepts it the same way."),
    },

    section(heading = "The swap navigator") {
        p(code("SwapNavigator"), " manages a flat set of co-equal screens. \
           There is no depth: selecting a screen swaps the one visible \
           screen in the outlet. It's the behavior behind tab UIs, drawer / \
           sidebar UIs, wizards, and site shells — the chrome around the \
           outlet is what makes each of those look different."),

        code(rust, r##"
            use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator};
            use idea_ui_nav::{TabBar, TabItem};
            use runtime_core::{ui, Ref, Screen};

            let nav: Ref<SwapHandle> = Ref::new();

            let builder = SwapNavigator::new(&HOME)
                .screen(HOME,    |_| Screen::new(home_page()))
                .screen(SEARCH,  |_| Screen::new(search_page()))
                .screen(PROFILE, |p: ProfileParams| Screen::new(profile_page(p)))
                // The layout OWNS the chrome tree and splats `{nav.outlet}`
                // where the active screen renders. This one is a tab UI:
                .layout(|nav| ui! {
                    view(style = fill_column) {
                        view(style = grow) { { nav.outlet } }
                        TabBar(
                            items = vec![
                                TabItem::new("home", "Home"),
                                TabItem::new("search", "Search"),
                                TabItem::new("profile", "Profile"),
                            ],
                            active_route = nav.active_route,
                            on_select = nav.on_select,
                        )
                    }
                });

            ui! { builder.bind(nav) }
        "##),

        p("The ", code(".layout(|nav| …)"), " closure receives a ",
          code("SwapContext"), ":"),

        list(
            [code("nav.outlet"),
             " — splat it (", code("{ nav.outlet }"),
             ") where the active screen mounts. It's a one-shot, non-",
             code("Clone"),
             " value: splat it exactly once, in one stable spot. It cannot \
              be branched into a reactive ", code("if"), " / ", code("when"),
             " — responsive layouts keep the outlet pinned and reactively \
              restyle the chrome around it."],
            [code("nav.active_route"), " — ",
             code("Signal<&'static str>"),
             ", the active screen's route name. Read it to highlight the \
              live tab or sidebar link."],
            [code("nav.active_path"), " — the active screen's full path."],
            [code("nav.on_select"),
             " — switch to a sibling screen by route name. What a tab bar \
              or sidebar link calls."],
        ),

        p("For programmatic switching with typed params, ", code(".bind(...)"),
          " fills a ", code("Ref<SwapHandle>"), " and ",
          code("handle.select(&PROFILE, ProfileParams { id: 42 })"),
          " builds the URL from the params and dispatches ", code("Select"),
          ". Selecting the already-active screen is a no-op."),

        p("If you skip ", code(".layout(...)"),
          " entirely, the outlet fills the navigator with no surrounding \
           chrome — useful when a parent already draws the chrome."),
    },

    section(heading = "Swap state preservation — MountPolicy") {
        p("How an inactive swap screen behaves is controlled by ",
          code("MountPolicy"), ", set navigator-wide via ",
          code(".mount_policy(...)"), ":"),

        list(
            [code("LazyPersistent"),
             " (default) — mount the screen the first time it's selected, \
              then keep it mounted. Switching away preserves its state \
              (scroll position, nested stack depth, form fields); switching \
              back is instant. Matches React Navigation's tab default."],
            [code("EagerPersistent"),
             " — mount every screen at navigator creation. Higher memory; \
              switches are pure visibility toggles. Use when all screens \
              are \"always live.\""],
            [code("LazyDisposing"),
             " — drop the inactive screen's scope (and its background work) \
              on switch; re-mount fresh on return. Lowest memory; loses \
              state. Matches browser semantics — both doc sites use it so \
              navigating away from a page tears its work down."],
        ),
    },

    section(heading = "The stack navigator") {
        p(code("StackNavigator"), " is the push/pop sibling: ", code("push"),
          " mounts a screen on top of a back-stack, ", code("pop"),
          " removes the top and reveals the one below. The visible screen \
           is the top of the stack, swapped into the same single outlet. \
           Chrome is author layout here too — typically a ",
          code("StackHeader"), " above the outlet:"),

        code(rust, r##"
            use stack_navigator::{
                header_state, StackBuilder, StackHandle, StackNavigator, StackScreenExt,
            };
            use idea_ui_nav::StackHeader;
            use runtime_core::{rx, ui, Ref, Screen};

            let nav: Ref<StackHandle> = Ref::new();

            let builder = StackNavigator::new(&LIST)
                .screen(LIST, {
                    let nav = nav.clone();
                    move |_| Screen::new(list_page(nav.clone())).title("Inbox")
                })
                .screen(DETAIL, |p: DetailParams| {
                    Screen::new(detail_page(p)).title("Detail")
                })
                .layout(|nav| {
                    let screen_chrome = nav.screen_chrome;
                    let state = rx!(header_state(&screen_chrome));
                    ui! {
                        view(style = fill_column) {
                            StackHeader(
                                state = state,
                                show_back = nav.can_go_back,
                                on_back = Some(nav.pop.clone()),
                            )
                            view(style = grow) { { nav.outlet } }
                        }
                    }
                });

            ui! { builder.bind(nav) }
        "##),

        p("The handle's commands (via the bound ", code("Ref<StackHandle>"),
          "):"),

        list(
            [code("nav.push(&DETAIL, DetailParams { id: 42 })"),
             " — push a new screen onto the top."],
            [code("nav.pop()"), " — pop the top screen (no-op at the root)."],
            [code("nav.replace(&route, params)"), " — replace the top of the stack."],
            [code("nav.reset(&route, params)"),
             " — clear the stack, mount the new route as the root. Useful \
              for post-login redirects."],
        ),

        p("The ", code(".layout(|nav| …)"), " closure receives a ",
          code("StackContext"), " — the same ", code("outlet"), " / ",
          code("active_route"), " / ", code("active_path"),
          " as swap, plus stack-specific state:"),

        list(
            [code("nav.depth"), " — stack depth (1 at the root)."],
            [code("nav.can_go_back"),
             " — whether a pop is possible (depth > 1); gate the back \
              affordance on it."],
            [code("nav.pop"), " — pop the top screen."],
            [code("nav.screen_chrome"),
             " — the active screen's header slots, updated on every \
              navigation. Read it via ", code("stack_navigator::header_state"),
             " inside ", code("rx!"), " and feed the result to a ",
             code("StackHeader"), "."],
        ),

        p("Each pushed screen runs inside its own reactive scope. Popping \
           drops the scope — every signal, effect, and ref allocated inside \
           that screen is freed in one shot. You don't write screen-teardown \
           code."),
    },

    section(heading = "Stack retention — what happens below the top") {
        p("Covered screens follow ", code("StackRetention"),
          ", resolved per platform by default:"),

        list(
            [code("Retain"),
             " — covered screens stay alive (scope + detached node); pop \
              reveals them with all state intact. Native-stack semantics; \
              the default everywhere except web."],
            [code("Rebuild"),
             " — covered screens are disposed on push; pop re-mounts the \
              revealed screen from its URL (route pattern → typed params), \
              exactly like a fresh navigation after a refresh. Browser \
              semantics; the default on web. Also applies to cold-start \
              deep links: the synthesized parent entry is URL-only and \
              never mounts until you actually pop to it."],
            [code("PlatformDefault"),
             " — the default: ", code("Rebuild"), " on web, ",
             code("Retain"), " everywhere else."],
        ),

        p("Force either with ", code(".retention(...)"),
          " on the builder. Both are right for their platform — a native \
           stack keeps covered screens alive so back reveals them with \
           state intact, while a browser treats every navigation as \
           URL-driven."),
    },

    section(heading = "Chrome is author layout — idea-ui-nav") {
        p("Because chrome is just layout, you can build any navigation UI \
           from plain views. ", code("idea_ui_nav"),
          " packages the common, themed shapes so you don't have to:"),

        list(
            [code("TabBar"),
             " — a themed tab bar for a swap navigator. Wire ",
             code("active_route = nav.active_route"), " and ",
             code("on_select = nav.on_select"),
             "; the active tab highlights via the live route and taps \
              dispatch ", code("Select"), "."],
            [code("Drawer"),
             " — a themed side panel wrapping the outlet: sliding panel, \
              tap-to-close scrim, mount-once. ", code("is_open"),
             " is a plain ", code("Signal<bool>"),
             " the author owns — a hamburger opens it, sidebar links close \
              it after selecting."],
            [code("AppShell"),
             " — the responsive \"pinned sidebar on desktop, drawer on \
              mobile\" shell. The sidebar builds exactly ONCE; pinned vs \
              drawer is static breakpoint styling that compiles to real ",
             code("@media"),
             " rules on web and SSR, so the server-rendered first paint is \
              viewport-correct with no JS and crossing the breakpoint never \
              remounts anything. Configure with ", code("is_open"), ", ",
             code("pin_at"), " (default ", code("Breakpoint::Lg"), "), and ",
             code("width"), "; use ", code("sidebar_pinned(pin_at)"),
             " to hide the hamburger / skip drawer-closing while pinned."],
            [code("StackHeader"),
             " — a themed top bar for stack screens on web/desktop. It \
              reads the active screen's ", code("StackHeaderState"),
             " and self-suppresses when a native bar is already rendering \
              it (mobile) or the screen set ", code("hide_header"),
             " — so you place it unconditionally and nothing doubles."],
        ),

        code(rust, r##"
            // The docs site's own shell: swap navigator + AppShell.
            SwapNavigator::new(&OVERVIEW)
                .screen(OVERVIEW, |_| Screen::new(overview_page()))
                // ...more screens...
                .mount_policy(MountPolicy::LazyDisposing)
                .layout(move |nav| ui! {
                    AppShell(
                        sidebar = vec![sidebar(nav.active_route)],
                        is_open = drawer_open,
                        pin_at = Breakpoint::Lg,
                        width = 260.0,
                    ) {
                        // The one stable spot for the one-shot outlet:
                        { nav.outlet }
                    }
                })
        "##),
    },

    section(heading = "What backends do") {
        p("Both SDKs drive everything through the framework's ",
          code("NavigatorHost"),
          " callbacks (build the layout with an outlet, mount/release \
           per-screen subtrees, swap nodes), so ONE backend-neutral handler \
           serves every backend — there are no per-backend twins to drift \
           apart. Two surfaces get extra treatment:"),

        list(
            [code("Web"),
             " — URL sync is substrate-provided and automatic: ",
             code("Select"), " and ", code("Push"), " write ",
             code("history.pushState"),
             " entries, browser back/forward reconcile into ordinary nav \
              commands, cold-start deep links seed the right screen, and \
              per-entry scroll offsets restore on back. The handlers never \
              touch the URL themselves."],
            [code("iOS / Android"),
             " — the stack still FEELS native: inside the outlet lives a \
              real ", code("UINavigationController"),
             " (interactive swipe-back, native transitions) or the \
              Fragment back-stack (system back button, back-lock). The \
              native bar is always hidden — the header is the author's ",
             code("StackHeader"),
             ", driven by the same ", code("screen_chrome"),
             " state, so every backend renders the same chrome with native \
              transition mechanics underneath."],
        ),

        p("For SSR/SSG, register the handlers on the SSR backend with ",
          code("swap_navigator::register_generic"), " / ",
          code("stack_navigator::register_generic"),
          "; for ", code("idealyst dev"), " non-local mode, the sidecar \
           registers the ", code("recording"),
          " modules so screen swaps ship as plain node ops over the wire. \
           On the app backends (web, iOS, Android, macOS) the SDKs \
           self-register — building the navigator is enough."),
    },

    section(heading = "The Link primitive") {
        p("Imperative navigation works fine — ",
          code("nav.push(&route, params)"), " from a button's ",
          code("on_click"), " — but it has costs:"),

        list(
            ["You have to thread a handle ", code("Ref"),
             " through every component that needs to navigate."],
            ["The browser's link semantics (right-click \"copy link\", \
              cmd-click new tab, hover URL preview, keyboard activation, \
              the ", code("link"),
             " accessibility role) need separate wiring."],
            ["Static analysis tooling can't see what links your screens \
              expose, because they're hidden inside click handlers."],
        ),

        p(code("Link"), " solves all three:"),

        code(rust, r##"
            ui! {
                link(route = &PROFILE, params = ProfileParams { id: 42 }) {
                    text { "Open profile" }
                }
            }
        "##),

        p("What you get:"),

        list(
            [code("Web"), ": emits a real ", code("<a href=\"/profile/42\">"),
             " so the browser's link contract works. Right-click, \
              cmd-click, keyboard activation, screen readers — all of it."],
            [code("Native"),
             ": an invisible tappable wrapper. The press dispatches \
              in-process against the captured navigator."],
            ["Static introspection: future tooling can extract the link \
              graph by walking the primitive tree."],
            ["No prop drilling: the ", code("Link"),
             " reads the ambient navigator — the closest enclosing \
              navigator whose screen is building this subtree — and \
              dispatches through it."],
        ),

        p("The command a link dispatches follows the ambient navigator's \
           behavior: inside a swap navigator the installed link activator \
           rewrites activation to ", code("Select"),
          " (links switch, never push); inside a stack it defaults to ",
          code("Push"), ". A bare ", code("link(route = ..., params = ...)"),
          " therefore does the right thing in any context."),
    },

    section(heading = "Nested navigators") {
        p("Navigators nest by putting one navigator inside another's \
           screen. A common app shape — drawer shell at the root, tabs in \
           one section, a stack inside a tab — is three navigators deep, \
           every level author-chromed (this is exactly the ",
          code("examples/nav-showcase"), " structure):"),

        code(rust, r##"
            // Root: swap + Drawer chrome
            SwapNavigator::new(&D_HOME)
                .screen(D_HOME,     |_| Screen::new(tabs_section()))
                .screen(D_SETTINGS, |_| Screen::new(settings_stack()))
                .layout(move |nav| ui! {
                    Drawer(sidebar = sidebar(nav.on_select.clone()), is_open = drawer_open) {
                        { nav.outlet }
                    }
                });

            // "Home" section: swap + TabBar; the Feed tab hosts a stack
            fn tabs_section() -> Element {
                let builder = SwapNavigator::new(&T_FEED)
                    .screen(T_FEED,   |_| Screen::new(feed_stack()))
                    .screen(T_ALERTS, |_| Screen::new(alerts_page()))
                    .layout(|nav| ui! {
                        view(style = fill_column) {
                            view(style = grow) { { nav.outlet } }
                            TabBar(
                                items = vec![
                                    TabItem::new("feed", "Feed"),
                                    TabItem::new("alerts", "Alerts"),
                                ],
                                active_route = nav.active_route,
                                on_select = nav.on_select,
                            )
                        }
                    });
                ui! { builder }
            }

            // Feed: a stack (list → detail) inside the Feed tab
            fn feed_stack() -> Element { /* StackNavigator::new(&F_LIST)… */ }
        "##),

        p("A ", code("Link"), " inside the feed stack's detail screen \
           targets that stack — not the tabs, not the drawer. The \
           ambient-navigator stack pushes each navigator's control plane \
           as its screens build, so ", code("Link"),
          " always finds the innermost navigator by default."),

        p("If you need to break out — e.g. a \"log out\" link inside a \
           deeply nested screen needs to reset the root navigator — \
           capture the outer navigator's handle ", code("Ref"),
          " and call its imperative methods directly."),

        p("Combined with the swap default ", code("MountPolicy::LazyPersistent"),
          ", nested stacks keep their state: navigate three levels deep \
           into the Feed tab, switch to Alerts, switch back — you're still \
           three levels deep. The nested stack's screens are still \
           mounted; their signals still hold their values."),
    },

    section(heading = "Sizing — navigators and outlets fill") {
        p("The navigator's root fills its container by default (width/height \
           100% + ", code("flex-grow: 1"),
          "), so an app whose root is a navigator fills the viewport on \
           every backend. The outlet fills too: a style-less ",
          code("{nav.outlet}"),
          " defaults to a bounded, fillable flex region (",
          code("flex: 1 1 0"), " + ", code("min-height: 0"),
          "), so screens that assume they can fill — and scroll views that \
           need a bounded height — work with zero configuration. Override \
           either by styling it directly: ", code(".with_style(...)"),
          " on the navigator builder, ",
          code("ctx.outlet.with_style(...)"), " on the outlet."),
    },

    section(heading = "Scopes and lifecycle, recap") {
        p("Three lifecycle properties worth keeping in mind:"),

        list(
            ["Each mounted screen has its own reactive scope. When the \
              screen unmounts (pop with ", code("Rebuild"),
             " retention, swap-away with ", code("LazyDisposing"),
             "), the scope drops. Every signal, effect, and ref allocated \
              inside is freed."],
            ["Backend nodes survive when their identity is stable. Hot \
              reload preserves a screen's nodes if the screen's place in \
              the tree hasn't moved."],
            ["Navigation state itself is reactive. The layout closure's \
              context signals (", code("active_route"), ", ",
             code("active_path"), ", ", code("depth"), ", ",
             code("can_go_back"), ", ", code("screen_chrome"),
             ") are the walker's scoped nav-state mirrors — chrome and \
              external code subscribe to them like any other signal, and \
              they're freed with the navigator."],
        ),
    },

    section(heading = "Where to read more") {
        list(
            ["Routes and params — the full ", code("RouteParams"),
             " trait, the pattern matching algorithm, and the URL ↔ \
              typed-payload story."],
            ["The ", code("Link"), " primitive — every prop and how the \
              ambient stack works in detail."],
            [link("Primitives", to = "primitives"), " — where ",
             code("Element::Navigator"),
             " and the outlet sit among the other primitives."],
            [link("Writing a backend", to = "writing-a-backend"),
             " — the ", code("create_navigator"), " / ",
             code("NavigatorHost"), " contract a backend implements."],
            ["Hot reload — how navigation state survives source edits."],
        ),
    },
}
