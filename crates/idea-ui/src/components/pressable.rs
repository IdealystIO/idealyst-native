//! `Pressable` — a styled wrapper around the `Button` primitive.
//!
//! Carries `kind` (visual treatment: primary / secondary / ghost /
//! danger) and `size` variants, plus standard `label`, `on_click`,
//! and `disabled` props. Reactive props work the same way they do on
//! the underlying `Button` — closures that read signals subscribe
//! automatically.

use framework_core::{ui, Primitive};
use std::rc::Rc;

use crate::stylesheets::Pressable;
pub use crate::stylesheets::{PressableKind, PressableSize};

pub struct PressableProps {
    pub label: String,
    pub on_click: Rc<dyn Fn()>,
    pub kind: PressableKind,
    pub size: PressableSize,
    pub disabled: Option<Rc<dyn Fn() -> bool>>,
}

impl Default for PressableProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            on_click: Rc::new(|| {}),
            kind: PressableKind::default(),
            size: PressableSize::default(),
            disabled: None,
        }
    }
}

/// A button with theme-aware styling.
///
/// ```ignore
/// ui! {
///     Pressable(
///         label = "Save",
///         on_click = move || save(),
///         kind = PressableKind::Primary,
///         size = PressableSize::Md
///     )
/// }
/// ```
pub fn pressable(props: &PressableProps) -> Primitive {
    let label = props.label.clone();
    let on_click = props.on_click.clone();
    let kind = props.kind;
    let size = props.size;
    let disabled = props.disabled.clone();
    let style = Pressable().kind(kind).size(size);

    match disabled {
        Some(d) => ui! {
            Button(
                label = label,
                on_click = move || (on_click)(),
                style = style,
                disabled = move || (d)()
            )
        },
        None => ui! {
            Button(
                label = label,
                on_click = move || (on_click)(),
                style = style
            )
        },
    }
}
