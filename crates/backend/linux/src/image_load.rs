//! The GTK-free half of `image()` loading: which path a `src` takes, how its
//! bytes decode, and how a remote fetch reports failure. The contract is
//! `docs/image-src.md`; `runtime_shared::image_src` is its parser. Everything
//! here is pure Rust, so the policy is tested without a display — `image.rs`
//! only turns a [`Decoded`] into a GDK paintable.
//!
//! ## Why the backend decodes instead of GdkPixbuf
//!
//! `gdk::Texture::from_bytes` hands bytes to GdkPixbuf, whose format loaders
//! are separate distro packages: WebP needs `webp-pixbuf-loader` (absent on a
//! stock Debian/Ubuntu/Fedora install) and SVG needs librsvg's loader. The
//! contract promises PNG, JPEG, GIF, WebP and SVG on every backend, so the
//! same `src` must not decode on one Linux box and fail on the next. Rasters
//! decode through the `image` crate, SVG through `resvg`
//! (`backend-image-source`, shared with iOS/macOS/Android) — both compiled
//! in, so the result is the same on every host.
//!
//! ## Why remote loads use an HTTP client instead of GIO
//!
//! The previous path, `gio::File::load_bytes_async` on an `http(s)` URI,
//! needs a GVfs http backend (missing on minimal installs, so remote images
//! silently failed) and exposes no status code: a 404 error page came back
//! as "bytes", and a server that answers 404 with a placeholder PNG would
//! have rendered it as a successful load. The contract says a non-2xx is an
//! error, so the fetch has to see the status.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use backend_image_source::{classify, looks_like_svg, parse_data_uri, ImageSrc, RejectReason, SvgImage};
use runtime_shared::assets::AssetSource;

/// Where an `image()` `src` loads from, after classification and asset
/// lookup.
pub(crate) enum Route<'a> {
    /// Fetch this `http(s)` URL off the UI thread.
    Remote(Cow<'a, str>),
    /// Decode these bytes in place (a `data:` payload or an asset's bytes).
    Bytes(Cow<'a, [u8]>),
    /// Nothing to load: report `on_error`.
    Fail(Failure),
}

/// Why a `src` produced no bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The contract rejects the form outright (relative path, `file:`, …).
    Rejected(RejectReason),
    /// A `data:` URI whose payload is malformed (bad base64 / `%XX`).
    BadDataUri,
    /// `asset://<id>` for an id never registered as an image asset.
    UnregisteredAsset(u64),
    /// An `asset!` file not found in any bundled-asset location.
    BundledAssetMissing,
}

impl Failure {
    /// Log the failure. A rejected form goes through the shared
    /// [`backend_image_source::warn_rejected`] (every build) so the message
    /// is the same on every backend; the asset failures are this backend's
    /// own diagnosis, in debug builds (the payload failures are visible as
    /// `on_error` alone).
    pub(crate) fn warn(self, src: &str) {
        match self {
            Failure::Rejected(reason) => backend_image_source::warn_rejected(src, reason),
            #[cfg(debug_assertions)]
            Failure::UnregisteredAsset(id) => runtime_shared::log_warn!(
                "image(): asset {id} is not registered as an image asset"
            ),
            #[cfg(debug_assertions)]
            Failure::BundledAssetMissing => runtime_shared::log_warn!(
                "image(): bundled asset for {src:?} not found under <exe dir>/assets or <cwd>/assets"
            ),
            _ => {}
        }
    }
}

