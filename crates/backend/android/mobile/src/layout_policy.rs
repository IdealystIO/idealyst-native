//! Pure layout-scheduling policy for the Android backend, kept un-gated so the
//! regression coverage runs from any host (`cargo test -p backend-android-mobile`).
//!
//! The JNI-driven `insert` path lives in `imp` (`target_os = "android"`); this
//! module holds only the decision it makes, so the policy is testable without a
//! live `View` tree. Same rationale as `sticky_compute` — see its module docs.

use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;

/// Should a just-completed `insert` kick a coalesced layout pass?
///
/// A subtree that mounts AFTER the initial build's `finish()` layout pass — a
/// portal opening, or any reactive control-flow child (`when` toggling true, a
/// `switch`/`match` branch swapping, an `Each` row inserting, a `presence`
/// entering) — has no upcoming `finish()` to size it. It must request its own
/// pass or it renders at default 0×0 `LayoutParams` and is invisible (the
/// "`when`-mounted camera widget never appears" bug).
///
/// - `is_portal_parent`: the parent is a portal content holder (always a
///   dynamic mount, regardless of attachment).
/// - `parent_attached_to_window`: the parent is already live in the window
///   hierarchy — Android's `View.isAttachedToWindow()` is the signal that the
///   initial `finish()` pass has run, so an insert now is a later dynamic
///   mount. A floating, mid-build parent is `false` here, so its inserts defer
///   to the upcoming `finish()` pass; scheduling against a partial tree would
///   compute and cache wrong sizes (the iOS mirror is `parent.window != nil`).
///
/// The coalescing flag in `imp::scheduler` (`LAYOUT_PASS_QUEUED`) collapses a
/// burst of sibling inserts in one runloop turn into a single pass, so a `true`
/// result is cheap even when a list mounts many rows at once.
// Consumed by `imp::insert` (android-only) and the tests below; on a host
// non-test lib build neither references it, hence the allow.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn insert_needs_layout_pass(
    is_portal_parent: bool,
    parent_attached_to_window: bool,
) -> bool {
    is_portal_parent || parent_attached_to_window
}

/// `Gravity.VERTICAL_GRAVITY_MASK`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const VERTICAL_GRAVITY_MASK: i32 = 0x70;

/// The gravity for `text_align` on a view whose gravity is `current`.
///
/// `setGravity` takes both axes at once, and `text_align` only speaks to the
/// horizontal, so the view's own vertical gravity stays: a single-line `EditText`
/// centres its text vertically and a label or text area sits at the top.
/// `Gravity.LEFT`, `CENTER_HORIZONTAL` and `RIGHT`. No `TextView` justify mode, so
/// `Justify` is left.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn text_align_gravity(current: i32, align: runtime_shared::TextAlign) -> i32 {
    use runtime_shared::TextAlign;
    let horizontal = match align {
        TextAlign::Left | TextAlign::Justify => 3,
        TextAlign::Center => 1,
        TextAlign::Right => 5,
    };
    (current & VERTICAL_GRAVITY_MASK) | horizontal
}

/// `Gravity.RELATIVE_HORIZONTAL_GRAVITY_MASK`: the horizontal bits, `START`'s
/// layout-direction flag included.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const HORIZONTAL_GRAVITY_MASK: i32 = 0x0080_0007;

/// A `setGravity` for a change of `text_align`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) struct TextAlignUpdate {
    pub(crate) gravity: i32,
    /// What to keep in `NodeAnim::natural_horizontal_gravity`.
    pub(crate) natural_horizontal: Option<i32>,
}

