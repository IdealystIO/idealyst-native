//! `idea-ui` — a cross-platform component library built on the
//! idealyst framework's primitives.
//!
//! # Theme
//!
//! Idea-ui's stylesheets read from an [`IdeaTheme`](theme::IdeaTheme)
//! — a *trait*, not a struct. Apps install a concrete theme value
//! (typically [`light_theme()`] or [`dark_theme()`], or their own
//! type implementing the trait) via [`install_idea_theme`]. See the
//! `theme` module for how to extend the theme with custom fields.
//!
//! # Intents
//!
//! Themed components (Pressable, Badge, …) take an [`Intent`] —
//! a marker type implementing the [`Intent`](intent::Intent) trait —
//! that determines their semantic coloring. Ships with `Primary`,
//! `Secondary`, `Neutral`, `Ghost`, `Success`, `Warning`, `Danger`.
//! Apps add new intents by implementing the trait on their own
//! marker types; the same marker then works in every intent-aware
//! component. See the `intent` module for details.
//!
//! Quick start:
//!
//! ```ignore
//! use runtime_core::{component, signal, ui, Element};
//! use idea_ui::{install_idea_theme, light_theme, ButtonKind, IntentTag, StackGap};
//!
//! #[component]
//! pub fn app() -> Element {
//!     install_idea_theme(light_theme());
//!     let count = signal(0);
//!     ui! {
//!         Stack(gap = StackGap::Lg) {
//!             Typography(content = "Hello, idea-ui".to_string(), kind = TypographyKind::H1)
//!             Card {
//!                 Typography(content = format!("Count: {}", count.get()))
//!                 // PascalCase `Button` is idea-ui's themed clickable;
//!                 // lowercase `button` is the framework's raw `<button>`
//!                 // primitive (use it when you want native chrome with
//!                 // no idea-ui styling).
//!                 Button(
//!                     label = "Increment".to_string(),
//!                     on_click = std::rc::Rc::new(move || count.update(|n| *n += 1)),
//!                     tone = tone::Primary,
//!                     variant = variant::Filled,
//!                 )
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! # Cargo features
//!
//! - `table` (default) — the themed [`Table`] component and its `table`
//!   SDK dependency. The one feature here that still removes linked code.
//! - `docs` — a `DocControls` impl on every `*Props` for reflective
//!   control panels (what the idea-ui-docs app renders).
//! - `robot` — forward an optional `test_id` prop to each component's
//!   root interactive primitive for robot/E2E location.
//!
//! ## Primitive families per component (historical, non-gating)
//!
//! Components used to be `#[cfg]`-deleted per primitive family through six
//! `prim-*` features paired with the old core's `runtime-core/prim-*`
//! gating of walker dispatch arms, authoring builder fns, and `Backend`
//! trait methods. None of those exist now: handlers are registered into a
//! `runtime_scene::Registry` by
//! `runtime_vocabulary::handlers::register_builtins`, and reachability from
//! that boot seam (plus LTO), not a cargo feature, decides what links.
//! `runtime-vocabulary` has no `prim-*` equivalent, so the features could
//! only have deleted components while shrinking nothing — they are gone and
//! the component set is unconditional. See this crate's Cargo.toml for the
//! full rationale and where a real per-family gate would belong.
//!
//! The family map is recorded here because it is the thing that would have
//! to be recovered if `register_builtins` is ever split behind features and
//! the author-facing half is restored:
//!
//! | Family | Components that (transitively) render it |
//! | --- | --- |
//! | `icon` | Icon, IconButton, Breadcrumbs, Checkbox, Switch, Slider, Pagination (+ Button, Alert, Field, Select, Toast) |
//! | `image` | Image, Avatar |
//! | `text-input` | Textarea (+ Field, Autocomplete) |
//! | `activity` | Spinner (+ Button's loading state, Alert, Field, Toast) |
//! | `portal` | Menu, Popover, Tooltip (+ Select, Autocomplete, Modal, Toast) |
//! | `presence` | (+ Modal, Toast) |
//!
//! Multi-family components need every family listed for them: Button =
//! icon + activity, Alert = icon + activity, Select = icon + portal,
//! Modal = portal + presence, Field = icon + activity + text-input,
//! Autocomplete = text-input + portal, Toast / ToastHost = icon +
//! activity + portal + presence.

// Self-alias so derive macros (like `DocControls`) that expand to
// `::idea_ui::...` paths resolve correctly when compiling idea-ui
// itself — without this, `idea_ui` looks like an unknown external
// crate from inside its own source.
#[cfg(feature = "docs")]
extern crate self as idea_ui;

