//! Image-source decoding shared by the native backends whose platform
//! image decoder reads neither SVG nor `data:` URIs: iOS and macOS
//! (`UIImage`/`NSImage(data:)`) and Android (`BitmapFactory`).
//!
//! An `image("data:image/svg+xml,…")` used to draw an empty slot with no
//! warning on every native backend while the web rendered it. This crate
//! owns the missing pieces so every native backend behaves identically
//! (CLAUDE.md §7):
//!
//! - [`parse_data_uri`] — RFC 2397 `data:` URIs, base64 or percent-encoded.
//! - [`looks_like_svg`] / [`decode_bytes`] — sniff SVG out of raw bytes, so
//!   one check covers embedded assets, `data:` URIs and fetched URLs.
//! - [`SvgImage`] — parse once with `usvg`, rasterize with `resvg` at the
//!   pixel density the view is actually displayed at. A vector source has
//!   no natural pixel size, so the view re-rasterizes when its on-screen
//!   size or screen scale changes ([`SvgImage::raster_scale`]) — a logo
//!   drawn at 3× its intrinsic size stays sharp instead of upscaling a 1×
//!   bitmap.
//! - [`classify_src`] — which loading path an `image()` `src` string takes.
//!
//! Everything here is pure Rust and host-testable. The platform wrappers
//! (CoreGraphics `CGImage` in `backend-apple-core::image_source`, the
//! `android.graphics.Bitmap` upload in `backend-android-mobile`) stay with
//! their platform. See this crate's `Cargo.toml` for why it is a crate of
//! its own (resvg must stay out of the web build).
//!
//! **SVG `<text>` is not rendered**: `resvg` is built without its text stack
//! (fontdb + shaping + system-font scanning), which would add most of the
//! dependency's size to every native binary for a feature logos rarely use.
//! Convert text to outlines in the source SVG.

use std::borrow::Cow;

/// A decoded `data:` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataUri {
    /// Lower-cased media type without parameters (`image/svg+xml`), or
    /// `text/plain` when the URI omits it (RFC 2397's default).
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Parse a `data:[<mediatype>][;base64],<data>` URI. Returns `None` for a
/// non-`data:` string or a malformed payload (bad base64, bad `%XX`).
pub fn parse_data_uri(src: &str) -> Option<DataUri> {
    let rest = strip_prefix_ignore_case(src, "data:")?;
    let comma = rest.find(',')?;
    let (meta, payload) = (&rest[..comma], &rest[comma + 1..]);
    let mut params = meta.split(';');
    let mime = params.next().unwrap_or("").trim().to_ascii_lowercase();
    let mime = if mime.is_empty() { "text/plain".to_string() } else { mime };
    let is_base64 = params.any(|p| p.trim().eq_ignore_ascii_case("base64"));
    // A base64 payload may itself be percent-encoded (`%2B` for `+`) when it
    // came through a URL encoder, so unescape first in both cases.
    let unescaped = percent_decode(payload)?;
    let bytes = if is_base64 { base64_decode(&unescaped)? } else { unescaped };
    Some(DataUri { mime, bytes })
}

fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

