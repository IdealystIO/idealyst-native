//! Setting a literal on a BUILT primitive payload.
//!
//! A built `Element::Item` carries its primitive as `Box<dyn Any>`
//! holding a [`PrimCell<T>`], so there is no generic way to write a
//! field by name. This module is the table that supplies one: downcast
//! to each payload type, match the prop name, and coerce the literal to
//! the field's type.
//!
//! `PrimCell::with_mut` is the seam — the same one the navigator
//! handlers use to fold screen style overrides into a not-yet-mounted
//! payload. It is a no-op after the payload has been taken for mounting,
//! which is exactly right: an `Element` is single-use, and a patch that
//! arrived after realize belongs to the NEXT build of the site.
//!
//! # What is patchable, and why that is the line
//!
//! A prop is here when its type can be reconstructed FROM a
//! [`LiteralValue`] — a `String`, a number, a bool. That is exactly the
//! set a descriptor records as data, which is the same line the split
//! pass draws, which is why the two never disagree about what an edit
//! can reach.
//!
//! Three kinds of prop are deliberately absent:
//!
//! - **`test_id`** is `Option<&'static str>`. A patched value arrives at
//!   runtime and there is no `'static` to give it. Leaking a `String`
//!   to fake one would trade a dev-time convenience for an unbounded
//!   leak in a build that may be a release build (the feature is a
//!   cargo feature precisely so it CAN be).
//! - **`style`** is a `StyleProp` built from a `stylesheet!` call or a
//!   style-token accessor. The descriptor records those as
//!   [`LiteralValue::Path`] — source TEXT, because a value of an
//!   arbitrary type is not reconstructible from a string without a
//!   `FromStr` bound on every style type. A differ can see a style
//!   changed; nothing here can apply it, and saying so is the honest
//!   answer.
//! - **a reactive prop** (`Value::Dyn`, a signal-backed content) is
//!   code. Overwriting it with a constant would silently disconnect the
//!   binding, so a `Dyn`-valued field is refused rather than replaced.
//!
//! Every refusal returns `false`. The applier reports it; nothing
//! panics. An edit the running program cannot honour is a dev-loop
//! event — the answer is "rebuild" — not a crash in the user's app.

use std::any::Any;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_template::LiteralValue;
use runtime_world::Value;

use crate::prims::{
    ButtonPrim, PressablePrim, PrimCell, ScrollViewPrim, SliderPrim, TextInputPrim, TextPrim,
    TextSourceProp, TogglePrim, ViewPrim,
};

/// Downcast to one payload type and run `$body` on it, returning early
/// with whether the write landed.
macro_rules! on_prim {
    ($data:expr, $ty:ty, |$p:ident| $body:expr) => {
        if let Some(cell) = $data.downcast_ref::<PrimCell<$ty>>() {
            let mut applied = false;
            cell.with_mut(|$p: &mut $ty| applied = $body);
            return applied;
        }
    };
}

/// Write `name = value` on a built primitive payload.
///
/// Returns whether it was applied. `false` means "this program cannot
/// make that change without a rebuild", never "something went wrong".
pub(crate) fn set_literal(data: &dyn Any, name: &str, value: &LiteralValue) -> bool {
    on_prim!(data, TextPrim, |p| match name {
        "content" => set_text_source(&mut p.content, value),
        _ => set_a11y(&mut p.a11y, name, value),
    });
    on_prim!(data, ButtonPrim, |p| match name {
            "label" => set_const_string(&mut p.label, value),
            "disabled" => match (&mut p.disabled, as_bool(value)) {
                (slot @ None, Some(b)) => {
                    *slot = Some(Value::Const(b));
                    true
                }
                (Some(v), Some(b)) => set_const_bool(v, b),
                _ => false,
            },
        _ => set_a11y(&mut p.a11y, name, value),
    });
    on_prim!(data, ViewPrim, |p| match name {
        "preserves_focus" => set_field_bool(&mut p.preserves_focus, value),
        "is_container" => set_field_bool(&mut p.is_container, value),
        _ => set_a11y(&mut p.a11y, name, value),
    });
    on_prim!(data, PressablePrim, |p| set_a11y(&mut p.a11y, name, value));
    on_prim!(data, ScrollViewPrim, |p| match name {
            "horizontal" => set_field_bool(&mut p.horizontal, value),
            "end_reached_threshold" => match as_f32(value) {
                Some(f) => {
                    p.end_reached_threshold = f;
                    true
                }
                None => false,
            },
        _ => set_a11y(&mut p.a11y, name, value),
    });
    on_prim!(data, TextInputPrim, |p| match name {
            "placeholder" => match (&mut p.placeholder, as_str(value)) {
                (Value::Const(slot), Some(s)) => {
                    *slot = Some(s.to_string());
                    true
                }
                _ => false,
            },
            "secure" => match as_bool(value) {
                Some(b) => set_const_bool(&mut p.secure, b),
                None => false,
            },
        _ => set_a11y(&mut p.a11y, name, value),
    });
    on_prim!(data, TogglePrim, |p| set_a11y(&mut p.a11y, name, value));
    on_prim!(data, SliderPrim, |p| match name {
            "min" => set_f32(&mut p.min, value),
            "max" => set_f32(&mut p.max, value),
            "step" => match as_f32(value) {
                Some(f) => {
                    p.step = Some(f);
                    true
                }
                None => false,
            },
        _ => set_a11y(&mut p.a11y, name, value),
    });
    false
}

