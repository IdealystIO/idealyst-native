//! `z_index` (and `position: Sticky`'s implicit raise) → `View.setTranslationZ`
//! mapping, shared by the static style apply and the animated
//! `AnimProp::ZIndex` path, plus the skip-if-unchanged decision.
//!
//! Un-gated (compiles on any host) so the tests run from any platform,
//! same pattern as `text_truncation_policy` / `sticky_compute`. The JNI
//! half lives in `imp/style.rs` (`apply_translation_z`) and
//! `imp/mod.rs` (`set_animated_f32_impl`).
//!
//! ## Why translationZ orders siblings
//!
//! `ViewGroup` orders BOTH drawing and touch dispatch by `View.getZ()`
//! (`elevation + translationZ`): `buildOrderedChildList` is a stable
//! insertion sort by Z (strict `>` comparison, so ties keep child-index
//! order), used by software `dispatchDraw` and, reversed, by
//! `buildTouchDispatchChildList`. Hardware-accelerated drawing hands
//! the same ordering to HWUI through `insertReorderBarrier`, which
//! stable-sorts the barrier's children by Z — but treats any
//! `|Z| <= 0.001` (`MathUtils::NON_ZERO_EPSILON`) as in-plane, drawn
//! in child order. That is the paint-order contract of
//! `StyleRules::z_index` (ascending z, ties in tree order, hit-testing
//! follows paint), so no framework container overrides
//! `getChildDrawingOrder` or routes touches itself:
//! `RustVirtualGrid` explicitly disables custom drawing order and its
//! `onInterceptTouchEvent` only claims drags.
//!
//! ## Virtualizer rows
//!
//! A child's Z orders it among its OWN parent's children only. The
//! virtualizer (`RecyclerView` + `RustListAdapter`) mounts each item
//! root inside a per-holder `RustRowContainer`, and those containers are
//! the siblings RecyclerView draws and dispatches touches over — so the
//! item root's Z alone would order it inside a one-child container and
//! leave the rows in adapter order, where web (item roots are plain
//! siblings) orders them by `z_index`. Every stacking Z write therefore
//! goes through the static `RustRowContainer.setStackingZ`
//! ([`SET_STACKING_Z`]), which raises the row container to the item
//! root's Z when the view's parent is one; bind copies the freshly
//! realized root's Z onto the container and release resets it to `0`,
//! so a recycled holder never keeps a previous row's raise. The
//! container does not clip its child, so a row drawn past its slot
//! overlaps its neighbours as on web.
//!
//! ## Why the step is tiny
//!
//! Z also drives the material elevation shadow a view with an outline
//! (any background drawable) casts. One z unit maps to [`Z_STEP_PX`]
//! device px: above HWUI's 0.001 px in-plane epsilon, so `z_index: 1`
//! still reorders, and small enough that ordinary values (`1`..`100`
//! → at most 1 px) cast no perceptible shadow. API 28+ additionally
//! clears the view's outline shadow colors whenever it is raised
//! (`imp/style.rs`), so even large values (`9999`) draw no shadow
//! there; on API 21–27 a very large `z_index` can cast a faint one.
//! The mapping is linear, so strict ordering survives for every
//! integer z an `f32` represents exactly (`|z| <= 2^24`).
//!
//! Static and animated writes use the SAME mapping, so a static
//! `z_index: 2` and an animated `ZIndex` of `2.0` order identically
//! against siblings — Android has no dp unit for Z ordering; a
//! density-scaled Z would make `2.0` animated outrank `2` static on
//! every display denser than 1×.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use runtime_shared::StyleRules;

/// Device px of `translationZ` per unit of z. Larger than HWUI's
/// in-plane epsilon (0.001 px) so a z of `1` reorders; small enough
/// that its elevation shadow stays imperceptible.
pub(crate) const Z_STEP_PX: f32 = 0.01;

/// JNI name of the Kotlin row container that owns [`SET_STACKING_Z`].
pub(crate) const ROW_CONTAINER_CLASS: &str = "io/idealyst/runtime/RustRowContainer";

/// Static writer for every stacking `translationZ`: sets it on the view
/// and, for a mounted virtualizer item root, on its row container too.
pub(crate) const SET_STACKING_Z: &str = "setStackingZ";

/// JNI signature of [`SET_STACKING_Z`]: `(view: View, z: Float)`.
pub(crate) const SET_STACKING_Z_SIG: &str = "(Landroid/view/View;F)V";

