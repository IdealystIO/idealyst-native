//! The `image()` `src` contract — one set of rules every backend applies.
//!
//! Without one contract an `image()` src means something different per
//! backend (a relative `/attachments/<id>` rendering on web and drawing a
//! blank tile on iOS with no error). The full contract is `docs/image-src.md`; this module
//! is its parser, shared by every backend (web included — it is pure Rust and
//! tiny, unlike the decoders in `backend-image-source`).
//!
//! | `src`                                   | [`ImageSrc`]     |
//! |-----------------------------------------|------------------|
//! | `http://…`, `https://…`                 | `Remote`         |
//! | `data:…` (base64 or percent-encoded)    | `Data`           |
//! | `asset://<id>` (what `image_asset` sets) | `Asset`          |
//! | anything else                           | `Rejected`       |
//!
//! Scheme matching is case-insensitive, like a browser's. A rejected `src`
//! is a load error on every backend: `on_error` fires and [`warn_rejected`]
//! logs why.
//!
//! A root-relative `src` (`/uploads/x.png`) is resolved before it is
//! classified: the `image` mount handler passes every `src` through
//! [`with_base`], which prefixes the base set by [`set_base_url`]
//! (`server::configure` sets it). With no base, the path stays relative;
//! backends that run in a page (web, SSR, wgpu in the browser) hand it to
//! the browser, which resolves it against the page origin, and every other
//! backend rejects it.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::{Mutex, RwLock};

/// The sentinel `image_asset(..)` hands a backend: `asset://{id}`, after
/// `register_asset(id, AssetTag::Image, ..)`.
pub const ASSET_URL_PREFIX: &str = "asset://";

/// Which loading path an `image()` `src` takes. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSrc<'a> {
    /// `http(s)://…` — fetch asynchronously, off the UI thread. Carries the
    /// src unchanged.
    Remote(&'a str),
    /// `data:` URI — decode in place with [`parse_data_uri`]. Carries the
    /// src unchanged (parsing allocates, so it is the caller's call).
    Data(&'a str),
    /// `asset://<id>` — the asset registered under that id.
    Asset(u64),
    /// Not a form the contract accepts. Report it as a load error.
    Rejected(RejectReason),
}

/// Why a `src` was rejected — the text of the warning and the
/// `on_error` diagnosis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// `""`.
    Empty,
    /// `/x.png`, `x.png`, `./x.png`, `../x.png`: no scheme. A root-relative
    /// path only reaches [`classify`] unresolved when no base is set (see
    /// the module docs); a document-relative one is rejected everywhere,
    /// because on web it would resolve against the current route.
    Relative,
    /// `//host/x.png`: scheme-relative, same reason.
    SchemeRelative,
    /// `asset://` with an id that is not a number.
    BadAssetId,
    /// `file:`, `blob:`, `ftp:`, `content:`, … — a scheme only some
    /// platforms can read.
    UnsupportedScheme,
}

impl RejectReason {
    /// What an author should do instead, for the warning.
    pub fn explain(self) -> &'static str {
        match self {
            RejectReason::Empty => "the src is empty",
            RejectReason::Relative => {
                "a relative path needs a base URL. A root-relative /path resolves against \
                 the base set by server::configure(..) or set_image_base_url(..) (and, in a \
                 browser, against the page); none is set here. A document-relative path \
                 (logo.png, ./x.png) never resolves. For a file shipped with the app use \
                 asset!(..)"
            }
            RejectReason::SchemeRelative => {
                "a scheme-relative //host URL has no scheme on native platforms; \
                 spell out https://"
            }
            RejectReason::BadAssetId => "asset:// must carry a numeric asset id (set by image_asset)",
            RejectReason::UnsupportedScheme => {
                "only https://, http://, data: and image_asset(..) load on every platform"
            }
        }
    }
}

