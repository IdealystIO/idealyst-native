//! Builders for the built-in primitives — the `ui!` contract (P2b).
//!
//! Convention (ports idea-lite's builder shapes, prop names aligned with
//! the current `ui!` surface so the P3 macro retarget is mechanical):
//!
//! - `view() -> ViewBuilder`, props as methods (`.style(...)`,
//!   `.on_touch(...)`, …), children via `.child(...)`, and
//!   `.build() -> runtime_scene::Element`.
//! - Reactive-capable props take `impl IntoValue<T>` — the caller picks
//!   reactivity (literal → `Const`, signal/memo/closure → `Dyn`). Inert
//!   props take concrete types.
//! - `.child(...)` accepts an `Element`, another builder (built
//!   implicitly), or a **closure** returning either — closures become
//!   structural holes ([`dyn_element`](runtime_scene::dyn_element)), so
//!   `match sig.get() { … }` as a child is reactive layout for free.
//! - Text coercion ([`TextContent`]) is this vocabulary's policy:
//!   `&str`/`String` are static, closures-of-`ToString` and
//!   signals/memos of `ToString` values are dynamic.
//!
//! Tag names match `canonical_primitive`'s snake_case list
//! (`crates/runtime/macros/src/primitives.rs`): `view`, `text`, `button`,
//! `pressable`, `image`, `icon`, `toggle`, `slider`,
//! `activity_indicator`, `link`, `scroll_view`, `text_input`,
//! `text_area`. The remaining tags (`virtualizer`/`flat_list`,
//! `graphics`, `overlay`/`anchored_overlay`, `presence`, navigator) are
//! deferred — see the crate docs. (`when` needs no builder: it lowers
//! to the scene's `dyn_keyed` hole directly.)

use runtime_scene::{dyn_element, Element};
use runtime_world::{Memo, ReadSignal, Signal, Value};

mod media;
mod text;
mod view;
mod widgets;

pub use media::{icon, image, link, IconBuilder, ImageBuilder, LinkBuilder};
pub use text::{button, text, ButtonBuilder, TextBuilder};
pub use view::{
    pressable, scroll_view, view, PressableBuilder, ScrollViewBuilder, ViewBuilder,
};
pub use widgets::{
    activity_indicator, slider, text_area, text_input, toggle, ActivityIndicatorBuilder,
    SliderBuilder, TextAreaBuilder, TextInputBuilder, ToggleBuilder,
};

/// Anything that can finish into an [`Element`] — implemented by
/// `Element` itself and by every builder (implicit `.build()`), so
/// `.child(text().content("hi"))` works without a trailing `.build()`.
pub trait IntoSceneElement {
    fn into_scene_element(self) -> Element;
}

impl IntoSceneElement for Element {
    fn into_scene_element(self) -> Element {
        self
    }
}

/// What container builders accept as a child: a finished/finishable
/// element, or a closure producing one — the closure becomes a
/// structural hole that rebuilds when its eager signal reads change.
pub trait SceneChild {
    fn into_child(self) -> Element;
}

impl SceneChild for Element {
    fn into_child(self) -> Element {
        self
    }
}

impl<F, E> SceneChild for F
where
    F: Fn() -> E + 'static,
    E: IntoSceneElement,
{
    fn into_child(self) -> Element {
        dyn_element(move || self().into_scene_element())
    }
}

/// Implement `IntoSceneElement` + `SceneChild` for a builder type by
/// delegating to its `.build()`.
macro_rules! impl_builder_child {
    ($($builder:ident),+ $(,)?) => {$(
        impl IntoSceneElement for $builder {
            fn into_scene_element(self) -> Element {
                self.build()
            }
        }
        impl SceneChild for $builder {
            fn into_child(self) -> Element {
                self.build()
            }
        }
    )+};
}

impl_builder_child!(
    ViewBuilder,
    PressableBuilder,
    ScrollViewBuilder,
    TextBuilder,
    ButtonBuilder,
    ImageBuilder,
    IconBuilder,
    LinkBuilder,
    ToggleBuilder,
    SliderBuilder,
    ActivityIndicatorBuilder,
    TextInputBuilder,
    TextAreaBuilder,
);

/// Text-content coercion — the vocabulary's display policy (ports
/// idea-lite's `TextContent`): `&str`/`String` are static; closures of
/// any `ToString` are dynamic; signals/memos of `ToString` values are
/// dynamic through their own `get()`.
pub trait TextContent {
    fn into_content(self) -> Value<String>;
}

impl TextContent for &str {
    fn into_content(self) -> Value<String> {
        Value::Const(self.to_string())
    }
}

impl TextContent for String {
    fn into_content(self) -> Value<String> {
        Value::Const(self)
    }
}

impl TextContent for Value<String> {
    fn into_content(self) -> Value<String> {
        self
    }
}

impl<F, D> TextContent for F
where
    F: Fn() -> D + 'static,
    D: ToString,
{
    fn into_content(self) -> Value<String> {
        Value::Dyn(Box::new(move || self().to_string()))
    }
}

impl<T> TextContent for Signal<T>
where
    T: ToString + PartialEq + Clone + 'static,
{
    fn into_content(self) -> Value<String> {
        Value::Dyn(Box::new(move || self.get().to_string()))
    }
}

impl<T> TextContent for ReadSignal<T>
where
    T: ToString + PartialEq + Clone + 'static,
{
    fn into_content(self) -> Value<String> {
        Value::Dyn(Box::new(move || self.get().to_string()))
    }
}

impl<T> TextContent for Memo<T>
where
    T: ToString + PartialEq + Clone + 'static,
{
    fn into_content(self) -> Value<String> {
        Value::Dyn(Box::new(move || self.get().to_string()))
    }
}

/// Bare displayable scalars are static content (`text { 42 }`). A
/// blanket `T: ToString` impl would collide with the closure/handle
/// impls, so the common scalar set is enumerated — same approach as
/// `runtime_world`'s `IntoValue` Const impls.
macro_rules! impl_scalar_text_content {
    ($($t:ty),+ $(,)?) => {$(
        impl TextContent for $t {
            fn into_content(self) -> Value<String> {
                Value::Const(self.to_string())
            }
        }
    )+};
}

impl_scalar_text_content! {
    bool, char,
    i8, i16, i32, i64, i128, isize,
    u8, u16, u32, u64, u128, usize,
    f32, f64,
}
