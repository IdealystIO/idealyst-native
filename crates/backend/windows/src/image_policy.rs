//! Pure decisions behind the Win32 `image()` primitive: how a `src` routes,
//! how bytes decode before GDI+ sees them, how `on_load`/`on_error` are
//! delivered, and how a fetch URL splits for WinHTTP. No Win32 calls, so
//! the regression coverage runs on every host — `tests/image_src.rs`
//! compiles this file directly (`#[path]`), the same arrangement as
//! `pointer_policy.rs`. The GDI+ / WinHTTP half lives in `image.rs` and
//! `image_fetch.rs`.
//!
//! # The bugs this replaced
//!
//! - `http(s)://` resolved to "no image" with no event: a silently blank
//!   box (there was no fetch at all).
//! - `on_load` / `on_error` were never installed (the capability trait's
//!   no-op defaults), so every failure was silent and an author's error
//!   fallback never ran.
//! - `file://` and bare paths were read relative to the working directory
//!   — readable on Windows, rejected on Apple/Android, so the same `src`
//!   meant different things per platform.
//! - Only lower-case, base64 `data:` URIs decoded; the percent-encoded SVG
//!   form (`data:image/svg+xml,%3Csvg…`) was blank, and GDI+ has no SVG or
//!   WebP codec at all.
//!
//! # The contract
//!
//! `docs/image-src.md`; the parser is the shared
//! `runtime_shared::image_src` (re-exported by `backend_image_source`).
//! This module only maps its verdicts onto the Win32 loaders.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Cursor;

use backend_image_source::{
    classify, looks_like_svg, parse_data_uri, warn_rejected, ImageSrc, RejectReason, SvgImage,
};
use runtime_shared::assets::AssetSource;
use runtime_shared::primitives::image::{ImageErrorHandler, ImageLoadEvent, ImageLoadHandler};

// =========================================================================
// Routing
// =========================================================================