/// What a style's `text_align` does to a view at gravity `current`, or `None` when
/// there's nothing to write. `natural_horizontal` is the horizontal gravity the view
/// had before any alignment touched it.
///
/// With no alignment the view goes back to those bits rather than a fixed value:
/// a label or input starts at `START`, which follows right-to-left text, and a
/// `Button` starts centred.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn text_align_update(
    current: i32,
    natural_horizontal: Option<i32>,
    align: Option<runtime_shared::TextAlign>,
) -> Option<TextAlignUpdate> {
    match (align, natural_horizontal) {
        (Some(align), natural) => Some(TextAlignUpdate {
            gravity: text_align_gravity(current, align),
            natural_horizontal: natural.or(Some(current & HORIZONTAL_GRAVITY_MASK)),
        }),
        (None, Some(natural)) => Some(TextAlignUpdate {
            gravity: (current & VERTICAL_GRAVITY_MASK) | natural,
            natural_horizontal: None,
        }),
        (None, None) => None,
    }
}

/// The value a style's prop leaves on the view: what it states; else, once a style
/// has stated it, `rest`, the value with no style; else nothing, so a prop no style
/// ever stated is not written and an animation on the same field is left alone.
///
/// The drop remembers `rest` rather than forgetting, so the next stated value
/// transitions from where the view is.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn stated_or_rest<T: Copy>(stated: Option<T>, last: Option<T>, rest: T) -> Option<T> {
    stated.or(last.map(|_| rest))
}

/// `alpha` with no style.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const ALPHA_OPAQUE: f32 = 1.0;
/// `backgroundColor` with no style: transparent.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const BACKGROUND_CLEAR: i32 = 0;

/// Transform slots, in the mask `NodeAnim::last_static_transform` keeps. A View's
/// translation, scale and rotation are the fields animations and presences write
/// too, so a restyle only touches what the style owns.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const AXIS_TRANSLATE_X: u8 = 1 << 0;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const AXIS_TRANSLATE_Y: u8 = 1 << 1;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const AXIS_SCALE_X: u8 = 1 << 2;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const AXIS_SCALE_Y: u8 = 1 << 3;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const AXIS_ROTATE: u8 = 1 << 4;

/// The slots an apply writes: the ones the style states, and the ones the last style
/// owned and this one dropped, which go back to identity.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn transform_axes_to_write(last_owned: u8, stated: u8) -> u8 {
    last_owned | stated
}

/// Android `ViewGroup.LayoutParams` sentinels. Named because `-1` and `-2`
/// in a size field are the two magic numbers this file exists to keep honest.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const MATCH_PARENT: i32 = -1;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const WRAP_CONTENT: i32 = -2;

/// What a style `width`/`height` becomes in `LayoutParams`, for every
/// `Length` that is not `Px` (which needs a live `View` for the dp→px
/// conversion and so is resolved by the caller).
///
/// `Full` maps to `WRAP_CONTENT`, with `Auto`. It is a corner-radius
/// concept — "half the shorter side, resolved when painted" — and carries
/// no meaning as a box dimension, so the framework's own shared mapping
/// in `runtime_layout` already folds the two together
/// (`FwLength::Auto | FwLength::Full => Dimension::Auto`). Android agreeing
/// is what keeps the same style tree the same size on every backend.
///
/// A function rather than a `match` at each call site so a new `Length`
/// variant is handled once, in host-testable code — per-site matches
/// only fail to compile on `aarch64-linux-android`, which nothing on a
/// desktop host builds.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn non_px_layout_param(length: &runtime_shared::Length) -> Option<i32> {
    match length {
        runtime_shared::Length::Px(_) => None,
        runtime_shared::Length::Percent(_) => Some(MATCH_PARENT),
        runtime_shared::Length::Auto | runtime_shared::Length::Full | runtime_shared::Length::FitContent => Some(WRAP_CONTENT),
    }
}

/// What the layout pass does with one Taffy root.
///
/// A mounted `virtual_grid` cell's root view has no Taffy PARENT — the
/// grid engine places it in the scroller's content space — so it is a
/// root, and the pass did two things to a root that are exactly wrong
/// for a cell: computed it against the viewport (so the cell's SUBTREE
/// laid out against the screen) and then handed the result to
/// `applyFrames`. Kotlin's `setCellFrame` happened to re-apply the
/// stored frame afterwards, which is why the symptom here was milder
/// than iOS's full-height cells — but the subtree was still wrong. The
/// iOS twin is `root_box_policy`; the JNI half is verified on a
/// device.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) struct RootPass {
    /// The box Taffy computes the root against.
    pub compute_against: (f32, f32),
    /// `true` when the grid engine owns the root's frame and the apply
    /// batch must leave it out.
    pub engine_owns_frame: bool,
}

