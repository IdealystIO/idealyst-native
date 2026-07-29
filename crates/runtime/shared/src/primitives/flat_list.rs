//! `flat_list<T>` — typed wrapper around `Virtualizer`.
//!
//! Author-facing API. Captures their `Signal<Vec<T>>` + closures and
//! produces a `Element::Virtualizer` whose callbacks read the
//! current `Vec<T>` snapshot at call time. Reactive: if `data`
//! changes (insertions, deletions, reorders), the framework's
//! backend re-runs its diff and updates the mounted set.
//!
//! Stable identity via the required `key` closure: the framework
//! uses the returned `u64` to decide which mounted items to preserve
//! across data updates.

use std::rc::Rc;

/// Typed size strategy. `Known` is fastest; use it whenever you can
/// compute size from data alone. `Measured` is for cases where the
/// rendered size depends on layout/content the framework can't see
/// (e.g. wrapped text in a flex container of unknown width).
pub enum FlatListItemSize<T> {
    Known(Rc<dyn Fn(usize, &T) -> f32>),
    Measured(Rc<dyn Fn(usize, &T) -> f32>),
}


/// Convenience helper for the common case where every item has the
/// same fixed height.
pub fn fixed_size<T: 'static>(size: f32) -> FlatListItemSize<T> {
    FlatListItemSize::Known(Rc::new(move |_, _| size))
}

