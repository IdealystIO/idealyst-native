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
pub const REACTIVITY_ROUTE: Route<()> = Route::<()>::new("reactivity", "/reactivity");
pub const STYLING_ROUTE: Route<()> = Route::<()>::new("styling", "/styling");
pub const NAVIGATION_ROUTE: Route<()> = Route::<()>::new("navigation", "/navigation");
pub const WHY_RUST_ROUTE: Route<()> = Route::<()>::new("why-rust", "/why-rust");

// ---- Demo ----
pub const DEMO_ROUTE: Route<()> = Route::<()>::new("demo", "/demo");

// ---- Reference ----
pub const ARCHITECTURE_ROUTE: Route<()> = Route::<()>::new("architecture", "/architecture");
pub const BACKENDS_ROUTE: Route<()> = Route::<()>::new("backends", "/backends");
pub const AGENTIC_ROUTE: Route<()> = Route::<()>::new("agentic", "/agentic");
pub const ROADMAP_ROUTE: Route<()> = Route::<()>::new("roadmap", "/roadmap");
pub const FURTHER_READING_ROUTE: Route<()> = Route::<()>::new("further-reading", "/further-reading");

// ---- Tangent pages (reachable from inline links, not in sidebar) ----
//
// `/targets` — the "we mean every target" footnote-page reached from
// the home hero's asterisk. Lists every platform idealyst can run on
// (shipped backends + extension paths) for the casual visitor who
// wants the long answer.
pub const TARGETS_ROUTE: Route<()> = Route::<()>::new("targets", "/targets");

// `/comparisons` and `/comparisons/...` — "Why Idealyst over X" pages.
// Reachable from inline links on the home page (and from each other
// via the comparisons index). Each individual comparison stands alone;
// the index is the hub that lists them all.
pub const COMPARISONS_ROUTE: Route<()> = Route::<()>::new("comparisons", "/comparisons");
pub const COMPARE_ELECTRON_ROUTE: Route<()> =
    Route::<()>::new("compare-electron", "/comparisons/electron");
pub const COMPARE_REACT_ROUTE: Route<()> =
    Route::<()>::new("compare-react", "/comparisons/react");
pub const COMPARE_DIOXUS_ROUTE: Route<()> =
    Route::<()>::new("compare-dioxus", "/comparisons/dioxus");
pub const COMPARE_FLUTTER_ROUTE: Route<()> =
    Route::<()>::new("compare-flutter", "/comparisons/flutter");
pub const COMPARE_WEB_FRAMEWORKS_ROUTE: Route<()> =
    Route::<()>::new("compare-web-frameworks", "/comparisons/web-frameworks");
pub const COMPARE_WHEN_NOT_ROUTE: Route<()> =
    Route::<()>::new("compare-when-not", "/comparisons/when-not");

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
        "home" => "Home",
        "targets" => "Targets",
        "comparisons" => "How Idealyst compares",
        "compare-electron" => "Idealyst vs Electron",
        "compare-react" => "Idealyst vs React / React Native",
        "compare-dioxus" => "Idealyst vs Dioxus",
        "compare-flutter" => "Idealyst vs Flutter",
        "compare-web-frameworks" => "Idealyst vs Vue, Angular, Svelte",
        "compare-when-not" => "When not to use Idealyst",
        _ => "",
    }
}

/// Sidebar layout — `title` is the section header; entries render in
/// order beneath. The active route highlight matches by `name`.
pub const SECTIONS: &[IndexSection] = &[
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
            IndexEntry { route: &REACTIVITY_ROUTE, label: "Reactivity & animation" },
            IndexEntry { route: &STYLING_ROUTE, label: "Styling & theming" },
            IndexEntry { route: &NAVIGATION_ROUTE, label: "Navigation" },
            IndexEntry { route: &WHY_RUST_ROUTE, label: "Why Rust" },
        ],
    },
    IndexSection {
        title: "Reference",
        entries: &[
            IndexEntry { route: &DEMO_ROUTE, label: "Demo" },
            IndexEntry { route: &ARCHITECTURE_ROUTE, label: "Architecture" },
            IndexEntry { route: &BACKENDS_ROUTE, label: "Backends" },
            IndexEntry { route: &AGENTIC_ROUTE, label: "Robot & MCP" },
            IndexEntry { route: &ROADMAP_ROUTE, label: "Roadmap" },
            IndexEntry { route: &FURTHER_READING_ROUTE, label: "Further reading" },
        ],
    },
];