/// `cell_box` is `virtual_grid::cell_box(view_key)` — `Some` only for a
/// mounted grid cell's root; `viewport` is what every other root fills.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn root_pass(cell_box: Option<(f32, f32)>, viewport: (f32, f32)) -> RootPass {
    match cell_box {
        Some(cell) => RootPass {
            compute_against: cell,
            engine_owns_frame: true,
        },
        None => RootPass {
            compute_against: viewport,
            engine_owns_frame: false,
        },
    }
}

/// The box a mounted virtualizer row is computed against: its slot in the
/// list. `cross` is the extent the list gave the row's container; the main
/// extent is the author's size, or for a measured list the row's content
/// (`natural`), read only then because a measure is a layout of the row.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn list_row_box(
    horizontal: bool,
    cross: f32,
    known_main: Option<f32>,
    natural: impl FnOnce() -> f32,
) -> (f32, f32) {
    let main = known_main.unwrap_or_else(natural);
    if horizontal { (main, cross) } else { (cross, main) }
}

/// Does a virtualizer need the `RustGridSpacingDecoration` that applies its
/// main/cross gaps?
///
/// Any non-zero gap does, negative included: a negative `gap` overlaps
/// successive rows (each row starts `|gap|` before the previous one ends),
/// which is what the web and iOS virtualizers render, and the stacking of the
/// overlap is then `z_index`'s job. Only an all-zero layout skips it, so a
/// plain list pays nothing. Not `> 0.0`: that dropped negative gaps on
/// Android, so overlapping rows laid out flush.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn needs_spacing_decoration(layout: runtime_shared::VirtualLayout) -> bool {
    layout.main_spacing != 0.0 || layout.cross_spacing != 0.0
}

/// Diameter of an [`ActivityIndicatorSize::Small`] spinner, in dp.
///
/// The diameters are the ones `UIActivityIndicatorView` reports on iOS. A
/// `ProgressBar`'s own metrics (16dp / 48dp / 76dp under Material) would make
/// the same author tree a different size per phone platform, and its drawable
/// scales to its frame, so nothing depends on them.
pub(crate) const ACTIVITY_INDICATOR_SMALL_DP: f32 = 20.0;
/// Diameter of an [`ActivityIndicatorSize::Large`] spinner, in dp.
pub(crate) const ACTIVITY_INDICATOR_LARGE_DP: f32 = 37.0;

/// What a spinner's layout leaf measures: an author-pinned dimension wins,
/// the other falls back to the diameter for `size`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn activity_indicator_measure(
    known: (Option<f32>, Option<f32>),
    size: ActivityIndicatorSize,
) -> (f32, f32) {
    let diameter = match size {
        ActivityIndicatorSize::Small => ACTIVITY_INDICATOR_SMALL_DP,
        ActivityIndicatorSize::Large => ACTIVITY_INDICATOR_LARGE_DP,
    };
    (known.0.unwrap_or(diameter), known.1.unwrap_or(diameter))
}

#[cfg(test)]
mod root_pass_tests {
    use super::*;

    /// Regression: a cell root computed against the viewport laid its
    /// subtree out against the screen and was then framed at the origin.
    #[test]
    fn regression_grid_cell_root_is_computed_against_its_box_and_left_to_the_engine() {
        let pass = root_pass(Some((40.0, 44.0)), (411.0, 891.0));
        assert_eq!(pass.compute_against, (40.0, 44.0));
        assert!(pass.engine_owns_frame);
    }

