//! One lazy wasm chunk per docs page.
//!
//! Every catalog entry's `body` routes through a `#[component(lazy,
//! retryable)]` wrapper here, so each page's demo tree — its sections,
//! per-page signals, and snippet text — compiles into its own chunk
//! (`module_<n>___lazy_body.wasm`; the component's readable name is in
//! its `__wasm_split.js` loader symbol) fetched on first navigation
//! instead of shipping in `main.wasm`. The chrome (sidebar / header /
//! the `screen_frame` scroll shell) stays eager, but the page frame
//! itself (overline, title, status badge, lead, Usage panel —
//! `shell::page_frame_content`) renders INSIDE the chunk: while the
//! chunk fetches, the loading fallback (a centered Simulated-mode
//! Progress bar) owns the whole screen instead of sandwiching a loader
//! between eager frame pieces. On native there is
//! no bundle to split: the bodies compile inline and mount
//! synchronously, so the placeholder never shows (see the framework's
//! lazy-loading guide).
//!
//! `routes::Entry.body` is a plain `fn() -> Element`, so each chunk
//! component is paired with a snake_case shim that instantiates it with
//! the shared loading / error fallbacks below. `retryable` derives
//! `Clone` on the (zero-field) props so the error UI's Retry button can
//! re-invoke the chunk loader.

use idea_ui::{Button, ControlSize, Progress, ProgressMode, Stack, StackGap, Typography};
use runtime_core::primitives::lazy::LazyError;
use runtime_core::{component, ui, Element};

use crate::pages;
use crate::styles::{ChunkFallback, ChunkLoaderBar};

/// Shared fallback while a page's chunk is fetching. `ChunkFallback`
/// fills the scroller's viewport, centering a slim Simulated-mode
/// Progress bar — a chunk fetch is exactly the "data is loading, no
/// measurable progress" case that mode exists for. The creep state
/// lives in the fallback's scope, so every navigation starts the bar
/// from empty and the chunk's arrival swaps it out.
fn chunk_loading() -> Element {
    ui! {
        view(style = ChunkFallback()) {
            view(style = ChunkLoaderBar()) {
                Progress(mode = ProgressMode::Simulated, size = ControlSize::Sm)
            }
        }
    }
}

/// Shared error UI: name the failure, offer a retry.
fn chunk_error(e: &LazyError) -> Element {
    let message = format!("Couldn't load this page: {}", e.message());
    let retry = e.retry();
    ui! {
        view(style = ChunkFallback()) {
            Stack(gap = StackGap::Md) {
                Typography(content = message)
                Button(label = "Retry".to_string(), on_click = retry)
            }
        }
    }
}

/// One chunk boundary + catalog shim per page. The `#[component]`
/// wrapper is the unit wasm-split carves out; the shim is what the
/// `routes::CATALOG` table points at.
///
/// The 4-arg form is the standard component page: the chunk wraps the
/// demo body in `shell::page_frame_content` (looked up from the catalog
/// by the route id) so the WHOLE visible page — frame included — is
/// behind the loading state. The 3-arg form is for bodies that own
/// their entire layout (the Overview landing).
macro_rules! lazy_page {
    ($Chunk:ident, $shim:ident, $body:path, $route:path) => {
        #[component(lazy, retryable)]
        fn $Chunk() -> Element {
            let entry = crate::routes::entry_for($route.name())
                .expect("lazy_page! route is in the catalog");
            // Collect the body's `Section` anchors for the frame's
            // "On this page" panel while building it.
            let (body, toc) = crate::shell::collect_toc(|| $body());
            crate::shell::page_frame_content(entry, body, toc)
        }

        pub fn $shim() -> Element {
            ui! {
                $Chunk(
                    loading = || chunk_loading(),
                    error = |e: &LazyError| chunk_error(e),
                )
            }
        }
    };
    ($Chunk:ident, $shim:ident, $body:path) => {
        #[component(lazy, retryable)]
        fn $Chunk() -> Element {
            $body()
        }

        pub fn $shim() -> Element {
            ui! {
                $Chunk(
                    loading = || chunk_loading(),
                    error = |e: &LazyError| chunk_error(e),
                )
            }
        }
    };
}