/// Standard-alphabet base64 (RFC 4648), padding optional, ASCII whitespace
/// ignored (data URIs are sometimes line-wrapped).
fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in input {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            break;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// `true` when `bytes` look like an SVG document: after an optional UTF-8
/// BOM, whitespace, an XML declaration, comments or a doctype, the first
/// element is `<svg`. Sniffing (rather than trusting a mime type or file
/// extension) lets one check cover all three ways an SVG reaches an image
/// view — `embed_asset!` bytes, a `data:` URI, and a fetched URL.
pub fn looks_like_svg(bytes: &[u8]) -> bool {
    /// How far into the document to look for the root element. Long license
    /// comments ahead of `<svg` fit comfortably; a bitmap never matches.
    const SNIFF_WINDOW: usize = 4096;
    let head = &bytes[..bytes.len().min(SNIFF_WINDOW)];
    let head = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    let Ok(text) = std::str::from_utf8(head).or_else(|e| {
        // The window may have cut a multi-byte character in half.
        std::str::from_utf8(&head[..e.valid_up_to()])
    }) else {
        return false;
    };
    let mut rest = text.trim_start();
    loop {
        if rest.starts_with("<svg") {
            return true;
        }
        let skip_to = if rest.starts_with("<?") {
            rest.find("?>").map(|i| i + 2)
        } else if rest.starts_with("<!--") {
            rest.find("-->").map(|i| i + 3)
        } else if rest.starts_with("<!") {
            rest.find('>').map(|i| i + 1)
        } else {
            return false;
        };
        match skip_to {
            Some(i) => rest = rest[i..].trim_start(),
            None => return false,
        }
    }
}

/// The largest raster edge, in pixels, an SVG is drawn at. A view stretched
/// to an absurd size (or a scale computed from a transient huge frame) would
/// otherwise allocate `edge² × 4` bytes per re-raster. 4096 px covers a
/// full-screen image on the largest phone / desktop displays at native
/// density.
pub const MAX_SVG_RASTER_EDGE_PX: f64 = 4096.0;

/// A parsed SVG, ready to rasterize at any density.
pub struct SvgImage {
    tree: resvg::usvg::Tree,
}

/// Premultiplied RGBA8 pixels, R,G,B,A bytes in memory order (tiny-skia's
/// layout). CoreGraphics reads it as `kCGImageAlphaPremultipliedLast` with
/// default byte order; Android's `Bitmap.Config.ARGB_8888` stores exactly
/// this byte order premultiplied, so `copyPixelsFromBuffer` takes it as-is.
pub struct SvgRaster {
    pub width_px: u32,
    pub height_px: u32,
    pub rgba_premultiplied: Vec<u8>,
    /// Pixels per intrinsic point. The platform keeps reporting the SVG's
    /// intrinsic size (not the raster's pixel size) for measurement and
    /// `on_load`, so neither changes when the view re-rasterizes at a new
    /// density.
    pub scale: f64,
}

impl SvgImage {
    /// Parse SVG bytes. `None` when they aren't a valid SVG document.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
        Some(Self { tree })
    }

    /// The document's intrinsic size in points (`width`/`height`, else the
    /// `viewBox` size — usvg resolves that).
    pub fn intrinsic_size(&self) -> (f64, f64) {
        let size = self.tree.size();
        (size.width() as f64, size.height() as f64)
    }

    /// Pixels per intrinsic point needed for a view whose bounds are
    /// `bounds` points on a `screen_scale` display: enough that neither axis
    /// is upscaled under any `ObjectFit` (cover fills the larger ratio), and
    /// never less than one intrinsic point per screen pixel group, so an
    /// unlaid-out (0×0) view still gets a usable first raster. Capped at
    /// [`MAX_SVG_RASTER_EDGE_PX`] on the longer edge.
    pub fn raster_scale(&self, bounds: (f64, f64), screen_scale: f64) -> f64 {
        let (iw, ih) = self.intrinsic_size();
        let screen_scale = if screen_scale > 0.0 { screen_scale } else { 1.0 };
        let fit = (bounds.0 / iw).max(bounds.1 / ih);
        let fit = if fit.is_finite() && fit > 1.0 { fit } else { 1.0 };
        let scale = fit * screen_scale;
        let cap = MAX_SVG_RASTER_EDGE_PX / iw.max(ih);
        scale.min(cap)
    }

    /// Rasterize at `scale` pixels per intrinsic point. `None` for a
    /// degenerate (zero-area) document.
    pub fn rasterize(&self, scale: f64) -> Option<SvgRaster> {
        let (iw, ih) = self.intrinsic_size();
        let width_px = (iw * scale).round().max(1.0) as u32;
        let height_px = (ih * scale).round().max(1.0) as u32;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(width_px, height_px)?;
        // Scale per axis from the rounded pixel size so the drawing fills
        // the pixmap exactly (no half-pixel transparent fringe).
        let transform = resvg::tiny_skia::Transform::from_scale(
            width_px as f32 / iw as f32,
            height_px as f32 / ih as f32,
        );
        resvg::render(&self.tree, transform, &mut pixmap.as_mut());
        Some(SvgRaster {
            width_px,
            height_px,
            rgba_premultiplied: pixmap.take(),
            scale: width_px as f64 / iw,
        })
    }
}