    #[test]
    fn an_ordinary_root_fills_the_viewport_and_is_framed_by_the_pass() {
        let pass = root_pass(None, (411.0, 891.0));
        assert_eq!(pass.compute_against, (411.0, 891.0));
        assert!(!pass.engine_owns_frame);
    }
}

#[cfg(test)]
mod text_align_gravity_tests {
    use super::*;
    use runtime_shared::TextAlign;

    const CENTER_VERTICAL: i32 = 0x10;
    const TOP: i32 = 0x30;
    /// `Gravity.START`, what a `TextView` starts with on its horizontal axis.
    const START: i32 = 0x0080_0003;

    /// Regression: `Field(text_align)` pinned `TOP` along with the horizontal
    /// alignment, so an aligned single-line input lost its vertical centring and
    /// its text sat high in the box.
    #[test]
    fn regression_text_align_keeps_an_input_vertically_centred() {
        let input = START | CENTER_VERTICAL;
        assert_eq!(text_align_gravity(input, TextAlign::Right), CENTER_VERTICAL | 5);
        assert_eq!(text_align_gravity(input, TextAlign::Center), CENTER_VERTICAL | 1);
        assert_eq!(text_align_gravity(input, TextAlign::Left), CENTER_VERTICAL | 3);
        assert_eq!(text_align_gravity(input, TextAlign::Justify), CENTER_VERTICAL | 3);
    }

    /// The horizontal bits of a label's or input's `START`.
    const START_BITS: i32 = 0x0080_0003;

    /// Regression: a `Field(text_align)` flipping from `Right` back to `Natural`,
    /// or a stylesheet variant dropping `text_align`, kept `RIGHT` on the view:
    /// the style path only wrote gravity when a style stated an alignment. It goes
    /// back to the view's own start, which is what follows right-to-left text, and
    /// the input's vertical centring stays.
    #[test]
    fn regression_android_removed_text_align_restores_natural_gravity() {
        let input = START | CENTER_VERTICAL;
        let aligned = text_align_update(input, None, Some(TextAlign::Right)).expect("a first alignment writes");
        assert_eq!(aligned.gravity, CENTER_VERTICAL | 5);
        assert_eq!(aligned.natural_horizontal, Some(START_BITS));

        let removed = text_align_update(aligned.gravity, aligned.natural_horizontal, None).expect("removal writes");
        assert_eq!(removed.gravity, input);
        assert_eq!(removed.natural_horizontal, None, "nothing is left to restore");
    }

    /// `Button` is a `TextView` that starts centred, so its natural isn't `START`.
    #[test]
    fn removed_text_align_restores_what_the_view_started_with() {
        const BUTTON: i32 = 0x11; // Gravity.CENTER
        let aligned = text_align_update(BUTTON, None, Some(TextAlign::Left)).unwrap();
        let removed = text_align_update(aligned.gravity, aligned.natural_horizontal, None).unwrap();
        assert_eq!(removed.gravity, BUTTON);
    }

    /// A second alignment keeps what the view started with, not the first alignment.
    #[test]
    fn changing_text_align_keeps_the_natural_gravity() {
        let input = START | CENTER_VERTICAL;
        let right = text_align_update(input, None, Some(TextAlign::Right)).unwrap();
        let center = text_align_update(right.gravity, right.natural_horizontal, Some(TextAlign::Center)).unwrap();
        assert_eq!(center.gravity, CENTER_VERTICAL | 1);
        let removed = text_align_update(center.gravity, center.natural_horizontal, None).unwrap();
        assert_eq!(removed.gravity, input);
    }

    /// A view no alignment ever touched is left alone. The caller skips the JNI read.
    #[test]
    fn no_text_align_on_an_untouched_view_writes_nothing() {
        assert_eq!(text_align_update(START | TOP, None, None), None);
    }

    #[test]
    fn text_align_keeps_a_label_or_text_area_at_the_top() {
        assert_eq!(text_align_gravity(START | TOP, TextAlign::Right), TOP | 5);
    }
}

