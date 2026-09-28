//! Prop introspection for the robot component registry — what the
//! inspector's "Props" table reads.
//!
//! `#[component]` (and `#[props]`) emit one [`entry`] per prop. Each entry
//! classifies the prop by its TYPE, never by scanning source:
//!
//! | prop type                                   | [`PropMode`]                 |
//! |---------------------------------------------|------------------------------|
//! | `Reactive<T>`                               | `Static` / `Live` (its arm)  |
//! | `Signal<T>` / `ReadSignal<T>` / `Memo<T>`   | `Signal`                     |
//! | `WriteSignal<T>`                            | `Signal` (no readable value) |
//! | `Rc<dyn Fn(..)>` (0–2 args), `Option` of one | `Handler`                    |
//! | `Vec<Element>`                              | `Children`                   |
//! | any other `T: Debug + Clone`                | `Value`                      |
//! | anything else                               | `Opaque`                     |
//!
//! # How the classification dispatches
//!
//! Autoref specialization: the macro writes
//! `(&&&&&PropProbe(&prop)).__probe()` — FIVE `&`s for the five levels,
//! because a `&self` method on `Self = &&&&PropProbe<_>` has receiver type
//! `&&&&&PropProbe<_>`. Method lookup matches that most specific level
//! first, then peels one `&` per level until a trait whose bounds hold is
//! found. The struct-level probe is two levels, so it takes `&&`. That
//! works because the emission site is CONCRETE (a component's own
//! parameter or struct field). In a generic context every bound is
//! unprovable and lookup would error rather than fall through, so the
//! macro skips generic components entirely (their Props table is empty).
//!
//! # Cost
//!
//! Only robot builds ever call the probes: the macro passes them inside a
//! closure that the non-robot [`__inspect_component`] never invokes, so
//! the optimizer drops them. In a robot build each prop costs one clone of
//! its value (a `Reactive`, `Signal` or `Rc` clone is a pointer copy; a
//! plain `T` is cloned once) and its `Debug` rendering happens only when
//! the bridge is asked for it.
//!
//! [`__inspect_component`]: crate::robot_methods::__inspect_component

use std::fmt::Debug;
use std::rc::Rc;

use runtime_scene::Element;
use runtime_world::{untrack, Memo, ReadSignal, Signal, WriteSignal};

use crate::glue::Reactive;

/// How a prop's value reaches the component.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PropMode {
    /// A `Reactive<T>` holding a fixed value.
    Static,
    /// A `Reactive<T>` holding a closure: the view updates in place.
    Live,
    /// A signal handle the component reads (and, for a unified `Signal`,
    /// may write).
    Signal,
    /// A callback.
    Handler,
    /// Child elements.
    Children,
    /// A plain (non-`Reactive`) value.
    Value,
    /// A value the registry cannot render.
    Opaque,
}

impl PropMode {
    /// The wire spelling (`get_component`'s `mode` field).
    pub fn as_str(self) -> &'static str {
        match self {
            PropMode::Static => "static",
            PropMode::Live => "live",
            PropMode::Signal => "signal",
            PropMode::Handler => "handler",
            PropMode::Children => "children",
            PropMode::Value => "value",
            PropMode::Opaque => "opaque",
        }
    }
}

/// Renders a prop's CURRENT value on demand (untracked).
pub type PropReader = Rc<dyn Fn() -> String>;

/// One prop of a registered component instance.
#[derive(Clone)]
pub struct PropEntry {
    pub name: &'static str,
    /// The declared type, as the macro rendered it.
    pub ty: &'static str,
    pub mode: PropMode,
    /// `None` when the value can't be rendered (no `Debug`, a handler, a
    /// write-only signal).
    pub read: Option<PropReader>,
}

/// Build one entry from a probe result. Macro emission target.
#[doc(hidden)]
pub fn entry(name: &'static str, ty: &'static str, probe: (PropMode, Option<PropReader>)) -> PropEntry {
    PropEntry { name, ty, mode: probe.0, read: probe.1 }
}

/// The autoref-specialization receiver (see the module docs).
#[doc(hidden)]
pub struct PropProbe<'a, T: ?Sized>(pub &'a T);

fn debug_reader<T: Debug + 'static>(value: T) -> PropReader {
    Rc::new(move || format!("{value:?}"))
}

/// Traits the macro brings into scope with a glob import. Each level has
/// the same method name, `__probe`; lookup picks the most specific level
/// whose bounds hold.
#[doc(hidden)]
pub mod probe {
    use super::*;

    /// Level 4 — reactive props whose value renders.
    pub trait ProbeReadable {
        fn __probe(&self) -> (PropMode, Option<PropReader>);
    }

    impl<T: Debug + Clone + 'static> ProbeReadable for &&&&PropProbe<'_, Reactive<T>> {
        fn __probe(&self) -> (PropMode, Option<PropReader>) {
            match self.0 {
                Reactive::Static(v) => (PropMode::Static, Some(debug_reader(v.clone()))),
                Reactive::Dynamic(f) => {
                    let f = f.clone();
                    (PropMode::Live, Some(Rc::new(move || format!("{:?}", untrack(|| f())))))
                }
            }
        }
    }

