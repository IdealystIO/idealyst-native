//! `image()` `src` in a rendered email — the `docs/image-src.md` contract,
//! email half. An email travels without the app's `/assets/` directory,
//! so every asset is inlined as a `data:` URI; a src that fails on every
//! backend gets no `src`.

use runtime_shared::assets::{Asset, AssetId, AssetSource};
use runtime_shared::image_src::{classify, vectors, ImageSrc};
use runtime_vocabulary::builders::{image, view};

const PNG_HEAD: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

fn html(build: impl FnOnce() -> runtime_scene::Element) -> String {
    backend_email::newcore::render_email(build).html
}

/// REGRESSION: an `image_asset(..)` rendered `<img src="asset://N">` —
/// the internal sentinel, which no mail client can load.
#[test]
fn regression_email_inlines_image_assets() {
    // A bundled file, staged where a shipped binary finds it.
    let rel = format!("email-image-src-{}/logo.png", std::process::id());
    let exe_assets = std::env::current_exe().unwrap().parent().unwrap().join("assets");
    let file = exe_assets.join(&rel);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, PNG_HEAD).unwrap();
    let bundled: &'static str = Box::leak(rel.into_boxed_str());

    let out = html(|| {
        view()
            .child(image().asset(Asset::new(AssetId(21), AssetSource::Bundled { path: bundled })))
            .child(image().asset(Asset::new(
                AssetId(22),
                AssetSource::Embedded { bytes: PNG_HEAD, extension: "png" },
            )))
            .child(image().asset(Asset::new(
                AssetId(23),
                AssetSource::Bundled { path: "email-image-src-missing/none.png" },
            )).alt("missing"))
            .build()
    });
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();

    assert!(!out.contains("asset://"), "{out}");
    let data = css::data_uri("image/png", PNG_HEAD);
    assert_eq!(out.matches(&format!(r#"<img src="{data}""#)).count(), 2, "{out}");
    // A bundled file that isn't there: no src, never a dangling path.
    assert!(out.contains(r#"<img alt="missing""#), "{out}");
}

/// Every conformance vector: `http(s)`/`data:` reach the HTML unchanged;
/// every other `Errors` vector renders with no `src`.
#[test]
fn email_conformance_vectors() {
    for &(src, _expect) in vectors::SYNC {
        let out = html(|| image().src(src).alt("x").build());
        match classify(src) {
            ImageSrc::Data(_) | ImageSrc::Remote(_) => {
                assert!(out.contains(&format!(r#"src="{src}""#)), "{src:?}: {out}")
            }
            ImageSrc::Asset(_) | ImageSrc::Rejected(_) => {
                assert!(!out.contains("src="), "{src:?}: {out}")
            }
        }
    }
}

/// REGRESSION: a relative src went into the email verbatim, where a
/// webmail client resolves it against its own origin.
#[test]
fn regression_email_relative_src_has_no_src_attribute() {
    let out = html(|| image().src("/attachments/abc").alt("a").build());
    assert!(out.contains(r#"<img alt="a""#), "{out}");
}