/// The a11y props every primitive carries. Handled once rather than per
/// payload: the field names are the author-facing prop names, and they
/// mean the same thing everywhere.
fn set_a11y(a11y: &mut AccessibilityProps, name: &str, value: &LiteralValue) -> bool {
    match name {
        "a11y_label" => match as_str(value) {
            Some(s) => {
                a11y.label = Some(s.to_string());
                true
            }
            None => false,
        },
        "a11y_hint" => match as_str(value) {
            Some(s) => {
                a11y.hint = Some(s.to_string());
                true
            }
            None => false,
        },
        "a11y_hidden" => set_field_bool(&mut a11y.hidden, value),
        _ => false,
    }
}

/// A text node's content. Refused when the content is reactive or
/// styled-runs: those are code, and replacing one with a constant would
/// silently drop the binding.
fn set_text_source(source: &mut TextSourceProp, value: &LiteralValue) -> bool {
    match (source, as_str(value)) {
        (TextSourceProp::Value(v), Some(s)) => set_const_string_str(v, s),
        _ => false,
    }
}

fn set_const_string(slot: &mut Value<String>, value: &LiteralValue) -> bool {
    match as_str(value) {
        Some(s) => set_const_string_str(slot, s),
        None => false,
    }
}

fn set_const_string_str(slot: &mut Value<String>, s: &str) -> bool {
    match slot {
        Value::Const(existing) => {
            *existing = s.to_string();
            true
        }
        // Reactive: the compiled closure owns this value.
        Value::Dyn(_) => false,
    }
}

fn set_const_bool(slot: &mut Value<bool>, b: bool) -> bool {
    match slot {
        Value::Const(existing) => {
            *existing = b;
            true
        }
        Value::Dyn(_) => false,
    }
}

fn set_field_bool(slot: &mut bool, value: &LiteralValue) -> bool {
    match as_bool(value) {
        Some(b) => {
            *slot = b;
            true
        }
        None => false,
    }
}

fn set_f32(slot: &mut f32, value: &LiteralValue) -> bool {
    match as_f32(value) {
        Some(f) => {
            *slot = f;
            true
        }
        None => false,
    }
}

fn as_str(value: &LiteralValue) -> Option<&str> {
    match value {
        LiteralValue::Str(s) => Some(s.as_ref()),
        _ => None,
    }
}

fn as_bool(value: &LiteralValue) -> Option<bool> {
    match value {
        LiteralValue::Bool(b) => Some(*b),
        _ => None,
    }
}

/// An author writes `min = 0` as often as `min = 0.0`, and the split
/// pass records each as what it is. Accepting both here is what keeps
/// "change a number" from depending on whether a decimal point was
/// typed.
fn as_f32(value: &LiteralValue) -> Option<f32> {
    match value {
        LiteralValue::Float(f) => Some(*f as f32),
        LiteralValue::Int(i) => Some(*i as f32),
        _ => None,
    }
}
