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

#[cfg(test)]
mod tests {
    //! `fixed_size` / `FlatListItemSize` coverage.
    //!
    //! RELOCATED from `runtime-core/src/primitives/flat_list.rs`
    //! (deletion baseline §4.2, SV-R). These only ever exercised the
    //! free-function helpers that compose the constructor's arguments —
    //! the `Bound<VirtualizerHandle>` constructor they were filed next to
    //! is the part that dies. Same three assertions, same code.
    use super::*;

    #[test]
    fn fixed_size_returns_known_variant_with_constant_value() {
        let sz: FlatListItemSize<u32> = fixed_size(42.0);
        match sz {
            FlatListItemSize::Known(f) => {
                // The closure must return the same value regardless
                // of (idx, item).
                assert_eq!(f(0, &7u32), 42.0);
                assert_eq!(f(1000, &0u32), 42.0);
                assert_eq!(f(usize::MAX, &u32::MAX), 42.0);
            }
            FlatListItemSize::Measured(_) => {
                panic!("fixed_size should always return the Known variant");
            }
        }
    }

    #[test]
    fn fixed_size_is_consistent_across_clones_of_the_closure() {
        // The Known variant carries an Rc; cloning it must NOT
        // re-evaluate `size` (no captured mutable state).
        let sz: FlatListItemSize<u32> = fixed_size(7.5);
        match sz {
            FlatListItemSize::Known(f) => {
                let f2 = f.clone();
                assert_eq!(f(0, &0), 7.5);
                assert_eq!(f2(0, &0), 7.5);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn fixed_size_works_with_non_copy_item_type() {
        // The `T` parameter goes unused inside `fixed_size`, but the
        // trait bound is `T: 'static`. Verify it accepts a non-Copy
        // type — the closure body discards its argument.
        let sz: FlatListItemSize<String> = fixed_size(99.0);
        match sz {
            FlatListItemSize::Known(f) => {
                assert_eq!(f(0, &"row".to_string()), 99.0);
            }
            _ => unreachable!(),
        }
    }
}