/// A registered image asset (`register_asset(id, AssetTag::Image, ..)`),
/// resolved to the loader it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AssetEntry {
    /// `embed_asset!` (and `face!`-style `BundledEmbedded`): bytes in the
    /// binary, decoded from memory.
    Bytes(&'static [u8]),
    /// `asset!("images/logo.png")`: a file the build ships, read through
    /// `runtime_shared::image_src::read_bundled_asset` (`<exe
    /// dir>/assets/<path>`, then `<cwd>/assets/<path>`).
    Bundled(&'static str),
    /// `AssetSource::Remote { url }`: fetched like an `https://` `src`. The
    /// URL is classified at load time, so a non-http(s) one is rejected.
    Remote(&'static str),
}

/// Map an `AssetSource` onto its Win32 loader.
pub(crate) fn asset_entry(source: &AssetSource) -> AssetEntry {
    match *source {
        AssetSource::Embedded { bytes, .. } | AssetSource::BundledEmbedded { bytes, .. } => {
            AssetEntry::Bytes(bytes)
        }
        AssetSource::Bundled { path } => AssetEntry::Bundled(path),
        AssetSource::Remote { url } => AssetEntry::Remote(url),
    }
}

/// Why a load failed — the `on_error` diagnosis, logged in debug builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The contract rejects the `src` itself (relative path, `file:`,
    /// empty, …). Logged through the shared [`warn_rejected`] so the
    /// message matches every other backend.
    Rejected(RejectReason),
    /// A `data:` URI with bad base64 or a bad `%XX` escape.
    MalformedData,
    /// `asset://<id>` with no `register_asset` for that id.
    UnregisteredAsset(u64),
    /// `asset!` path that is unsafe (absolute / `..`) or not shipped.
    MissingBundledAsset(&'static str),
    /// `AssetSource::Remote` whose URL is not `http(s)://`.
    RemoteAssetNotHttp(&'static str),
    /// Bytes no decoder accepts (or an SVG that does not parse).
    Undecodable,
    /// Transport failure (DNS, TLS, connection reset, unsupported URL form).
    Network(String),
    /// A response with a non-2xx status.
    HttpStatus(u32),
}

impl Failure {
    /// Log the failure in debug builds. Release builds stay quiet:
    /// `on_error` is the contract, the log is a development aid.
    pub(crate) fn report(&self, src: &str) {
        if let Failure::Rejected(why) = self {
            warn_rejected(src, *why);
            return;
        }
        #[cfg(debug_assertions)]
        runtime_shared::log_warn!("image(): failed to load src {:?}: {:?}", short(src), self);
    }
}

#[cfg(debug_assertions)]
fn short(src: &str) -> &str {
    // A data: URI can be megabytes; its head identifies it.
    const MAX: usize = 120;
    match src.char_indices().nth(MAX) {
        Some((i, _)) => &src[..i],
        None => src,
    }
}

/// What `create_image` / `update_image_src` do with a `src`.
pub(crate) enum Resolution<'a> {
    /// The node already shows (or is fetching) this exact `src`. The
    /// walker's src binding fires `update_image_src` once at mount with the
    /// value `create_image` already loaded; reloading would decode twice and
    /// fire `on_load` twice. Same guard as iOS/Android.
    Unchanged,
    /// Decoded synchronously — hand to the GDI+ realizer.
    Ready(Decoded<'a>),
    /// Fetch off the UI thread (`image_fetch.rs`).
    Fetch(&'a str),
    /// Fails now: report + `on_error`.
    Failed(Failure),
}

/// Route and (for every synchronous source) decode `src`.
///
/// `current` is the `src` the node holds (`None` at create). `read_bundled`
/// reads an `asset!` path — `runtime_shared::image_src::read_bundled_asset`
/// in the backend, a fixture in tests.
pub(crate) fn resolve<'a>(
    current: Option<&str>,
    src: &'a str,
    assets: &HashMap<u64, AssetEntry>,
    read_bundled: impl Fn(&str) -> Option<Vec<u8>>,
) -> Resolution<'a> {
    if current == Some(src) {
        return Resolution::Unchanged;
    }
    let bytes: Cow<'a, [u8]> = match classify(src) {
        ImageSrc::Rejected(why) => return Resolution::Failed(Failure::Rejected(why)),
        ImageSrc::Remote(url) => return Resolution::Fetch(url),
        ImageSrc::Data(uri) => match parse_data_uri(uri) {
            Some(d) => Cow::Owned(d.bytes),
            None => return Resolution::Failed(Failure::MalformedData),
        },
        ImageSrc::Asset(id) => match assets.get(&id) {
            None => return Resolution::Failed(Failure::UnregisteredAsset(id)),
            Some(AssetEntry::Bytes(b)) => Cow::Borrowed(*b),
            Some(AssetEntry::Bundled(path)) => match read_bundled(path) {
                Some(b) => Cow::Owned(b),
                None => return Resolution::Failed(Failure::MissingBundledAsset(path)),
            },
            Some(AssetEntry::Remote(url)) => match classify(url) {
                ImageSrc::Remote(u) => return Resolution::Fetch(u),
                _ => return Resolution::Failed(Failure::RemoteAssetNotHttp(url)),
            },
        },
    };
    match decode(bytes) {
        Ok(d) => Resolution::Ready(d),
        Err(f) => Resolution::Failed(f),
    }
}

// =========================================================================
// Decoding
// =========================================================================

/// Bytes sorted by who decodes them. GDI+ ships PNG / JPEG / GIF / BMP /
/// TIFF / ICO codecs but none for WebP or SVG, so those two decode here in
/// pure Rust to pixels GDI+ wraps without a codec.
pub(crate) enum Decoded<'a> {
    /// For GDI+ (`GdipLoadImageFromStream`). Whether they really decode is
    /// GDI+'s call; a refusal is reported as [`Failure::Undecodable`].
    Platform(Cow<'a, [u8]>),
    /// WebP, decoded by `image-webp` (first frame of an animation, like
    /// GIF on every backend).
    Pixels(BgraImage),
    /// SVG, rasterized by resvg at the size it is displayed (`image.rs`
    /// re-rasterizes when the box grows).
    Svg(SvgImage),
}

/// Premultiplied BGRA8 pixels, tightly packed (`stride = width * 4`): the
/// memory layout of GDI+'s `PixelFormat32bppPARGB` (little-endian
/// `0xAARRGGBB`), which `GdipCreateBitmapFromScan0` wraps without a copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BgraImage {
    pub width: u32,
    pub height: u32,
    pub bgra_premultiplied: Vec<u8>,
}

