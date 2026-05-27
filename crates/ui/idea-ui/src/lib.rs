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
//! use runtime_core::{component, signal, ui, Primitive};
//! use idea_ui::{install_idea_theme, light_theme, ButtonKind, IntentTag, StackGap};
//!
//! #[component]
//! pub fn app() -> Primitive {
//!     install_idea_theme(light_theme());
//!     let count = signal!(0);
//!     ui! {
//!         Stack(gap = StackGap::Lg) {
//!             Typography(content = "Hello, idea-ui".to_string(), kind = TypographyKind::H1)
//!             Card {
//!                 Typography(content = format!("Count: {}", count.get()))
//!                 // `Btn` is idea-ui's styled clickable. `Button` (capital B)
//!                 // is the framework's `<button>` primitive — useful when
//!                 // you need a native button without the idea-ui styling.
//!                 Btn(
//!                     label = "Increment".to_string(),
//!                     on_click = std::rc::Rc::new(move || count.update(|n| *n += 1)),
//!                     intent = IntentTag::Primary,
//!                     kind = ButtonKind::Solid,
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
pub mod extensible;
pub mod intent;
pub mod invocations;
pub mod stylesheets;
pub mod theme;
mod theme_runtime;

// `theme`, `intent`, `theme_runtime`, and the extensible-system trait
// surface now live in the sibling crate `idea-theme`. The local
// `theme`/`intent`/`theme_runtime` modules above are thin shims so
// that internal code and existing consumers can keep using
// `idea_ui::theme::*` / `idea_ui::intent::*` paths unchanged.

// Convenience re-exports at the crate root — mirror the API surface
// that existed before the split so apps using `use idea_ui::Btn,
// install_idea_theme, IntentTag` keep compiling.
pub use idea_theme::theme::{
    dark_theme, idea_color, idea_header, install_idea_theme, light_theme, set_idea_theme, Colors,
    IdeaTheme, IdeaThemeDefaults, IdeaThemeRef, IntentColors, Intents, Radius, Spacing, Typography,
};
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

pub use components::alert::{alert, AlertProps};
pub use components::avatar::{avatar, AvatarColor, AvatarProps, AvatarSize};
pub use components::badge::{badge, BadgeKind, BadgeProps};
pub use components::button::{button, ButtonKind, ButtonProps, ButtonSize, IntentTag};
pub use components::card::{card, CardPadding, CardProps, CardTone};
pub use components::center::{center, CenterProps};
pub use components::divider::{divider, DividerAxis, DividerProps};
pub use components::field::{field, FieldProps, FieldSize, FieldTone};
pub use components::icon_button::{icon_button, IconButtonProps, IconButtonSize};
pub use components::modal::{modal, ModalProps};
pub use components::popover::{popover, PopoverProps};
pub use components::select::{select, SelectOption, SelectProps, SelectSize};
pub use components::skeleton::{skeleton, SkeletonProps, SkeletonWidth};
pub use components::spacer::{spacer, SpacerProps};
pub use components::spinner::{spinner, SpinnerProps, SpinnerSize};
pub use components::stack::{
    stack, StackAlign, StackAxis, StackGap, StackJustify, StackPadding, StackProps,
};
pub use components::switch::{switch, SwitchProps};
pub use components::tabs::{tabs, Tab, TabsProps};
pub use components::tag::{tag, TagProps};
pub use components::typography::{
    typography, TypographyAlign, TypographyKind, TypographyProps, TypographyTone,
};

pub use stylesheets::TabPanel;