/// `translationZ` (device px) for a z value. Non-positive and NaN map
/// to `0` (the `StyleRules::z_index` contract: negative is treated as
/// `0`; an animated spring overshooting below `0` lands there too).
pub(crate) fn translation_z_px(z: f32) -> f32 {
    if z.is_nan() || z <= 0.0 {
        0.0
    } else {
        z * Z_STEP_PX
    }
}

/// `translationZ` (device px) for a style's effective stacking value
/// (`StyleRules::stacking_z`: the `z_index`, else the sticky raise,
/// else `0`).
pub(crate) fn static_translation_z_px(rules: &StyleRules) -> f32 {
    translation_z_px(rules.stacking_z() as f32)
}

/// The `translationZ` to write on this style apply, or `None` to skip
/// the JNI call. `last` is the value the previous apply wrote (`None`
/// = never written; the view still has its constructor default `0`),
/// so an un-raised view costs no call, and a removed `z_index` (or a
/// `Sticky` → `Relative` flip) writes `0` back.
pub(crate) fn translation_z_update(last: Option<f32>, want: f32) -> Option<f32> {
    match last {
        None if want == 0.0 => None,
        Some(prev) if prev == want => None,
        _ => Some(want),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::style::z_paint_order;
    use runtime_shared::Position;

    fn z_rules(z: Option<i32>, position: Option<Position>) -> StyleRules {
        StyleRules { z_index: z, position, ..Default::default() }
    }

    /// A static `z_index: N` and an animated `ZIndex` of `N.0` must land
    /// on the same `translationZ`, or a static sibling and an animated
    /// one at the same z would order by unit accident instead of tree
    /// order.
    #[test]
    fn regression_static_and_animated_z_index_map_to_same_translation_z() {
        for z in [0, 1, 2, 7, 100, 9_999] {
            assert_eq!(
                static_translation_z_px(&z_rules(Some(z), None)),
                translation_z_px(z as f32),
                "z_index {z}",
            );
        }
    }

    /// The sticky raise moved from `imp::sticky` into the style apply;
    /// a sticky node without a `z_index` must keep the 0.01 px raise
    /// that module wrote (`STICKY_RAISE_Z_PX`).
    #[test]
    fn regression_sticky_without_z_index_keeps_its_raise() {
        let sticky = z_rules(None, Some(Position::Sticky));
        assert_eq!(static_translation_z_px(&sticky), 0.01);
        assert_eq!(
            static_translation_z_px(&sticky),
            translation_z_px(runtime_shared::style::STICKY_Z as f32),
        );
    }

    /// An explicit `z_index` on a sticky node replaces the implicit raise
    /// (including `0`, which opts out of it) — the sticky registry must
    /// not overwrite it.
    #[test]
    fn regression_sticky_explicit_z_index_wins_over_raise() {
        assert_eq!(
            static_translation_z_px(&z_rules(Some(5), Some(Position::Sticky))),
            translation_z_px(5.0),
        );
        assert_eq!(static_translation_z_px(&z_rules(Some(0), Some(Position::Sticky))), 0.0);
    }

    #[test]
    fn regression_negative_or_nan_z_never_sinks_below_siblings() {
        assert_eq!(static_translation_z_px(&z_rules(Some(-3), None)), 0.0);
        assert_eq!(translation_z_px(-0.2), 0.0);
        assert_eq!(translation_z_px(f32::NAN), 0.0);
        assert_eq!(static_translation_z_px(&z_rules(None, None)), 0.0);
    }

    /// HWUI draws any child with `|Z| <= 0.001` px in child order, so the
    /// smallest raise (`z_index: 1`) must clear that epsilon or it would
    /// reorder touch dispatch (Java-side Z sort) but not drawing.
    #[test]
    fn regression_z_index_one_clears_hwui_in_plane_epsilon() {
        const HWUI_NON_ZERO_EPSILON: f32 = 0.001;
        assert!(translation_z_px(1.0) > HWUI_NON_ZERO_EPSILON);
    }

    #[test]
    fn regression_mapping_preserves_strict_order_between_integer_z() {
        let mut prev = translation_z_px(0.0);
        for z in 1..=100_000 {
            let cur = translation_z_px(z as f32);
            assert!(cur > prev, "z {z} collapsed onto z {}", z - 1);
            prev = cur;
        }
    }

    /// ViewGroup's Z order over the mapped values must be the shared
    /// paint order over the raw z (ties in tree order).
    #[test]
    fn regression_translation_z_paint_order_matches_shared_rule() {
        let zs = [3.0_f32, 0.0, 1.0, 3.0, 0.0, 2.0];
        let mapped: Vec<f32> = zs.iter().map(|z| translation_z_px(*z)).collect();
        assert_eq!(z_paint_order(&mapped), z_paint_order(&zs));
    }

    #[test]
    fn regression_unraised_view_costs_no_translation_z_call() {
        assert_eq!(translation_z_update(None, 0.0), None);
        assert_eq!(translation_z_update(None, 0.02), Some(0.02));
    }

    /// Removing a `z_index` (or flipping off `Sticky`) must write `0`
    /// back — a skipped write would leave the view raised forever.
    #[test]
    fn regression_removed_z_index_restores_zero() {
        assert_eq!(translation_z_update(Some(0.02), 0.0), Some(0.0));
        assert_eq!(translation_z_update(Some(0.02), 0.02), None);
        assert_eq!(translation_z_update(Some(0.0), 0.0), None);
    }

    // The row-container half lives in Kotlin (`RustListAdapter.kt`),
    // which has no JVM test harness in this repo and needs a real
    // `ViewGroup` to observe Z ordering; these pin the contract the
    // Rust writer and the adapter rely on at the closest host-reachable
    // layer — the Kotlin source the APK is compiled from.

    const ADAPTER_KT: &str =
        include_str!("../runtime/kotlin/io/idealyst/runtime/RustListAdapter.kt");

    /// "Does the Kotlin contain this code", ignoring whitespace and `//`
    /// line comments so statements match across comment lines.
    fn kt_has(needle: &str) -> bool {
        let squash = |s: &str| {
            s.lines()
                .map(|l| l.split_once("//").map_or(l, |(code, _)| code))
                .flat_map(str::split_whitespace)
                .collect::<String>()
        };
        squash(ADAPTER_KT).contains(&squash(needle))
    }

    /// The Rust writer binds `setStackingZ` by class name + signature;
    /// a rename or signature drift on either side silently falls back
    /// to the item-root-only write, leaving rows in adapter order.
    #[test]
    fn regression_virtualizer_row_z_writer_is_bound_by_matching_jni_name() {
        let class = ROW_CONTAINER_CLASS.rsplit('/').next().unwrap();
        let package = ROW_CONTAINER_CLASS.rsplit_once('/').unwrap().0.replace('/', ".");
        assert!(kt_has(&format!("package {package}")));
        assert!(kt_has(&format!("class {class}(context: Context) : FrameLayout(context)")));
        assert_eq!(SET_STACKING_Z_SIG, "(Landroid/view/View;F)V");
        assert!(
            kt_has(&format!("@JvmStatic fun {SET_STACKING_Z}(view: View, z: Float)")),
            "RustListAdapter.kt must declare a static `{SET_STACKING_Z}(View, Float)`",
        );
    }

    /// A `z_index` on an item root must order its ROW against the
    /// neighbouring rows, as on web. RecyclerView orders its children —
    /// the holder containers — so a later Z write on the item root must
    /// raise its container too.
    #[test]
    fn regression_virtualizer_item_z_raises_its_row_container() {
        assert!(kt_has("view.translationZ = z"));
        assert!(kt_has("(view.parent as? RustRowContainer)?.translationZ = z"));
        assert!(kt_has("val container = RustRowContainer(parent.context)"));
    }

    /// The item root's Z is written during realize, before it has the
    /// container as parent, so bind must copy it up; and the copy must
    /// be unconditional (and release must reset), or a holder recycled
    /// from a raised row keeps that raise for a flat one.
    #[test]
    fn regression_virtualizer_recycled_row_container_drops_previous_z() {
        assert!(kt_has("holder.container.translationZ = mount.view.translationZ"));
        assert!(kt_has("holder.container.removeAllViews() holder.container.translationZ = 0f"));
    }

    /// The container's default child clip would hide the very overlap a
    /// row's Z orders (a translated row reaching into its neighbour).
    #[test]
    fn regression_virtualizer_row_container_does_not_clip_its_row() {
        assert!(kt_has("init { clipChildren = false }"));
    }
}