/// Sort raw image bytes (any source) to their decoder.
pub(crate) fn decode(bytes: Cow<'_, [u8]>) -> Result<Decoded<'_>, Failure> {
    if looks_like_svg(&bytes) {
        return SvgImage::parse(&bytes).map(Decoded::Svg).ok_or(Failure::Undecodable);
    }
    if looks_like_webp(&bytes) {
        return decode_webp(&bytes).map(Decoded::Pixels).ok_or(Failure::Undecodable);
    }
    Ok(Decoded::Platform(bytes))
}

/// RIFF container whose form type is `WEBP` (RFC 9649 §2.5).
pub(crate) fn looks_like_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// Decode a WebP's first frame to premultiplied BGRA.
pub(crate) fn decode_webp(bytes: &[u8]) -> Option<BgraImage> {
    let mut dec = image_webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
    let (width, height) = dec.dimensions();
    let has_alpha = dec.has_alpha();
    let mut buf = vec![0u8; dec.output_buffer_size()?];
    dec.read_image(&mut buf).ok()?;
    let channels = if has_alpha { 4 } else { 3 };
    let mut out = Vec::with_capacity(width as usize * height as usize * 4);
    for px in buf.chunks_exact(channels) {
        let a = if has_alpha { px[3] } else { 255 };
        out.extend_from_slice(&[premul(px[2], a), premul(px[1], a), premul(px[0], a), a]);
    }
    Some(BgraImage { width, height, bgra_premultiplied: out })
}

/// Straight → premultiplied channel, rounded (`c * a / 255`).
fn premul(c: u8, a: u8) -> u8 {
    ((c as u32 * a as u32 + 127) / 255) as u8
}

/// resvg's premultiplied RGBA → GDI+'s premultiplied BGRA (swap R and B).
pub(crate) fn rgba_premul_to_bgra(width: u32, height: u32, mut rgba: Vec<u8>) -> BgraImage {
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    BgraImage { width, height, bgra_premultiplied: rgba }
}

/// An SVG's natural size for layout and `on_load`: its intrinsic size in
/// points — logical pixels on this backend (see `dpi.rs`).
pub(crate) fn svg_natural(svg: &SvgImage) -> (f32, f32) {
    let (w, h) = svg.intrinsic_size();
    (w as f32, h as f32)
}

// =========================================================================
// on_load / on_error delivery
// =========================================================================

/// How one load ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Outcome {
    Loaded { width: f32, height: f32 },
    Failed,
}

/// A handler call ready to run — returned (never run) by [`ImageEvents`] so
/// the backend can invoke it with no `RefCell` borrow held: author handlers
/// write signals, signal writes run effects, and effects re-enter the
/// backend.
pub(crate) enum Fire {
    Load(ImageLoadHandler, ImageLoadEvent),
    Error(ImageErrorHandler),
}

impl Fire {
    pub(crate) fn call(self) {
        match self {
            Fire::Load(h, ev) => h(&ev),
            Fire::Error(h) => h(),
        }
    }
}