/// Route `src` per the contract. `assets` is the backend's image-asset
/// table (`register_asset(.., AssetTag::Image, ..)`).
pub(crate) fn route<'a>(src: &'a str, assets: &HashMap<u64, AssetSource>) -> Route<'a> {
    match classify(src) {
        ImageSrc::Remote(url) => Route::Remote(Cow::Borrowed(url)),
        ImageSrc::Data(uri) => match parse_data_uri(uri) {
            Some(d) => Route::Bytes(Cow::Owned(d.bytes)),
            None => Route::Fail(Failure::BadDataUri),
        },
        ImageSrc::Asset(id) => match assets.get(&id) {
            None => Route::Fail(Failure::UnregisteredAsset(id)),
            Some(AssetSource::Embedded { bytes, .. })
            | Some(AssetSource::BundledEmbedded { bytes, .. }) => Route::Bytes(Cow::Borrowed(bytes)),
            Some(AssetSource::Bundled { path }) => {
                match runtime_shared::image_src::read_bundled_asset(path) {
                    Some(bytes) => Route::Bytes(Cow::Owned(bytes)),
                    None => Route::Fail(Failure::BundledAssetMissing),
                }
            }
            // A remote asset is a URL like any other — but it must still be
            // one the contract accepts (`http(s)`), not e.g. a `file:` URL
            // smuggled in through an asset declaration.
            Some(AssetSource::Remote { url }) => match classify(url) {
                ImageSrc::Remote(u) => Route::Remote(Cow::Borrowed(u)),
                ImageSrc::Rejected(reason) => Route::Fail(Failure::Rejected(reason)),
                _ => Route::Fail(Failure::Rejected(RejectReason::UnsupportedScheme)),
            },
        },
        ImageSrc::Rejected(reason) => Route::Fail(Failure::Rejected(reason)),
    }
}

/// A decoded raster: straight (non-premultiplied) RGBA8, tightly packed.
pub(crate) struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// What bytes decode to, ready for the GTK side to wrap.
pub(crate) enum Decoded {
    Raster(Raster),
    /// Rasterized lazily at the size it is drawn (see `image::SvgPaintable`).
    Svg(SvgImage),
}

impl Decoded {
    /// Natural size reported to `on_load`: pixels for a raster, the
    /// document's intrinsic size for SVG (a vector has no pixel size).
    pub(crate) fn natural_size(&self) -> (f32, f32) {
        match self {
            Decoded::Raster(r) => (r.width as f32, r.height as f32),
            Decoded::Svg(s) => {
                let (w, h) = s.intrinsic_size();
                (w as f32, h as f32)
            }
        }
    }
}

/// Decode PNG / JPEG / GIF (first frame) / WebP via the `image` crate.
fn decode_raster(bytes: &[u8]) -> Option<Raster> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(Raster { width, height, rgba: img.into_raw() })
}

/// Decode bytes on the UI thread (a `data:` payload, an asset).
pub(crate) fn decode(bytes: &[u8]) -> Option<Decoded> {
    if looks_like_svg(bytes) {
        SvgImage::parse(bytes).map(Decoded::Svg)
    } else {
        decode_raster(bytes).map(Decoded::Raster)
    }
}

/// The half of decoding that can run on a worker thread. A raster is fully
/// decoded there (the expensive part of a large photo); SVG stays as bytes
/// and is parsed on the UI thread by [`Prepared::finish`], because
/// `usvg::Tree` makes no `Send` promise.
pub(crate) enum Prepared {
    Raster(Raster),
    Svg(Vec<u8>),
}

impl Prepared {
    pub(crate) fn new(bytes: Vec<u8>) -> Option<Self> {
        if looks_like_svg(&bytes) {
            Some(Prepared::Svg(bytes))
        } else {
            decode_raster(&bytes).map(Prepared::Raster)
        }
    }

    pub(crate) fn finish(self) -> Option<Decoded> {
        match self {
            Prepared::Raster(r) => Some(Decoded::Raster(r)),
            Prepared::Svg(bytes) => SvgImage::parse(&bytes).map(Decoded::Svg),
        }
    }
}

/// Why a remote fetch failed.
#[derive(Debug)]
pub(crate) enum FetchError {
    /// Connection, TLS, timeout, or body-read failure.
    Transport(String),
    /// The server answered with a non-2xx status (after redirects).
    Status(u16),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Transport(e) => write!(f, "{e}"),
            FetchError::Status(code) => write!(f, "HTTP status {code}"),
        }
    }
}

