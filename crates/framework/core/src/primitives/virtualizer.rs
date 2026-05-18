//! Virtualizer — type-erased windowed/recycled list primitive.
//!
//! Authors don't call this directly. They use the generic
//! `flat_list<T>(...)` wrapper in `primitives::flat_list`, which
//! captures their typed data + closures and feeds the type-erased
//! callbacks Virtualizer needs.
//!
//! # Per-backend strategy
//!
//! - **Web**: a JS-side scroll handler (`backend-web/runtime/ts/virtualizer.ts`)
//!   owns the scroll listener + visible-range diff. It calls back
//!   into Rust only when items enter/leave the window. Per-item
//!   scopes are dropped on exit, so signals/effects for unmounted
//!   items are freed.
//! - **iOS**: `UICollectionView` with a flow layout that consults
//!   our `item_height` callback. Real cell recycling: `prepareForReuse`
//!   releases the item subtree, `cellForItemAt` builds the next one.
//! - **Android**: `RecyclerView` with a `ListAdapter` + `DiffUtil`.
//!   `onBindViewHolder` calls Rust to build the subtree;
//!   `onViewRecycled` releases it.
//!
//! All three backends see the same Rust contract.
//!
//! # Stable identity
//!
//! Every item carries a stable `u64` key (typically a hash of its
//! database id or content). When the data changes, the framework
//! diffs old keys against new keys to decide what to preserve.
//! Items whose key still exists keep their mounted subtree intact
//! — they may move in the layout, but their internal signals,
//! refs, and mounted state survive.
//!
//! # Size resolution
//!
//! Two modes per `ItemSize`:
//! - `Known`: author provides exact size per item before mount.
//!   Layout is deterministic. Cheapest.
//! - `Measured`: author provides an *estimate* per item; backend
//!   measures the actual rendered size on mount and stores it.
//!   Subsequent layout uses the measured value. If the item's
//!   rendered size changes later (its content updated), the
//!   backend's layout-observation primitive (ResizeObserver on web,
//!   `layoutSubviews` on iOS, `OnLayoutChangeListener` on Android)
//!   re-fires and refreshes the stored size.

use crate::{Bound, Primitive, Ref, RefFill};
use std::any::Any;
use std::rc::Rc;

/// Stable identity for an item. The user-facing API takes a closure
/// `Fn(usize, &T) -> u64`; the framework keeps `MountedItem`s keyed
/// by this u64 across data updates. Two distinct items with the
/// same key are a user bug — the framework treats them as the same
/// identity and will silently drop one.
pub type ItemKey = u64;

/// Size-knowledge strategy. `flat_list<T>` accepts either variant
/// at the typed layer; this is the type-erased form Virtualizer
/// sees.
pub enum ItemSize {
    /// Author tells us the exact size. Backend never measures.
    Known(Rc<dyn Fn(usize) -> f32>),
    /// Author provides an estimate; backend measures on mount and
    /// updates. Use this when items have data-driven content
    /// whose size you can't predict from data alone (e.g. wrapped
    /// text where the wrap width depends on the container).
    Measured(Rc<dyn Fn(usize) -> f32>),
}

impl ItemSize {
    /// Get the size for an index — either the author's known value
    /// or their estimate. Backends call this for the initial layout
    /// before any measurement.
    pub fn initial(&self, idx: usize) -> f32 {
        match self {
            ItemSize::Known(f) | ItemSize::Measured(f) => f(idx),
        }
    }

    /// True if this is `Measured` — backends use this to decide
    /// whether to install a layout observer on each mounted item.
    pub fn is_measured(&self) -> bool {
        matches!(self, ItemSize::Measured(_))
    }
}

/// Handle for `Ref<VirtualizerHandle>`. Future methods: scroll to
/// index, scroll to top, get visible range, etc.
#[derive(Clone)]
pub struct VirtualizerHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn VirtualizerOps,
}

impl VirtualizerHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn VirtualizerOps) -> Self {
        Self { node, ops }
    }

    /// Scroll the list so the item at `index` is in view.
    pub fn scroll_to_index(&self, index: usize) {
        self.ops.scroll_to_index(&*self.node, index);
    }
}

pub trait VirtualizerOps {
    fn scroll_to_index(&self, node: &dyn Any, index: usize);
}

/// Construct a Virtualizer. Authors typically don't call this
/// directly — `flat_list<T>(...)` is the typed wrapper.
///
/// All callbacks are type-erased: the wrapper holds the actual
/// `T`-typed closures and bridges through these `usize`-only
/// callbacks. The framework's build path handles per-item scope
/// management — `render_item(idx)` runs inside a fresh `Scope`,
/// and that scope is dropped when the item is released.
pub fn virtualizer(
    item_count: Box<dyn Fn() -> usize>,
    item_key: Box<dyn Fn(usize) -> ItemKey>,
    item_size: ItemSize,
    render_item: Rc<dyn Fn(usize) -> Primitive>,
) -> Bound<VirtualizerHandle> {
    // Closure-driven entry point: produce a `Derived<usize>` with
    // empty metadata (`is_opaque() == true`) so runtime backends
    // pick up the closure but generator backends report a clear
    // build-time error.
    let item_count = crate::derive::IntoDerived::<usize>::into_derived(item_count);
    Bound::new(Primitive::Virtualizer {
        item_count,
        item_key,
        item_size,
        render_item,
        row_template: None,
        row_index_signal_id: None,
        overscan: 1.0,
        horizontal: false,
        style: None,
        ref_fill: None,
    })
}

impl Bound<VirtualizerHandle> {
    /// Buffer factor outside the visible window. Default `1.0`
    /// (one viewport extent above and below). Higher = smoother
    /// fast-scroll, more memory.
    pub fn overscan(mut self, factor: f32) -> Self {
        if let Primitive::Virtualizer { overscan, .. } = &mut self.primitive {
            *overscan = factor;
        }
        self
    }

    /// Horizontal scrolling instead of the default vertical.
    pub fn horizontal(mut self, h: bool) -> Self {
        if let Primitive::Virtualizer { horizontal, .. } = &mut self.primitive {
            *horizontal = h;
        }
        self
    }

    pub fn bind(mut self, r: Ref<VirtualizerHandle>) -> Self {
        if let Primitive::Virtualizer { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Virtualizer(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
