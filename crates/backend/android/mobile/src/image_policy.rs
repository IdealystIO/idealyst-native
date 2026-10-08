//! Pure decisions behind the Android `image()` primitive, kept un-gated so
//! the regression coverage runs from any host
//! (`cargo test -p backend-android-mobile`). The JNI half
//! (`imp::primitives::image`) feeds these functions measurements and
//! applies the result. Same rationale as `layout_policy`.
//!
//! # The bug this replaced
//!
//! `image()` on Android created a bare `ImageView` and ignored `src`
//! entirely — no `asset://` lookup, no `data:` URI, no network fetch, no
//! SVG. It also had no Taffy measure function, so an unsized image collapsed
//! to 0×0, and `on_load` / `on_error` were never delivered. Every other
//! native backend loads all of these. The decoding rules now come from the
//! shared `backend_image_source` crate (the same one iOS and macOS use);
//! this module holds the Android-side policy around them.

use backend_image_source::{classify_src, SrcKind};

/// What `create_image` / `update_image_src` must do with a `src`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadPlan<'a> {
    /// The view already shows (or is fetching) this exact `src` — the
    /// walker's reactive-`src` effect fires once at mount with the value
    /// `create_image` already loaded; reloading would double-fetch and
    /// double-fire `on_load`. Mirrors iOS's `current_src` guard.
    Unchanged,
    /// Show the registered asset with this id (decoded at `register_asset`).
    Asset(u64),
    /// Decode the `data:` URI synchronously.
    DataUri,
    /// Fetch off the UI thread, decode, then show on the UI thread.
    Remote(&'a str),
    /// No loader understands this `src`: fire `on_error` + log a warning
    /// rather than leave a silently blank slot.
    Unsupported,
}

/// Route `src` given the `src` the view currently holds (`None` at create).
// Consumed by `imp` (android-only) and the tests below.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn plan_load<'a>(current: Option<&str>, src: &'a str) -> LoadPlan<'a> {
    if current == Some(src) {
        return LoadPlan::Unchanged;
    }
    match classify_src(src) {
        SrcKind::Asset(id) => LoadPlan::Asset(id),
        SrcKind::DataUri => LoadPlan::DataUri,
        SrcKind::Remote(url) => LoadPlan::Remote(url),
        SrcKind::Unsupported => LoadPlan::Unsupported,
    }
}

/// The image's Taffy measurement: an explicit (known) dimension wins on its
/// axis, otherwise the natural size — `(0, 0)` until something has decoded.
/// Exactly iOS `install_image_measure` / macOS's (`intrinsicContentSize`
/// per axis), so the same unsized `image()` gets the same box everywhere.
///
/// `natural` is in dp: a bitmap's pixel count read as dp (the `UIImage`
/// scale-1 / web `naturalWidth` convention), an SVG's intrinsic size.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn measure(known: (Option<f32>, Option<f32>), natural: Option<(f32, f32)>) -> (f32, f32) {
    let (nw, nh) = natural.unwrap_or((0.0, 0.0));
    (known.0.unwrap_or(nw), known.1.unwrap_or(nh))
}

/// The SVG raster density (px per intrinsic dp) a view displayed at
/// `frame_dp` on a `density` screen needs, or `None` when the current
/// raster (`current_scale`, 0 = none) is already that density. The scale
/// itself is the shared `SvgImage::raster_scale` rule iOS/macOS use; this
/// adds only the "is a re-raster due?" gate the layout pass asks every
/// frame.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn svg_reraster_scale(
    svg: &backend_image_source::SvgImage,
    frame_dp: (f32, f32),
    density: f32,
    current_scale: f64,
) -> Option<f64> {
    let scale = svg.raster_scale((frame_dp.0 as f64, frame_dp.1 as f64), density as f64);
    (scale != current_scale).then_some(scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use backend_image_source::SvgImage;

    #[test]
    fn regression_android_image_src_ignored() {
        // Before: every one of these left a blank 0×0 ImageView with no
        // event. Each now routes to a loader (or an explicit error).
        assert_eq!(plan_load(None, "asset://12"), LoadPlan::Asset(12));
        assert_eq!(plan_load(None, "data:image/png;base64,iVBORw0KGgo="), LoadPlan::DataUri);
        assert_eq!(plan_load(None, "data:image/svg+xml,%3Csvg%2F%3E"), LoadPlan::DataUri);
        assert_eq!(
            plan_load(None, "https://example.com/a.webp"),
            LoadPlan::Remote("https://example.com/a.webp")
        );
        assert_eq!(plan_load(None, "not a url"), LoadPlan::Unsupported);
        // …and the decoded natural size reaches layout instead of 0×0.
        assert_eq!(measure((None, None), Some((64.0, 32.0))), (64.0, 32.0));
    }

    #[test]
    fn unchanged_src_does_not_reload() {
        assert_eq!(plan_load(Some("https://a/b.png"), "https://a/b.png"), LoadPlan::Unchanged);
        assert_eq!(
            plan_load(Some("https://a/b.png"), "https://a/c.png"),
            LoadPlan::Remote("https://a/c.png")
        );
    }

    #[test]
    fn measure_prefers_known_axes_like_ios() {
        assert_eq!(measure((None, None), None), (0.0, 0.0), "nothing decoded yet");
        assert_eq!(measure((Some(100.0), None), Some((64.0, 32.0))), (100.0, 32.0));
        assert_eq!(measure((None, Some(10.0)), Some((64.0, 32.0))), (64.0, 10.0));
    }

    #[test]
    fn svg_rerasters_only_when_display_density_changes() {
        let svg = SvgImage::parse(br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16"/>"#)
            .unwrap();
        // First raster before layout: 1 intrinsic dp per dp × density.
        assert_eq!(svg_reraster_scale(&svg, (0.0, 0.0), 2.75, 0.0), Some(2.75));
        assert_eq!(svg_reraster_scale(&svg, (0.0, 0.0), 2.75, 2.75), None);
        // Laid out at 4× its intrinsic width: re-raster sharp.
        assert_eq!(svg_reraster_scale(&svg, (128.0, 64.0), 2.75, 2.75), Some(11.0));
        assert_eq!(svg_reraster_scale(&svg, (128.0, 64.0), 2.75, 11.0), None);
    }
}