/// How long one image request may take end to end before it is reported as
/// an error. Without a bound, a server that accepts the connection and never
/// answers would leave the image `Pending` forever and `on_error` would
/// never run; 30 s matches the default request timeout of the platform HTTP
/// stacks the other native backends use (`NSURLSession`'s
/// `timeoutIntervalForRequest` is 60 s idle; browsers have no total bound),
/// erring short so a hung load surfaces while the user is still looking.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// One shared client: reqwest's blocking client owns a background runtime
/// thread, which should be paid for once per process, not per image.
fn client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .build()
            .expect("reqwest blocking client builds with rustls (no system TLS needed)")
    })
}

/// Fetch `url`. BLOCKING — call from a worker thread. A non-2xx status is
/// an error even when the body is a decodable image (see module docs).
pub(crate) fn fetch(url: &str) -> Result<Vec<u8>, FetchError> {
    let resp = client().get(url).send().map_err(|e| FetchError::Transport(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(FetchError::Status(status.as_u16()));
    }
    resp.bytes()
        .map(|b| b.to_vec())
        .map_err(|e| FetchError::Transport(e.to_string()))
}

/// Fetch and prepare off the UI thread. `None` = the load failed (transport,
/// status, or undecodable bytes); debug builds log which.
pub(crate) fn fetch_and_prepare(url: &str) -> Option<Prepared> {
    match fetch(url) {
        Ok(bytes) => Prepared::new(bytes),
        Err(_e) => {
            #[cfg(debug_assertions)]
            runtime_shared::log_warn!("image(): fetching {url:?} failed: {_e}");
            None
        }
    }
}

/// Per-image-node load bookkeeping that decides whether a `src` write
/// starts a load and whether a finished load is still wanted.
#[derive(Default)]
pub(crate) struct LoadSlot {
    /// The `src` the node currently shows (or is loading).
    src: Option<String>,
    /// Bumped by every load. An async completion carries the generation it
    /// started under and is discarded unless it still matches — so a slow
    /// earlier response never replaces a newer `src`.
    generation: u64,
}

impl LoadSlot {
    /// Start loading `src`, returning the load's generation — or `None` when
    /// `src` is what the node already shows. The vocabulary writes the same
    /// `src` back once right after mount (its first reactive pass); treating
    /// that as a new load re-decoded the image and fired `on_load` twice.
    pub(crate) fn begin(&mut self, src: &str) -> Option<u64> {
        if self.src.as_deref() == Some(src) {
            return None;
        }
        self.src = Some(src.to_string());
        self.generation += 1;
        Some(self.generation)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }
}

#[cfg(test)]
mod tests {
    //! Pure policy tests — no GTK, so they run in the lib test binary
    //! without touching the display thread.
    use super::*;
    use runtime_shared::image_src::vectors::{self, Expect};

    fn no_assets() -> HashMap<u64, AssetSource> {
        HashMap::new()
    }

    /// What the backend would report for `src` given only synchronous
    /// sources: `Some(size)` = `on_load`, `None` = `on_error`.
    fn sync_outcome(src: &str, assets: &HashMap<u64, AssetSource>) -> Option<(f32, f32)> {
        match route(src, assets) {
            Route::Bytes(b) => decode(&b).map(|d| d.natural_size()),
            Route::Fail(_) => None,
            Route::Remote(_) => panic!("{src:?} routed remote"),
        }
    }

    #[test]
    fn conformance_vectors_sync() {
        for (src, expect) in vectors::SYNC {
            let got = sync_outcome(src, &no_assets());
            match expect {
                Expect::Loads { width, height } => {
                    assert_eq!(got, Some((*width as f32, *height as f32)), "{src:?}")
                }
                Expect::Errors => assert_eq!(got, None, "{src:?}"),
            }
        }
    }

    /// The old path decoded base64 payloads only, so the percent-encoded SVG
    /// a CrewForge `BrandLockup` builds drew nothing on Linux.
    #[test]
    fn regression_percent_encoded_svg_data_uri_loads() {
        assert_eq!(sync_outcome(vectors::SVG_2X1_PERCENT, &no_assets()), Some((2.0, 1.0)));
    }

    /// The old path read any unrecognized `src` as a filesystem path relative
    /// to the working directory — a `/attachments/<id>` that worked on web
    /// read the local disk on Linux. Every relative form is now rejected
    /// before any filesystem access.
    #[test]
    fn regression_relative_path_no_longer_read_from_cwd() {
        // A real, decodable file at a relative path the old code would have
        // opened (cwd-relative).
        let name = format!("idealyst-relative-src-{}.png", std::process::id());
        let png = parse_data_uri(vectors::PNG_1X1_BASE64).unwrap().bytes;
        std::fs::write(&name, &png).unwrap();
        let relative_rejected: Vec<bool> = [name.clone(), format!("./{name}")]
            .iter()
            .map(|src| {
                matches!(route(src, &no_assets()), Route::Fail(Failure::Rejected(RejectReason::Relative)))
            })
            .collect();
        // An absolute filesystem path is no exception: `/x.png` is a
        // root-relative URL path, not a file.
        let abs = std::env::current_dir().unwrap().join(&name);
        let abs_routed_fail = matches!(
            route(abs.to_str().unwrap(), &no_assets()),
            Route::Fail(Failure::Rejected(RejectReason::Relative))
        );
        std::fs::remove_file(&name).unwrap();
        assert_eq!(relative_rejected, [true, true], "relative srcs must be rejected, not read from the cwd");
        assert!(abs_routed_fail, "{abs:?} must be rejected, not read from disk");
    }

    #[test]
    fn remote_forms_route_remote() {
        for src in ["https://x.io/a.png", "HTTP://x.io/a.png"] {
            assert!(matches!(route(src, &no_assets()), Route::Remote(u) if u == src));
        }
    }

    #[test]
    fn image_assets_route_by_source() {
        let png: &'static [u8] =
            Box::leak(parse_data_uri(vectors::PNG_1X1_BASE64).unwrap().bytes.into_boxed_slice());
        let mut assets = no_assets();
        assets.insert(1, AssetSource::Embedded { bytes: png, extension: "png" });
        assets.insert(2, AssetSource::BundledEmbedded { path: "x.png", bytes: png, extension: "png" });
        assets.insert(3, AssetSource::Remote { url: "https://cdn.io/x.png" });
        assets.insert(4, AssetSource::Remote { url: "file:///etc/x.png" });
        assets.insert(5, AssetSource::Bundled { path: "definitely/not/shipped.png" });
        assets.insert(6, AssetSource::Bundled { path: "../escape.png" });
        assert_eq!(sync_outcome("asset://1", &assets), Some((1.0, 1.0)));
        assert_eq!(sync_outcome("asset://2", &assets), Some((1.0, 1.0)));
        assert!(matches!(route("asset://3", &assets), Route::Remote(u) if u == "https://cdn.io/x.png"));
        assert!(matches!(
            route("asset://4", &assets),
            Route::Fail(Failure::Rejected(RejectReason::UnsupportedScheme))
        ));
        assert!(matches!(route("asset://5", &assets), Route::Fail(Failure::BundledAssetMissing)));
        assert!(matches!(route("asset://6", &assets), Route::Fail(Failure::BundledAssetMissing)));
        assert!(matches!(route("asset://7", &assets), Route::Fail(Failure::UnregisteredAsset(7))));
    }

    /// `register_asset` dropped every non-font asset, so `image_asset(asset!(..))`
    /// never loaded on Linux. The bundled half reads `<exe dir>/assets/<path>`.
    #[test]
    fn regression_bundled_image_asset_loads() {
        let rel = format!("idealyst-bundled-test-{}/logo.png", std::process::id());
        let exe_dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
        let file = exe_dir.join("assets").join(&rel);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, parse_data_uri(vectors::PNG_1X1_BASE64).unwrap().bytes).unwrap();
        let mut assets = no_assets();
        assets.insert(9, AssetSource::Bundled { path: Box::leak(rel.into_boxed_str()) });
        let got = sync_outcome("asset://9", &assets);
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
        assert_eq!(got, Some((1.0, 1.0)));
    }

    /// The formats the contract promises, decoded without GdkPixbuf. Each
    /// fixture is encoded by the `image` crate itself so it is known-good.
    #[test]
    fn every_contract_raster_format_decodes() {
        use image::{ImageFormat, RgbaImage};
        let img = RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]));
        for fmt in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Gif, ImageFormat::WebP] {
            let mut buf = std::io::Cursor::new(Vec::new());
            let encodable = if fmt == ImageFormat::Jpeg {
                image::DynamicImage::ImageRgba8(img.clone()).to_rgb8().write_to(&mut buf, fmt)
            } else {
                img.write_to(&mut buf, fmt)
            };
            encodable.unwrap_or_else(|e| panic!("encode {fmt:?}: {e}"));
            let d = decode(buf.get_ref()).unwrap_or_else(|| panic!("{fmt:?} must decode"));
            assert_eq!(d.natural_size(), (3.0, 2.0), "{fmt:?}");
        }
    }

    #[test]
    fn prepare_off_thread_matches_decode() {
        let png = parse_data_uri(vectors::PNG_1X1_BASE64).unwrap().bytes;
        let svg = parse_data_uri(vectors::SVG_2X1_PERCENT).unwrap().bytes;
        let p = std::thread::spawn(move || Prepared::new(png)).join().unwrap().unwrap();
        assert_eq!(p.finish().unwrap().natural_size(), (1.0, 1.0));
        let s = std::thread::spawn(move || Prepared::new(svg)).join().unwrap().unwrap();
        assert_eq!(s.finish().unwrap().natural_size(), (2.0, 1.0));
        assert!(Prepared::new(b"not an image".to_vec()).is_none());
    }

    #[test]
    fn regression_redundant_same_src_write_does_not_reload() {
        let mut slot = LoadSlot::default();
        let first = slot.begin("data:a").expect("first src loads");
        assert_eq!(slot.begin("data:a"), None, "same src again must not start a load");
        assert!(slot.is_current(first));
    }

    /// A slow earlier response must never replace a newer `src`.
    #[test]
    fn superseded_load_is_not_current() {
        let mut slot = LoadSlot::default();
        let a = slot.begin("https://slow/a.png").unwrap();
        let b = slot.begin("data:b").unwrap();
        assert!(!slot.is_current(a));
        assert!(slot.is_current(b));
        // Going back to `a` starts a NEW load; the old `a` response is
        // still stale.
        let a2 = slot.begin("https://slow/a.png").unwrap();
        assert!(!slot.is_current(a) && slot.is_current(a2));
    }

    /// Serve one canned HTTP response on a loopback port; returns the URL.
    fn serve_once(status_line: &'static str, body: Vec<u8>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = s.read(&mut req);
            let head = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&body);
        });
        format!("http://{addr}/img.png")
    }

    /// gio's file loader exposed no status, so an error response came back
    /// as bytes. A 404 whose body is a perfectly decodable PNG (CDN
    /// placeholder) must still be an error.
    #[test]
    fn regression_http_error_status_is_an_error_not_a_decoded_page() {
        let png = parse_data_uri(vectors::PNG_1X1_BASE64).unwrap().bytes;
        let url = serve_once("404 Not Found", png.clone());
        assert!(matches!(fetch(&url), Err(FetchError::Status(404))));
        assert!(fetch_and_prepare(&serve_once("404 Not Found", png.clone())).is_none());

        let ok = serve_once("200 OK", png);
        let p = fetch_and_prepare(&ok).expect("a 2xx PNG loads");
        assert_eq!(p.finish().unwrap().natural_size(), (1.0, 1.0));
    }

    #[test]
    fn remote_transport_failure_is_an_error() {
        // Bind then drop: the port is closed, so the connect is refused.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(matches!(fetch(&format!("http://127.0.0.1:{port}/x.png")), Err(FetchError::Transport(_))));
    }
}
