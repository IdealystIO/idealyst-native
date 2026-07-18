//! The component catalog — the single source of truth for the docs.
//!
//! Mirrors the idea-ui reference design: components are grouped into
//! Foundations / Primitives / Layout / Status / Actions / Forms /
//! Overlays / Navigation / Data. Each [`Entry`] carries everything the
//! chrome needs — the route, display name, status, a token hint, a one
//! line description (the page lead), the page `body` builder, and an
//! optional `Usage` code snippet.
//!
//! Three consumers read this one table:
//!   * the **sidebar** (grouped nav links with status dots + search),
//!   * the **header** (the active entry's token hint), and
//!   * the **navigator** wiring + central **page frame** in `lib.rs`
//!     (group overline, title, status badge, lead, body, Usage panel).

use runtime_core::{Element, Route};

use crate::pages;

/// How finished a component's reference page is. Drives the page's
/// status badge and the sidebar dot.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Status {
    /// A fully fleshed-out page (playground, matrices, props). Renders a
    /// green dot + "Detailed" badge.
    Detailed,
    /// A live preview of the component. Renders a muted dot + "Preview"
    /// badge.
    Preview,
}

/// One catalog row = one documented component / foundation.
pub struct Entry {
    /// Route key + path. `route.name()` is the stable id used for
    /// active-state matching.
    pub route: &'static Route<()>,
    /// Display name (page H1 + sidebar label).
    pub name: &'static str,
    /// Detailed vs Preview.
    pub status: Status,
    /// Short token hint shown in the header (e.g. `tone × variant × size`).
    pub token: &'static str,
    /// One-line description — the page lead under the title.
    pub desc: &'static str,
    /// Builds the page body (demo sections only; the frame adds the
    /// title block + Usage panel).
    pub body: fn() -> Element,
    /// Optional `Usage` snippet. Empty string = no panel.
    pub code: &'static str,
}

/// A labelled group of entries in the sidebar.
pub struct Group {
    pub label: &'static str,
    pub entries: &'static [Entry],
}

// =============================================================================
// Routes — one per catalog entry. `name` is the stable id.
// =============================================================================

// Get started
pub const OVERVIEW_ROUTE: Route<()> = Route::<()>::new("overview", "/overview");
/// Every component on one page — the cross-platform render-parity fixture.
pub const ALL_ROUTE: Route<()> = Route::<()>::new("all", "/all");
// Foundations
pub const COLORS_ROUTE: Route<()> = Route::<()>::new("colors", "/foundations/color");
pub const INTENTS_ROUTE: Route<()> = Route::<()>::new("intents", "/foundations/intents");
pub const SCALE_ROUTE: Route<()> = Route::<()>::new("scale", "/foundations/scale");
// Primitives
pub const TYPOGRAPHY_ROUTE: Route<()> = Route::<()>::new("typography", "/primitives/typography");
pub const ICON_ROUTE: Route<()> = Route::<()>::new("icon", "/primitives/icon");
pub const IMAGE_ROUTE: Route<()> = Route::<()>::new("image", "/primitives/image");
pub const DIVIDER_ROUTE: Route<()> = Route::<()>::new("divider", "/primitives/divider");
pub const SPACER_ROUTE: Route<()> = Route::<()>::new("spacer", "/primitives/spacer");
pub const SURFACE_ROUTE: Route<()> = Route::<()>::new("surface", "/primitives/surface");
// Layout
pub const STACK_ROUTE: Route<()> = Route::<()>::new("stack", "/layout/stack");
pub const GRID_ROUTE: Route<()> = Route::<()>::new("grid", "/layout/grid");
pub const CENTER_ROUTE: Route<()> = Route::<()>::new("center", "/layout/center");
// Status
pub const SPINNER_ROUTE: Route<()> = Route::<()>::new("spinner", "/status/spinner");
pub const SKELETON_ROUTE: Route<()> = Route::<()>::new("skeleton", "/status/skeleton");
pub const PROGRESS_ROUTE: Route<()> = Route::<()>::new("progress", "/status/progress");
pub const BADGE_ROUTE: Route<()> = Route::<()>::new("badge", "/status/badge");
pub const TAG_ROUTE: Route<()> = Route::<()>::new("tag", "/status/tag");
pub const CHIP_ROUTE: Route<()> = Route::<()>::new("chip", "/status/chip");
// Actions
pub const BUTTON_ROUTE: Route<()> = Route::<()>::new("button", "/actions/button");
pub const ICON_BUTTON_ROUTE: Route<()> = Route::<()>::new("icon-button", "/actions/icon-button");
pub const LINK_ROUTE: Route<()> = Route::<()>::new("link", "/actions/link");
pub const AVATAR_ROUTE: Route<()> = Route::<()>::new("avatar", "/actions/avatar");
// Forms
pub const CHECKBOX_ROUTE: Route<()> = Route::<()>::new("checkbox", "/forms/checkbox");
pub const RADIO_ROUTE: Route<()> = Route::<()>::new("radio", "/forms/radio");
pub const SWITCH_ROUTE: Route<()> = Route::<()>::new("switch", "/forms/switch");
pub const SLIDER_ROUTE: Route<()> = Route::<()>::new("slider", "/forms/slider");
pub const FIELD_ROUTE: Route<()> = Route::<()>::new("field", "/forms/field");
pub const TEXTAREA_ROUTE: Route<()> = Route::<()>::new("textarea", "/forms/textarea");
pub const SELECT_ROUTE: Route<()> = Route::<()>::new("select", "/forms/select");
pub const SEGMENTED_ROUTE: Route<()> =
    Route::<()>::new("segmentedcontrol", "/forms/segmented-control");
