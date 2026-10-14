//! The Win32 `image()` `src` policy against `docs/image-src.md`.
//!
//! The crate body is `cfg(target_os = "windows")`; this test compiles the
//! pure policy module directly (`#[path]`) so it runs on every host. What
//! it cannot reach from a non-Windows host — GDI+ actually decoding the
//! `Platform` bytes, WinHTTP actually fetching — is the thin shell in
//! `image.rs` / `image_fetch.rs`; everything that decides WHAT happens
//! (routing, rejection, SVG/WebP decoding, event delivery, stale-fetch
//! supersession, URL splitting, status policy) is here.

#[allow(dead_code)]
#[path = "../src/image_policy.rs"]
mod image_policy;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use image_policy::*;
use runtime_shared::image_src::vectors::{self, Expect};
use runtime_shared::image_src::RejectReason;
use runtime_shared::primitives::image::ImageLoadEvent;

fn no_assets() -> HashMap<u64, AssetEntry> {
    HashMap::new()
}

fn no_bundle(_: &str) -> Option<Vec<u8>> {
    None
}

/// The sync outcome the backend reaches for `src`, with GDI+ stood in for
/// by "the bytes are a PNG" (GDI+ itself only runs on Windows).
fn sync_outcome(src: &str, assets: &HashMap<u64, AssetEntry>) -> Option<Outcome> {
    match resolve(None, src, assets, no_bundle) {
        Resolution::Unchanged => panic!("{src:?}: no current src"),
        Resolution::Fetch(_) => None,
        Resolution::Failed(_) => Some(Outcome::Failed),
        Resolution::Ready(Decoded::Svg(svg)) => {
            let (width, height) = svg_natural(&svg);
            Some(Outcome::Loaded { width, height })
        }
        Resolution::Ready(Decoded::Pixels(p)) => {
            Some(Outcome::Loaded { width: p.width as f32, height: p.height as f32 })
        }
        Resolution::Ready(Decoded::Platform(bytes)) => {
            assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "{src:?}: not a PNG");
            // PNG IHDR: width/height are the big-endian u32s at 16..24.
            let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
            let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
            Some(Outcome::Loaded { width: w as f32, height: h as f32 })
        }
    }
}

#[test]
fn sync_vectors_conform() {
    for (src, expect) in vectors::SYNC {
        let got = sync_outcome(src, &no_assets());
        let want = match expect {
            Expect::Loads { width, height } => {
                Outcome::Loaded { width: *width as f32, height: *height as f32 }
            }
            Expect::Errors => Outcome::Failed,
        };
        assert_eq!(got, Some(want), "{src:?}");
    }
}

/// Before: `decode_src` returned `None` for `http(s)://` — no fetch, no
/// event, a blank box. Now the URL is fetched (WinHTTP, off the UI thread)
/// and a failed fetch reaches `on_error`.
#[test]
fn regression_http_src_no_longer_silently_blank() {
    for url in ["https://example.com/a.png", "HTTP://example.com/a.png?x=1"] {
        match resolve(None, url, &no_assets(), no_bundle) {
            Resolution::Fetch(u) => assert_eq!(u, url),
            _ => panic!("{url:?} must be fetched"),
        }
        let target = split_http_url(url).expect("WinHTTP can be handed the URL");
        assert_eq!(target.host, "example.com");
    }
    // A remote asset fetches the same way.
    let mut assets = no_assets();
    assets.insert(7, AssetEntry::Remote("https://cdn.example.com/x.webp"));
    assert!(matches!(
        resolve(None, "asset://7", &assets, no_bundle),
        Resolution::Fetch("https://cdn.example.com/x.webp")
    ));
    // ...and a 404 is an error, not a blank success.
    assert!(!status_is_success(404));
    let errors = Rc::new(Cell::new(0));
    let mut ev = ImageEvents::default();
    let e = errors.clone();
    ev.install_error(Rc::new(move || e.set(e.get() + 1)));
    let gen = ev.begin();
    assert!(ev.settle(gen, Outcome::Failed));
    ev.take_fire().expect("on_error owed").call();
    assert_eq!(errors.get(), 1);
}