#[cfg(test)]
mod length_param_tests {
    use super::*;
    use runtime_shared::Length;

    /// Regression: `Length::Full` was added without these two matches
    /// learning about it, and `backend-android-mobile` stopped compiling
    /// for `aarch64-linux-android` — a whole backend dark, with the break
    /// invisible to `cargo test` because `imp/style.rs` is android-only.
    ///
    /// It folds in with `Auto` because that is what the framework's own
    /// shared mapping does (`runtime_layout`:
    /// `FwLength::Auto | FwLength::Full => Dimension::Auto`). `Full` is a
    /// corner-radius idea with no meaning as a box dimension.
    #[test]
    fn regression_full_sizes_like_auto() {
        assert_eq!(non_px_layout_param(&Length::Full), Some(WRAP_CONTENT));
        assert_eq!(
            non_px_layout_param(&Length::Full),
            non_px_layout_param(&Length::Auto),
            "Full and Auto must not diverge as dimensions"
        );
    }

    #[test]
    fn percent_fills_the_parent_and_px_defers_to_the_caller() {
        assert_eq!(non_px_layout_param(&Length::Percent(50.0)), Some(MATCH_PARENT));
        assert_eq!(
            non_px_layout_param(&Length::Px(12.0)),
            None,
            "Px needs a live View for dp->px, so the caller resolves it"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::insert_needs_layout_pass;

    /// Regression: a `when`/`Each`/`presence` child mounting into a live
    /// (window-attached) non-portal parent MUST schedule a layout pass, not
    /// only portal inserts — otherwise the dynamically-mounted subtree stays
    /// at 0×0 (e.g. a `when`-mounted camera widget never appears).
    #[test]
    fn dynamic_mount_into_attached_nonportal_parent_schedules_pass() {
        assert!(insert_needs_layout_pass(false, true));
    }

    /// A mid-build insert into a floating (not-yet-attached) parent must NOT
    /// schedule — the upcoming `finish()` pass sizes it, and a pass against a
    /// partial tree would cache wrong sizes.
    #[test]
    fn mid_build_insert_into_floating_parent_defers() {
        assert!(!insert_needs_layout_pass(false, false));
    }

    /// Portals always schedule, regardless of attachment state at insert time.
    #[test]
    fn portal_parent_always_schedules() {
        assert!(insert_needs_layout_pass(true, false));
        assert!(insert_needs_layout_pass(true, true));
    }
}

#[cfg(test)]
mod spacing_decoration_tests {
    use super::needs_spacing_decoration;
    use runtime_shared::VirtualLayout;

    /// Regression: a `flat_list(gap = -40.0)` laid its rows out flush on
    /// Android because the decoration was only installed for positive gaps,
    /// so rows never overlapped and `z_index` had nothing to order.
    #[test]
    fn regression_android_negative_virtualizer_gap_overlaps_rows() {
        let overlap = VirtualLayout { main_spacing: -40.0, ..VirtualLayout::default() };
        assert!(needs_spacing_decoration(overlap));
        let lanes = VirtualLayout { cross_spacing: -8.0, ..VirtualLayout::default() };
        assert!(needs_spacing_decoration(lanes));
    }

    #[test]
    fn plain_and_positive_gap_layouts() {
        assert!(!needs_spacing_decoration(VirtualLayout::default()));
        let gap = VirtualLayout { main_spacing: 8.0, ..VirtualLayout::default() };
        assert!(needs_spacing_decoration(gap));
    }
}

#[cfg(test)]
mod activity_indicator_tests {
    use super::*;

    /// The spinner's leaf had no measurer, so an unsized spinner laid out at
    /// 0x0 and drew nothing. Every `activity_indicator` the framework mounts
    /// with no pinned size goes through this answer.
    #[test]
    fn regression_android_activity_indicator_collapsed_to_nothing() {
        for size in [ActivityIndicatorSize::Small, ActivityIndicatorSize::Large] {
            let (w, h) = activity_indicator_measure((None, None), size);
            assert!(w > 0.0 && h > 0.0, "{size:?} measured {w}x{h}");
        }
    }

    #[test]
    fn activity_indicator_size_picks_the_diameter_and_a_pinned_dimension_wins() {
        let measure = activity_indicator_measure;
        assert_eq!(measure((None, None), ActivityIndicatorSize::Small), (20.0, 20.0));
        assert_eq!(measure((None, None), ActivityIndicatorSize::Large), (37.0, 37.0));
        assert_eq!(measure((Some(64.0), None), ActivityIndicatorSize::Small), (64.0, 20.0));
        assert_eq!(measure((None, Some(8.0)), ActivityIndicatorSize::Large), (37.0, 8.0));
    }
}

#[cfg(test)]
mod list_row_box_tests {
    use super::*;

    /// A row's root is a layout root like a screen's, and was computed against
    /// the viewport: the first pass after it mounted wrote it a screen tall,
    /// its list stopped binding rows after the first, and a row added to a
    /// short list never appeared. Run against the real tree: a text row
    /// computed against this box is its slot, not the screen.
    #[test]
    fn regression_android_flat_list_row_stretched_to_the_viewport() {
        let (w, h) = list_row_box(false, 411.0, Some(32.0), || panic!("a known size is not measured"));
        assert_eq!((w, h), (411.0, 32.0));

        let mut layout = runtime_layout::LayoutTree::new();
        let row = layout.new_node();
        layout.compute(row, w, h);
        let frame = layout.frame_of(row);
        assert_eq!((frame.width, frame.height), (411.0, 32.0));
    }

    #[test]
    fn a_measured_row_takes_its_content_and_a_horizontal_row_swaps_the_axes() {
        assert_eq!(list_row_box(false, 411.0, None, || 57.0), (411.0, 57.0));
        assert_eq!(list_row_box(true, 200.0, Some(80.0), || 0.0), (80.0, 200.0));
        assert_eq!(list_row_box(true, 200.0, None, || 96.0), (96.0, 200.0));
    }
}

#[cfg(test)]
mod style_reset_tests {
    use super::*;

    /// A variant with `opacity: 0.5` flipping to one without it stayed at 0.5: the
    /// style path only wrote `setAlpha` when a style stated one.
    #[test]
    fn regression_android_dropped_opacity_kept_its_old_value() {
        assert_eq!(stated_or_rest(None, Some(0.5), ALPHA_OPAQUE), Some(1.0));
        assert_eq!(stated_or_rest(Some(0.25), Some(0.5), ALPHA_OPAQUE), Some(0.25));
    }

    /// Same shape for a flat fill: `last_bg` kept the colour on the view.
    #[test]
    fn regression_android_dropped_background_kept_its_old_value() {
        assert_eq!(stated_or_rest(None, Some(0x7f00_ff00_u32 as i32), BACKGROUND_CLEAR), Some(0));
    }

    /// A prop no style stated is never written, whatever an animation put there.
    #[test]
    fn a_prop_no_style_stated_is_left_alone() {
        assert_eq!(stated_or_rest(None, None, ALPHA_OPAQUE), None);
    }

    /// A variant with a `transform` flipping to one without it stayed moved: only
    /// the axes the new style stated were written.
    #[test]
    fn regression_android_dropped_transform_axis_kept_its_old_value() {
        let owned = AXIS_TRANSLATE_X | AXIS_SCALE_X | AXIS_SCALE_Y;
        assert_eq!(transform_axes_to_write(owned, 0), owned);
        assert_eq!(transform_axes_to_write(owned, AXIS_ROTATE), owned | AXIS_ROTATE);
    }

    /// The Switch thumb: a theme re-fire on a style with no transform owns nothing,
    /// and an animated translate is left where it is.
    #[test]
    fn a_style_owning_no_axes_writes_none() {
        assert_eq!(transform_axes_to_write(0, 0), 0);
    }
}
