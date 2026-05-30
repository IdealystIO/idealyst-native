//! Route constants + the sidebar index for the idea-ui docs.
//!
//! `SECTIONS` is the single source of truth: the sidebar walks it in
//! order to render section headers + page links, and the navigator
//! pulls per-route screens from the same constants in `lib.rs`.

use runtime_core::Route;

// =============================================================================
// Getting Started
// =============================================================================
pub const OVERVIEW_ROUTE: Route<()> = Route::<()>::new("overview", "/");
pub const INSTALL_ROUTE: Route<()> = Route::<()>::new("install", "/getting-started/install");
pub const HELLO_ROUTE: Route<()> = Route::<()>::new("hello", "/getting-started/hello");

// =============================================================================
// Theming
// =============================================================================
pub const TOKENS_ROUTE: Route<()> = Route::<()>::new("tokens", "/theming/tokens");
pub const INTENTS_ROUTE: Route<()> = Route::<()>::new("intents", "/theming/intents");
pub const LIGHT_DARK_ROUTE: Route<()> = Route::<()>::new("light-dark", "/theming/light-dark");
pub const CUSTOM_THEME_ROUTE: Route<()> = Route::<()>::new("custom-theme", "/theming/custom");
pub const MODIFIERS_ROUTE: Route<()> = Route::<()>::new("modifiers", "/theming/modifiers");

// =============================================================================
// Layout
// =============================================================================
pub const STACK_ROUTE: Route<()> = Route::<()>::new("stack", "/layout/stack");
pub const CARD_ROUTE: Route<()> = Route::<()>::new("card", "/layout/card");
pub const TABLE_ROUTE: Route<()> = Route::<()>::new("table", "/layout/table");
pub const DIVIDER_ROUTE: Route<()> = Route::<()>::new("divider", "/layout/divider");
pub const CENTER_ROUTE: Route<()> = Route::<()>::new("center", "/layout/center");
pub const SPACER_ROUTE: Route<()> = Route::<()>::new("spacer", "/layout/spacer");

// =============================================================================
// Typography
// =============================================================================
pub const TYPOGRAPHY_ROUTE: Route<()> = Route::<()>::new("typography", "/typography");

// =============================================================================
// Actions
// =============================================================================
pub const BUTTON_ROUTE: Route<()> = Route::<()>::new("button", "/actions/button");
pub const ICON_BUTTON_ROUTE: Route<()> = Route::<()>::new("icon-button", "/actions/icon-button");
pub const BADGE_ROUTE: Route<()> = Route::<()>::new("badge", "/actions/badge");
pub const TAG_ROUTE: Route<()> = Route::<()>::new("tag", "/actions/tag");

// =============================================================================
// Inputs
// =============================================================================
pub const FIELD_ROUTE: Route<()> = Route::<()>::new("field", "/inputs/field");
pub const SWITCH_ROUTE: Route<()> = Route::<()>::new("switch", "/inputs/switch");
pub const SELECT_ROUTE: Route<()> = Route::<()>::new("select", "/inputs/select");

// =============================================================================
// Feedback
// =============================================================================
pub const ALERT_ROUTE: Route<()> = Route::<()>::new("alert", "/feedback/alert");
pub const SPINNER_ROUTE: Route<()> = Route::<()>::new("spinner", "/feedback/spinner");
pub const SKELETON_ROUTE: Route<()> = Route::<()>::new("skeleton", "/feedback/skeleton");
pub const AVATAR_ROUTE: Route<()> = Route::<()>::new("avatar", "/feedback/avatar");

// =============================================================================
// Overlays
// =============================================================================
pub const MODAL_ROUTE: Route<()> = Route::<()>::new("modal", "/overlays/modal");
pub const POPOVER_ROUTE: Route<()> = Route::<()>::new("popover", "/overlays/popover");
pub const DRAWER_ROUTE: Route<()> = Route::<()>::new("drawer-pattern", "/overlays/drawer");

// =============================================================================
// Stateful
// =============================================================================
pub const TABS_ROUTE: Route<()> = Route::<()>::new("tabs", "/stateful/tabs");
pub const COLLAPSIBLE_ROUTE: Route<()> = Route::<()>::new("collapsible", "/stateful/collapsible");

// =============================================================================
// Extending
// =============================================================================
pub const EXT_CUSTOM_TONE_ROUTE: Route<()> =
    Route::<()>::new("ext-custom-tone", "/extending/custom-tone");
pub const EXT_CUSTOM_VARIANT_ROUTE: Route<()> =
    Route::<()>::new("ext-custom-variant", "/extending/custom-variant");