/// Classify an `image()` `src`. See the module docs.
pub fn classify(src: &str) -> ImageSrc<'_> {
    let trimmed = src.trim();
    if trimmed.is_empty() {
        return ImageSrc::Rejected(RejectReason::Empty);
    }
    if let Some(rest) = strip_prefix_ignore_case(trimmed, ASSET_URL_PREFIX) {
        return rest
            .parse()
            .map(ImageSrc::Asset)
            .unwrap_or(ImageSrc::Rejected(RejectReason::BadAssetId));
    }
    if strip_prefix_ignore_case(trimmed, "data:").is_some() {
        return ImageSrc::Data(src);
    }
    if strip_prefix_ignore_case(trimmed, "http://").is_some()
        || strip_prefix_ignore_case(trimmed, "https://").is_some()
    {
        return ImageSrc::Remote(src);
    }
    if trimmed.starts_with("//") {
        return ImageSrc::Rejected(RejectReason::SchemeRelative);
    }
    if has_scheme(trimmed) {
        return ImageSrc::Rejected(RejectReason::UnsupportedScheme);
    }
    ImageSrc::Rejected(RejectReason::Relative)
}

/// RFC 3986 scheme: `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`. A
/// Windows drive path (`C:\x.png`) reads as a one-letter scheme and is
/// rejected either way.
fn has_scheme(s: &str) -> bool {
    let Some(colon) = s.find(':') else { return false };
    let scheme = &s[..colon];
    let mut chars = scheme.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// The base a root-relative `src` resolves against. Process-wide, like the
/// `server` client config that sets it.
static BASE_URL: RwLock<Option<String>> = RwLock::new(None);

/// Set the base URL a root-relative `image()` src (`/uploads/x.png`)
/// resolves against, e.g. `https://api.example.com`. `server::configure`
/// calls this with its base URL, so an app that configures a server needs
/// nothing more. Only an absolute `http(s)://` base is kept: `""` (the
/// usual same-origin web config) or any other value clears it, and the
/// browser then resolves the path against the page.
pub fn set_base_url(base: &str) {
    let base = base.trim().trim_end_matches('/');
    let keep = matches!(classify(base), ImageSrc::Remote(_)).then(|| base.to_string());
    *BASE_URL.write().unwrap_or_else(|e| e.into_inner()) = keep;
}

/// The base set by [`set_base_url`], if any.
pub fn base_url() -> Option<String> {
    BASE_URL.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// `true` for a root-relative path: `/x.png`, but not the scheme-relative
/// `//host/x.png`, nor `/\host/x.png`, which browsers also read as
/// scheme-relative.
pub fn is_root_relative(src: &str) -> bool {
    let t = src.trim();
    t.starts_with('/') && !matches!(t.as_bytes().get(1), Some(b'/' | b'\\'))
}

/// `src` resolved against `base`: a root-relative path is appended to the
/// base (an absolute `http(s)://` URL, without a trailing `/`); anything
/// else, or no base, returns `src` unchanged. Appending rather than RFC 3986
/// resolution keeps a base's path (`https://x.io/api` + `/files/1` →
/// `https://x.io/api/files/1`), the same join `server::base_url()`'s docs
/// show for links.
pub fn join_base<'a>(src: &'a str, base: Option<&str>) -> Cow<'a, str> {
    match base {
        Some(base) if is_root_relative(src) => Cow::Owned(format!("{base}{}", src.trim())),
        _ => Cow::Borrowed(src),
    }
}

/// [`join_base`] against the base set by [`set_base_url`]. The `image`
/// mount handler applies this to every `src` before a backend sees it.
pub fn with_base(src: &str) -> Cow<'_, str> {
    if !is_root_relative(src) {
        return Cow::Borrowed(src);
    }
    let base = BASE_URL.read().unwrap_or_else(|e| e.into_inner());
    match join_base(src, base.as_deref()) {
        Cow::Owned(s) => Cow::Owned(s),
        Cow::Borrowed(_) => Cow::Borrowed(src),
    }
}

/// How many distinct rejected srcs [`warn_rejected`] remembers. A src that
/// comes from data (a feed of thumbnails) can be different every time, so
/// the set is capped; past it, rejections stop being logged.
const MAX_WARNED_SRCS: usize = 256;

/// The srcs [`warn_rejected`] has logged, and whether it hit the cap.
static WARNED: Mutex<(BTreeSet<String>, bool)> = Mutex::new((BTreeSet::new(), false));