/// Per-image event state: the current load's generation and outcome, the
/// installed handlers, and whether that outcome has been delivered.
///
/// - **Supersession**: every `src` change starts a new generation
///   ([`Self::begin`]); a fetch that completes for an older generation is
///   dropped by [`Self::settle`], so a slow earlier response never
///   replaces a newer `src`.
/// - **Replay**: the walker installs handlers AFTER `create_image`, which
///   has already decoded a `data:` / asset source. The outcome is kept, and
///   [`Self::take_fire`] delivers it to whichever handler is installed when
///   the (deferred) delivery runs.
/// - **Once**: an outcome is delivered at most once per generation —
///   several queued deliveries (one from the load, one per install)
///   collapse to one call.
#[derive(Default)]
pub(crate) struct ImageEvents {
    generation: u64,
    outcome: Option<Outcome>,
    /// The generation whose outcome a handler has received.
    delivered: Option<u64>,
    on_load: Option<ImageLoadHandler>,
    on_error: Option<ImageErrorHandler>,
}

impl ImageEvents {
    /// A new `src` starts loading: supersede whatever was in flight.
    pub(crate) fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.outcome = None;
        self.generation
    }

    /// Record the outcome of load `generation`. `false` (and no change)
    /// when a newer `src` superseded it.
    pub(crate) fn settle(&mut self, generation: u64, outcome: Outcome) -> bool {
        if generation != self.generation {
            return false;
        }
        self.outcome = Some(outcome);
        true
    }

    pub(crate) fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    pub(crate) fn install_load(&mut self, h: ImageLoadHandler) {
        self.on_load = Some(h);
    }

    pub(crate) fn install_error(&mut self, h: ImageErrorHandler) {
        self.on_error = Some(h);
    }

    /// The handler call the current outcome owes, if any: none while
    /// pending, none once delivered, none when the matching handler is not
    /// installed (yet — a later install still receives it).
    pub(crate) fn take_fire(&mut self) -> Option<Fire> {
        if self.delivered == Some(self.generation) {
            return None;
        }
        let fire = match self.outcome? {
            Outcome::Loaded { width, height } => {
                Fire::Load(self.on_load.clone()?, ImageLoadEvent { width, height })
            }
            Outcome::Failed => Fire::Error(self.on_error.clone()?),
        };
        self.delivered = Some(self.generation);
        Some(fire)
    }
}

// =========================================================================
// HTTP
// =========================================================================

/// An `http(s)` URL split into what `WinHttpConnect` / `WinHttpOpenRequest`
/// take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HttpTarget {
    pub secure: bool,
    /// Host name or IP literal (IPv6 without its brackets).
    pub host: String,
    pub port: u16,
    /// Path plus query (`/a/b.png?v=2`), never empty; fragment dropped.
    pub path: String,
}

/// Split an absolute `http(s)` URL. `None` for anything WinHTTP can't be
/// handed as-is: another scheme, an empty host, a bad port, or user info
/// (`user:pass@host`) — `image()` sends no credentials on any platform
/// (`docs/image-src.md`, "Not a `src`"), so that is a load error.
pub(crate) fn split_http_url(url: &str) -> Option<HttpTarget> {
    let url = url.trim();
    let scheme_end = url.find("://")?;
    let secure = match &url[..scheme_end] {
        s if s.eq_ignore_ascii_case("https") => true,
        s if s.eq_ignore_ascii_case("http") => false,
        _ => return None,
    };
    let rest = &url[scheme_end + 3..];
    let rest = rest.split('#').next().unwrap_or("");
    let auth_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(auth_end);
    if authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let close = v6.find(']')?;
        let after = &v6[close + 1..];
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p),
            None if after.is_empty() => None,
            None => return None,
        };
        (&v6[..close], port)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return None;
    }
    let port = match port {
        Some(p) => p.parse().ok().filter(|p| *p != 0)?,
        None if secure => 443,
        None => 80,
    };
    let path = match tail {
        "" => "/".to_string(),
        t if t.starts_with('?') => format!("/{t}"),
        t => t.to_string(),
    };
    Some(HttpTarget { secure, host: host.to_string(), port, path })
}

/// The statuses that count as a successful image response (2xx). WinHTTP
/// follows redirects itself, so a 3xx only reaches here when it could not
/// be followed — a failure like any other non-2xx.
pub(crate) fn status_is_success(status: u32) -> bool {
    (200..300).contains(&status)
}
