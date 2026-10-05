//! What a value looks like on the wire, as text: a type's SHAPE.
//!
//! The release build lists, for every app component a bundle uses, the
//! shape of each prop it sets; the app lists the shapes its own components
//! take. A bundle runs on an app only where they agree (`remote-bundle`'s
//! `check`), because values cross positionally: a `#[derive(Remote)]`
//! struct that gained a field, or a prop that went from `u32` to `f64`,
//! would otherwise be misread rather than refused. Both sides compute a
//! shape with this same code, so equal types give equal strings.
//!
//! The grammar (no spaces):
//!
//! | Rust | Shape |
//! |---|---|
//! | numbers, `bool`, `char` | their name (`u32`, `f64`) |
//! | `String`, `&str` | `str` |
//! | `()` | `()` |
//! | `Option<T>` / `Vec<T>` / `[T; N]` | `opt<T>` / `list<T>` / `[T;N]` |
//! | `Result<T, E>` / tuples | `result<T,E>` / `(A,B)` |
//! | `Reactive<T>` / `Signal<T>` / `ReadSignal<T>` | `reactive<T>` / `signal<T>` / `read<T>` |
//! | callbacks | `fn(A,B)->R`, `fn(A)` |
//! | `#[derive(Remote)]` | `Name{a:T,b:U}`, `Name(T)`, `Name`, enums `Name[A,B(T),C{x:T}]` |
//! | a value crossing by key (`ToneRef`) | `key<ToneRef>` |
//! | the framework's own value types | their name (`Color`, `Element`) |
//! | anything else | `?` — not checked |
//!
//! A derived type that contains itself is written in full once; the inner
//! occurrence is just its name. The framework's value types (`Color`,
//! `StyleRules`) are names, not structure: they change with the framework,
//! and the codec version (`CODEC_VERSION`) covers that.

use super::Arg;

/// The text for "not known": matches any shape.
pub const UNKNOWN: &str = "?";

/// A type with a known shape.
pub trait RemoteShape {
    fn shape(s: &mut Shaper);
}

/// Builds one shape.
#[derive(Default)]
pub struct Shaper {
    out: String,
    /// The derived types being written, innermost last (recursion).
    open: Vec<&'static str>,
}

impl Shaper {
    pub fn push(&mut self, text: &str) {
        self.out.push_str(text);
    }

    pub fn of<T: RemoteShape + ?Sized>(&mut self) {
        T::shape(self)
    }

    /// `head<T>`.
    pub fn wrap<T: RemoteShape + ?Sized>(&mut self, head: &str) {
        self.push(head);
        self.push("<");
        T::shape(self);
        self.push(">");
    }

    /// A named type whose `body` follows its name — unless it is already
    /// being written further out, when it is just its name (a type that
    /// contains itself).
    pub fn named(&mut self, name: &'static str, body: impl FnOnce(&mut Self)) {
        self.push(name);
        if self.open.contains(&name) {
            return;
        }
        self.open.push(name);
        body(self);
        self.open.pop();
    }

    pub fn finish(self) -> String {
        self.out
    }
}

/// `T`'s shape.
pub fn shape_of<T: RemoteShape + ?Sized>() -> String {
    let mut s = Shaper::default();
    T::shape(&mut s);
    s.finish()
}

/// [`Arg`]'s probe for a shape: [`ViaShape`] when the type has one, else
/// [`ViaNoShape`] (`?`). How a macro writes the shape of a field whose
/// type it only knows by name.
#[doc(hidden)]
pub trait ViaShape<T> {
    fn shape_into(&self, s: &mut Shaper);
}

impl<T: RemoteShape> ViaShape<T> for Arg<T> {
    fn shape_into(&self, s: &mut Shaper) {
        T::shape(s)
    }
}

#[doc(hidden)]
pub trait ViaNoShape<T> {
    fn shape_into(&self, s: &mut Shaper);
}

impl<T> ViaNoShape<T> for &Arg<T> {
    fn shape_into(&self, s: &mut Shaper) {
        s.push(UNKNOWN)
    }
}

