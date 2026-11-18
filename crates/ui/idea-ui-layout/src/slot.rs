//! [`Slot`] and [`OptionalSlot`] — an element a layout builds more than
//! once.

use std::rc::Rc;

use runtime_core::{ui, Element};

/// A builder for content a layout mounts on demand and may mount again:
/// a sheet's body on each open, a list's rows each time it stops being
/// empty. An `Element` is built once and moved, so content that comes
/// and goes is passed as the closure that makes it.
///
/// `From<closure>` lets a call site write `content = move || ui! { … }`.
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