    impl<T: Debug + Clone + PartialEq + 'static> ProbeReadable for &&&&PropProbe<'_, Signal<T>> {
        fn __probe(&self) -> (PropMode, Option<PropReader>) {
            let sig = *self.0;
            (PropMode::Signal, Some(Rc::new(move || untrack(|| sig.with(|v| format!("{v:?}"))))))
        }
    }

    impl<T: Debug + Clone + PartialEq + 'static> ProbeReadable for &&&&PropProbe<'_, ReadSignal<T>> {
        fn __probe(&self) -> (PropMode, Option<PropReader>) {
            let sig = *self.0;
            (PropMode::Signal, Some(Rc::new(move || untrack(|| sig.with(|v| format!("{v:?}"))))))
        }
    }

    impl<T: Debug + Clone + PartialEq + 'static> ProbeReadable for &&&&PropProbe<'_, Memo<T>> {
        fn __probe(&self) -> (PropMode, Option<PropReader>) {
            let memo = *self.0;
            (PropMode::Signal, Some(Rc::new(move || untrack(|| memo.with(|v| format!("{v:?}"))))))
        }
    }

    /// Level 3 — reactive props whose value does NOT render: the mode is
    /// still known from the type.
    pub trait ProbeReactive {
        fn __probe(&self) -> (PropMode, Option<PropReader>);
    }

    impl<T: 'static> ProbeReactive for &&&PropProbe<'_, Reactive<T>> {
        fn __probe(&self) -> (PropMode, Option<PropReader>) {
            let mode = if self.0.is_static() { PropMode::Static } else { PropMode::Live };
            (mode, None)
        }
    }

    impl<T: 'static> ProbeReactive for &&&PropProbe<'_, Signal<T>> {
        fn __probe(&self) -> (PropMode, Option<PropReader>) {
            (PropMode::Signal, None)
        }
    }

    impl<T: 'static> ProbeReactive for &&&PropProbe<'_, ReadSignal<T>> {
        fn __probe(&self) -> (PropMode, Option<PropReader>) {
            (PropMode::Signal, None)
        }
    }

    impl<T: 'static> ProbeReactive for &&&PropProbe<'_, WriteSignal<T>> {
        fn __probe(&self) -> (PropMode, Option<PropReader>) {
            (PropMode::Signal, None)
        }
    }

    impl<T: 'static> ProbeReactive for &&&PropProbe<'_, Memo<T>> {
        fn __probe(&self) -> (PropMode, Option<PropReader>) {
            (PropMode::Signal, None)
        }
    }

    /// Level 2 — structural shapes: callbacks and children.
    pub trait ProbeShape {
        fn __probe(&self) -> (PropMode, Option<PropReader>);
    }

    macro_rules! handler_shapes {
        ($( ($($arg:ident),*) ),* $(,)?) => {$(
            impl<$($arg: 'static,)*> ProbeShape for &&PropProbe<'_, Rc<dyn Fn($($arg),*)>> {
                fn __probe(&self) -> (PropMode, Option<PropReader>) {
                    (PropMode::Handler, None)
                }
            }
            impl<$($arg: 'static,)*> ProbeShape for &&PropProbe<'_, Option<Rc<dyn Fn($($arg),*)>>> {
                fn __probe(&self) -> (PropMode, Option<PropReader>) {
                    let label = if self.0.is_some() { "Some(<callback>)" } else { "None" };
                    (PropMode::Handler, Some(Rc::new(move || label.to_string())))
                }
            }
        )*};
    }
    handler_shapes!((), (A), (A, B));

    impl ProbeShape for &&PropProbe<'_, Vec<Element>> {
        fn __probe(&self) -> (PropMode, Option<PropReader>) {
            let n = self.0.len();
            let label = if n == 1 { "1 element".to_string() } else { format!("{n} elements") };
            (PropMode::Children, Some(Rc::new(move || label.clone())))
        }
    }

    /// Level 1 — any renderable plain value.
    pub trait ProbeValue {
        fn __probe(&self) -> (PropMode, Option<PropReader>);
    }

    impl<T: Debug + Clone + 'static> ProbeValue for &PropProbe<'_, T> {
        fn __probe(&self) -> (PropMode, Option<PropReader>) {
            (PropMode::Value, Some(debug_reader(self.0.clone())))
        }
    }

    /// Level 0 — everything else.
    pub trait ProbeOpaque {
        fn __probe(&self) -> (PropMode, Option<PropReader>);
    }

    impl<T: ?Sized> ProbeOpaque for PropProbe<'_, T> {
        fn __probe(&self) -> (PropMode, Option<PropReader>) {
            (PropMode::Opaque, None)
        }
    }

    /// Struct-level probe for the explicit-props form (`props: &FooProps`):
    /// a `#[props]` struct implements [`InspectProps`]; any other props
    /// struct falls back to no entries.
    pub trait ProbeStruct {
        fn __props(&self) -> Vec<PropEntry>;
    }

