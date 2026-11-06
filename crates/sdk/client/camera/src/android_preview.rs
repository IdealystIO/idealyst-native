//! The two decisions behind Android's native preview, as pure functions: what the
//! repeating request targets, and what size the preview output takes. The Kotlin
//! shim (`RustCamera2Helper.kt`) asks for both over JNI, the same arrangement as
//! `android_rotation`: there is no JVM test setup in this repo, so anything worth
//! a test lives in Rust.
//!
//! ## Targets
//!
//! A capture session's outputs are fixed when it is created, but each request
//! picks which of them it fills, and changing the repeating request is cheap and
//! leaves the running exposure alone. So the session holds the preview and the
//! `ImageReader` and demand only moves the request:
//!
//! | preview attached | CPU wanted | targets | why |
//! | --- | --- | --- | --- |
//! | no | no | reader, frames discarded | the sensor keeps running, so exposure and focus have settled when a first pull comes |
//! | no | yes | reader | today's CPU path |
//! | yes | no | preview | a displayed preview costs no CPU copy |
//! | yes | yes | preview + reader | both |
//!
//! ## Preview size
//!
//! A capture of a frame has to contain what the preview showed. Each output is
//! cropped from the sensor to its own aspect ratio, so the preview takes the
//! reader's exact aspect ratio or there is no native preview at all.
//!
//! Camera2 guarantees `PRIV PREVIEW + YUV PREVIEW` on every hardware level, where
//! `PREVIEW` is the smaller of the display's size and 1080p. So the reader's own
//! size is preferred when it falls within that bound, which also keeps every
//! pixel of the two streams in register; otherwise the largest size within it.

/// The preview `Surface` is a target. Bit of [`repeating_targets`]'s result.
pub(crate) const TARGET_PREVIEW: i32 = 1;
/// The `ImageReader` is a target. Bit of [`repeating_targets`]'s result.
pub(crate) const TARGET_READER: i32 = 2;

/// What the repeating request targets. `preview_live` means a surface is attached
/// AND is an output of the current session; a session being rebuilt for a new
/// surface has none live until it is configured.
pub(crate) fn repeating_targets(preview_live: bool, cpu_wanted: bool) -> i32 {
    match (preview_live, cpu_wanted) {
        (false, _) => TARGET_READER,
        (true, false) => TARGET_PREVIEW,
        (true, true) => TARGET_PREVIEW | TARGET_READER,
    }
}

/// `PREVIEW` as Camera2 defines it, as (long edge, short edge): the display's size,
/// capped at 1080p. Sizes from a stream configuration map are compared by edge
/// because the display may be held either way up.
fn preview_bound(display_width: u32, display_height: u32) -> (u32, u32) {
    let long = display_width.max(display_height);
    let short = display_width.min(display_height);
    (long.min(1920), short.min(1080))
}

/// Pick the preview's size from `sizes` (what the device can output to a
/// `SurfaceTexture`), for a reader of `reader` size on this display. The index into
/// `sizes`, or `None` when none shares the reader's aspect ratio within the bound:
/// no native preview, so the stream displays through the CPU path.
pub(crate) fn choose_preview_size(
    reader: (u32, u32),
    sizes: &[(u32, u32)],
    display: (u32, u32),
) -> Option<usize> {
    let (long, short) = preview_bound(display.0, display.1);
    let usable = |&(_, &(w, h)): &(usize, &(u32, u32))| {
        // Cross-multiplied, so 1920x1080 and 1280x720 agree exactly.
        w > 0
            && h > 0
            && u64::from(w) * u64::from(reader.1) == u64::from(h) * u64::from(reader.0)
            && w.max(h) <= long
            && w.min(h) <= short
    };
    let mut candidates = sizes.iter().enumerate().filter(usable);
    if let Some((index, _)) = candidates.clone().find(|(_, &size)| size == reader) {
        return Some(index);
    }
    candidates.by_ref().max_by_key(|(_, &(w, h))| u64::from(w) * u64::from(h)).map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_a_displayed_preview_alone_leaves_the_reader_out() {
        // The bug this module exists for: a reader in the request is a YUV-to-RGBA
        // conversion per frame, and a slow one holds the whole request back.
        assert_eq!(repeating_targets(true, false) & TARGET_READER, 0);
    }

    #[test]
    fn every_demand_state_has_its_targets() {
        assert_eq!(repeating_targets(false, false), TARGET_READER);
        assert_eq!(repeating_targets(false, true), TARGET_READER);
        assert_eq!(repeating_targets(true, false), TARGET_PREVIEW);
        assert_eq!(repeating_targets(true, true), TARGET_PREVIEW | TARGET_READER);
    }

    #[test]
    fn no_state_leaves_the_request_with_no_target() {
        // A request with no target is an IllegalArgumentException, and a stopped
        // sensor would give a first pull an unexposed frame.
        for preview in [false, true] {
            for cpu in [false, true] {
                assert_ne!(repeating_targets(preview, cpu), 0, "{preview} {cpu}");
            }
        }
    }

    const TABLET: (u32, u32) = (1200, 1920);
    const P1080: (u32, u32) = (1920, 1080);

    #[test]
    fn the_readers_own_size_wins_within_the_bound() {
        let sizes = [(3840, 2160), (2560, 1440), P1080, (1280, 720), (640, 480)];
        assert_eq!(choose_preview_size(P1080, &sizes, TABLET), Some(2));
        let small = (1280, 720);
        assert_eq!(choose_preview_size(small, &sizes, TABLET), Some(3), "not the largest");
    }

    #[test]
    fn a_small_display_takes_the_largest_matching_size_it_can_guarantee() {
        let sizes = [P1080, (1280, 720), (960, 540), (800, 600)];
        // 1280x800: 1080p exceeds PREVIEW, 1280x720 does not.
        assert_eq!(choose_preview_size(P1080, &sizes, (800, 1280)), Some(1));
    }

    #[test]
    fn a_reader_past_1080p_takes_the_same_shape_below_it() {
        let sizes = [(3840, 2160), P1080, (1440, 1080)];
        assert_eq!(choose_preview_size((3840, 2160), &sizes, (2160, 3840)), Some(1));
        assert_eq!(choose_preview_size((4032, 3024), &sizes, (2160, 3840)), Some(2));
    }

    /// A preview at another aspect ratio crops the sensor differently, so what the
    /// person lined up would not be what the frame contains.
    #[test]
    fn regression_no_preview_at_a_different_aspect_ratio() {
        assert_eq!(choose_preview_size((640, 480), &[P1080, (1280, 720)], TABLET), None);
    }

    #[test]
    fn nothing_usable_is_none() {
        assert_eq!(choose_preview_size(P1080, &[], TABLET), None);
        assert_eq!(choose_preview_size(P1080, &[(0, 0)], TABLET), None);
        assert_eq!(choose_preview_size((0, 0), &[(0, 0)], TABLET), None);
        // Above the bound: a stream combination no device promises.
        assert_eq!(choose_preview_size((3840, 2160), &[(3840, 2160)], TABLET), None);
    }

    #[test]
    fn the_bound_reads_the_display_either_way_up() {
        assert_eq!(preview_bound(1200, 1920), (1920, 1080));
        assert_eq!(preview_bound(1920, 1200), (1920, 1080));
        assert_eq!(preview_bound(800, 1280), (1280, 800));
        assert_eq!(preview_bound(1440, 3200), (1920, 1080));
    }
}