/// Before: `on_load`/`on_error` were never installed on Windows, so a
/// rejected / undecodable / missing source was a silent blank. Every
/// failing route now produces `Outcome::Failed`, and the handler receives
/// it whether installed before or after the outcome — exactly once.
#[test]
fn regression_image_failures_fire_on_error() {
    let mut assets = no_assets();
    assets.insert(1, AssetEntry::Bundled("images/missing.png"));
    assets.insert(2, AssetEntry::Bytes(b"<svg"));
    assets.insert(3, AssetEntry::Remote("ftp://x.io/a.png"));
    for src in ["", "logo.png", "asset://1", "asset://2", "asset://3", "asset://99"] {
        assert_eq!(sync_outcome(src, &assets), Some(Outcome::Failed), "{src:?}");
    }

    // Installed AFTER the outcome (the walker's order for a sync failure).
    let errors = Rc::new(Cell::new(0));
    let mut ev = ImageEvents::default();
    let gen = ev.begin();
    ev.settle(gen, Outcome::Failed);
    assert!(ev.take_fire().is_none(), "nothing installed yet");
    let e = errors.clone();
    ev.install_error(Rc::new(move || e.set(e.get() + 1)));
    // Two deliveries queued (the load's and the install's) → one call.
    while let Some(f) = ev.take_fire() {
        f.call();
    }
    assert_eq!(errors.get(), 1);
    assert!(ev.take_fire().is_none());

    // A new src that also fails fires again.
    let gen = ev.begin();
    ev.settle(gen, Outcome::Failed);
    ev.take_fire().expect("second failure delivered").call();
    assert_eq!(errors.get(), 2);
}

/// Before: `file://` (and any bare path) was read relative to the working
/// directory on Windows while Apple/Android rejected it. Now it is a load
/// error with the shared diagnosis.
#[test]
fn regression_file_url_rejected() {
    for (src, why) in [
        ("file:///C:/img/x.png", RejectReason::UnsupportedScheme),
        ("C:\\img\\x.png", RejectReason::UnsupportedScheme),
        ("assets/x.png", RejectReason::Relative),
        ("/attachments/abc", RejectReason::Relative),
        ("//cdn.example.com/x.png", RejectReason::SchemeRelative),
    ] {
        match resolve(None, src, &no_assets(), |_: &str| panic!("{src:?} must not touch disk")) {
            Resolution::Failed(f) => assert_eq!(f, Failure::Rejected(why), "{src:?}"),
            _ => panic!("{src:?} must be rejected"),
        }
    }
}

/// Before: `decode_data_url` required a lower-case `data:` and a base64
/// payload; the percent-encoded SVG form CrewForge's `BrandLockup` builds
/// was blank (and GDI+ has no SVG codec anyway).
#[test]
fn regression_percent_encoded_data_uri_loads() {
    assert_eq!(
        sync_outcome(vectors::SVG_2X1_PERCENT, &no_assets()),
        Some(Outcome::Loaded { width: 2.0, height: 1.0 })
    );
    // Upper-case scheme + a base64 payload whose `+`/`/`/`=` were
    // percent-escaped by a URL encoder.
    let b64 = vectors::PNG_1X1_BASE64.split_once(',').unwrap().1;
    let escaped = b64.replace('+', "%2B").replace('/', "%2F").replace('=', "%3D");
    let src = format!("DATA:image/png;base64,{escaped}");
    assert_eq!(
        sync_outcome(&src, &no_assets()),
        Some(Outcome::Loaded { width: 1.0, height: 1.0 })
    );
}

#[test]
fn unchanged_src_does_not_reload() {
    // The walker's mount-time `update_image_src` repeats create's src.
    assert!(matches!(
        resolve(Some("https://a.io/x.png"), "https://a.io/x.png", &no_assets(), no_bundle),
        Resolution::Unchanged
    ));
    assert!(matches!(
        resolve(Some("https://a.io/x.png"), "https://a.io/y.png", &no_assets(), no_bundle),
        Resolution::Fetch(_)
    ));
}

#[test]
fn assets_route_to_their_loader() {
    let mut assets = no_assets();
    assets.insert(1, AssetEntry::Bytes(include_bytes!("fixtures/1x1.png")));
    assets.insert(2, AssetEntry::Bundled("images/logo.png"));
    assert_eq!(sync_outcome("asset://1", &assets), Some(Outcome::Loaded { width: 1.0, height: 1.0 }));
    let read = |p: &str| {
        assert_eq!(p, "images/logo.png");
        Some(include_bytes!("fixtures/1x1.png").to_vec())
    };
    assert!(matches!(
        resolve(None, "asset://2", &assets, read),
        Resolution::Ready(Decoded::Platform(_))
    ));
    use runtime_shared::assets::AssetSource;
    assert_eq!(
        asset_entry(&AssetSource::Embedded { bytes: b"x", extension: "png" }),
        AssetEntry::Bytes(b"x")
    );
    assert_eq!(asset_entry(&AssetSource::Bundled { path: "a.png" }), AssetEntry::Bundled("a.png"));
    assert_eq!(
        asset_entry(&AssetSource::Remote { url: "https://a.io/x" }),
        AssetEntry::Remote("https://a.io/x")
    );
}