// Overlays
pub const TOOLTIP_ROUTE: Route<()> = Route::<()>::new("tooltip", "/overlays/tooltip");
pub const POPOVER_ROUTE: Route<()> = Route::<()>::new("popover", "/overlays/popover");
pub const MODAL_ROUTE: Route<()> = Route::<()>::new("modal", "/overlays/modal");
pub const COLLAPSIBLE_ROUTE: Route<()> = Route::<()>::new("collapsible", "/overlays/collapsible");
pub const ALERT_ROUTE: Route<()> = Route::<()>::new("alert", "/overlays/alert");
pub const TOAST_ROUTE: Route<()> = Route::<()>::new("toast", "/overlays/toast");
// Navigation
pub const BREADCRUMBS_ROUTE: Route<()> = Route::<()>::new("breadcrumbs", "/navigation/breadcrumbs");
pub const TABS_ROUTE: Route<()> = Route::<()>::new("tabs", "/navigation/tabs");
pub const PAGINATION_ROUTE: Route<()> = Route::<()>::new("pagination", "/navigation/pagination");
pub const MENU_ROUTE: Route<()> = Route::<()>::new("menu", "/navigation/menu");
pub const LIST_ROUTE: Route<()> = Route::<()>::new("list", "/navigation/list");
// Data
pub const CARD_ROUTE: Route<()> = Route::<()>::new("card", "/data/card");
pub const TABLE_ROUTE: Route<()> = Route::<()>::new("table", "/data/table");

// =============================================================================
// The catalog.
// =============================================================================

use Status::{Detailed, Preview};