/// `T`'s shape through the probe, as a string: a macro's field types.
#[doc(hidden)]
#[macro_export]
macro_rules! __shape_of {
    ($t:ty) => {{
        #[allow(unused_imports)]
        use $crate::remote::shape::{ViaNoShape as _, ViaShape as _};
        let mut __s = $crate::remote::shape::Shaper::default();
        (&$crate::remote::Arg::<$t>::new()).shape_into(&mut __s);
        __s.finish()
    }};
}

/// `name(T, U)` field lists and `Name{a:T}` bodies, for the derive.
#[doc(hidden)]
#[macro_export]
macro_rules! __shape_field {
    ($s:expr, $t:ty) => {{
        #[allow(unused_imports)]
        use $crate::remote::shape::{ViaNoShape as _, ViaShape as _};
        (&$crate::remote::Arg::<$t>::new()).shape_into($s);
    }};
}

macro_rules! named_shape {
    ($($t:ty => $name:expr),* $(,)?) => {$(
        impl RemoteShape for $t {
            fn shape(s: &mut Shaper) {
                s.push($name)
            }
        }
    )*};
}

named_shape!(
    u8 => "u8", i8 => "i8", u16 => "u16", i16 => "i16", u32 => "u32", i32 => "i32",
    u64 => "u64", i64 => "i64", u128 => "u128", i128 => "i128", usize => "usize", isize => "isize",
    f32 => "f32", f64 => "f64", bool => "bool", char => "char",
    String => "str", str => "str", () => "()",
    runtime_scene::Element => "Element",
    runtime_shared::StyleRules => "StyleRules",
    std::rc::Rc<runtime_shared::StyleSheet> => "StyleSheet",
    runtime_shared::primitives::icon::IconData => "IconData",
    runtime_shared::primitives::portal::AnchorTarget => "AnchorTarget",
    crate::prims::NavHandle => "NavHandle",
    crate::host_types::KeyBytes => "KeyBytes",
    crate::host_types::OpaqueBytes => "OpaqueBytes",
    runtime_shared::style::Color => "Color",
    runtime_shared::style::Length => "Length",
    runtime_shared::style::FlexDirection => "FlexDirection",
    runtime_shared::style::FlexWrap => "FlexWrap",
    runtime_shared::style::JustifyContent => "JustifyContent",
    runtime_shared::style::AlignItems => "AlignItems",
    runtime_shared::style::AlignSelf => "AlignSelf",
    runtime_shared::style::Position => "Position",
    runtime_shared::style::FontWeight => "FontWeight",
    runtime_shared::style::FontStyle => "FontStyle",
    runtime_shared::style::TextAlign => "TextAlign",
    runtime_shared::style::TextTransform => "TextTransform",
    runtime_shared::style::Overflow => "Overflow",
    runtime_shared::style::ObjectFit => "ObjectFit",
    runtime_shared::style::Cursor => "Cursor",
    runtime_shared::style::Easing => "Easing",
    runtime_shared::accessibility::Role => "Role",
    runtime_shared::primitives::portal::ElementSide => "ElementSide",
    runtime_shared::primitives::portal::ElementAlign => "ElementAlign",
    runtime_shared::primitives::portal::ViewportRect => "ViewportRect",
    runtime_shared::primitives::key::KeyEvent => "KeyEvent",
    runtime_shared::primitives::key::KeyOutcome => "KeyOutcome",
    runtime_shared::primitives::image::ImageLoadEvent => "ImageLoadEvent",
    runtime_shared::touch::TouchPoint => "TouchPoint",
    runtime_shared::host::ColorScheme => "ColorScheme",
);

/// A host function's `&T` argument crosses as `T`.
impl<T: RemoteShape + ?Sized> RemoteShape for &T {
    fn shape(s: &mut Shaper) {
        T::shape(s)
    }
}

impl<T: RemoteShape> RemoteShape for Option<T> {
    fn shape(s: &mut Shaper) {
        s.wrap::<T>("opt")
    }
}

impl<T: RemoteShape> RemoteShape for Vec<T> {
    fn shape(s: &mut Shaper) {
        s.wrap::<T>("list")
    }
}

impl<T: RemoteShape, const N: usize> RemoteShape for [T; N] {
    fn shape(s: &mut Shaper) {
        s.push("[");
        T::shape(s);
        s.push(&format!(";{N}]"));
    }
}

