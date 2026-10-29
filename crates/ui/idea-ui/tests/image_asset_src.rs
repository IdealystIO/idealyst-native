//! idea-ui's `Image` takes an `ImageSource`, so a bundled `asset!` image
//! reaches the primitive's asset path, not just a URL string.

use idea_ui::{install_idea_theme, light_theme, Image};
use runtime_core::assets::{kinds, Asset};
use runtime_core::{asset, memo, rx, signal, ui};

static LOGO: Asset<kinds::Image> = asset!("logo.svg");

/// REGRESSION: `Image` only took `src: String`, so idea-ui apps could not
/// use `asset!`, the fix the rejected-src warning pointed them to. An
/// asset `src` registers the asset with the backend and renders through
/// the `asset://<id>` sentinel.
#[test]
fn regression_image_accepts_a_bundled_asset() {
    let harness = host_mock::Harness::new();
    harness.record_all();
    let tree = harness.world.enter(|| {
        install_idea_theme(light_theme());
        ui! { Image(src = LOGO, alt = Some("Logo".to_string())) }
    });
    let _realized = harness.mount(tree);
    harness.flush();
    let ops = harness.ops().join("\n");
    assert!(ops.contains(&format!("register_asset {:?} Image", LOGO.id)), "asset registered:\n{ops}");
    assert!(ops.contains(&format!("update_image_src n")) && ops.contains(&format!("\"asset://{}\"", LOGO.id.0)), "{ops}");
}

/// A string `src` still works as before: a literal, and each live form an
/// author holds (`signal`, `memo`, `rx!`), which repaints in place.
#[test]
fn string_and_live_srcs_still_render() {
    let harness = host_mock::Harness::new();
    harness.record_all();
    let (tree, url) = harness.world.enter(|| {
        install_idea_theme(light_theme());
        let url = signal(String::from("https://a.io/1.png"));
        let tree = ui! {
            view() {
                Image(src = "https://a.io/static.png")
                Image(src = url)
                Image(src = memo(move || format!("{}?memo", url.get())))
                Image(src = rx!(format!("{}?rx", url.get())))
            }
        };
        (tree, url)
    });
    let _realized = harness.mount(tree);
    harness.flush();
    harness.world.enter(|| url.set("https://a.io/2.png".to_string()));
    harness.flush();
    let ops = harness.ops().join("\n");
    for want in [
        "https://a.io/static.png",
        "https://a.io/1.png",
        "https://a.io/2.png",
        "https://a.io/2.png?memo",
        "https://a.io/2.png?rx",
    ] {
        assert!(ops.contains(want), "{want}:\n{ops}");
    }
}
