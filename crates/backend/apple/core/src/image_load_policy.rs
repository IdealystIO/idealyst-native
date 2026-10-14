//! The decisions behind `image()` loading on iOS and macOS, as pure code.
//!
//! `UIImageView` and the AppKit layer view differ in how they paint, but
//! what they do with a `src` is the same (`docs/image-src.md`), so the
//! choices live here, shared and host-testable. The views only perform them.
//!
//! - [`plan_src`](crate::image_load_policy::plan_src): what a `src` turns into. A cached asset, a decoded
//!   `data:` URI, a URL to fetch, or a failure. It routes through
//!   `runtime_shared::image_src::classify`, the parser every backend uses.
//! - [`asset_plan`](crate::image_load_policy::asset_plan) / [`bundled_asset_path`](crate::image_load_policy::bundled_asset_path): where an `image_asset(..)`
//!   gets its bytes. `asset!` files are read from `assets/<path>` in the
//!   app bundle's resources.
//! - [`fetch_verdict`](crate::image_load_policy::fetch_verdict): what a finished `NSURLSession` data task means.
//!   A transport error, a missing response or a non-2xx status is a load
//!   error even when the body holds bytes. Before this check an HTTP error
//!   page that happened to decode was drawn as the image.
//! - [`ImageLoadState`](crate::image_load_policy::ImageLoadState) /
//!   [`LoadGeneration`](crate::image_load_policy::LoadGeneration): every `src` change starts a new generation, and
//!   a fetch that completes for an older one is dropped (no image, no
//!   `on_load`/`on_error`). Before this, a slow response for the previous
//!   `src` could land after the newer one and replace it.
//! - [`ImageLoadState`](crate::image_load_policy::ImageLoadState): the
//!   generation plus the current load's outcome, the only thing a
//!   late-installed handler is replayed from (exactly once per handler).
//!
//! NOT OS-gated, so the regression tests run on any host.

use std::borrow::Cow;
use std::cell::Cell;
use std::path::{Component, Path, PathBuf};

use backend_image_source::{self as image_source, DecodedSource, ImageSrc, RejectReason};
use runtime_shared::AssetSource;

/// What an image view does with a `src`.
pub enum SrcPlan<'a> {
    /// `asset://<id>`: show the registered asset with this id. An id with
    /// nothing registered is a load error ([`SrcFailure::UnknownAsset`]).
    Asset(u64),
    /// A `data:` URI, already decoded. A bitmap still goes through the
    /// platform decoder, which can fail too.
    Decoded(DecodedSource<'static>),
    /// `http(s)://`: fetch off the main thread.
    Remote(&'a str),
    /// Fire `on_error`.
    Fail(SrcFailure),
}

/// Why a `src` produced no image. Logged next to `on_error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrcFailure {
    /// Not a form `docs/image-src.md` accepts. Log it with
    /// `image_source::warn_rejected` so every backend says the same thing.
    Rejected(RejectReason),
    /// A `data:` URI whose payload is malformed, or an SVG that doesn't parse.
    UndecodableDataUri,
    /// `asset://<id>` with nothing registered under the id.
    UnknownAsset,
    /// The asset's file is missing from the app bundle, or its bytes don't
    /// decode.
    AssetUnavailable,
    /// The URL string is one `NSURL` can't parse, or the session refused to
    /// start the task.
    UnstartableFetch,
    /// The fetch finished without a usable image (see [`FetchError`]).
    Fetch(FetchError),
    /// The platform decoder (`UIImage`/`NSImage(data:)`) rejected the bytes.
    UndecodableBitmap,
}