/// GDI+ has no WebP codec: WebP decodes in Rust to premultiplied BGRA.
#[test]
fn webp_decodes_to_premultiplied_bgra() {
    // 2×1: opaque red, half-transparent green.
    let rgba = [255u8, 0, 0, 255, 0, 255, 0, 128];
    let mut webp = Vec::new();
    image_webp::WebPEncoder::new(&mut webp)
        .encode(&rgba, 2, 1, image_webp::ColorType::Rgba8)
        .unwrap();
    assert!(looks_like_webp(&webp));
    let Ok(Decoded::Pixels(img)) = decode(webp.into()) else { panic!("WebP must decode") };
    assert_eq!((img.width, img.height), (2, 1));
    assert_eq!(&img.bgra_premultiplied[0..4], &[0, 0, 255, 255], "red, B/R swapped");
    assert_eq!(&img.bgra_premultiplied[4..8], &[0, 128, 0, 128], "green, premultiplied");
    assert!(matches!(decode(b"RIFF\0\0\0\0WEBPjunk".as_slice().into()), Err(Failure::Undecodable)));
}

#[test]
fn svg_raster_swaps_to_bgra() {
    let img = rgba_premul_to_bgra(1, 1, vec![10, 20, 30, 40]);
    assert_eq!(img.bgra_premultiplied, vec![30, 20, 10, 40]);
}

#[test]
fn stale_fetch_never_replaces_a_newer_src() {
    let loads = Rc::new(RefCell::new(Vec::new()));
    let mut ev = ImageEvents::default();
    let l = loads.clone();
    ev.install_load(Rc::new(move |e: &ImageLoadEvent| l.borrow_mut().push((e.width, e.height))));
    let slow = ev.begin(); // src A: slow fetch
    let fast = ev.begin(); // src B supersedes it
    assert!(ev.settle(fast, Outcome::Loaded { width: 4.0, height: 2.0 }));
    assert!(!ev.settle(slow, Outcome::Loaded { width: 9.0, height: 9.0 }), "A is stale");
    while let Some(f) = ev.take_fire() {
        f.call();
    }
    assert_eq!(*loads.borrow(), vec![(4.0, 2.0)]);
    // Pending (begun, not settled) owes nothing.
    ev.begin();
    assert!(ev.take_fire().is_none());
}

#[test]
fn load_handler_installed_after_a_sync_load_is_replayed_once() {
    let loads = Rc::new(Cell::new(0));
    let mut ev = ImageEvents::default();
    let gen = ev.begin();
    ev.settle(gen, Outcome::Loaded { width: 1.0, height: 1.0 });
    let l = loads.clone();
    ev.install_load(Rc::new(move |_: &ImageLoadEvent| l.set(l.get() + 1)));
    ev.install_error(Rc::new(|| panic!("a load is not an error")));
    while let Some(f) = ev.take_fire() {
        f.call();
    }
    assert_eq!(loads.get(), 1);
}

#[test]
fn http_urls_split_for_winhttp() {
    let t = split_http_url("https://Example.com/a/b.png?v=2#frag").unwrap();
    assert_eq!(t, HttpTarget { secure: true, host: "Example.com".into(), port: 443, path: "/a/b.png?v=2".into() });
    let t = split_http_url("http://localhost:8080").unwrap();
    assert_eq!((t.secure, t.host.as_str(), t.port, t.path.as_str()), (false, "localhost", 8080, "/"));
    let t = split_http_url("http://h.io?q=1").unwrap();
    assert_eq!(t.path, "/?q=1");
    let t = split_http_url("https://[::1]:8443/x").unwrap();
    assert_eq!((t.host.as_str(), t.port), ("::1", 8443));
    assert_eq!(split_http_url("https://[::1]/x").unwrap().port, 443);
    for bad in [
        "ftp://h.io/x",
        "https:///x",
        "https://h.io:99999/x",
        "https://h.io:0/x",
        "https://user:pw@h.io/x",
        "https://[::1/x",
        "https://[::1]x/",
    ] {
        assert_eq!(split_http_url(bad), None, "{bad:?}");
    }
}

#[test]
fn only_2xx_is_success() {
    assert!(status_is_success(200));
    assert!(status_is_success(204));
    for s in [0, 199, 301, 304, 404, 500] {
        assert!(!status_is_success(s), "{s}");
    }
}
