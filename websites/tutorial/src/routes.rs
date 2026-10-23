//! Route constants, one per written lesson. Their order, numbering and
//! grouping into chapters live in [`crate::course`].

use runtime_core::Route;

// ---- Intro ----
pub const HOME_ROUTE: Route<()> = Route::<()>::new("home", "/");

// ---- Getting started: install, scaffold, run ----
pub const START_INSTALL_ROUTE: Route<()> = Route::<()>::new("start-install", "/start/install");
pub const START_CREATE_ROUTE: Route<()> = Route::<()>::new("start-create", "/start/create");
pub const START_RUN_ROUTE: Route<()> = Route::<()>::new("start-run", "/start/run");
pub const START_EDITOR_ROUTE: Route<()> = Route::<()>::new("start-editor", "/start/editor");
pub const START_MCP_ROUTE: Route<()> = Route::<()>::new("start-mcp", "/start/ai-assisted");
pub const START_ANATOMY_ROUTE: Route<()> = Route::<()>::new("start-anatomy", "/start/anatomy");

// ---- Building UI: primitives, components, state, control flow ----
pub const UI_PRIMITIVES_ROUTE: Route<()> = Route::<()>::new("ui-primitives", "/ui/primitives");
pub const UI_COMPONENTS_ROUTE: Route<()> = Route::<()>::new("ui-components", "/ui/components");
pub const UI_CHILDREN_ROUTE: Route<()> = Route::<()>::new("ui-children", "/ui/children");
pub const UI_STATE_ROUTE: Route<()> = Route::<()>::new("ui-state", "/ui/state");
pub const UI_FLOW_ROUTE: Route<()> = Route::<()>::new("ui-flow", "/ui/conditions-and-lists");
pub const UI_REACTIVE_PROPS_ROUTE: Route<()> = Route::<()>::new("ui-reactive-props", "/ui/reactive-props");

// ---- Styling: stylesheets, layout, themes ----
pub const STY_SHEETS_ROUTE: Route<()> = Route::<()>::new("sty-sheets", "/styling/stylesheets");
pub const STY_LAYOUT_ROUTE: Route<()> = Route::<()>::new("sty-layout", "/styling/layout");
pub const STY_THEMES_ROUTE: Route<()> = Route::<()>::new("sty-themes", "/styling/themes");

// ---- Component stylesheets: variants, states, slots, restyling ----
pub const CS_VARIANTS_ROUTE: Route<()> = Route::<()>::new("cs-variants", "/component-styles/variants");
pub const CS_STATES_ROUTE: Route<()> = Route::<()>::new("cs-states", "/component-styles/states");
pub const CS_OVERRIDES_ROUTE: Route<()> = Route::<()>::new("cs-overrides", "/component-styles/overrides");
pub const CS_SLOTS_ROUTE: Route<()> = Route::<()>::new("cs-slots", "/component-styles/slots");
pub const CS_RESTYLE_ROUTE: Route<()> = Route::<()>::new("cs-restyle", "/component-styles/restyle");

// ---- Idealyst 101: the unifying mental model ----
pub const CORE_ENGINE_ROUTE: Route<()> = Route::<()>::new("core-engine", "/core/engine");
pub const CORE_FLUSH_ROUTE: Route<()> = Route::<()>::new("core-flush", "/core/flush");

// ---- Architecture: how the framework is layered ----
pub const ARCH_OVERVIEW_ROUTE: Route<()> = Route::<()>::new("arch-overview", "/architecture/overview");
pub const ARCH_BACKENDS_ROUTE: Route<()> = Route::<()>::new("arch-backends", "/architecture/backends");
pub const ARCH_CATALOG_ROUTE: Route<()> = Route::<()>::new("arch-catalog", "/architecture/catalog");
pub const ARCH_SDKS_ROUTE: Route<()> = Route::<()>::new("arch-sdks", "/architecture/sdks");

// ---- Track 1: Reactivity ----
pub const RX_SIGNALS_ROUTE: Route<()> = Route::<()>::new("rx-signals", "/reactivity/signals");
pub const RX_FLUSH_ROUTE: Route<()> = Route::<()>::new("rx-flush", "/reactivity/flush");
pub const RX_EFFECTS_ROUTE: Route<()> = Route::<()>::new("rx-effects", "/reactivity/effects");
pub const RX_DERIVED_ROUTE: Route<()> = Route::<()>::new("rx-derived", "/reactivity/derived");

// ---- Track 3: Media queries ----
pub const MQ_BREAKPOINTS_ROUTE: Route<()> = Route::<()>::new("mq-breakpoints", "/media/breakpoints");
pub const MQ_MOBILE_FIRST_ROUTE: Route<()> = Route::<()>::new("mq-mobile-first", "/media/mobile-first");
pub const MQ_SIGNAL_ROUTE: Route<()> = Route::<()>::new("mq-signal", "/media/breakpoint-signal");
pub const MQ_CONTAINER_ROUTE: Route<()> = Route::<()>::new("mq-container", "/media/container-queries");

// ---- Accessibility ----
pub const A11Y_DEFAULTS_ROUTE: Route<()> = Route::<()>::new("a11y-defaults", "/accessibility/defaults");
pub const A11Y_MODEL_ROUTE: Route<()> = Route::<()>::new("a11y-model", "/accessibility/model");

// ---- Advanced (authored later) ----
pub const ADV_BACKENDS_ROUTE: Route<()> = Route::<()>::new("adv-backends", "/advanced/custom-backends");
pub const ADV_CLI_ROUTE: Route<()> = Route::<()>::new("adv-cli", "/advanced/interactive-cli");
pub const ADV_EMBEDDED_ROUTE: Route<()> = Route::<()>::new("adv-embedded", "/advanced/embedded-rendering");