impl SrcFailure {
    /// A short reason for the log line.
    pub fn describe(self) -> String {
        match self {
            SrcFailure::Rejected(reason) => reason.explain().to_string(),
            SrcFailure::UndecodableDataUri => "the data: URI does not decode".into(),
            SrcFailure::UnknownAsset => "no asset is registered with that id".into(),
            SrcFailure::AssetUnavailable => {
                "the asset's file is not in the app bundle or does not decode".into()
            }
            SrcFailure::UnstartableFetch => "the URL cannot be fetched (unparseable URL)".into(),
            SrcFailure::Fetch(FetchError::Transport) => "the request failed".into(),
            SrcFailure::Fetch(FetchError::NoHttpResponse) => "no HTTP response".into(),
            SrcFailure::Fetch(FetchError::HttpStatus(code)) => format!("HTTP status {code}"),
            SrcFailure::Fetch(FetchError::EmptyBody) => "the response body is empty".into(),
            SrcFailure::Fetch(FetchError::UndecodableSvg) => "the fetched SVG does not parse".into(),
            SrcFailure::UndecodableBitmap => "the bytes are not an image format this OS decodes".into(),
        }
    }
}

/// Decide what to do with `src`.
pub fn plan_src(src: &str) -> SrcPlan<'_> {
    match image_source::classify(src) {
        ImageSrc::Asset(id) => SrcPlan::Asset(id),
        ImageSrc::Remote(url) => SrcPlan::Remote(url.trim()),
        ImageSrc::Data(uri) => match decode_data_uri(uri) {
            Some(decoded) => SrcPlan::Decoded(decoded),
            None => SrcPlan::Fail(SrcFailure::UndecodableDataUri),
        },
        ImageSrc::Rejected(reason) => SrcPlan::Fail(SrcFailure::Rejected(reason)),
    }
}

/// Parse a `data:` URI and sniff its payload. `None` when the URI is
/// malformed or holds an SVG that doesn't parse.
pub fn decode_data_uri(src: &str) -> Option<DecodedSource<'static>> {
    let uri = image_source::parse_data_uri(src)?;
    Some(match image_source::decode_bytes(&uri.bytes)? {
        DecodedSource::Svg(svg) => DecodedSource::Svg(svg),
        DecodedSource::Bitmap(_) => DecodedSource::Bitmap(Cow::Owned(uri.bytes)),
    })
}

/// Where an image asset's bytes come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetPlan<'a> {
    /// `embed_asset!` (and the bytes half of `BundledEmbedded`).
    Bytes(&'a [u8]),
    /// `asset!("images/logo.png")`: a file in the app bundle, found with
    /// [`bundled_asset_path`].
    BundleFile(&'a str),
    /// `AssetSource::Remote`: load it like an `https://` `src`.
    Remote(&'a str),
}

/// `'static` because every `AssetSource` payload is (`asset!` /
/// `embed_asset!` literals).
pub fn asset_plan(source: &AssetSource) -> AssetPlan<'static> {
    match source {
        AssetSource::Embedded { bytes, .. } | AssetSource::BundledEmbedded { bytes, .. } => {
            AssetPlan::Bytes(bytes)
        }
        AssetSource::Bundled { path } => AssetPlan::BundleFile(path),
        AssetSource::Remote { url } => AssetPlan::Remote(url),
    }
}

/// `<resource_dir>/assets/<path>`, where the build copies the declaring
/// crate's `assets/` directory: `<App>.app/assets/` on iOS,
/// `<App>.app/Contents/Resources/assets/` on macOS (`resource_dir` is
/// `NSBundle.mainBundle.resourcePath` on both).
///
/// `None` for a path that could leave the assets directory (absolute, or
/// with a `..` component) or is empty. This is the same rule as
/// `runtime_shared::image_src::bundled_asset_candidates`, which resolves
/// against the executable and working directory instead of a bundle.
pub fn bundled_asset_path(resource_dir: &Path, path: &str) -> Option<PathBuf> {
    let rel = Path::new(path);
    if path.is_empty()
        || rel.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    Some(resource_dir.join("assets").join(rel))
}

/// What a finished data task handed back.
#[derive(Debug, Clone, Copy)]
pub struct FetchResult<'a> {
    /// The completion's `error` argument was non-nil.
    pub transport_error: bool,
    /// `NSHTTPURLResponse.statusCode`, or `None` when the response is nil
    /// or not an HTTP response.
    pub http_status: Option<i64>,
    /// The body, `None` when `data` was nil.
    pub body: Option<&'a [u8]>,
}

