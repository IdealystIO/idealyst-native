//! The safe-area inset a `.safe_area(...)` view still owes, from the
//! window's system-bar and display-cutout insets.
//!
//! Un-gated (compiles on any host) so the tests run from any platform,
//! same pattern as `text_truncation_policy`. The JNI half that reads the
//! insets lives in `imp/mod.rs` (`platform_safe_area_insets`).
//!
//! The host root is always kept clear of the system bars (margins from
//! `RustKeyboardInsets`, or the window fitting them), so the bars are
//! never owed again. Only a cutout reaching past the bars is: with the
//! bars hidden (pinned or fullscreen) the window draws into the cutout
//! under `SHORT_EDGES`.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

/// Per side, in `[top, right, bottom, left]` order: the cutout minus the
/// bars (never negative), converted from px to dp.
pub(crate) fn owed_insets(bars_px: [i32; 4], cutout_px: [i32; 4], density: f32) -> [f32; 4] {
    let density = if density > 0.0 { density } else { 1.0 };
    std::array::from_fn(|i| (cutout_px[i] - bars_px[i]).max(0) as f32 / density)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tablet's 60dp taskbar is already a margin on the root; owing it
    /// again left a band the taskbar's height above it.
    #[test]
    fn regression_tablet_taskbar_is_not_inset_twice() {
        assert_eq!(owed_insets([0, 0, 120, 0], [0, 0, 0, 0], 2.0), [0.0; 4]);
    }

    /// A notch inside the shown status bar is covered by the root's margin.
    #[test]
    fn cutout_within_the_bars_owes_nothing() {
        assert_eq!(owed_insets([48, 0, 120, 0], [40, 0, 0, 0], 2.0), [0.0; 4]);
    }

    /// Bars hidden (pinned / fullscreen): the cutout is owed in full.
    #[test]
    fn cutout_with_hidden_bars_is_owed() {
        assert_eq!(owed_insets([0; 4], [80, 0, 0, 0], 2.0), [40.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn zero_density_reads_as_one() {
        assert_eq!(owed_insets([0; 4], [0, 10, 0, 0], 0.0), [0.0, 10.0, 0.0, 0.0]);
    }
}