pub mod breakpoint;
pub mod components;
/// Civil date/time values + token formatting for the date components
/// (`Calendar`, `DatePicker`, `DateInput`, …) — chrono-free by design.
pub mod date;
/// Smart-typing mask for the typed date/time inputs — auto-inserted
/// delimiters + segment auto-advance, derived from the format tokens.
pub(crate) mod date_mask;
#[cfg(feature = "docs")]
pub mod doc_controls;
pub mod intent;
// Compile-checked usage examples for idea-ui components. `recipe!`
// self-gates on the `catalog` feature, so this module is empty (zero
// cost) in production and only materializes when the catalog is built.
// `pub` so the catalog-docs build script can reference each no-arg
// recipe fn by path (`idea_ui::recipes::<name>`) and render it live —
// the whole module is still nothing in production (catalog off).
pub mod recipes;
pub mod slot_override;
/// Build-tree introspection for this crate's (and idea-ui-nav's) unit
/// tests — see the module docs. Hidden: test support, not surface.
#[doc(hidden)]
pub mod test_support;
pub mod stylesheets;
pub mod theme;
mod theme_runtime;

// `theme`, `intent`, `theme_runtime`, and the extensible-system trait
// surface now live in the sibling crate `idea-theme`. The local
// `theme`/`intent`/`theme_runtime` modules above are thin shims so
// that internal code and existing consumers can keep using
// `idea_ui::theme::*` / `idea_ui::intent::*` paths unchanged.

// Convenience re-exports at the crate root — mirror the API surface
// that existed before the split so apps using `use idea_ui::Button,
// install_idea_theme, …` keep compiling.
pub use idea_theme::theme::{
    dark_theme, idea_color, idea_header, install_idea_theme, install_idea_theme_reactive,
    light_theme, set_idea_theme, Colors, IdeaTheme, IdeaThemeDefaults, IdeaThemeRef, IntentColors,
    Intents, Radius, Spacing,
};
// NB: `idea_theme::theme::Typography` (the typography *theme* struct) is
// intentionally NOT re-exported at this crate root — the root `Typography`
// name is the component tag alias (below). Reach the theme struct via
// `idea_theme::theme::Typography` if you need it for theme construction.
pub use idea_theme::{
    active_theme, active_theme_untracked, install_theme, install_themes, set_theme,
    theme_installed, ThemeTokens, TokenEntry, TokenValue, Tokenized,
};
// Canonical token references for app-authored `stylesheet!`s — reference a
// theme color/length by name without restating a fallback hex. See the
// theming guide. `theme_token!`/`theme_length!` are the compile-checked
// macros (name validated against the canonical set); `theme_color`/
// `theme_length` the string-driven fns the macros delegate to. The
// `theme_length` name resolves to BOTH the macro and the fn (distinct
// namespaces), so one re-export covers both call forms.
pub use idea_theme::{
    canonical_color, canonical_length, is_canonical_color_token, is_canonical_length_token,
    theme_color, theme_length, theme_token, CANONICAL_INTENT_TOKENS, CANONICAL_LENGTH_TOKENS,
    CANONICAL_NEUTRAL_TOKENS,
};
pub use idea_theme::{
    Danger, Info, Intent, IntoRcIntent, Neutral, Primary, Secondary, Success, Warning,
};

pub use breakpoint::{
    breakpoints, current_breakpoint, install_breakpoints, Breakpoint, Breakpoints,
};