pub const EXT_BUILD_COMPONENT_ROUTE: Route<()> =
    Route::<()>::new("ext-build-component", "/extending/build-component");
pub const EXT_DOC_CONTROLS_ROUTE: Route<()> =
    Route::<()>::new("ext-doc-controls", "/extending/doc-controls");

// =============================================================================
// Sidebar index — drives both the sidebar and (via lib.rs) the
// navigator's `.screen(...)` wiring.
// =============================================================================

pub struct IndexEntry {
    pub route: &'static Route<()>,
    pub label: &'static str,
}

pub struct IndexSection {
    pub title: &'static str,
    pub entries: &'static [IndexEntry],
}

pub const SECTIONS: &[IndexSection] = &[
    IndexSection {
        title: "",
        entries: &[IndexEntry { route: &OVERVIEW_ROUTE, label: "Overview" }],
    },
    IndexSection {
        title: "Getting Started",
        entries: &[
            IndexEntry { route: &INSTALL_ROUTE, label: "Installation" },
            IndexEntry { route: &HELLO_ROUTE, label: "First component" },
        ],
    },
    IndexSection {
        title: "Theming",
        entries: &[
            IndexEntry { route: &TOKENS_ROUTE, label: "Theme tokens" },
            IndexEntry { route: &INTENTS_ROUTE, label: "Intents" },
            IndexEntry { route: &LIGHT_DARK_ROUTE, label: "Light & dark" },
            IndexEntry { route: &CUSTOM_THEME_ROUTE, label: "Custom themes" },
            IndexEntry { route: &MODIFIERS_ROUTE, label: "Modifiers (tone/variant/size/shape)" },
        ],
    },
    IndexSection {
        title: "Layout",
        entries: &[
            IndexEntry { route: &STACK_ROUTE, label: "Stack" },
            IndexEntry { route: &CARD_ROUTE, label: "Card" },
            IndexEntry { route: &TABLE_ROUTE, label: "Table" },
            IndexEntry { route: &DIVIDER_ROUTE, label: "Divider" },
            IndexEntry { route: &CENTER_ROUTE, label: "Center" },
            IndexEntry { route: &SPACER_ROUTE, label: "Spacer" },
        ],
    },
    IndexSection {
        title: "Typography",
        entries: &[IndexEntry { route: &TYPOGRAPHY_ROUTE, label: "Typography" }],
    },
    IndexSection {
        title: "Actions",
        entries: &[
            IndexEntry { route: &BUTTON_ROUTE, label: "Button" },
            IndexEntry { route: &ICON_BUTTON_ROUTE, label: "IconButton" },
            IndexEntry { route: &BADGE_ROUTE, label: "Badge" },
            IndexEntry { route: &TAG_ROUTE, label: "Tag" },
        ],
    },
    IndexSection {
        title: "Inputs",
        entries: &[
            IndexEntry { route: &FIELD_ROUTE, label: "Field" },
            IndexEntry { route: &SWITCH_ROUTE, label: "Switch" },
            IndexEntry { route: &SELECT_ROUTE, label: "Select" },
        ],
    },
    IndexSection {
        title: "Feedback",
        entries: &[
            IndexEntry { route: &ALERT_ROUTE, label: "Alert" },
            IndexEntry { route: &SPINNER_ROUTE, label: "Spinner" },
            IndexEntry { route: &SKELETON_ROUTE, label: "Skeleton" },
            IndexEntry { route: &AVATAR_ROUTE, label: "Avatar" },
        ],
    },
    IndexSection {
        title: "Overlays",
        entries: &[
            IndexEntry { route: &MODAL_ROUTE, label: "Modal" },
            IndexEntry { route: &POPOVER_ROUTE, label: "Popover" },
            IndexEntry { route: &DRAWER_ROUTE, label: "Drawer pattern" },
        ],
    },
    IndexSection {
        title: "Stateful",
        entries: &[
            IndexEntry { route: &TABS_ROUTE, label: "Tabs" },
            IndexEntry { route: &COLLAPSIBLE_ROUTE, label: "Collapsible & Accordion" },
        ],
    },
    IndexSection {
        title: "Extending",
        entries: &[
            IndexEntry { route: &EXT_CUSTOM_TONE_ROUTE, label: "Adding a custom tone" },
            IndexEntry { route: &EXT_CUSTOM_VARIANT_ROUTE, label: "Adding a custom variant" },
            IndexEntry { route: &EXT_BUILD_COMPONENT_ROUTE, label: "Building a component" },
            IndexEntry { route: &EXT_DOC_CONTROLS_ROUTE, label: "DocControls derive" },
        ],
    },
];