/// Log why a `src` was rejected, once per distinct src, in release builds
/// too: a rejected image is a broken image in production, and most apps
/// don't wire `on_error`. Every backend calls this on the rejection path,
/// next to firing `on_error`, so the message is the same everywhere.
pub fn warn_rejected(src: &str, reason: RejectReason) {
    if let Some(line) = rejection_line(src, reason) {
        crate::log_warn!("{line}");
    }
}

/// The line [`warn_rejected`] logs, or `None` when it stays quiet.
fn rejection_line(src: &str, reason: RejectReason) -> Option<String> {
    let shown = truncate(src);
    let mut warned = WARNED.lock().unwrap_or_else(|e| e.into_inner());
    let (seen, capped) = &mut *warned;
    if *capped || seen.contains(shown) {
        return None;
    }
    if seen.len() == MAX_WARNED_SRCS {
        *capped = true;
        return Some(format!("image(): {MAX_WARNED_SRCS} rejected srcs logged; not logging more"));
    }
    seen.insert(shown.to_string());
    Some(format!("image(): rejected src {:?}: {}", shown, reason.explain()))
}

fn truncate(src: &str) -> &str {
    // A data: URI can be megabytes; the scheme and a little of the payload
    // identify it.
    const MAX: usize = 120;
    match src.char_indices().nth(MAX) {
        Some((i, _)) => &src[..i],
        None => src,
    }
}

/// Where a desktop or server process looks for a bundled asset
/// (`asset!("images/logo.png")`), in order: `<exe dir>/assets/<path>` (what
/// the build tool ships next to the binary), then `<cwd>/assets/<path>` (a
/// `cargo run` from the project root). Linux, Windows, the wgpu renderer and
/// the email/SSR renderers all resolve through this, so a bundled image is
/// found the same way everywhere a filesystem exists. Mobile bundles have
/// their own lookup (iOS/macOS `NSBundle` resources, Android `AssetManager`),
/// at the same `assets/<path>` location inside the bundle.
///
/// `None` for a path that could escape the assets directory (absolute, or
/// with a `..` component): an asset path names a file the build shipped,
/// nothing else.
#[cfg(not(target_arch = "wasm32"))]
pub fn bundled_asset_candidates(path: &str) -> Option<Vec<std::path::PathBuf>> {
    use std::path::{Component, Path};
    let rel = Path::new(path);
    if path.is_empty()
        || rel.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    let mut out = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        out.push(dir.join("assets").join(rel));
    }
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.join("assets").join(rel));
    }
    Some(out)
}

/// Read a bundled asset's bytes through [`bundled_asset_candidates`].
#[cfg(not(target_arch = "wasm32"))]
pub fn read_bundled_asset(path: &str) -> Option<Vec<u8>> {
    read_first(&bundled_asset_candidates(path)?)
}

#[cfg(not(target_arch = "wasm32"))]
fn read_first(candidates: &[std::path::PathBuf]) -> Option<Vec<u8>> {
    candidates.iter().find_map(|p| std::fs::read(p).ok())
}