// Get started — the Overview landing owns its whole layout (no frame).
lazy_page!(OverviewPage, overview, pages::overview::overview);
// Foundations
lazy_page!(ColorsPage, colors, pages::foundations::colors, crate::routes::COLORS_ROUTE);
lazy_page!(IntentsPage, intents, pages::foundations::intents, crate::routes::INTENTS_ROUTE);
lazy_page!(ScalePage, scale, pages::foundations::scale, crate::routes::SCALE_ROUTE);
lazy_page!(
    ThemeEditorPage,
    theme_editor,
    pages::foundations::theme_editor,
    crate::routes::THEME_EDITOR_ROUTE
);
// Primitives
lazy_page!(
    TypographyPage,
    typography,
    pages::primitives::typography,
    crate::routes::TYPOGRAPHY_ROUTE
);
lazy_page!(IconPage, icon, pages::primitives::icon, crate::routes::ICON_ROUTE);
lazy_page!(ImagePage, image, pages::primitives::image, crate::routes::IMAGE_ROUTE);
lazy_page!(DividerPage, divider, pages::primitives::divider, crate::routes::DIVIDER_ROUTE);
lazy_page!(SpacerPage, spacer, pages::primitives::spacer, crate::routes::SPACER_ROUTE);
lazy_page!(SurfacePage, surface, pages::primitives::surface, crate::routes::SURFACE_ROUTE);
// Layout
lazy_page!(StackPage, stack, pages::layout::stack, crate::routes::STACK_ROUTE);
lazy_page!(GridPage, grid, pages::layout::grid, crate::routes::GRID_ROUTE);
lazy_page!(CenterPage, center, pages::layout::center, crate::routes::CENTER_ROUTE);
// Status
lazy_page!(SpinnerPage, spinner, pages::status::spinner, crate::routes::SPINNER_ROUTE);
lazy_page!(SkeletonPage, skeleton, pages::status::skeleton, crate::routes::SKELETON_ROUTE);
lazy_page!(ProgressPage, progress, pages::status::progress, crate::routes::PROGRESS_ROUTE);
lazy_page!(BadgePage, badge, pages::status::badge, crate::routes::BADGE_ROUTE);
lazy_page!(TagPage, tag, pages::status::tag, crate::routes::TAG_ROUTE);
lazy_page!(ChipPage, chip, pages::status::chip, crate::routes::CHIP_ROUTE);
// Actions
lazy_page!(ButtonPage, button, pages::actions::button, crate::routes::BUTTON_ROUTE);
lazy_page!(
    IconButtonPage,
    icon_button,
    pages::actions::icon_button,
    crate::routes::ICON_BUTTON_ROUTE
);
lazy_page!(LinkPage, link, pages::actions::link, crate::routes::LINK_ROUTE);
lazy_page!(AvatarPage, avatar, pages::actions::avatar, crate::routes::AVATAR_ROUTE);
// Forms
lazy_page!(CheckboxPage, checkbox, pages::forms::checkbox, crate::routes::CHECKBOX_ROUTE);
lazy_page!(RadioPage, radio, pages::forms::radio, crate::routes::RADIO_ROUTE);
lazy_page!(SwitchPage, switch, pages::forms::switch, crate::routes::SWITCH_ROUTE);
lazy_page!(SliderPage, slider, pages::forms::slider, crate::routes::SLIDER_ROUTE);
lazy_page!(FieldPage, field, pages::forms::field, crate::routes::FIELD_ROUTE);
lazy_page!(TextareaPage, textarea, pages::forms::textarea, crate::routes::TEXTAREA_ROUTE);
lazy_page!(SelectPage, select, pages::forms::select, crate::routes::SELECT_ROUTE);
lazy_page!(
    AutocompletePage,
    autocomplete,
    pages::forms::autocomplete,
    crate::routes::AUTOCOMPLETE_ROUTE
);
lazy_page!(
    SegmentedControlPage,
    segmented_control,
    pages::forms::segmented_control,
    crate::routes::SEGMENTED_ROUTE
);
lazy_page!(CalendarPage, calendar, pages::forms::calendar, crate::routes::CALENDAR_ROUTE);
lazy_page!(
    DatePickerPage,
    date_picker,
    pages::forms::date_picker,
    crate::routes::DATE_PICKER_ROUTE
);
lazy_page!(
    DateInputPage,
    date_input,
    pages::forms::date_input,
    crate::routes::DATE_INPUT_ROUTE
);
lazy_page!(
    TimeInputPage,
    time_input,
    pages::forms::time_input,
    crate::routes::TIME_INPUT_ROUTE
);
// Overlays
lazy_page!(TooltipPage, tooltip, pages::overlays::tooltip, crate::routes::TOOLTIP_ROUTE);
lazy_page!(PopoverPage, popover, pages::overlays::popover, crate::routes::POPOVER_ROUTE);
lazy_page!(ModalPage, modal, pages::overlays::modal, crate::routes::MODAL_ROUTE);
lazy_page!(
    CollapsiblePage,
    collapsible,
    pages::overlays::collapsible,
    crate::routes::COLLAPSIBLE_ROUTE
);
lazy_page!(AlertPage, alert, pages::overlays::alert, crate::routes::ALERT_ROUTE);
lazy_page!(ToastPage, toast, pages::overlays::toast, crate::routes::TOAST_ROUTE);
// Navigation
lazy_page!(
    BreadcrumbsPage,
    breadcrumbs,
    pages::navigation::breadcrumbs,
    crate::routes::BREADCRUMBS_ROUTE
);
lazy_page!(TabsPage, tabs, pages::navigation::tabs, crate::routes::TABS_ROUTE);
lazy_page!(
    PaginationPage,
    pagination,
    pages::navigation::pagination,
    crate::routes::PAGINATION_ROUTE
);
lazy_page!(MenuPage, menu, pages::navigation::menu, crate::routes::MENU_ROUTE);
lazy_page!(ListPage, list, pages::navigation::list, crate::routes::LIST_ROUTE);
// Data
lazy_page!(CardPage, card, pages::data::card, crate::routes::CARD_ROUTE);
lazy_page!(TablePage, table, pages::data::table, crate::routes::TABLE_ROUTE);