/// What a source string / byte blob decodes to before the platform wraps it.
pub enum DecodedSource<'a> {
    /// Bytes for the platform bitmap decoder (`UIImage`/`NSImage(data:)`,
    /// `BitmapFactory.decodeByteArray`).
    Bitmap(Cow<'a, [u8]>),
    Svg(SvgImage),
}

/// Classify raw image bytes: SVG (parsed) or a bitmap for the platform
/// decoder. `None` when the bytes sniff as SVG but don't parse.
pub fn decode_bytes(bytes: &[u8]) -> Option<DecodedSource<'_>> {
    if looks_like_svg(bytes) {
        SvgImage::parse(bytes).map(DecodedSource::Svg)
    } else {
        Some(DecodedSource::Bitmap(Cow::Borrowed(bytes)))
    }
}


/// The asset sentinel the walker hands a backend for `image_asset(..)`:
/// `asset://{id}` after `register_asset(id, AssetTag::Image, ..)`.
pub const ASSET_URL_PREFIX: &str = "asset://";

/// Which loading path an `image()` `src` string takes. Every native
/// backend routes on the same rules so one `src` behaves identically
/// everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrcKind<'a> {
    /// `asset://{id}` — look the id up in the backend's registered-asset
    /// cache. Carries the parsed id.
    Asset(u64),
    /// `data:` URI — decode synchronously with [`parse_data_uri`].
    DataUri,
    /// `http(s)://` — fetch asynchronously off the UI thread.
    Remote(&'a str),
    /// Anything else (an empty string, a bare path, an unknown scheme, a
    /// malformed `asset://` id). Backends report it as a load error.
    Unsupported,
}

/// Classify an `image()` `src` (see [`SrcKind`]). Scheme matching is
/// case-insensitive, like a browser's.
pub fn classify_src(src: &str) -> SrcKind<'_> {
    if let Some(rest) = strip_prefix_ignore_case(src, ASSET_URL_PREFIX) {
        return rest.parse().map(SrcKind::Asset).unwrap_or(SrcKind::Unsupported);
    }
    if strip_prefix_ignore_case(src, "data:").is_some() {
        return SrcKind::DataUri;
    }
    if strip_prefix_ignore_case(src, "http://").is_some()
        || strip_prefix_ignore_case(src, "https://").is_some()
    {
        return SrcKind::Remote(src);
    }
    SrcKind::Unsupported
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape CrewForge's `BrandLockup` builds: a percent-encoded SVG
    /// data URI (`svg_data_uri(include_str!("…/cf-mark-light.svg"))`).
    const MARK_SVG: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<!-- brand mark -->
<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16" viewBox="0 0 32 16">
  <rect x="0" y="0" width="16" height="16" fill="#ff0000"/>
  <rect x="16" y="0" width="16" height="16" fill="#0000ff"/>
