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
//!
//! # `src` contract (`docs/image-src.md`)
//!
//! Routing is the shared `runtime_shared::image_src::classify`: a relative
//! path, scheme-relative URL, `file:`/`content:`/… or empty `src` is
//! rejected (`warn_rejected` + `on_error`) exactly as on every other
//! backend. `asset!` images load from the APK's `assets/` through
//! `AssetManager` ([`plan_asset`]), not only from embedded bytes.

use backend_image_source::{
    classify, decode_bytes, parse_data_uri, DecodedSource, ImageSrc, RejectReason, SvgImage,
};
use runtime_shared::AssetSource;

// The `src` contract itself (what is accepted, what is rejected and why) is
// `runtime_shared::image_src` — see `docs/image-src.md`. This module only
// maps its verdicts onto Android's loaders.

/// What `create_image` / `update_image_src` must do with a `src`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadPlan<'a> {
    /// The view already shows (or is fetching) this exact `src` — the
    /// vocabulary's reactive-`src` effect fires once at mount with the value
    /// `create_image` already loaded; reloading would double-fetch and
    /// double-fire `on_load`. Mirrors iOS's `current_src` guard.
    Unchanged,
    /// Show the registered asset with this id (see [`plan_asset`]).
    Asset(u64),
    /// Decode the `data:` URI synchronously ([`decode_data_uri`]).
    DataUri,
    /// Fetch off the UI thread, decode, then show on the UI thread.
    Remote(&'a str),
    /// Not a `src` the contract accepts (relative path, `file:`, empty, …):
    /// `warn_rejected` + `on_error`, never a silently blank slot.
    Rejected(RejectReason),
}

/// Route `src` given the `src` the view currently holds (`None` at create).
// Consumed by `imp` (android-only) and the tests below.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn plan_load<'a>(current: Option<&str>, src: &'a str) -> LoadPlan<'a> {
    if current == Some(src) {
        return LoadPlan::Unchanged;
    }
    match classify(src) {
        ImageSrc::Asset(id) => LoadPlan::Asset(id),
        ImageSrc::Data(_) => LoadPlan::DataUri,
        ImageSrc::Remote(url) => LoadPlan::Remote(url),
        ImageSrc::Rejected(why) => LoadPlan::Rejected(why),
    }
}

/// A `data:` URI's payload, ready for the view: an SVG (rasterized by
/// resvg at the displayed size) or bitmap bytes for `BitmapFactory`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) enum DecodedData {
    Svg(SvgImage),
    Bitmap(Vec<u8>),
}

/// Parse + sniff a `data:` URI. `Err` carries the `on_error` diagnosis (bad
/// base64 / `%XX`, or bytes that sniff as SVG but don't parse). Whether the
/// bitmap bytes actually decode is `BitmapFactory`'s call on device.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn decode_data_uri(src: &str) -> Result<DecodedData, &'static str> {
    let uri = parse_data_uri(src).ok_or("malformed data URI")?;
    match decode_bytes(&uri.bytes) {
        Some(DecodedSource::Svg(svg)) => Ok(DecodedData::Svg(svg)),
        Some(DecodedSource::Bitmap(_)) => Ok(DecodedData::Bitmap(uri.bytes)),
        None => Err("data URI is not a valid SVG"),
    }
}

