//! Size pins for the hot-path primitive payloads.
//!
//! The create-rows benchmark residual (+23% vs the old core at 10k
//! rows) was pure payload copying: `StyleProp::Sheet` carried a
//! `StyleApplication` INLINE (~2.3 KB — the `overrides: StyleRules`
//! field), which set every prim payload's size to ~2.5 KB, and each
//! builder-chain move / `PrimCell` box / `take()` memcpy'd the whole
//! struct per row. Boxing the two big variants (`Sheet`,
//! `SignalClass`; `TextSourceProp::JsBinding` likewise) dropped the
//! per-row moving parts to ~200-250 B and closed the bench gate.
//!
//! These pins fail if someone re-inlines a large field into the
//! payload structs. They are UPPER bounds with headroom for small
//! additions (a new Option prop is fine; a new inline `StyleRules` or
//! `StyleApplication` field is not — box it instead).

use std::mem::size_of;

use runtime_vocabulary::prims::{PrimCell, TextPrim, TextSourceProp, ViewPrim};
use runtime_vocabulary::StyleProp;

#[test]
fn regression_style_prop_stays_pointer_sized_per_variant() {
    // Was 2288 (inline StyleApplication). Boxed: tag + payload ptr(s).
    assert!(
        size_of::<StyleProp>() <= 64,
        "StyleProp grew to {} bytes — a variant is carrying a large payload \
         inline; box it (see the Sheet variant's doc comment)",
        size_of::<StyleProp>()
    );
}

#[test]
fn regression_text_source_prop_stays_small() {
    // Was 136 (inline JsTextBinding).
    assert!(
        size_of::<TextSourceProp>() <= 48,
        "TextSourceProp grew to {} bytes — box large variants",
        size_of::<TextSourceProp>()
    );
}

#[test]
fn regression_batchable_prim_payloads_stay_small() {
    // Was ~2.5 KB each (via the inline StyleProp). These two are the
    // payloads the repeat fast path constructs PER ROW — their size is
    // a per-row memcpy multiplier on the create benchmark.
    assert!(
        size_of::<ViewPrim>() <= 384,
        "ViewPrim grew to {} bytes — the repeat enqueue loop copies this \
         per row; box large fields",
        size_of::<ViewPrim>()
    );
    assert!(
        size_of::<TextPrim>() <= 384,
        "TextPrim grew to {} bytes — the repeat enqueue loop copies this \
         per row; box large fields",
        size_of::<TextPrim>()
    );
    // PrimCell adds only the RefCell borrow flag.
    assert!(size_of::<PrimCell<ViewPrim>>() <= size_of::<ViewPrim>() + 16);
}