pub const CATALOG: &[Group] = &[
    Group {
        label: "Get started",
        entries: &[
            Entry {
                route: &OVERVIEW_ROUTE,
                name: "Overview",
                // Detailed → green sidebar dot, matching the design's
                // primary-tinted "Overview" nav item.
                status: Detailed,
                // No token hint in the header on the landing screen.
                token: "",
                // The landing renders full-bleed (no page frame), so this
                // lead is never shown — kept for catalog completeness.
                desc: "The idea-ui component library — a token-driven UI kit where every \
                    component composes from a shared, swappable design vocabulary.",
                body: pages::overview::overview,
                code: "",
            },
            Entry {
                route: &ALL_ROUTE,
                name: "All Components",
                status: Detailed,
                token: "render-parity fixture",
                desc: "Every component rendered on one page, each section anchored with a \
                    stable test_id. The cross-platform render-parity fixture — capture it on \
                    web and macОS and diff (see `tests/parity.rs`).",
                body: pages::all::all,
                code: "",
            },
        ],
    },
    Group {
        label: "Foundations",
        entries: &[
            Entry {
                route: &COLORS_ROUTE,
                name: "Color",
                status: Detailed,
                token: "color-*",
                desc: "Neutral, non-intent tokens — the canvas every component is painted on: \
                    background, surface, text, border and focus ring.",
                body: pages::foundations::colors,
                code: "install_idea_theme(light_theme());\n// → registers color-surface, color-text, …\nset_idea_theme(dark_theme()); // rebinds values",
            },
            Entry {
                route: &INTENTS_ROUTE,
                name: "Intents",
                status: Detailed,
                token: "intent-*",
                desc: "Seven semantic palettes, each exposing six slots: solid-bg, solid-text, \
                    soft-bg, soft-text, fg, border.",
                body: pages::foundations::intents,
                code: "tone::Primary  // intent-primary-*\nvariant::Filled // → solid-bg / solid-text\nvariant::Soft   // → soft-bg / soft-text",
            },
            Entry {
                route: &SCALE_ROUTE,
                name: "Spacing & Radius",
                status: Detailed,
                token: "spacing-* · radius-*",
                desc: "The spatial system: a six-step spacing scale and four corner radii, \
                    shared by every component.",
                body: pages::foundations::scale,
                code: "padding: spacing-md;   // 12px\nborder-radius: radius-lg; // 12px",
            },
        ],
    },
    Group {
        label: "Primitives",
        entries: &[
            Entry {
                route: &TYPOGRAPHY_ROUTE,
                name: "Typography",
                status: Detailed,
                token: "typography-*",
                desc: "The type roles, from display down to overline, mapped to the typography \
                    size tokens.",
                body: pages::primitives::typography,
                code: "Typography(content = \"Heading\".into(), kind = typography_kind::H1)",
            },
            Entry {
                route: &ICON_ROUTE,
                name: "Icon",
                status: Preview,
                token: "color-text",
                desc: "Line icons on a 24px grid. Inherit currentColor and size from their context.",
                body: pages::primitives::icon,
                code: "Icon(data = icons_lucide::HEART, size = 24.0)",
            },
            Entry {
                route: &IMAGE_ROUTE,
                name: "Image",
                status: Preview,
                token: "radius-*",
                desc: "Responsive media with aspect-ratio, object-fit and radius control.",
                body: pages::primitives::image,
                code: "",
            },
            Entry {
                route: &DIVIDER_ROUTE,
                name: "Divider",
                status: Preview,
                token: "color-border",
                desc: "A hairline rule for separating content, horizontal or vertical.",
                body: pages::primitives::divider,
                code: "",
            },
            Entry {
                route: &SPACER_ROUTE,
                name: "Spacer",
                status: Preview,
                token: "spacing-*",
                desc: "An invisible box that injects a spacing-token-sized gap between siblings.",
                body: pages::primitives::spacer,
                code: "",
            },
            Entry {
                route: &SURFACE_ROUTE,
                name: "Surface",
                status: Preview,
                token: "color-surface",
                desc: "The base container — background, border and elevation drawn from neutral tokens.",
                body: pages::primitives::surface,
                code: "",
            },
        ],
    },
    Group {
        label: "Layout",
        entries: &[
            Entry {
                route: &STACK_ROUTE,
                name: "Stack",
                status: Preview,
                token: "spacing-*",
                desc: "Flex layout primitive. Lays children along an axis with a token-sized gap.",
                body: pages::layout::stack,
                code: "",
            },
            Entry {
                route: &GRID_ROUTE,
                name: "Grid",
                status: Preview,
                token: "spacing-*",
                desc: "Responsive grid with column count and gap driven by spacing tokens.",
                body: pages::layout::grid,
                code: "",
            },
            Entry {
                route: &CENTER_ROUTE,
                name: "Center",
                status: Preview,
                token: "—",
                desc: "Centers its child on both axes, with an optional max-width.",
                body: pages::layout::center,
                code: "",
            },
        ],
    },
    Group {
        label: "Status",
        entries: &[
            Entry {
                route: &SPINNER_ROUTE,
                name: "Spinner",
                status: Preview,
                token: "intent-*-fg",
                desc: "An indeterminate loading indicator, tinted by tone and sized by token.",
                body: pages::status::spinner,
                code: "",
            },
            Entry {
                route: &SKELETON_ROUTE,
                name: "Skeleton",
                status: Preview,
                token: "color-surface-alt",
                desc: "Shimmering placeholders that hold layout while content loads.",
                body: pages::status::skeleton,
                code: "",
            },
            Entry {
                route: &PROGRESS_ROUTE,
                name: "Progress",
                status: Preview,
                token: "intent-*-solid-bg",
                desc: "Determinate and indeterminate progress.",
                body: pages::status::progress,
                code: "",
            },
            Entry {
                route: &BADGE_ROUTE,
                name: "Badge",
                status: Detailed,
                token: "intent-*-soft-*",
                desc: "A compact status label. Solid or soft, across all seven intents, plus dot form.",
                body: pages::status::badge,
                code: "Badge(text = \"Active\".into(), tone = tone::Success)",
            },
            Entry {
                route: &TAG_ROUTE,
                name: "Tag",
                status: Preview,
                token: "intent-*-soft-*",
                desc: "A removable label for categorising or filtering, with optional leading icon.",
                body: pages::status::tag,
                code: "",
            },
            Entry {
                route: &CHIP_ROUTE,
                name: "Chip",
                status: Preview,
                token: "intent-primary-*",
                desc: "An interactive, selectable token — filters, choices and input chips.",
                body: pages::status::chip,
                code: "",
            },
        ],
    },
    Group {
        label: "Actions",
        entries: &[
            Entry {
                route: &BUTTON_ROUTE,
                name: "Button",
                status: Detailed,
                token: "tone × variant × size × shape",
                desc: "The workhorse. A composition of tone, variant, size and shape.",
                body: pages::actions::button,
                code: "Button(\n    label = \"Create idea\".into(),\n    tone = tone::Primary,\n    variant = variant::Filled,\n)",
            },
            Entry {
                route: &ICON_BUTTON_ROUTE,
                name: "IconButton",
                status: Preview,
                token: "tone × variant",
                desc: "A square button carrying a single icon. Shares Button's tone and variant axes.",
                body: pages::actions::icon_button,
                code: "",
            },
            Entry {
                route: &LINK_ROUTE,
                name: "Link",
                status: Preview,
                token: "intent-primary-fg",
                desc: "Inline and standalone navigation, with external and muted treatments.",
                body: pages::actions::link,
                code: "",
            },
            Entry {
                route: &AVATAR_ROUTE,
                name: "Avatar",
                status: Preview,
                token: "intent-*-soft-*",
                desc: "Represents a user or entity — image, initials or icon, with status and grouping.",
                body: pages::actions::avatar,
                code: "",
            },
        ],
    },
    Group {
        label: "Forms",
        entries: &[
            Entry {
                route: &CHECKBOX_ROUTE,
                name: "Checkbox",
                status: Detailed,
                token: "intent-primary-solid-*",
                desc: "A binary (and indeterminate) selection control with a full state matrix.",
                body: pages::forms::checkbox,
                code: "Checkbox(value = accepted, on_change = set_accepted, label = \"I accept\".into())",
            },
            Entry {
                route: &RADIO_ROUTE,
                name: "Radio",
                status: Detailed,
                token: "intent-primary-solid-bg",
                desc: "Single-choice selection within a group, as plain controls or selectable cards.",
                body: pages::forms::radio,
                code: "",
            },
            Entry {
                route: &SWITCH_ROUTE,
                name: "Switch",
                status: Detailed,
                token: "intent-*-solid-bg",
                desc: "An instant on/off toggle, sized and toned by token.",
                body: pages::forms::switch,
                code: "Switch(value = dark, on_change = toggle, label = \"Dark mode\".into())",
            },
            Entry {
                route: &SLIDER_ROUTE,
                name: "Slider",
                status: Preview,
                token: "intent-primary-solid-bg",
                desc: "Select a value from a continuous range; tinted with the primary token.",
                body: pages::forms::slider,
                code: "",
            },
            Entry {
                route: &FIELD_ROUTE,
                name: "Field",
                status: Preview,
                token: "color-border · focus-ring",
                desc: "A text input wrapper: label, control, helper text and error state.",
                body: pages::forms::field,
                code: "",
            },
            Entry {
                route: &TEXTAREA_ROUTE,
                name: "Textarea",
                status: Preview,
                token: "color-border · focus-ring",
                desc: "Multi-line input with helper text and character count.",
                body: pages::forms::textarea,
                code: "",
            },
            Entry {
                route: &SELECT_ROUTE,
                name: "Select",
                status: Preview,
                token: "color-surface · border",
                desc: "A single-choice dropdown built on the menu surface.",
                body: pages::forms::select,
                code: "",
            },
            Entry {
                route: &SEGMENTED_ROUTE,
                name: "SegmentedControl",
                status: Preview,
                token: "color-surface-alt",
                desc: "A compact, mutually-exclusive switch between a small set of options.",
                body: pages::forms::segmented_control,
                code: "",
            },
        ],
    },
    Group {
        label: "Overlays",
        entries: &[
            Entry {
                route: &TOOLTIP_ROUTE,
                name: "Tooltip",
                status: Preview,
                token: "color-text · inverse",
                desc: "A small contextual label revealed on hover or focus.",
                body: pages::overlays::tooltip,
                code: "",
            },
            Entry {
                route: &POPOVER_ROUTE,
                name: "Popover",
                status: Preview,
                token: "color-surface · overlay",
                desc: "A floating surface anchored to a trigger, for rich transient content.",
                body: pages::overlays::popover,
                code: "",
            },
            Entry {
                route: &MODAL_ROUTE,
                name: "Modal",
                status: Detailed,
                token: "color-overlay · surface",
                desc: "A focused dialog over a scrim. Demonstrates the overlay and surface tokens together.",
                body: pages::overlays::modal,
                code: "Modal(open = open, on_dismiss = Some(close), content = move || ui! { … })",
            },
            Entry {
                route: &COLLAPSIBLE_ROUTE,
                name: "Collapsible",
                status: Preview,
                token: "color-border",
                desc: "A header that expands and collapses a region of content.",
                body: pages::overlays::collapsible,
                code: "",
            },
            Entry {
                route: &ALERT_ROUTE,
                name: "Alert",
                status: Detailed,
                token: "intent-*-soft-*",
                desc: "An inline message conveying status. All seven intents, soft and solid, dismissible.",
                body: pages::overlays::alert,
                code: "Alert(tone = tone::Warning, title = \"Unsaved changes\".into())",
            },
            Entry {
                route: &TOAST_ROUTE,
                name: "Toast",
                status: Preview,
                token: "intent-*",
                desc: "A transient, auto-dismissing notification stacked in a corner.",
                body: pages::overlays::toast,
                code: "",
            },
        ],
    },
    Group {
        label: "Navigation",
        entries: &[
            Entry {
                route: &BREADCRUMBS_ROUTE,
                name: "Breadcrumbs",
                status: Preview,
                token: "color-text-muted",
                desc: "A hierarchical trail of links with separators and overflow collapsing.",
                body: pages::navigation::breadcrumbs,
                code: "",
            },
            Entry {
                route: &TABS_ROUTE,
                name: "Tabs",
                status: Detailed,
                token: "intent-primary-*",
                desc: "Switch between sibling panels. Underline and pill variants, fully interactive.",
                body: pages::navigation::tabs,
                code: "Tabs(value = tab, on_change = set_tab, indicator = TabIndicator::Underline) { … }",
            },
            Entry {
                route: &PAGINATION_ROUTE,
                name: "Pagination",
                status: Preview,
                token: "intent-primary-soft-*",
                desc: "Navigate paged collections with ellipsis truncation.",
                body: pages::navigation::pagination,
                code: "",
            },
            Entry {
                route: &MENU_ROUTE,
                name: "Menu",
                status: Preview,
                token: "color-surface · surface-alt",
                desc: "A list of actions on a floating surface — sections, icons, shortcuts, destructive items.",
                body: pages::navigation::menu,
                code: "",
            },
            Entry {
                route: &LIST_ROUTE,
                name: "List",
                status: Preview,
                token: "color-border",
                desc: "Vertical rows with leading/trailing content and dividers.",
                body: pages::navigation::list,
                code: "",
            },
        ],
    },
    Group {
        label: "Data",
        entries: &[
            Entry {
                route: &CARD_ROUTE,
                name: "Card",
                status: Detailed,
                token: "color-surface · radius-lg",
                desc: "A composable container: header, media, body and footer regions.",
                body: pages::data::card,
                code: "Card(variant = CardVariant::Elevated) { … }",
            },
            Entry {
                route: &TABLE_ROUTE,
                name: "Table",
                status: Preview,
                token: "color-border · surface-alt",
                desc: "Tabular data with sortable headers, selection, zebra rows and density.",
                body: pages::data::table,
                code: "",
            },
        ],
    },
];

/// The route the docs open on. The design's slight change makes the
/// Overview landing the base screen (it was Button before).
pub const DEFAULT_ROUTE: &Route<()> = &OVERVIEW_ROUTE;

/// Find the catalog entry for a route id (`route.name()`).
pub fn entry_for(route_name: &str) -> Option<&'static Entry> {
    CATALOG
        .iter()
        .flat_map(|g| g.entries.iter())
        .find(|e| e.route.name() == route_name)
}

/// The group label an entry belongs to.
pub fn group_for(route_name: &str) -> Option<&'static str> {
    for g in CATALOG {
        if g.entries.iter().any(|e| e.route.name() == route_name) {
            return Some(g.label);
        }
    }
    None
}
