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
//! use framework_core::{component, signal, ui, Primitive};
//! use idea_ui::{install_idea_theme, light_theme, IntoRcIntent, Primary, StackGap};
//!
//! #[component]
//! pub fn app() -> Primitive {
//!     install_idea_theme(light_theme());
//!     let count = signal!(0);
//!     ui! {
//!         VStack(gap = StackGap::Lg) {
//!             Heading(content = "Hello, idea-ui".to_string())
//!             Card {
//!                 Body(content = format!("Count: {}", count.get()))
//!                 Pressable(
//!                     label = "Increment".to_string(),
//!                     on_click = std::rc::Rc::new(move || count.update(|n| *n += 1)),
//!                     intent = Primary.into_rc()
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

pub mod components;
#[cfg(feature = "docs")]
pub mod doc_controls;
pub mod intent;
pub mod invocations;
pub mod stylesheets;
pub mod theme;

pub use theme::{
    dark_theme, install_idea_theme, light_theme, set_idea_theme, Colors, IdeaTheme,
    IdeaThemeDefaults, IdeaThemeRef, Radius, Spacing, Typography,
};

pub use intent::{
    apply_palette, Danger, Ghost, Intent, IntentPalette, IntoRcIntent, Neutral, Primary, Secondary,
    Success, Warning,
};

pub use components::alert::{alert, AlertProps};
pub use components::avatar::{avatar, AvatarProps, AvatarSize};
pub use components::badge::{badge, BadgeProps};
pub use components::body::{body, BodyProps};
pub use components::caption::{caption, CaptionProps};
pub use components::card::{card, CardPadding, CardProps, CardTone};
pub use components::center::{center, CenterProps};
pub use components::divider::{divider, DividerAxis, DividerProps};
pub use components::field::{field, FieldProps, FieldSize, FieldTone};
pub use components::heading::{heading, HeadingProps};
pub use components::icon_button::{icon_button, IconButtonProps, IconButtonSize};
pub use components::modal::{modal, ModalProps};
pub use components::popover::{popover, PopoverProps};
pub use components::pressable::{pressable, PressableProps, PressableSize};
pub use components::select::{select, SelectOption, SelectProps, SelectSize};
pub use components::skeleton::{skeleton, SkeletonProps, SkeletonWidth};
pub use components::spacer::{spacer, SpacerProps};
pub use components::spinner::{spinner, SpinnerProps, SpinnerSize};
pub use components::stack::{
    hstack, stack, vstack, HStackProps, StackAlign, StackAxis, StackGap, StackJustify, StackProps,
    VStackProps,
};
pub use components::switch::{switch, SwitchProps};
pub use components::tabs::{tabs, Tab, TabsProps};
pub use components::tag::{tag, TagProps};

pub use stylesheets::{BodyAlign, BodyTone, CaptionAlign, CaptionTone, HeadingAlign, HeadingKind};