/// A decoded `data:` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataUri {
    /// Lower-cased media type without parameters (`image/svg+xml`), or
    /// `text/plain` when the URI omits it (RFC 2397's default).
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Parse a `data:[<mediatype>][;base64],<data>` URI (RFC 2397). Returns
/// `None` for a non-`data:` string or a malformed payload (bad base64, bad
/// `%XX`).
pub fn parse_data_uri(src: &str) -> Option<DataUri> {
    let rest = strip_prefix_ignore_case(src.trim(), "data:")?;
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
///
/// Lives here (not only in `backend-image-source`) because the wgpu
/// renderer on the web needs the same rule without linking resvg: it hands
/// SVG to the browser, which only renders an SVG blob typed
/// `image/svg+xml` (browsers never sniff SVG).
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

/// Pixels per intrinsic point an SVG of `intrinsic` size needs for a view
/// whose bounds are `bounds` points on a `screen_scale` display: enough that
/// neither axis is upscaled under any `ObjectFit` (cover fills the larger
/// ratio), and never less than one intrinsic point per screen pixel group,
/// so an unlaid-out (0×0) view still gets a usable first raster. Capped at
/// [`MAX_SVG_RASTER_EDGE_PX`] on the longer edge.
///
/// One rule for every SVG rasterizer (resvg on native, the browser's
/// canvas under the wgpu renderer on the web), so an SVG re-rasterizes at
/// the same densities everywhere.
pub fn svg_raster_scale(intrinsic: (f64, f64), bounds: (f64, f64), screen_scale: f64) -> f64 {
    let (iw, ih) = intrinsic;
    let screen_scale = if screen_scale > 0.0 { screen_scale } else { 1.0 };
    let fit = (bounds.0 / iw).max(bounds.1 / ih);
    let fit = if fit.is_finite() && fit > 1.0 { fit } else { 1.0 };
    let scale = fit * screen_scale;
    let cap = MAX_SVG_RASTER_EDGE_PX / iw.max(ih);
    scale.min(cap)
}

/// The cross-backend conformance table: every backend's tests run each
/// vector through its real src path and assert the outcome, so "the same
/// `src` behaves the same everywhere" is checked per backend, not assumed.
pub mod vectors {
    /// What a backend must do with a vector's `src`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Expect {
        /// Decodes to an image of this natural size (`on_load`).
        Loads { width: u32, height: u32 },
        /// Fails: `on_error` fires.
        Errors,
    }

    /// 1×1 opaque red PNG.
    pub const PNG_1X1_BASE64: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";
    /// 2×1 SVG, percent-encoded (the form CrewForge's `BrandLockup` builds,
    /// and the website's primitives page uses).
    pub const SVG_2X1_PERCENT: &str = "data:image/svg+xml,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%222%22%20height%3D%221%22%3E%3Crect%20width%3D%222%22%20height%3D%221%22%20fill%3D%22red%22%2F%3E%3C%2Fsvg%3E";
    /// Same PNG with an upper-case scheme — matching is case-insensitive.
    pub const PNG_1X1_UPPER_SCHEME: &str = "DATA:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

    /// `(src, expected)` for the synchronous half of the contract. Remote
    /// URLs and assets are covered by each backend's own fixtures (they need
    /// a server or a registered asset). A root-relative path is not here:
    /// with no base it loads in a page and fails elsewhere, so each backend
    /// tests its own outcome.
    pub const SYNC: &[(&str, Expect)] = &[
        (PNG_1X1_BASE64, Expect::Loads { width: 1, height: 1 }),
        (PNG_1X1_UPPER_SCHEME, Expect::Loads { width: 1, height: 1 }),
        (SVG_2X1_PERCENT, Expect::Loads { width: 2, height: 1 }),
        ("data:image/png;base64,@@not-base64@@", Expect::Errors),
        ("", Expect::Errors),
        ("logo.png", Expect::Errors),
        ("./logo.png", Expect::Errors),
        ("//cdn.example.com/x.png", Expect::Errors),
        ("file:///tmp/x.png", Expect::Errors),
        ("blob:https://example.com/0f2c", Expect::Errors),
        ("ftp://example.com/x.png", Expect::Errors),
        ("asset://not-a-number", Expect::Errors),
        ("asset://424242", Expect::Errors), // never registered
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_sniffing() {
        assert!(looks_like_svg(
            b"<?xml version=\"1.0\"?>\n<!-- mark -->\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>"
        ));
        assert!(looks_like_svg(b"\xEF\xBB\xBF  <svg xmlns='http://www.w3.org/2000/svg'/>"));
        assert!(looks_like_svg(b"<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x\"><svg/>"));
        assert!(!looks_like_svg(b"\x89PNG\r\n\x1a\n"));
        assert!(!looks_like_svg(b"<html><svg/></html>"));
        assert!(!looks_like_svg(b"<!-- unterminated"));
        let svg = parse_data_uri(vectors::SVG_2X1_PERCENT).unwrap();
        assert!(looks_like_svg(&svg.bytes));
        let png = parse_data_uri(vectors::PNG_1X1_BASE64).unwrap();
        assert!(!looks_like_svg(&png.bytes));
    }

    #[test]
    fn svg_raster_scale_tracks_display_size_and_density() {
        let i = (32.0, 16.0);
        // Unlaid-out view: one intrinsic point per screen point.
        assert_eq!(svg_raster_scale(i, (0.0, 0.0), 3.0), 3.0);
        // Displayed at its intrinsic size on a 2× screen.
        assert_eq!(svg_raster_scale(i, (32.0, 16.0), 2.0), 2.0);
        // Drawn 4× larger (square box → cover needs the larger ratio).
        assert_eq!(svg_raster_scale(i, (128.0, 128.0), 2.0), 16.0);
        // Never downsamples below intrinsic × screen.
        assert_eq!(svg_raster_scale(i, (8.0, 4.0), 2.0), 2.0);
        // A non-positive screen scale counts as 1×.
        assert_eq!(svg_raster_scale(i, (32.0, 16.0), 0.0), 1.0);
        // Capped on the longer edge.
        assert_eq!(svg_raster_scale(i, (100_000.0, 50_000.0), 3.0), MAX_SVG_RASTER_EDGE_PX / 32.0);
    }

    #[test]
    fn accepted_forms() {
        assert_eq!(classify("https://x.io/a.png"), ImageSrc::Remote("https://x.io/a.png"));
        assert_eq!(classify("HTTP://x.io/a.png"), ImageSrc::Remote("HTTP://x.io/a.png"));
        assert_eq!(classify("data:image/png;base64,AA=="), ImageSrc::Data("data:image/png;base64,AA=="));
        assert_eq!(classify("Asset://42"), ImageSrc::Asset(42));
    }

    /// The CrewForge kiosk's `/attachments/<id>`: a backend never sees it
    /// unresolved when a base is set, and `classify` rejects it when none is,
    /// so native never fetches it against nothing.
    #[test]
    fn regression_relative_attachment_path_is_rejected() {
        assert_eq!(classify("/attachments/abc"), ImageSrc::Rejected(RejectReason::Relative));
    }

    /// Since the contract landed, `/assets/logo.svg` was rejected on web
    /// with no way to make it load through `server::base_url()` (often
    /// `""` on same-origin web). A root-relative path now joins an absolute
    /// base, and passes through unchanged without one.
    #[test]
    fn regression_root_relative_src_joins_the_base() {
        let base = Some("https://api.example.com");
        assert_eq!(join_base("/assets/logo.svg", base), "https://api.example.com/assets/logo.svg");
        assert_eq!(join_base(" /uploads/x.png", base), "https://api.example.com/uploads/x.png");
        assert_eq!(join_base("/files/1", Some("https://x.io/api")), "https://x.io/api/files/1");
        assert_eq!(join_base("/assets/logo.svg", None), "/assets/logo.svg");
        // Only root-relative paths join.
        for src in ["logo.png", "./x.png", "../x.png", "//cdn.io/x.png", "/\\cdn.io/x.png", "https://a.io/x.png", "data:,x"] {
            assert_eq!(join_base(src, base), src, "{src:?}");
        }
    }

    #[test]
    fn root_relative_excludes_scheme_relative_forms() {
        assert!(is_root_relative("/a.png"));
        assert!(is_root_relative("  /a.png"));
        assert!(is_root_relative("/"));
        assert!(!is_root_relative("//cdn.io/a.png"));
        assert!(!is_root_relative("/\\cdn.io/a.png"));
        assert!(!is_root_relative("a.png"));
        assert!(!is_root_relative(""));
    }

    /// The process-wide base: only an absolute http(s) base is kept, a
    /// trailing `/` is dropped, and `""` (same-origin web) clears it. One
    /// test owns the global, so parallel tests can't race it.
    #[test]
    fn set_base_url_keeps_only_absolute_bases() {
        set_base_url("https://api.example.com/");
        assert_eq!(base_url().as_deref(), Some("https://api.example.com"));
        assert_eq!(with_base("/u/x.png"), "https://api.example.com/u/x.png");
        assert_eq!(with_base("logo.png"), "logo.png");
        for cleared in ["", "/api", "api.example.com", "ftp://x.io"] {
            set_base_url("https://api.example.com");
            set_base_url(cleared);
            assert_eq!(base_url(), None, "{cleared:?}");
            assert_eq!(with_base("/u/x.png"), "/u/x.png");
        }
    }

    /// A rejected image used to log only in debug builds, so a release
    /// build showed broken images with nothing in the console. It logs in
    /// every build, once per distinct src.
    #[test]
    fn regression_rejection_is_logged_once_per_src_in_every_build() {
        let src = "/regression-once-per-src.png";
        let line = rejection_line(src, RejectReason::Relative).expect("first rejection logs");
        assert!(line.contains(src) && line.contains("server::configure"), "{line}");
        assert_eq!(rejection_line(src, RejectReason::Relative), None);
        assert!(rejection_line("/regression-once-per-src-2.png", RejectReason::Relative).is_some());
    }

    #[test]
    fn rejected_forms() {
        use RejectReason::*;
        for (src, why) in [
            ("", Empty),
            ("   ", Empty),
            ("logo.png", Relative),
            ("./a/b.png", Relative),
            ("../b.png", Relative),
            ("//cdn.io/x.png", SchemeRelative),
            ("file:///x.png", UnsupportedScheme),
            ("blob:https://a.io/1", UnsupportedScheme),
            ("ftp://a.io/x.png", UnsupportedScheme),
            ("content://media/1", UnsupportedScheme),
            ("C:\\img\\x.png", UnsupportedScheme),
            ("asset://abc", BadAssetId),
            ("asset://", BadAssetId),
        ] {
            assert_eq!(classify(src), ImageSrc::Rejected(why), "{src:?}");
        }
    }

    #[test]
    fn vectors_agree_with_classify() {
        // Every vector expected to load is a data: URI (the sync half), and
        // every Errors vector except the bad-payload and unregistered-asset
        // ones is rejected by classify itself.
        for (src, expect) in vectors::SYNC {
            match (classify(src), expect) {
                (ImageSrc::Data(_), vectors::Expect::Loads { .. }) => {
                    assert!(parse_data_uri(src).is_some(), "{src}")
                }
                (ImageSrc::Data(_), vectors::Expect::Errors) => {
                    assert!(parse_data_uri(src).is_none(), "{src}")
                }
                (ImageSrc::Asset(_), vectors::Expect::Errors) => {}
                (ImageSrc::Rejected(_), vectors::Expect::Errors) => {}
                other => panic!("{src:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn bundled_asset_paths_cannot_escape_the_assets_dir() {
        assert!(bundled_asset_candidates("../secret.png").is_none());
        assert!(bundled_asset_candidates("images/../../x").is_none());
        assert!(bundled_asset_candidates("/etc/passwd").is_none());
        assert!(bundled_asset_candidates("").is_none());
        let c = bundled_asset_candidates("images/logo.png").unwrap();
        assert!(c.iter().all(|p| p.ends_with("assets/images/logo.png")));
    }

    /// First existing candidate wins (exe dir before cwd); missing ones are
    /// skipped. Exercised on explicit paths: changing the process cwd in a
    /// test would race every other test thread.
    #[test]
    fn read_bundled_asset_takes_the_first_existing_candidate() {
        let dir = std::env::temp_dir().join(format!("img-src-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("second.png"), b"second").unwrap();
        let got = read_first(&[dir.join("missing.png"), dir.join("second.png")]);
        assert_eq!(got.as_deref(), Some(&b"second"[..]));
        assert_eq!(read_first(&[dir.join("missing.png")]), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn data_uri_base64_and_percent() {
        let png = parse_data_uri(vectors::PNG_1X1_BASE64).unwrap();
        assert_eq!(png.mime, "image/png");
        assert_eq!(&png.bytes[1..4], b"PNG");
        let svg = parse_data_uri(vectors::SVG_2X1_PERCENT).unwrap();
        assert_eq!(svg.mime, "image/svg+xml");
        assert!(svg.bytes.starts_with(b"<svg"));
        assert_eq!(parse_data_uri("data:,hi").unwrap().mime, "text/plain");
        assert!(parse_data_uri("data:image/png;base64,%ZZ").is_none());
        assert!(parse_data_uri("data:nocomma").is_none());
    }
}
