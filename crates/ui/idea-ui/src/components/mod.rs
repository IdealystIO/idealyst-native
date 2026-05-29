//! Component implementations. Each module exports a plain `fn`
//! plus the variant enums its stylesheet uses. Invocation macros
//! live in `crate::invocations` so all of them are `#[macro_export]`
//! at the crate root.

use runtime_core::{text, Element, IntoElement, IntoStyleSource, Reactive};

/// Render an optional, possibly-reactive text prop
/// (`Reactive<Option<String>>`) as an optional styled text node:
///
/// - `Static(None)` → `None` (no node — no layout slot for an absent
///   label).
/// - `Static(Some(s))` → a static text node.
/// - `Dynamic(f)` → a reactive text node that re-paints when `f`'s
///   signals change, showing `""` while the value is `None`.
///
/// Shared by the components with an optional text prop (Switch/Field
/// `label`, Alert `body`). Coercion is uniform: a call-site
/// `label = Some("x".to_string())` or `label = None` lands here via the
/// `ui!`/`jsx!` dispatch's per-field `.into()` (blanket
/// `From<Option<String>>`), and a `Signal<Option<String>>` /
/// `rx!(Some(...))` arrives `Dynamic`.
pub(crate) fn optional_reactive_text(
    content: Reactive<Option<String>>,
    style: impl IntoStyleSource,
) -> Option<Element> {
    match content {
        Reactive::Static(None) => None,
        Reactive::Static(Some(s)) => Some(text(s).with_style(style).into_element()),
        Reactive::Dynamic(f) => {
            Some(text(move || f().unwrap_or_default()).with_style(style).into_element())
        }
    }
}

pub mod alert;
pub mod avatar;
pub mod badge;
pub mod button;
pub mod card;
pub mod center;
pub mod divider;
pub mod field;
pub mod icon_button;
pub mod modal;
pub mod popover;
pub mod select;
pub mod skeleton;
pub mod spacer;
pub mod spinner;
pub mod stack;
pub mod switch;
pub mod tabs;
pub mod tag;
pub mod typography;