// Each component re-exports both the PascalCase tag (which is both the
// function and a `pub type Tag = TagProps` alias emitted by `#[component]`)
// and the `*Props` struct + any companion enums. `ui! { Foo(...) }` resolves
// `Foo` via the type alias, while direct fn-call sites resolve to the fn —
// they coexist in different namespaces. See [[project_buildelement_dispatch]].
pub use components::alert::{Alert, AlertClose, AlertProps};
pub use components::autocomplete::{Autocomplete, AutocompleteProps};
pub use components::avatar::{Avatar, AvatarColor, AvatarProps, AvatarSize};
pub use components::badge::{Badge, BadgeProps};
pub use components::button::{Button, ButtonProps};
pub use components::breadcrumbs::{Breadcrumbs, BreadcrumbsProps, Crumb};
pub use components::calendar::{Calendar, CalendarProps, RangeCalendar, RangeCalendarProps};
pub use components::card::{Card, CardPadding, CardProps};
pub use components::date_input::{DateInput, DateInputProps, DateTimeInput, DateTimeInputProps};
pub use components::date_picker::{
    DatePicker, DatePickerProps, DateRangePicker, DateRangePickerProps, DateTimePicker,
    DateTimePickerProps,
};
pub use components::time_input::{TimeInput, TimeInputProps};
// The date components' value vocabulary — apps hold these in their state.
pub use date::{CivilDate, CivilDateTime, CivilTime, DateLabels, Weekday};
pub use components::center::{Center, CenterProps};
pub use components::checkbox::{Checkbox, CheckboxProps};
pub use components::chip::{Chip, ChipProps};
pub use components::grid::{Grid, GridProps};
pub use components::icon::{Icon, IconProps};
pub use components::image::{Image, ImageProps};
pub use components::link::{Link, LinkProps};
pub use components::list::{List, ListItem, ListItemProps, ListProps};
pub use components::menu::{
    Menu, MenuEntry, MenuItem, MenuItemProps, MenuLabel, MenuLabelProps, MenuProps, MenuSeparator,
    MenuSeparatorProps, SubMenu, SubMenuProps,
};
pub use components::pagination::{Pagination, PaginationProps};
pub use components::tooltip::{Tooltip, TooltipProps};
pub use components::radio::{
    Radio, RadioAxis, RadioGroup, RadioGroupProps, RadioOption, RadioProps,
};
pub use components::ControlSize;
pub use components::collapsible::{
    Accordion, AccordionExpand, AccordionItem, AccordionProps, Collapsible, CollapsibleProps,
    CollapsibleTransition,
};
pub use components::divider::{Divider, DividerAxis, DividerProps};
pub use components::field::{Adornment, Field, FieldProps};
// Style-level types, not component code — the Field/Textarea shared axes.
pub use stylesheets::{FieldAppearance, FieldSize};
pub use components::icon_button::{IconButton, IconButtonProps, IconButtonSize};
pub use components::modal::{Modal, ModalContent, ModalProps};
pub use components::popover::{Popover, PopoverProps};
pub use components::progress::{Progress, ProgressCap, ProgressMode, ProgressProps};
pub use components::segmented_control::{
    SegmentOption, SegmentedControl, SegmentedControlProps,
};
pub use components::select::{Select, SelectProps};
// Data/style types shared with Autocomplete.
pub use components::select::{SelectOption, SelectSize};
pub use components::skeleton::{Skeleton, SkeletonProps, SkeletonWidth};
pub use components::slider::{Slider, SliderProps};
pub use components::spacer::{Spacer, SpacerProps};
pub use components::spinner::{Spinner, SpinnerProps, SpinnerSize};
pub use components::stack::{
    Stack, StackAlign, StackAxis, StackGap, StackJustify, StackPadding, StackProps,
};
pub use components::surface::{Surface, SurfaceColor, SurfaceProps};
pub use components::switch::{Switch, SwitchProps};
#[cfg(feature = "table")]
pub use components::table::{Table, TableCell, TableCellProps, TableProps, TableRow, TableRowProps};
pub use components::tabs::{Tab, TabIndicator, Tabs, TabsProps};
pub use components::tag::{Tag, TagProps};
pub use components::textarea::{Textarea, TextareaProps};
pub use components::toast::{
    dismiss_toast, push_toast, push_toast_node, push_toast_with, Toast, ToastCard, ToastCardProps,
    ToastEntry, ToastHost, ToastHostProps, ToastPlacement,
};
pub use components::typography::{Typography, TypographyProps};

// Historical note: an earlier `Btn` alias for `Button` existed because the
// `ui!` macro routed PascalCase `Button` straight to the framework's native
// `<button>` primitive. Primitives are now snake_case (`button`) and the
// `ui!` macro deliberately doesn't recognize PascalCase `Button` as a
// primitive, so `ui! { Button(...) }` dispatches to this component directly
// — no alias needed.

// The trait surface + built-in modifier ZSTs come from idea-theme.
// Re-exported at the crate root so apps can write
// `use idea_ui::{tone, variant, size, shape, typography_kind}` for
// the namespaces.
pub use idea_theme::extensible::{
    installed_typography_sheet, tone, variant, size, shape, typography as typography_kind,
    ButtonSize, ButtonSizeRef, ResolutionCtx, Shape, ShapeRef, Tone, ToneRef, TypographyKind,
    TypographyKindRef, Variant, VariantRef,
};
// Macros from idea-theme. `#[macro_export]` macros live at the
// defining crate's root; re-exported here for convenience. The
// modifier macros (`tone!`, `variant!`) live in idea-theme's macro
// namespace; `app_theme!` bundles an app theme.
pub use idea_theme::{app_theme, color_token, tone, variant};

pub use stylesheets::TabPanel;
