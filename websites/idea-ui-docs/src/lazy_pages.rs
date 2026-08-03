//! One lazy wasm chunk per docs page.
//!
//! Every catalog entry's `body` routes through a `#[component(lazy,
//! retryable)]` wrapper here, so each page's demo tree — its sections,
//! per-page signals, and snippet text — compiles into its own chunk
//! (`module_<n>___lazy_body.wasm`; the component's readable name is in
//! its `__wasm_split.js` loader symbol) fetched on first navigation
//! instead of shipping in `main.wasm`. The chrome (sidebar / header /
//! page frame) stays eager. On native there is no
//! bundle to split: the bodies compile inline and mount synchronously,
//! so the placeholder never shows (see the framework's lazy-loading
//! guide).
//!
//! `routes::Entry.body` is a plain `fn() -> Element`, so each chunk
//! component is paired with a snake_case shim that instantiates it with
//! the shared loading / error fallbacks below. `retryable` derives
//! `Clone` on the (zero-field) props so the error UI's Retry button can
//! re-invoke the chunk loader.

use idea_ui::{Button, Center, Spinner, Stack, StackGap, Typography};
use runtime_core::primitives::lazy::LazyError;
use runtime_core::{component, ui, Element};

use crate::pages;

/// Shared fallback while a page's chunk is fetching.
fn chunk_loading() -> Element {
    ui! {
        Center() {
            Spinner()
        }
    }
}

/// Shared error UI: name the failure, offer a retry.
fn chunk_error(e: &LazyError) -> Element {
    let message = format!("Couldn't load this page: {}", e.message());
    let retry = e.retry();
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = message)
            Button(label = "Retry".to_string(), on_click = retry)
        }
    }
}

/// One chunk boundary + catalog shim per page. The `#[component]`
/// wrapper is the unit wasm-split carves out; the shim is what the
/// `routes::CATALOG` table points at.
macro_rules! lazy_page {
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

// Get started
lazy_page!(OverviewPage, overview, pages::overview::overview);
lazy_page!(AllPage, all, pages::all::all);
// Foundations
lazy_page!(ColorsPage, colors, pages::foundations::colors);
lazy_page!(IntentsPage, intents, pages::foundations::intents);
lazy_page!(ScalePage, scale, pages::foundations::scale);
// Primitives
lazy_page!(TypographyPage, typography, pages::primitives::typography);
lazy_page!(IconPage, icon, pages::primitives::icon);
lazy_page!(ImagePage, image, pages::primitives::image);
lazy_page!(DividerPage, divider, pages::primitives::divider);
lazy_page!(SpacerPage, spacer, pages::primitives::spacer);
lazy_page!(SurfacePage, surface, pages::primitives::surface);
// Layout
lazy_page!(StackPage, stack, pages::layout::stack);
lazy_page!(GridPage, grid, pages::layout::grid);
lazy_page!(CenterPage, center, pages::layout::center);
// Status
lazy_page!(SpinnerPage, spinner, pages::status::spinner);
lazy_page!(SkeletonPage, skeleton, pages::status::skeleton);
lazy_page!(ProgressPage, progress, pages::status::progress);
lazy_page!(BadgePage, badge, pages::status::badge);
lazy_page!(TagPage, tag, pages::status::tag);
lazy_page!(ChipPage, chip, pages::status::chip);
// Actions
lazy_page!(ButtonPage, button, pages::actions::button);
lazy_page!(IconButtonPage, icon_button, pages::actions::icon_button);
lazy_page!(LinkPage, link, pages::actions::link);
lazy_page!(AvatarPage, avatar, pages::actions::avatar);
// Forms
lazy_page!(CheckboxPage, checkbox, pages::forms::checkbox);
lazy_page!(RadioPage, radio, pages::forms::radio);
lazy_page!(SwitchPage, switch, pages::forms::switch);
lazy_page!(SliderPage, slider, pages::forms::slider);
lazy_page!(FieldPage, field, pages::forms::field);
lazy_page!(TextareaPage, textarea, pages::forms::textarea);
lazy_page!(SelectPage, select, pages::forms::select);
lazy_page!(AutocompletePage, autocomplete, pages::forms::autocomplete);
lazy_page!(
    SegmentedControlPage,
    segmented_control,
    pages::forms::segmented_control
);
lazy_page!(CalendarPage, calendar, pages::forms::calendar);
lazy_page!(DatePickerPage, date_picker, pages::forms::date_picker);
lazy_page!(DateInputPage, date_input, pages::forms::date_input);
lazy_page!(TimeInputPage, time_input, pages::forms::time_input);
// Overlays
lazy_page!(TooltipPage, tooltip, pages::overlays::tooltip);
lazy_page!(PopoverPage, popover, pages::overlays::popover);
lazy_page!(ModalPage, modal, pages::overlays::modal);
lazy_page!(CollapsiblePage, collapsible, pages::overlays::collapsible);
lazy_page!(AlertPage, alert, pages::overlays::alert);
lazy_page!(ToastPage, toast, pages::overlays::toast);
// Navigation
lazy_page!(BreadcrumbsPage, breadcrumbs, pages::navigation::breadcrumbs);
lazy_page!(TabsPage, tabs, pages::navigation::tabs);
lazy_page!(PaginationPage, pagination, pages::navigation::pagination);
lazy_page!(MenuPage, menu, pages::navigation::menu);
lazy_page!(ListPage, list, pages::navigation::list);
// Data
lazy_page!(CardPage, card, pages::data::card);
lazy_page!(TablePage, table, pages::data::table);