/// Why a fetch produced no image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    Transport,
    /// Only `http(s)` URLs are fetched, so a missing HTTP response means
    /// the request never got an answer.
    NoHttpResponse,
    /// Outside 200..=299. The body is an error page, not the image.
    HttpStatus(i64),
    EmptyBody,
    /// The body sniffs as SVG but doesn't parse.
    UndecodableSvg,
}

/// What to do with a fetched body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchVerdict {
    /// Parse and rasterize as SVG (on the main thread: the view keeps the
    /// document to re-rasterize on resize).
    Svg,
    /// Hand to `UIImage`/`NSImage(data:)`.
    Bitmap,
    Error(FetchError),
}

pub fn fetch_verdict(r: &FetchResult<'_>) -> FetchVerdict {
    if r.transport_error {
        return FetchVerdict::Error(FetchError::Transport);
    }
    match r.http_status {
        None => return FetchVerdict::Error(FetchError::NoHttpResponse),
        Some(code) if !(200..=299).contains(&code) => {
            return FetchVerdict::Error(FetchError::HttpStatus(code))
        }
        Some(_) => {}
    }
    match r.body {
        Some(bytes) if !bytes.is_empty() => {
            if image_source::looks_like_svg(bytes) {
                FetchVerdict::Svg
            } else {
                FetchVerdict::Bitmap
            }
        }
        _ => FetchVerdict::Error(FetchError::EmptyBody),
    }
}

/// One view's load counter. Every `src` applied to the view, sync or
/// async, calls [`begin`](Self::begin); an async completion carries the
/// ticket it started with and is applied only while
/// [`is_current`](Self::is_current) still holds. Main-thread state: the
/// background completion only carries its ticket to the main thread.
#[derive(Debug, Default)]
pub struct LoadGeneration(Cell<u64>);

/// The generation a load started under. `Copy + Send`, so it can ride the
/// background completion to the main thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadTicket(u64);

impl LoadGeneration {
    pub const fn new() -> Self {
        Self(Cell::new(0))
    }

    /// Start a load, superseding every earlier one.
    pub fn begin(&self) -> LoadTicket {
        let next = self.0.get().wrapping_add(1);
        self.0.set(next);
        LoadTicket(next)
    }

    /// Whether `ticket` is still the view's latest load.
    pub fn is_current(&self, ticket: LoadTicket) -> bool {
        self.0.get() == ticket.0
    }
}

/// How the view's current load ended, for replaying to a handler installed
/// after the fact. Replay reads ONLY this, never "does the view hold an
/// image": after a `src` change the previous bitmap stays on screen while
/// the new one loads, and replaying its size to a handler installed in that
/// window, then firing the live event when the new load lands, delivered
/// `on_load` twice for one load.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadOutcome {
    /// No outcome yet for the current `src` (or nothing applied).
    Pending,
    Loaded { width: f32, height: f32 },
    Failed,
}

/// A view's load bookkeeping: the generation that drops superseded fetches
/// plus the current load's outcome. The contract it implements
/// (`docs/image-src.md`): `on_load` / `on_error` fire once per outcome; a
/// handler installed after the outcome gets exactly one replay; a new `src`
/// starts over (a second distinct failing `src` fires `on_error` again).
#[derive(Debug)]
pub struct ImageLoadState {
    generation: LoadGeneration,
    outcome: Cell<LoadOutcome>,
}

impl Default for ImageLoadState {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageLoadState {
    pub const fn new() -> Self {
        Self { generation: LoadGeneration::new(), outcome: Cell::new(LoadOutcome::Pending) }
    }

    /// A new `src` is applied: supersede in-flight loads, forget the old
    /// outcome.
    pub fn begin(&self) -> LoadTicket {
        self.outcome.set(LoadOutcome::Pending);
        self.generation.begin()
    }

    pub fn is_current(&self, ticket: LoadTicket) -> bool {
        self.generation.is_current(ticket)
    }

