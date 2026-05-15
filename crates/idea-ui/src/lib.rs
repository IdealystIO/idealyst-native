//! `idea-ui` — a cross-platform component library built on the
//! idealyst framework's primitives.
//!
//! Components compose `View`, `Text`, `Button`, etc. from
//! `framework_core` and read their styles from a shared
//! [`IdeaTheme`](theme::IdeaTheme). Apps install a theme once
//! (`install_idea_theme(...)`) and components throughout the tree
//! pick it up automatically.
//!
//! Component names deliberately don't collide with the framework's
//! primitive names (`Text`, `Button`, `View`, `Toggle`, …). Inside
//! `ui! { … }` those names are reserved — they lower directly to
//! the primitive constructors. Idea-ui therefore uses semantic
//! names: `Heading` / `Body` / `Caption` for typography,
//! `Pressable` for a styled button, `Field` for an input, etc.
//!
//! Quick start:
//!
//! ```ignore
//! use framework_core::{component, signal, ui, Primitive};
//! use idea_ui::{install_idea_theme, light_theme, HeadingKind, PressableKind, StackGap};
//!
//! #[component]
//! pub fn app() -> Primitive {
//!     install_idea_theme(light_theme());
//!     let count = signal!(0);
//!     ui! {
//!         VStack(gap = StackGap::Lg) {
//!             Heading(content = "Hello, idea-ui".to_string(), kind = HeadingKind::H1)
//!             Card {
//!                 Body(content = format!("Count: {}", count.get()))
//!                 Pressable(
//!                     label = "Increment".to_string(),
//!                     on_click = std::rc::Rc::new(move || count.update(|n| *n += 1)),
//!                     kind = PressableKind::Primary
//!                 )
//!             }
//!         }
//!     }
//! }
//! ```

pub mod components;
pub mod invocations;
pub mod stylesheets;
pub mod theme;

pub use theme::{
    dark_theme, install_idea_theme, light_theme, set_idea_theme, Colors, IdeaTheme, Radius,
    Spacing, Typography,
};

pub use components::badge::{badge, BadgeProps, BadgeTone};
pub use components::body::{body, BodyProps};
pub use components::caption::{caption, CaptionProps};
pub use components::card::{card, CardPadding, CardProps, CardTone};
pub use components::divider::{divider, DividerAxis, DividerProps};
pub use components::field::{field, FieldProps, FieldSize, FieldTone};
pub use components::heading::{heading, HeadingProps};
pub use components::pressable::{pressable, PressableKind, PressableProps, PressableSize};
pub use components::spinner::{spinner, SpinnerProps, SpinnerSize};
pub use components::stack::{
    hstack, stack, vstack, HStackProps, StackAlign, StackAxis, StackGap, StackJustify, StackProps,
    VStackProps,
};
pub use components::switch::{switch, SwitchProps};

// Re-export the stylesheet-generated variant enums that components
// pass through verbatim (so users don't need a `stylesheets::` import
// to use them).
pub use stylesheets::{BodyAlign, BodyTone, CaptionAlign, CaptionTone, HeadingAlign, HeadingKind};
