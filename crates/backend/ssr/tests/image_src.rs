//! `image()` `src` in server-rendered HTML — the `docs/image-src.md`
//! contract, SSR half. SSR fires no events; what it owes the client is an
//! `<img>` whose `src` the browser can load, or no `src` at all for a
//! src that fails on every backend (the hydrating client then fails the
//! same `<img>` and fires `on_error`).

use runtime_shared::assets::{Asset, AssetId, AssetSource};
use runtime_shared::image_src::{classify, vectors, ImageSrc};
use runtime_vocabulary::builders::{image, view};

const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
];

fn img_html(build: impl FnOnce() -> runtime_scene::Element) -> String {
    backend_ssr::newcore::render_path("/", build).html
}

/// REGRESSION: an `image_asset(..)` rendered `<img src="asset://N">` —
/// the framework's internal sentinel, which no browser can load, so the
/// first paint showed a broken image until hydration rewrote it.
#[test]
fn regression_ssr_does_not_leak_asset_sentinel() {
    let html = img_html(|| {
        view()
            .child(image().asset(Asset::new(AssetId(11), AssetSource::Bundled { path: "images/logo.png" })))
            .child(image().asset(Asset::new(
                AssetId(12),
                AssetSource::Embedded { bytes: PNG_1X1, extension: "png" },
            )))
            .child(image().asset(Asset::new(
                AssetId(13),
                AssetSource::Remote { url: "https://cdn.example.com/x.png" },
            )))
            .build()
    });
    assert!(!html.contains("asset://"), "{html}");
    // Bundled: root-absolute, the same URL the live web build uses.
    assert!(html.contains(r#"<img src="/assets/images/logo.png""#), "{html}");
    // Embedded: no served file, so its bytes are inlined.
    let data = css::data_uri("image/png", PNG_1X1);
    assert!(html.contains(&format!(r#"<img src="{data}""#)), "{html}");
    assert!(html.contains(r#"<img src="https://cdn.example.com/x.png""#), "{html}");
}

/// A src that fails everywhere gets no `src` attribute — never its raw
/// text, which a browser would resolve against the page.
#[test]
fn regression_ssr_rejected_src_has_no_src_attribute() {
    let html = img_html(|| {
        view()
            .child(image().src("/attachments/abc").alt("a"))
            .child(image().src("asset://424242").alt("b"))
            .build()
    });
    assert!(html.contains(r#"<img alt="a">"#), "{html}");
    assert!(html.contains(r#"<img alt="b">"#), "{html}");
}

/// Every conformance vector: `http(s)`/`data:` reach the HTML unchanged
/// (the browser decodes, and fails, a bad `data:` payload itself); every
/// other `Errors` vector renders with no `src`.
#[test]
fn ssr_conformance_vectors() {
    for &(src, _expect) in vectors::SYNC {
        let html = img_html(|| image().src(src).alt("x").build());
        match classify(src) {
            ImageSrc::Data(_) | ImageSrc::Remote(_) => {
                assert!(html.contains(&format!(r#"src="{src}""#)), "{src:?}: {html}")
            }
            ImageSrc::Asset(_) | ImageSrc::Rejected(_) => {
                assert!(!html.contains("src="), "{src:?}: {html}")
            }
        }
    }
}