    /// Record a success; the caller fires the live `on_load`.
    pub fn loaded(&self, width: f32, height: f32) {
        self.outcome.set(LoadOutcome::Loaded { width, height });
    }

    /// Record a failure; the caller fires the live `on_error`.
    pub fn failed(&self) {
        self.outcome.set(LoadOutcome::Failed);
    }

    /// What a handler installed now should be replayed (`Pending`: nothing).
    pub fn outcome(&self) -> LoadOutcome {
        self.outcome.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::image_src::vectors::{Expect, SYNC};

    /// Natural size the platform decoder would report for a PNG: read
    /// from IHDR (the host has no UIImage). Bitmap-format coverage proper
    /// is the macOS `image_src` harness test and the iOS sim smoke test.
    fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
        (bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n")).then(|| {
            let be = |i: usize| u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap());
            (be(16), be(20))
        })
    }

    /// The cross-backend conformance table, through the Apple plan. No
    /// asset is registered, so every `asset://` id is unknown.
    #[test]
    fn sync_vectors_follow_the_contract() {
        for &(src, expect) in SYNC {
            let got = match plan_src(src) {
                SrcPlan::Decoded(DecodedSource::Bitmap(bytes)) => match png_size(&bytes) {
                    Some((width, height)) => Expect::Loads { width, height },
                    None => panic!("{src:?}: bitmap vector is not a PNG"),
                },
                SrcPlan::Decoded(DecodedSource::Svg(svg)) => {
                    let (w, h) = svg.intrinsic_size();
                    Expect::Loads { width: w as u32, height: h as u32 }
                }
                SrcPlan::Asset(_) | SrcPlan::Fail(_) => Expect::Errors,
                SrcPlan::Remote(_) => panic!("{src:?}: SYNC holds no remote vectors"),
            };
            assert_eq!(got, expect, "{src:?}");
        }
    }

    /// CrewForge kiosk: `/attachments/<id>` drew a blank tile on iOS with no
    /// error. It is now a rejection that fires `on_error`.
    #[test]
    fn regression_relative_src_fires_on_error() {
        assert!(matches!(
            plan_src("/attachments/abc"),
            SrcPlan::Fail(SrcFailure::Rejected(RejectReason::Relative))
        ));
    }

    #[test]
    fn remote_src_is_fetched_trimmed() {
        assert!(matches!(plan_src(" https://x.io/a.png "), SrcPlan::Remote("https://x.io/a.png")));
        assert!(matches!(plan_src("HTTP://x.io/a.png"), SrcPlan::Remote("HTTP://x.io/a.png")));
    }

    fn ok(body: &[u8]) -> FetchResult<'_> {
        FetchResult { transport_error: false, http_status: Some(200), body: Some(body) }
    }

    /// Before: the completion handler only looked at `data`, so a 404/500
    /// whose body decoded (a CDN's placeholder PNG, an SVG error page) was
    /// drawn as the image and fired `on_load`.
    #[test]
    fn regression_non_2xx_status_is_an_error() {
        let png = image_source::parse_data_uri(runtime_shared::image_src::vectors::PNG_1X1_BASE64)
            .unwrap()
            .bytes;
        for code in [404, 500, 304, 199, 302] {
            let r = FetchResult { http_status: Some(code), ..ok(&png) };
            assert_eq!(
                fetch_verdict(&r),
                FetchVerdict::Error(FetchError::HttpStatus(code)),
                "status {code} with a decodable body must still be an error"
            );
        }
        assert_eq!(fetch_verdict(&ok(&png)), FetchVerdict::Bitmap);
        assert_eq!(fetch_verdict(&FetchResult { http_status: Some(299), ..ok(&png) }), FetchVerdict::Bitmap);
        assert_eq!(fetch_verdict(&ok(b"<svg xmlns='http://www.w3.org/2000/svg'/>")), FetchVerdict::Svg);
    }

    #[test]
    fn transport_errors_and_empty_bodies_are_errors() {
        let r = FetchResult { transport_error: true, http_status: None, body: None };
        assert_eq!(fetch_verdict(&r), FetchVerdict::Error(FetchError::Transport));
        let r = FetchResult { transport_error: false, http_status: None, body: Some(b"x") };
        assert_eq!(fetch_verdict(&r), FetchVerdict::Error(FetchError::NoHttpResponse));
        assert_eq!(fetch_verdict(&ok(b"")), FetchVerdict::Error(FetchError::EmptyBody));
        let r = FetchResult { body: None, ..ok(b"") };
        assert_eq!(fetch_verdict(&r), FetchVerdict::Error(FetchError::EmptyBody));
    }

    /// Before: no generation. `src = A` started a slow fetch, `src = B`
    /// loaded, then A's response arrived and replaced B (and fired a second
    /// `on_load`). A completion is now applied only if its ticket is current.
    #[test]
    fn regression_stale_image_response_is_dropped() {
        let generation = LoadGeneration::new();
        let slow_a = generation.begin();
        let b = generation.begin();
        assert!(!generation.is_current(slow_a), "A's late response must be dropped");
        assert!(generation.is_current(b));
        // A synchronous src (data:, asset, rejection) supersedes too.
        let sync_c = generation.begin();
        assert!(!generation.is_current(b));
        assert!(generation.is_current(sync_c));
    }

    /// Web found `on_load` firing twice for one load: a replay at install
    /// plus the live event. On Apple the replay read "the view holds an
    /// image", which stays true for the PREVIOUS bitmap while a new `src`
    /// loads. Replay now reads the current load's outcome only.
    #[test]
    fn regression_handler_installed_mid_load_gets_no_stale_replay() {
        let state = ImageLoadState::new();
        state.begin();
        state.loaded(1.0, 1.0);
        // A handler installed now gets exactly the one replay.
        assert_eq!(state.outcome(), LoadOutcome::Loaded { width: 1.0, height: 1.0 });
        // A new src starts loading: the old 1x1 is still on screen, but a
        // handler installed now must wait for the live event.
        let t = state.begin();
        assert_eq!(state.outcome(), LoadOutcome::Pending);
        assert!(state.is_current(t));
        state.loaded(3.0, 2.0);
        assert_eq!(state.outcome(), LoadOutcome::Loaded { width: 3.0, height: 2.0 });
    }

    #[test]
    fn each_distinct_failing_src_reports_its_own_failure() {
        let state = ImageLoadState::new();
        state.begin();
        state.failed();
        assert_eq!(state.outcome(), LoadOutcome::Failed);
        state.begin();
        assert_eq!(state.outcome(), LoadOutcome::Pending, "the next src's failure is a new outcome");
        state.failed();
        assert_eq!(state.outcome(), LoadOutcome::Failed);
    }

    #[test]
    fn regression_bundled_asset_image_loads() {
        // Before: `AssetSource::Bundled` was skipped on Apple, so every
        // `image_asset(asset!(..))` fired on_error. It now names a file
        // under the bundle's resources.
        let src = AssetSource::Bundled { path: "images/logo.png" };
        assert_eq!(asset_plan(&src), AssetPlan::BundleFile("images/logo.png"));
        let res = Path::new("/App.app");
        assert_eq!(
            bundled_asset_path(res, "images/logo.png"),
            Some(PathBuf::from("/App.app/assets/images/logo.png"))
        );
        assert_eq!(
            bundled_asset_path(res, "./logo.png"),
            Some(PathBuf::from("/App.app/assets/./logo.png"))
        );
        for bad in ["", "../secret.png", "images/../../x.png", "/etc/passwd"] {
            assert_eq!(bundled_asset_path(res, bad), None, "{bad:?} must not resolve");
        }
        assert_eq!(
            asset_plan(&AssetSource::Remote { url: "https://cdn.example.com/a.png" }),
            AssetPlan::Remote("https://cdn.example.com/a.png")
        );
        assert_eq!(
            asset_plan(&AssetSource::BundledEmbedded { path: "a.png", bytes: b"x", extension: "png" }),
            AssetPlan::Bytes(b"x")
        );
    }
}
