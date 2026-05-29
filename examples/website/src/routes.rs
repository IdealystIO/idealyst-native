//! Route constants + the sidebar index. One entry per screen; the
//! sidebar walks `INDEX` in order, and the `Navigator` wires each
//! route to its page builder. Routes are grouped into buckets that
//! show as section headers in the sidebar.

use runtime_core::Route;

// ---- Home ----
pub const HOME_ROUTE: Route<()> = Route::<()>::new("home", "/");

// ---- Features ----
pub const FEATURES_ROUTE: Route<()> = Route::<()>::new("features", "/features");
pub const CROSS_PLATFORM_ROUTE: Route<()> =
    Route::<()>::new("cross-platform", "/features/cross-platform");
pub const PERFORMANCE_ROUTE: Route<()> = Route::<()>::new("performance", "/features/performance");
pub const TYPE_SAFETY_ROUTE: Route<()> = Route::<()>::new("type-safety", "/features/type-safety");
pub const SSR_ROUTE: Route<()> = Route::<()>::new("ssr", "/features/ssr");
pub const SERVER_FUNCTIONS_ROUTE: Route<()> = Route::<()>::new("server-functions", "/server-functions");
pub const CODE_SPLITTING_ROUTE: Route<()> = Route::<()>::new("code-splitting", "/code-splitting");

// ---- Instructions ----
pub const INSTALL_ROUTE: Route<()> = Route::<()>::new("install", "/install");
pub const QUICKSTART_ROUTE: Route<()> = Route::<()>::new("quickstart", "/quickstart");
pub const CONCEPTS_ROUTE: Route<()> = Route::<()>::new("concepts", "/concepts");
pub const WHY_RUST_ROUTE: Route<()> = Route::<()>::new("why-rust", "/why-rust");

// ---- Demos ----
pub const DEMO_COUNTER_ROUTE: Route<()> = Route::<()>::new("demo-counter", "/demos/counter");
pub const DEMO_COMPONENTS_ROUTE: Route<()> = Route::<()>::new("demo-components", "/demos/components");
pub const DEMO_ANIMATIONS_ROUTE: Route<()> = Route::<()>::new("demo-animations", "/demos/animations");
pub const DEMO_NAVIGATION_ROUTE: Route<()> = Route::<()>::new("demo-navigation", "/demos/navigation");

// ---- Reference ----
pub const BACKENDS_ROUTE: Route<()> = Route::<()>::new("backends", "/backends");
pub const AGENTIC_ROUTE: Route<()> = Route::<()>::new("agentic", "/agentic");
pub const FURTHER_READING_ROUTE: Route<()> = Route::<()>::new("further-reading", "/further-reading");

// ---- Tangent pages (reachable from inline links, not in sidebar) ----
//
// `/targets` — the "we mean every target" footnote-page reached from
// the home hero's asterisk. Lists every platform idealyst can run on
// (shipped backends + extension paths) for the casual visitor who
// wants the long answer.
pub const TARGETS_ROUTE: Route<()> = Route::<()>::new("targets", "/targets");

pub struct IndexEntry {
    /// The route this entry links to. Carrying the `Route` on the entry
    /// (instead of a bare name the sidebar re-matches against a `match`)
    /// means adding a sidebar item is a single edit here — there's no
    /// parallel arm to keep in sync, and a forgotten one can't silently
    /// degrade to unstyled, non-clickable text.
    pub route: &'static Route<()>,
    pub label: &'static str,
}

impl IndexEntry {
    /// The route's in-stack key — used for active-route highlighting and
    /// the mobile header's label lookup.
    pub fn name(&self) -> &'static str {
        self.route.name()
    }
}

pub struct IndexSection {
    pub title: &'static str,
    pub entries: &'static [IndexEntry],
}

/// Resolve a route name to a display label by walking `SECTIONS`.
/// Used by the mobile-header to mirror the sidebar's vocabulary so
/// the in-bar title agrees with the sidebar's selected link. Falls
/// back to the route name itself for tangent pages not in the
/// sidebar (e.g. `/targets`) — better than blanking the header.
pub fn label_for_route(name: &str) -> &'static str {
    for section in SECTIONS {
        for entry in section.entries {
            if entry.name() == name {
                return entry.label;
            }
        }
    }
    match name {
        "targets" => "Targets",
        _ => "",
    }
}

/// Sidebar layout — `title` is the section header; entries render in
/// order beneath. The active route highlight matches by `name`.
pub const SECTIONS: &[IndexSection] = &[
    IndexSection {
        title: "",
        entries: &[IndexEntry { route: &HOME_ROUTE, label: "Home" }],
    },
    IndexSection {
        title: "Features",
        entries: &[
            IndexEntry { route: &FEATURES_ROUTE, label: "Overview" },
            IndexEntry { route: &CROSS_PLATFORM_ROUTE, label: "Cross-platform" },
            IndexEntry { route: &PERFORMANCE_ROUTE, label: "High performance" },
            IndexEntry { route: &TYPE_SAFETY_ROUTE, label: "Type safety" },
            IndexEntry { route: &SSR_ROUTE, label: "Server-side rendering" },
            IndexEntry { route: &SERVER_FUNCTIONS_ROUTE, label: "Server functions" },
            IndexEntry { route: &CODE_SPLITTING_ROUTE, label: "Code splitting" },
        ],
    },
    IndexSection {
        title: "Instructions",
        entries: &[
            IndexEntry { route: &INSTALL_ROUTE, label: "Install the CLI" },
            IndexEntry { route: &QUICKSTART_ROUTE, label: "Quickstart" },
            IndexEntry { route: &CONCEPTS_ROUTE, label: "Core concepts" },
            IndexEntry { route: &WHY_RUST_ROUTE, label: "Why Rust" },
        ],
    },
    IndexSection {
        title: "Demos",
        entries: &[
            IndexEntry { route: &DEMO_COUNTER_ROUTE, label: "Counter" },
            IndexEntry { route: &DEMO_COMPONENTS_ROUTE, label: "Components" },
            IndexEntry { route: &DEMO_ANIMATIONS_ROUTE, label: "Animations" },
            IndexEntry { route: &DEMO_NAVIGATION_ROUTE, label: "Navigation" },
        ],
    },
    IndexSection {
        title: "Reference",
        entries: &[
            IndexEntry { route: &BACKENDS_ROUTE, label: "Backends" },
            IndexEntry { route: &AGENTIC_ROUTE, label: "Robot & MCP" },
            IndexEntry { route: &FURTHER_READING_ROUTE, label: "Further reading" },
        ],
    },
];