    impl<T: InspectProps + ?Sized> ProbeStruct for &PropsProbe<'_, T> {
        fn __props(&self) -> Vec<PropEntry> {
            self.0.__inspect_props()
        }
    }

    pub trait ProbeStructFallback {
        fn __props(&self) -> Vec<PropEntry>;
    }

    impl<T: ?Sized> ProbeStructFallback for PropsProbe<'_, T> {
        fn __props(&self) -> Vec<PropEntry> {
            Vec::new()
        }
    }
}

/// Implemented by `#[props]` for its struct: one [`PropEntry`] per field.
pub trait InspectProps {
    fn __inspect_props(&self) -> Vec<PropEntry>;
}

impl<T: InspectProps + ?Sized> InspectProps for &T {
    fn __inspect_props(&self) -> Vec<PropEntry> {
        (**self).__inspect_props()
    }
}

/// Struct-level autoref receiver (see [`probe::ProbeStruct`]).
#[doc(hidden)]
pub struct PropsProbe<'a, T: ?Sized>(pub &'a T);

#[cfg(test)]
mod tests {
    use super::probe::*;
    use super::*;
    use runtime_world::{signal, World};

    fn render(e: &PropEntry) -> Option<String> {
        e.read.as_ref().map(|r| r())
    }

    /// The whole table from the module docs, one row each — the dispatch
    /// is the contract, so every row is pinned.
    #[test]
    fn probes_classify_by_type() {
        let world = World::new();
        world.enter(|| {
            let fixed: Reactive<String> = Reactive::Static("hi".into());
            let s = signal(3i32);
            let live: Reactive<i32> = Reactive::derive(move || s.get() * 10);
            let sig = signal(String::from("a"));
            let read = sig.read_only();
            let cb: Rc<dyn Fn()> = Rc::new(|| {});
            let cb1: Option<Rc<dyn Fn(i32)>> = None;
            let kids: Vec<Element> = vec![runtime_scene::fragment(vec![])];
            let plain = 7u8;
            struct NoDebug;
            let opaque = NoDebug;
            let fixed_nd: Reactive<NoDebugClone> = Reactive::Static(NoDebugClone);
            #[derive(Clone)]
            struct NoDebugClone;

            let e = entry("a", "Reactive<String>", (&&&&&PropProbe(&fixed)).__probe());
            assert_eq!((e.mode, render(&e)), (PropMode::Static, Some("\"hi\"".into())));

            let e = entry("b", "Reactive<i32>", (&&&&&PropProbe(&live)).__probe());
            assert_eq!((e.mode, render(&e)), (PropMode::Live, Some("30".into())));
            s.set(4);
            world.flush();
            assert_eq!(render(&e), Some("40".into()), "a Live prop reads its current value");

            let e = entry("c", "Signal<String>", (&&&&&PropProbe(&sig)).__probe());
            assert_eq!((e.mode, render(&e)), (PropMode::Signal, Some("\"a\"".into())));

            let e = entry("d", "ReadSignal<String>", (&&&&&PropProbe(&read)).__probe());
            assert_eq!(e.mode, PropMode::Signal);

            let e = entry("e", "Rc<dyn Fn()>", (&&&&&PropProbe(&cb)).__probe());
            assert_eq!((e.mode, render(&e)), (PropMode::Handler, None));

            let e = entry("f", "Option<Rc<dyn Fn(i32)>>", (&&&&&PropProbe(&cb1)).__probe());
            assert_eq!((e.mode, render(&e)), (PropMode::Handler, Some("None".into())));

            let e = entry("g", "Vec<Element>", (&&&&&PropProbe(&kids)).__probe());
            assert_eq!((e.mode, render(&e)), (PropMode::Children, Some("1 element".into())));

            let e = entry("h", "u8", (&&&&&PropProbe(&plain)).__probe());
            assert_eq!((e.mode, render(&e)), (PropMode::Value, Some("7".into())));

            let e = entry("i", "NoDebug", (&&&&&PropProbe(&opaque)).__probe());
            assert_eq!((e.mode, render(&e)), (PropMode::Opaque, None));

            let e = entry("j", "Reactive<NoDebugClone>", (&&&&&PropProbe(&fixed_nd)).__probe());
            assert_eq!(
                (e.mode, render(&e)),
                (PropMode::Static, None),
                "a Reactive whose value can't render still reports its arm"
            );
        });
    }

    #[test]
    fn struct_probe_falls_back_to_no_entries() {
        struct Plain;
        struct Inspected;
        impl InspectProps for Inspected {
            fn __inspect_props(&self) -> Vec<PropEntry> {
                vec![entry("x", "u8", (PropMode::Value, None))]
            }
        }
        assert!((&&PropsProbe(&Plain)).__props().is_empty());
        assert_eq!((&&PropsProbe(&Inspected)).__props().len(), 1);
        // Through a reference, as the explicit form's `props: &FooProps`.
        let by_ref = &Inspected;
        assert_eq!((&&PropsProbe(&by_ref)).__props().len(), 1);
    }
}
