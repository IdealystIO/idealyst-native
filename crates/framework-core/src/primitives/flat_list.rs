//! `flat_list<T>` — typed wrapper around `Virtualizer`.
//!
//! Author-facing API. Captures their `Signal<Vec<T>>` + closures and
//! produces a `Primitive::Virtualizer` whose callbacks read the
//! current `Vec<T>` snapshot at call time. Reactive: if `data`
//! changes (insertions, deletions, reorders), the framework's
//! backend re-runs its diff and updates the mounted set.
//!
//! Stable identity via the required `key` closure: the framework
//! uses the returned `u64` to decide which mounted items to preserve
//! across data updates.

use crate::primitives::virtualizer::{
    virtualizer, ItemKey, ItemSize, VirtualizerHandle,
};
use crate::{Bound, Primitive, Signal};
use std::rc::Rc;

/// Typed size strategy. `Known` is fastest; use it whenever you can
/// compute size from data alone. `Measured` is for cases where the
/// rendered size depends on layout/content the framework can't see
/// (e.g. wrapped text in a flex container of unknown width).
pub enum FlatListItemSize<T> {
    Known(Rc<dyn Fn(usize, &T) -> f32>),
    Measured(Rc<dyn Fn(usize, &T) -> f32>),
}

/// Construct a `FlatList`.
///
/// - `data`: source-of-truth reactive list. The framework reads its
///   current snapshot whenever the virtualizer queries item count,
///   keys, or sizes.
/// - `key`: stable identity per item. Two items returning the same
///   key are treated as the same logical row across data updates;
///   their mounted subtree is preserved instead of torn down.
/// - `item_size`: known or measured sizing strategy.
/// - `render_item`: builds the subtree for one item. Re-run only
///   when an item enters the mount window (or when its data
///   identity changes via a key collision — rare).
pub fn flat_list<T, K, S, R>(
    data: Signal<Vec<T>>,
    key: K,
    item_size: FlatListItemSize<T>,
    render_item: R,
) -> Bound<VirtualizerHandle>
where
    T: Clone + 'static,
    K: Fn(usize, &T) -> ItemKey + 'static,
    S: 'static,
    R: Fn(usize, &T) -> Primitive + 'static,
    FlatListItemSize<T>: 'static,
{
    let _ = std::marker::PhantomData::<S>;

    // Wrap the typed closures with the type-erased ones the
    // Virtualizer primitive accepts. All four closures need to
    // share the data signal — that's the source of truth they read
    // from on each invocation.
    let key = Rc::new(key);
    let render_item = Rc::new(render_item);

    // item_count: read the current data length.
    let item_count: Box<dyn Fn() -> usize> = {
        let data = data;
        Box::new(move || data.get().len())
    };

    // item_key: read data[idx] and apply user's key fn.
    let item_key: Box<dyn Fn(usize) -> ItemKey> = {
        let data = data;
        let key = key.clone();
        Box::new(move |idx| {
            let v = data.get();
            if let Some(item) = v.get(idx) {
                key(idx, item)
            } else {
                // Out-of-range key — synthesize a sentinel so we
                // don't collide with valid keys. Indicates a race
                // between data change and a backend's stale index.
                u64::MAX - idx as u64
            }
        })
    };

    // item_size: dispatch to the typed Known/Measured variant.
    let item_size: ItemSize = match item_size {
        FlatListItemSize::Known(f) => {
            let data = data;
            ItemSize::Known(Rc::new(move |idx| {
                let v = data.get();
                v.get(idx).map(|item| f(idx, item)).unwrap_or(0.0)
            }))
        }
        FlatListItemSize::Measured(f) => {
            let data = data;
            ItemSize::Measured(Rc::new(move |idx| {
                let v = data.get();
                v.get(idx).map(|item| f(idx, item)).unwrap_or(0.0)
            }))
        }
    };

    // render_item: read data[idx], build the user's primitive.
    let render_item_erased: Rc<dyn Fn(usize) -> Primitive> = {
        let data = data;
        let render_item = render_item.clone();
        Rc::new(move |idx| {
            let v = data.get();
            match v.get(idx) {
                Some(item) => render_item(idx, item),
                // Stale index: produce an empty view. Backend's
                // bounds-checking should prevent this from being
                // visually noticeable.
                None => crate::view(Vec::new()).primitive,
            }
        })
    };

    virtualizer(item_count, item_key, item_size, render_item_erased)
}

/// Convenience helper for the common case where every item has the
/// same fixed height.
pub fn fixed_size<T: 'static>(size: f32) -> FlatListItemSize<T> {
    FlatListItemSize::Known(Rc::new(move |_, _| size))
}