/// How a registered image asset (`register_asset(id, AssetTag::Image, ..)`)
/// is loaded on Android.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AssetPlan {
    /// `embed_asset!` / a `face!`-style source: the bytes are in the binary;
    /// decode them once at registration.
    Bytes(&'static [u8]),
    /// `asset!("images/logo.png")`: the build copies the declaring crate's
    /// `assets/` directory into the APK's `assets/`, so the file is the APK
    /// entry `assets/<path>`, which `AssetManager.open(<path>)` reads (the
    /// argument is relative to the APK's `assets/` root). Carries the
    /// normalized `<path>`.
    ApkAsset(String),
    /// `AssetSource::Remote { url }` with an `http(s)` URL: the same fetch a
    /// `https://` `src` gets.
    Remote(&'static str),
    /// A bundled path that could escape the assets directory, or a remote
    /// asset whose URL the contract rejects. Every view showing it fires
    /// `on_error`.
    Invalid(String),
}

/// Map a registered image asset onto its Android loader. See [`AssetPlan`].
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn plan_asset(source: &AssetSource) -> AssetPlan {
    match *source {
        AssetSource::Embedded { bytes, .. } | AssetSource::BundledEmbedded { bytes, .. } => {
            AssetPlan::Bytes(bytes)
        }
        AssetSource::Bundled { path } => match apk_asset_path(path) {
            Some(p) => AssetPlan::ApkAsset(p),
            None => AssetPlan::Invalid(format!(
                "asset path {path:?} must be relative to the crate's assets/ dir (no leading /, no ..)"
            )),
        },
        AssetSource::Remote { url } => match classify(url) {
            ImageSrc::Remote(u) => AssetPlan::Remote(u),
            ImageSrc::Rejected(why) => {
                AssetPlan::Invalid(format!("remote asset {url:?}: {}", why.explain()))
            }
            _ => AssetPlan::Invalid(format!("remote asset {url:?} is not an http(s) URL")),
        },
    }
}

/// The `AssetManager.open` argument for an `asset!` path: `None` when it is
/// empty, absolute, or has a `..` component (an asset path names a file the
/// build shipped, nothing else — the same rule as the desktop
/// `bundled_asset_candidates`). `.` and empty components are dropped:
/// `AssetManager` does no path normalization, so `./a.png` would miss.
pub(crate) fn apk_asset_path(path: &str) -> Option<String> {
    if path.starts_with('/') || path.starts_with('\\') {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            p => parts.push(p),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
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
        assert_eq!(plan_load(None, "not a url"), LoadPlan::Rejected(RejectReason::Relative));
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

    /// Render one `vectors::SYNC` src through this backend's real host-side
    /// path: `plan_load`, then `decode_data_uri` for `data:`, then the
    /// registered-asset lookup (none registered — the vector table's
    /// `asset://424242` is "never registered"). `Some(size)` = loads.
    /// The bitmap decode itself is `BitmapFactory` on device; for the PNG
    /// vectors the size comes from the IHDR chunk, which is exactly what
    /// `BitmapFactory` reports for a non-density-scaled decode.
    fn host_outcome(src: &str) -> Option<(u32, u32)> {
        match plan_load(None, src) {
            LoadPlan::Unchanged => unreachable!("no current src"),
            LoadPlan::Rejected(_) => None,
            LoadPlan::Remote(_) => panic!("SYNC vectors are never remote: {src}"),
            // Nothing is registered in a host run; `image::load` reports an
            // unregistered id as `on_error`.
            LoadPlan::Asset(_) => None,
            LoadPlan::DataUri => match decode_data_uri(src).ok()? {
                DecodedData::Svg(svg) => {
                    let (w, h) = svg.intrinsic_size();
                    Some((w as u32, h as u32))
                }
                DecodedData::Bitmap(bytes) => {
                    assert!(bytes.starts_with(b"\x89PNG"), "{src}");
                    let be = |i: usize| u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap());
                    Some((be(16), be(20)))
                }
            },
        }
    }

    #[test]
    fn src_contract_vectors() {
        use runtime_shared::image_src::vectors::{Expect, SYNC};
        for &(src, expect) in SYNC {
            let got = host_outcome(src);
            match expect {
                Expect::Loads { width, height } => {
                    assert_eq!(got, Some((width, height)), "{src:?} should load")
                }
                Expect::Errors => assert_eq!(got, None, "{src:?} should fire on_error"),
            }
        }
    }

    /// CrewForge's kiosk: a relative `/attachments/<id>` was a blank tile
    /// on native. Every relative form is now a reported rejection.
    #[test]
    fn regression_relative_src_rejected() {
        for src in ["/attachments/abc", "logo.png", "./logo.png", "../x.png"] {
            assert_eq!(plan_load(None, src), LoadPlan::Rejected(RejectReason::Relative), "{src}");
        }
        assert_eq!(
            plan_load(None, "//cdn.example.com/x.png"),
            LoadPlan::Rejected(RejectReason::SchemeRelative)
        );
        for src in ["file:///sdcard/x.png", "content://media/external/images/1"] {
            assert_eq!(plan_load(None, src), LoadPlan::Rejected(RejectReason::UnsupportedScheme));
        }
        assert_eq!(plan_load(None, ""), LoadPlan::Rejected(RejectReason::Empty));
    }

    /// `asset!` images fired `on_error` on Android: `register_asset` only
    /// handled embedded bytes, so a `Bundled` source was never cached. It
    /// now plans an `AssetManager.open(<path>)` of the APK entry
    /// `assets/<path>`.
    #[test]
    fn regression_bundled_asset_image_loads_from_apk_assets() {
        assert_eq!(
            plan_asset(&AssetSource::Bundled { path: "images/logo.png" }),
            AssetPlan::ApkAsset("images/logo.png".into())
        );
        assert_eq!(
            plan_asset(&AssetSource::Bundled { path: "./images//logo.png" }),
            AssetPlan::ApkAsset("images/logo.png".into()),
            "AssetManager does not normalize; we do"
        );
    }

    #[test]
    fn bundled_asset_paths_cannot_escape_the_assets_dir() {
        for path in ["", "/etc/passwd", "../secret.png", "images/../../x.png", "\\x.png"] {
            assert!(
                matches!(plan_asset(&AssetSource::Bundled { path }), AssetPlan::Invalid(_)),
                "{path:?}"
            );
        }
    }

    #[test]
    fn remote_and_embedded_assets() {
        const BYTES: &[u8] = b"\x89PNG";
        assert_eq!(
            plan_asset(&AssetSource::Embedded { bytes: BYTES, extension: "png" }),
            AssetPlan::Bytes(BYTES)
        );
        assert_eq!(
            plan_asset(&AssetSource::BundledEmbedded { path: "a.png", bytes: BYTES, extension: "png" }),
            AssetPlan::Bytes(BYTES)
        );
        assert_eq!(
            plan_asset(&AssetSource::Remote { url: "https://cdn.example.com/a.png" }),
            AssetPlan::Remote("https://cdn.example.com/a.png")
        );
        assert!(matches!(
            plan_asset(&AssetSource::Remote { url: "file:///a.png" }),
            AssetPlan::Invalid(_)
        ));
    }
}