</svg>"##;

    fn percent_encode(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    fn base64_encode(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.iter().fold(0u32, |acc, &b| (acc << 8) | b as u32) << (8 * (3 - chunk.len()));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[test]
    fn regression_ios_svg_data_uri_image_rendered_blank() {
        // Before: the Apple image views only resolved `asset://` and
        // `http(s)://`, and decoded with `UIImage(data:)` which can't read
        // SVG — this source produced an empty slot. Now it decodes to an
        // SVG and rasterizes to real pixels.
        let src = format!("data:image/svg+xml,{}", percent_encode(MARK_SVG));
        let uri = parse_data_uri(&src).expect("percent-encoded data URI parses");
        assert_eq!(uri.mime, "image/svg+xml");
        assert_eq!(uri.bytes, MARK_SVG.as_bytes());

        let Some(DecodedSource::Svg(svg)) = decode_bytes(&uri.bytes) else {
            panic!("SVG bytes must decode as SVG, not be handed to UIImage(data:)");
        };
        assert_eq!(svg.intrinsic_size(), (32.0, 16.0));

        let raster = svg.rasterize(2.0).expect("rasterizes");
        assert_eq!((raster.width_px, raster.height_px), (64, 32));
        assert_eq!(raster.scale, 2.0);
        let px = |x: u32, y: u32| {
            let i = ((y * raster.width_px + x) * 4) as usize;
            &raster.rgba_premultiplied[i..i + 4]
        };
        assert_eq!(px(8, 16), [255, 0, 0, 255], "left half is the red plate");
        assert_eq!(px(56, 16), [0, 0, 255, 255], "right half is the blue plate");
    }

    #[test]
    fn base64_data_uri_decodes_including_percent_escaped_payload() {
        let b64 = base64_encode(MARK_SVG.as_bytes());
        let plain = parse_data_uri(&format!("data:image/svg+xml;base64,{b64}")).unwrap();
        assert_eq!(plain.bytes, MARK_SVG.as_bytes());
        // `+` / `/` / `=` percent-escaped by a URL encoder.
        let escaped = format!("DATA:image/svg+xml;charset=utf-8;base64,{}", percent_encode(&b64));
        assert_eq!(parse_data_uri(&escaped).unwrap().bytes, MARK_SVG.as_bytes());
        // PNG data URIs still go to the platform bitmap decoder.
        let png = parse_data_uri("data:image/png;base64,iVBORw0KGgo=").unwrap();
        assert_eq!(png.mime, "image/png");
        assert!(matches!(decode_bytes(&png.bytes), Some(DecodedSource::Bitmap(_))));
    }

    #[test]
    fn malformed_data_uris_are_rejected() {
        assert_eq!(parse_data_uri("https://example.com/a.svg"), None);
        assert_eq!(parse_data_uri("data:image/svg+xml"), None, "no comma");
        assert_eq!(parse_data_uri("data:,%G1"), None, "bad escape");
        assert_eq!(parse_data_uri("data:;base64,@@@@"), None, "bad base64");
        assert_eq!(parse_data_uri("data:,hi").unwrap().mime, "text/plain");
    }

    #[test]
    fn svg_sniffing() {
        assert!(looks_like_svg(MARK_SVG.as_bytes()));
        assert!(looks_like_svg(b"\xEF\xBB\xBF  <svg xmlns='http://www.w3.org/2000/svg'/>"));
        assert!(looks_like_svg(
            b"<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x\"><svg/>"
        ));
        assert!(!looks_like_svg(b"\x89PNG\r\n\x1a\n"));
        assert!(!looks_like_svg(b"<html><svg/></html>"));
        assert!(!looks_like_svg(b"<!-- unterminated"));
        // Sniffs as SVG but doesn't parse → no decode (caller reports error).
        assert!(decode_bytes(b"<svg").is_none());
    }

    #[test]
    fn classify_src_routes_every_scheme() {
        assert_eq!(classify_src("asset://42"), SrcKind::Asset(42));
        assert_eq!(classify_src("ASSET://7"), SrcKind::Asset(7));
        assert_eq!(classify_src("asset://nope"), SrcKind::Unsupported);
        assert_eq!(classify_src("data:image/png;base64,AA=="), SrcKind::DataUri);
        assert_eq!(classify_src("Data:,x"), SrcKind::DataUri);
        assert_eq!(classify_src("https://x.dev/a.png"), SrcKind::Remote("https://x.dev/a.png"));
        assert_eq!(classify_src("http://x.dev/a.png"), SrcKind::Remote("http://x.dev/a.png"));
        assert_eq!(classify_src(""), SrcKind::Unsupported);
        assert_eq!(classify_src("/local/file.png"), SrcKind::Unsupported);
        assert_eq!(classify_src("ftp://x/a.png"), SrcKind::Unsupported);
    }

    #[test]
    fn raster_scale_tracks_display_size_and_density() {
        let svg = SvgImage::parse(MARK_SVG.as_bytes()).unwrap(); // 32×16
        // Unlaid-out view: one intrinsic point per screen point.
        assert_eq!(svg.raster_scale((0.0, 0.0), 3.0), 3.0);
        // Displayed at its intrinsic size on a 2× screen.
        assert_eq!(svg.raster_scale((32.0, 16.0), 2.0), 2.0);
        // Drawn 4× larger (square box → cover needs the larger ratio).
        assert_eq!(svg.raster_scale((128.0, 128.0), 2.0), 16.0);
        // Never downsamples below intrinsic × screen.
        assert_eq!(svg.raster_scale((8.0, 4.0), 2.0), 2.0);
        // Capped on the longer edge.
        assert_eq!(svg.raster_scale((100_000.0, 50_000.0), 3.0), MAX_SVG_RASTER_EDGE_PX / 32.0);
    }
}
