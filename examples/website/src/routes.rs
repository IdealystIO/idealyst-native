//! Route constants + the sidebar index. One entry per screen; the
//! sidebar walks `INDEX` in order, and the `Navigator` wires each
//! route to its page builder. Routes are grouped into buckets that
//! show as section headers in the sidebar.

use runtime_core::Route;

// ---- Home ----
pub const HOME_ROUTE: Route<()> = Route::<()>::new("home", "/");

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
pub const SERVER_FUNCTIONS_ROUTE: Route<()> = Route::<()>::new("server-functions", "/server-functions");
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
    pub name: &'static str,
    pub label: &'static str,
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
            if entry.name == name {
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
        entries: &[IndexEntry { name: "home", label: "Home" }],
    },
    IndexSection {
        title: "Instructions",
        entries: &[
            IndexEntry { name: "install", label: "Install the CLI" },
            IndexEntry { name: "quickstart", label: "Quickstart" },
            IndexEntry { name: "concepts", label: "Core concepts" },
            IndexEntry { name: "why-rust", label: "Why Rust" },
        ],
    },
    IndexSection {
        title: "Demos",
        entries: &[
            IndexEntry { name: "demo-counter", label: "Counter" },
            IndexEntry { name: "demo-components", label: "Components" },
            IndexEntry { name: "demo-animations", label: "Animations" },
            IndexEntry { name: "demo-navigation", label: "Navigation" },
        ],
    },
    IndexSection {
        title: "Reference",
        entries: &[
            IndexEntry { name: "backends", label: "Backends" },
            IndexEntry { name: "server-functions", label: "Server functions" },
            IndexEntry { name: "agentic", label: "Robot & MCP" },
            IndexEntry { name: "further-reading", label: "Further reading" },
        ],
    },
];
