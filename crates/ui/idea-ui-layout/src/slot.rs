//! [`Slot`] and [`OptionalSlot`] — an element a layout builds more than
//! once.

use std::rc::Rc;

use runtime_core::{ui, Element};

/// A builder for content a layout mounts on demand and may mount again:
/// a sheet's body on each open, a list's rows each time it stops being
/// empty. An `Element` is built once and moved, so content that comes
/// and goes is passed as the closure that makes it.
///
/// `From<closure>` lets a call site write `content = move || ui! { … }`;
/// a builder already held as an `Rc<dyn Fn() -> Element>` (shared, or
/// kept in props) passes as it is.
#[derive(Clone)]
pub struct Slot(pub Rc<dyn Fn() -> Element>);

impl Slot {
    /// Build the content.
    pub fn build(&self) -> Element {
        (self.0)()
    }
}

impl<F: Fn() -> Element + 'static> From<F> for Slot {
    fn from(f: F) -> Self {
        Slot(Rc::new(f))
    }
}

impl From<Rc<dyn Fn() -> Element>> for Slot {
    fn from(f: Rc<dyn Fn() -> Element>) -> Self {
        Slot(f)
    }
}

impl Default for Slot {
    fn default() -> Self {
        Slot(Rc::new(|| ui! { view {} }))
    }
}

/// A [`Slot`] a layout may go without: unset draws nothing there.
#[derive(Clone, Default)]
pub struct OptionalSlot(pub Option<Rc<dyn Fn() -> Element>>);

impl OptionalSlot {
    /// Build the content, if there is any.
    pub fn build(&self) -> Option<Element> {
        self.0.as_ref().map(|f| f())
    }

    /// Whether the slot holds a builder.
    pub fn is_set(&self) -> bool {
        self.0.is_some()
    }
}

impl<F: Fn() -> Element + 'static> From<F> for OptionalSlot {
    fn from(f: F) -> Self {
        OptionalSlot(Some(Rc::new(f)))
    }
}

impl From<Rc<dyn Fn() -> Element>> for OptionalSlot {
    fn from(f: Rc<dyn Fn() -> Element>) -> Self {
        OptionalSlot(Some(f))
    }
}

impl From<Option<Rc<dyn Fn() -> Element>>> for OptionalSlot {
    fn from(f: Option<Rc<dyn Fn() -> Element>>) -> Self {
        OptionalSlot(f)
    }
}

impl From<Slot> for OptionalSlot {
    fn from(slot: Slot) -> Self {
        OptionalSlot(Some(slot.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A builder a caller already holds as an `Rc` is a slot without a
    /// wrapping closure: `Rc<dyn Fn()>` is not itself `Fn`, so the
    /// closure impl alone refused it.
    #[test]
    fn a_shared_builder_is_a_slot() {
        let body: Rc<dyn Fn() -> Element> = Rc::new(|| ui! { view {} });
        let slot: Slot = body.clone().into();
        assert!(Rc::ptr_eq(&slot.0, &body), "the same builder, not a wrapper round it");
        let set: OptionalSlot = body.clone().into();
        assert!(set.is_set());
        let some: OptionalSlot = Some(body).into();
        assert!(some.is_set());
        let none: OptionalSlot = None::<Rc<dyn Fn() -> Element>>.into();
        assert!(!none.is_set());
        let closure: Slot = (|| ui! { view {} }).into();
        let _: OptionalSlot = closure.into();
    }
}
