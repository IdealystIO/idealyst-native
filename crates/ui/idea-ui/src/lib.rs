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
//!     let count = signal!(0);
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

// Self-alias so derive macros (like `DocControls`) that expand to
// `::idea_ui::...` paths resolve correctly when compiling idea-ui
// itself — without this, `idea_ui` looks like an unknown external
// crate from inside its own source.
#[cfg(feature = "docs")]
extern crate self as idea_ui;

pub mod breakpoint;
pub mod components;
#[cfg(feature = "docs")]
pub mod doc_controls;
pub mod intent;
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
    dark_theme, idea_color, idea_header, install_idea_theme, light_theme, set_idea_theme, Colors,
    IdeaTheme, IdeaThemeDefaults, IdeaThemeRef, IntentColors, Intents, Radius, Spacing,
};
// NB: `idea_theme::theme::Typography` (the typography *theme* struct) is
// intentionally NOT re-exported at this crate root — the root `Typography`
// name is the component tag alias (below). Reach the theme struct via
// `idea_theme::theme::Typography` if you need it for theme construction.
pub use idea_theme::{
    active_theme, install_theme, install_themes, set_theme, ThemeTokens, TokenEntry, TokenValue,
    Tokenized,
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
pub use components::alert::{Alert, AlertProps};
pub use components::avatar::{Avatar, AvatarColor, AvatarProps, AvatarSize};
pub use components::badge::{Badge, BadgeProps};
pub use components::button::{Button, ButtonProps};
pub use components::breadcrumbs::{Breadcrumbs, BreadcrumbsProps, Crumb};
pub use components::card::{Card, CardPadding, CardProps};
pub use components::center::{Center, CenterProps};
pub use components::checkbox::{Checkbox, CheckboxProps};
pub use components::grid::{Grid, GridProps};
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
pub use components::field::{Field, FieldAppearance, FieldProps, FieldSize};
pub use components::icon_button::{IconButton, IconButtonProps, IconButtonSize};
pub use components::modal::{Modal, ModalProps};
pub use components::popover::{Popover, PopoverProps};
pub use components::progress::{Progress, ProgressProps};
pub use components::select::{Select, SelectOption, SelectProps, SelectSize};
pub use components::skeleton::{Skeleton, SkeletonProps, SkeletonWidth};
pub use components::spacer::{Spacer, SpacerProps};
pub use components::spinner::{Spinner, SpinnerProps, SpinnerSize};
pub use components::stack::{
    Stack, StackAlign, StackAxis, StackGap, StackJustify, StackPadding, StackProps,
};
pub use components::switch::{Switch, SwitchProps};
pub use components::table::{Table, TableCell, TableCellProps, TableProps, TableRow, TableRowProps};
pub use components::tabs::{Tab, Tabs, TabsProps};
pub use components::tag::{Tag, TagProps};
pub use components::textarea::{Textarea, TextareaProps};
pub use components::toast::{
    dismiss_toast, push_toast, push_toast_with, ToastCard, ToastCardProps, ToastEntry, ToastHost,
    ToastHostProps, ToastPlacement,
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
    tone, variant, size, shape, typography as typography_kind,
    ButtonSize, ButtonSizeRef, ResolutionCtx, Shape, ShapeRef, Tone, ToneRef, TypographyKind,
    TypographyKindRef, Variant, VariantRef,
};
// Macros from idea-theme. `#[macro_export]` macros live at the
// defining crate's root; re-exported here for convenience. The
// modifier macros (`tone!`, `variant!`) live in idea-theme's macro
// namespace; `app_theme!` bundles an app theme.
pub use idea_theme::{app_theme, color_token, tone, variant};

pub use stylesheets::TabPanel;
