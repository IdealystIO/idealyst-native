//! Route constants + the sidebar index. One route per tutorial step.
//!
//! `SECTIONS` is the single source of truth: the sidebar walks it in
//! order to render track headers + step links, and [`flat_order`]
//! flattens it into the linear step sequence that drives prev/next
//! navigation at the bottom of each step.

use runtime_core::Route;

// ---- Intro ----
pub const HOME_ROUTE: Route<()> = Route::<()>::new("home", "/");

// ---- Idealyst 101: the unifying mental model ----
pub const CORE_ENGINE_ROUTE: Route<()> = Route::<()>::new("core-engine", "/core/engine");
pub const CORE_PERF_ROUTE: Route<()> = Route::<()>::new("core-performance", "/core/performance");

// ---- Architecture: how the framework is layered ----
pub const ARCH_OVERVIEW_ROUTE: Route<()> = Route::<()>::new("arch-overview", "/architecture/overview");
pub const ARCH_BACKENDS_ROUTE: Route<()> = Route::<()>::new("arch-backends", "/architecture/backends");
pub const ARCH_CATALOG_ROUTE: Route<()> = Route::<()>::new("arch-catalog", "/architecture/catalog");
pub const ARCH_SDKS_ROUTE: Route<()> = Route::<()>::new("arch-sdks", "/architecture/sdks");

// ---- Track 1: Reactivity ----
pub const RX_SIGNALS_ROUTE: Route<()> = Route::<()>::new("rx-signals", "/reactivity/signals");
pub const RX_EFFECTS_ROUTE: Route<()> = Route::<()>::new("rx-effects", "/reactivity/effects");
pub const RX_DERIVED_ROUTE: Route<()> = Route::<()>::new("rx-derived", "/reactivity/derived");
pub const RX_BATCHING_ROUTE: Route<()> = Route::<()>::new("rx-batching", "/reactivity/batching");

// ---- Track 2: Stylesheets ----
pub const ST_TOKENS_ROUTE: Route<()> = Route::<()>::new("st-tokens", "/styles/tokens");
pub const ST_STYLESHEETS_ROUTE: Route<()> = Route::<()>::new("st-stylesheets", "/styles/stylesheets");
pub const ST_VARIANTS_ROUTE: Route<()> = Route::<()>::new("st-variants", "/styles/variants");

// ---- Track 3: Media queries ----
pub const MQ_BREAKPOINTS_ROUTE: Route<()> = Route::<()>::new("mq-breakpoints", "/media/breakpoints");
pub const MQ_MOBILE_FIRST_ROUTE: Route<()> = Route::<()>::new("mq-mobile-first", "/media/mobile-first");
pub const MQ_SIGNAL_ROUTE: Route<()> = Route::<()>::new("mq-signal", "/media/breakpoint-signal");

// ---- Accessibility ----
pub const A11Y_DEFAULTS_ROUTE: Route<()> = Route::<()>::new("a11y-defaults", "/accessibility/defaults");
pub const A11Y_MODEL_ROUTE: Route<()> = Route::<()>::new("a11y-model", "/accessibility/model");

// ---- Advanced (authored later) ----
pub const ADV_BACKENDS_ROUTE: Route<()> = Route::<()>::new("adv-backends", "/advanced/custom-backends");
pub const ADV_CLI_ROUTE: Route<()> = Route::<()>::new("adv-cli", "/advanced/interactive-cli");
pub const ADV_EMBEDDED_ROUTE: Route<()> = Route::<()>::new("adv-embedded", "/advanced/embedded-rendering");

pub struct IndexEntry {
    pub route: &'static Route<()>,
    pub label: &'static str,
}

pub struct IndexSection {
    pub title: &'static str,
    pub entries: &'static [IndexEntry],
}

/// Sidebar layout. `title` is the track header; entries render in
/// order beneath. Order here is also the linear step sequence.
pub const SECTIONS: &[IndexSection] = &[
    IndexSection {
        title: "",
        entries: &[IndexEntry { route: &HOME_ROUTE, label: "Quick start" }],
    },
    IndexSection {
        title: "Foundations",
        entries: &[
            IndexEntry { route: &CORE_ENGINE_ROUTE, label: "One reactive engine" },
            IndexEntry { route: &CORE_PERF_ROUTE, label: "Under the hood: batching" },
        ],
    },
    IndexSection {
        title: "Architecture",
        entries: &[
            IndexEntry { route: &ARCH_OVERVIEW_ROUTE, label: "The layered model" },
            IndexEntry { route: &ARCH_BACKENDS_ROUTE, label: "Direct vs hosted runtime" },
            IndexEntry { route: &ARCH_CATALOG_ROUTE, label: "Catalog, docs & MCP" },
            IndexEntry { route: &ARCH_SDKS_ROUTE, label: "SDKs" },
        ],
    },
    IndexSection {
        title: "Reactivity",
        entries: &[
            IndexEntry { route: &RX_SIGNALS_ROUTE, label: "Signals" },
            IndexEntry { route: &RX_EFFECTS_ROUTE, label: "Effects" },
            IndexEntry { route: &RX_DERIVED_ROUTE, label: "Derived state" },
            IndexEntry { route: &RX_BATCHING_ROUTE, label: "Controlling when effects fire" },
        ],
    },
    IndexSection {
        title: "Stylesheets",
        entries: &[
            IndexEntry { route: &ST_TOKENS_ROUTE, label: "Style tokens" },
            IndexEntry { route: &ST_STYLESHEETS_ROUTE, label: "Defining a stylesheet" },
            IndexEntry { route: &ST_VARIANTS_ROUTE, label: "Variants & states" },
        ],
    },
    IndexSection {
        title: "Media queries",
        entries: &[
            IndexEntry { route: &MQ_BREAKPOINTS_ROUTE, label: "Breakpoint overlays" },
            IndexEntry { route: &MQ_MOBILE_FIRST_ROUTE, label: "Mobile-first" },
            IndexEntry { route: &MQ_SIGNAL_ROUTE, label: "The breakpoint signal" },
        ],
    },
    IndexSection {
        title: "Accessibility",
        entries: &[
            IndexEntry { route: &A11Y_DEFAULTS_ROUTE, label: "Accessible by default" },
            IndexEntry { route: &A11Y_MODEL_ROUTE, label: "The accessibility model" },
        ],
    },
    IndexSection {
        title: "Advanced",
        entries: &[
            IndexEntry { route: &ADV_BACKENDS_ROUTE, label: "Custom backends" },
            IndexEntry { route: &ADV_CLI_ROUTE, label: "Interactive CLIs" },
            IndexEntry { route: &ADV_EMBEDDED_ROUTE, label: "Embedded rendering" },
        ],
    },
];

/// Flatten `SECTIONS` into the linear step sequence (route, label),
/// in sidebar order. Drives the prev/next footer on each step.
pub fn flat_order() -> Vec<(&'static Route<()>, &'static str)> {
    let mut out = Vec::new();
    for section in SECTIONS {
        for entry in section.entries {
            out.push((entry.route, entry.label));
        }
    }
    out
}

/// The (prev, next) neighbors of `current` in the linear step
/// sequence, or `None` at the ends.
pub fn neighbors(
    current: &str,
) -> (
    Option<(&'static Route<()>, &'static str)>,
    Option<(&'static Route<()>, &'static str)>,
) {
    let order = flat_order();
    let idx = order.iter().position(|(r, _)| r.name() == current);
    match idx {
        Some(i) => {
            let prev = if i > 0 { Some(order[i - 1]) } else { None };
            let next = order.get(i + 1).copied();
            (prev, next)
        }
        None => (None, None),
    }
}
