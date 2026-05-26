//! Route constants + the sidebar/drawer index.
//!
//! Routes are organized into hierarchical sections; the sidebar
//! renders each section's label as a small header and the entries
//! beneath it as nav links. On mobile, the same flat list flows
//! through the DrawerNavigator's drawer entries.

use runtime_core::Route;

// Introduction (architectural / "why it was built this way")
pub const INTRODUCTION_ROUTE: Route<()> =
    Route::<()>::new("introduction", "/introduction");

// Getting Started
pub const OVERVIEW_ROUTE: Route<()> = Route::<()>::new("overview", "/");
pub const QUICKSTART_ROUTE: Route<()> = Route::<()>::new("quickstart", "/quickstart");

// Core Concepts
pub const COMPONENTS_ROUTE: Route<()> = Route::<()>::new("components", "/concepts/components");
pub const REACTIVITY_ROUTE: Route<()> = Route::<()>::new("reactivity", "/concepts/reactivity");
pub const ASYNC_REACTIVITY_ROUTE: Route<()> =
    Route::<()>::new("async-reactivity", "/concepts/async-reactivity");
pub const SERVER_FUNCTIONS_ROUTE: Route<()> =
    Route::<()>::new("server-functions", "/concepts/server-functions");
pub const UI_DSL_ROUTE: Route<()> = Route::<()>::new("ui-dsl", "/concepts/ui-dsl");
pub const PRIMITIVES_ROUTE: Route<()> = Route::<()>::new("primitives", "/concepts/primitives");
pub const STYLES_ROUTE: Route<()> = Route::<()>::new("styles", "/concepts/styles");
pub const ANIMATION_ROUTE: Route<()> = Route::<()>::new("animation", "/concepts/animation");
pub const NAVIGATION_ROUTE: Route<()> = Route::<()>::new("navigation", "/concepts/navigation");

// Building Blocks (reference-y pages that deepen the conceptual ones)
pub const LISTS_ROUTE: Route<()> = Route::<()>::new("lists", "/reference/lists");
pub const ICONS_ROUTE: Route<()> = Route::<()>::new("icons", "/reference/icons");
pub const REFS_ROUTE: Route<()> = Route::<()>::new("refs", "/reference/refs");
pub const PORTAL_ROUTE: Route<()> = Route::<()>::new("portal", "/reference/portal");
pub const NET_ROUTE: Route<()> = Route::<()>::new("net", "/reference/net");

// Tooling
pub const ROBOT_ROUTE: Route<()> = Route::<()>::new("robot", "/tools/robot");
pub const DEV_TOOLS_ROUTE: Route<()> = Route::<()>::new("dev-tools", "/tools/dev-tools");

// Backends
pub const BACKENDS_ROUTE: Route<()> = Route::<()>::new("backends", "/backends");
pub const WRITING_A_BACKEND_ROUTE: Route<()> =
    Route::<()>::new("writing-a-backend", "/backends/writing");
pub const THIRD_PARTY_PRIMITIVES_ROUTE: Route<()> =
    Route::<()>::new("third-party-primitives", "/backends/third-party-primitives");

// Advanced
pub const WGPU_NATIVE_API_ROUTE: Route<()> =
    Route::<()>::new("wgpu-native-api", "/advanced/wgpu-native-api");
pub const SIMULATOR_ROUTE: Route<()> =
    Route::<()>::new("simulator", "/advanced/simulator");
pub const BUILDING_A_THEME_SYSTEM_ROUTE: Route<()> =
    Route::<()>::new("building-a-theme-system", "/advanced/building-a-theme-system");
pub const REACTIVE_TEXT_BINDINGS_ROUTE: Route<()> =
    Route::<()>::new("reactive-text-bindings", "/advanced/reactive-text-bindings");

// Reference (legacy hand-built pages — to be migrated to `docs!`)
pub const MACROS_ROUTE: Route<()> = Route::<()>::new("macros", "/reference/macros");
pub const CLI_ROUTE: Route<()> = Route::<()>::new("cli", "/reference/cli");
pub const PLATFORMS_ROUTE: Route<()> = Route::<()>::new("platforms", "/reference/platforms");

pub struct IndexEntry {
    pub name: &'static str,
    pub label: &'static str,
}

pub struct IndexSection {
    pub label: &'static str,
    pub items: &'static [IndexEntry],
}

/// Hierarchical sidebar: each section gets a heading and its
/// entries appear underneath. Order matters — sidebar renders in
/// declaration order.
pub const SECTIONS: &[IndexSection] = &[
    IndexSection {
        label: "Introduction",
        items: &[
            IndexEntry { name: "introduction", label: "Introduction" },
        ],
    },
    IndexSection {
        label: "Getting Started",
        items: &[
            IndexEntry { name: "overview", label: "Overview" },
            IndexEntry { name: "quickstart", label: "Getting Started" },
        ],
    },
    IndexSection {
        label: "Core Concepts",
        items: &[
            IndexEntry { name: "components", label: "Components" },
            IndexEntry { name: "reactivity", label: "Reactivity" },
            IndexEntry { name: "async-reactivity", label: "Async Reactivity" },
            IndexEntry { name: "server-functions", label: "Server Functions" },
            IndexEntry { name: "primitives", label: "Primitives" },
            IndexEntry { name: "styles", label: "Styles & Themes" },
            IndexEntry { name: "animation", label: "Animation" },
            IndexEntry { name: "ui-dsl", label: "UI DSL" },
        ],
    },
    IndexSection {
        label: "Building Blocks",
        items: &[
            IndexEntry { name: "navigation", label: "Navigation" },
            IndexEntry { name: "lists", label: "Lists" },
            IndexEntry { name: "icons", label: "Icons" },
            IndexEntry { name: "refs", label: "Refs" },
            IndexEntry { name: "portal", label: "Portal & Overlays" },
            IndexEntry { name: "net", label: "Net (HTTP)" },
        ],
    },
    IndexSection {
        label: "Tooling",
        items: &[
            IndexEntry { name: "robot", label: "Robot" },
            IndexEntry { name: "dev-tools", label: "Dev Tools" },
        ],
    },
    IndexSection {
        label: "Backends",
        items: &[
            IndexEntry { name: "backends", label: "Backends Overview" },
            IndexEntry { name: "writing-a-backend", label: "Writing a Backend" },
            IndexEntry { name: "third-party-primitives", label: "Third-party Primitives" },
        ],
    },
    IndexSection {
        label: "Advanced",
        items: &[
            IndexEntry { name: "building-a-theme-system", label: "Building a Theme System" },
            IndexEntry { name: "reactive-text-bindings", label: "Reactive Text Bindings" },
            IndexEntry { name: "wgpu-native-api", label: "wgpu Native API" },
            IndexEntry { name: "simulator", label: "Simulator (live preview)" },
        ],
    },
    IndexSection {
        label: "Reference",
        items: &[
            IndexEntry { name: "macros", label: "Macros" },
            IndexEntry { name: "cli", label: "CLI" },
            IndexEntry { name: "platforms", label: "Platforms" },
        ],
    },
];