impl<T: RemoteShape, E: RemoteShape> RemoteShape for Result<T, E> {
    fn shape(s: &mut Shaper) {
        s.push("result<");
        T::shape(s);
        s.push(",");
        E::shape(s);
        s.push(">");
    }
}

macro_rules! tuple_shape {
    ($first:ident $(, $n:ident)*) => {
        impl<$first: RemoteShape $(, $n: RemoteShape)*> RemoteShape for ($first, $($n,)*) {
            fn shape(s: &mut Shaper) {
                s.push("(");
                $first::shape(s);
                $( s.push(","); $n::shape(s); )*
                s.push(")");
            }
        }
    };
}
tuple_shape!(A);
tuple_shape!(A, B);
tuple_shape!(A, B, C);
tuple_shape!(A, B, C, D);

impl<T: RemoteShape> RemoteShape for crate::glue::Reactive<T> {
    fn shape(s: &mut Shaper) {
        s.wrap::<T>("reactive")
    }
}

impl<T: RemoteShape> RemoteShape for runtime_world::Signal<T> {
    fn shape(s: &mut Shaper) {
        s.wrap::<T>("signal")
    }
}

impl<T: RemoteShape> RemoteShape for runtime_world::ReadSignal<T> {
    fn shape(s: &mut Shaper) {
        s.wrap::<T>("read")
    }
}

impl<H: crate::prims::NavHandleType> RemoteShape for runtime_shared::Ref<H> {
    fn shape(s: &mut Shaper) {
        s.push("ref<");
        s.push(std::any::type_name::<H>().rsplit("::").next().unwrap_or("?"));
        s.push(">");
    }
}

impl RemoteShape for std::rc::Rc<dyn Fn()> {
    fn shape(s: &mut Shaper) {
        s.push("fn()")
    }
}

impl RemoteShape for std::rc::Rc<dyn Fn() -> runtime_scene::Element> {
    fn shape(s: &mut Shaper) {
        s.push("fn()->Element")
    }
}

impl<A: RemoteShape, B: RemoteShape> RemoteShape for std::rc::Rc<dyn Fn(A, B)> {
    fn shape(s: &mut Shaper) {
        s.push("fn(");
        A::shape(s);
        s.push(",");
        B::shape(s);
        s.push(")");
    }
}

impl<A: RemoteShape, R: RemoteShape> RemoteShape for std::rc::Rc<dyn Fn(A) -> R> {
    fn shape(s: &mut Shaper) {
        s.push("fn(");
        A::shape(s);
        s.push(")->");
        R::shape(s);
    }
}

// `Fn(&KeyEvent) -> KeyOutcome` is the one above: `&T` crosses as `T`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containers_spell_their_parts() {
        assert_eq!(shape_of::<Vec<Option<(u32, String)>>>(), "list<opt<(u32,str)>>");
        assert_eq!(shape_of::<[f64; 3]>(), "[f64;3]");
        assert_eq!(shape_of::<Result<String, i64>>(), "result<str,i64>");
        assert_eq!(shape_of::<crate::glue::Reactive<String>>(), "reactive<str>");
        assert_eq!(shape_of::<std::rc::Rc<dyn Fn(u32) -> bool>>(), "fn(u32)->bool");
    }

    struct NoShape;

    #[test]
    fn a_type_without_a_shape_is_unknown_through_the_probe() {
        assert_eq!(crate::__shape_of!(NoShape), UNKNOWN);
        assert_eq!(crate::__shape_of!(Vec<NoShape>), UNKNOWN, "a container of an unknown is unknown");
        assert_eq!(crate::__shape_of!(Vec<u8>), "list<u8>");
    }

    /// A type containing itself would recurse forever: the inner one is
    /// its name.
    #[test]
    fn a_type_that_contains_itself_is_named_inside() {
        struct Tree;
        impl RemoteShape for Tree {
            fn shape(s: &mut Shaper) {
                s.named("Tree", |s| {
                    s.push("{kids:");
                    s.wrap::<Tree>("list");
                    s.push("}");
                })
            }
        }
        assert_eq!(shape_of::<Tree>(), "Tree{kids:list<Tree>}");
    }
}
