//! The `image` mount handler resolves a root-relative `src` against the
//! image base (`runtime_shared::image_src::set_base_url`) before any
//! backend sees it, so every backend loads the same absolute URL.
//!
//! Its own test binary: the base is process-wide, and a test in a shared
//! binary that set it would race the others.

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::image_src::set_base_url;
use runtime_vocabulary::builders::image;
use runtime_world::signal;

fn image_ops(log: &[String]) -> Vec<String> {
    log.iter()
        .filter(|l| l.starts_with("create") || l.starts_with("update_image_src"))
        .cloned()
        .collect()
}

/// REGRESSION: a root-relative `src` (`/assets/logo.svg`) reached every
/// backend unresolved and was rejected, even after `server::configure`
/// named the server it lives on. Static and live srcs, at create and on
/// update, now arrive joined to the base; without a base they arrive
/// unchanged, for a page backend to resolve against the page.
#[test]
fn regression_root_relative_src_reaches_backends_joined_to_the_base() {
    let h = Harness::new();
    h.record_all();
    let world = h.world.clone();

    set_base_url("https://api.example.com/");
    let (realized, src) = world.enter(|| {
        let src = signal(String::from("/assets/logo.svg"));
        let realized = realize(&h.backend, &h.registry, image().src(move || src.get()).build());
        (realized, src)
    });
    let log = h.take_log();
    let ops = image_ops(&log);
    assert!(
        ops.iter().any(|l| l.contains("image \"https://api.example.com/assets/logo.svg\"")),
        "create_image gets the joined URL: {log:?}"
    );
    assert!(
        ops.contains(&"update_image_src n0 \"https://api.example.com/assets/logo.svg\"".to_string()),
        "the mount-time src apply gets the joined URL: {log:?}"
    );

    src.set("/uploads/x.png".to_string());
    world.flush();
    assert_eq!(
        image_ops(&h.take_log()),
        vec!["update_image_src n0 \"https://api.example.com/uploads/x.png\"".to_string()],
        "a live src change is joined too"
    );

    // Absolute and document-relative srcs are never touched.
    src.set("https://cdn.example.com/a.png".to_string());
    world.flush();
    src.set("logo.png".to_string());
    world.flush();
    assert_eq!(
        image_ops(&h.take_log()),
        vec![
            "update_image_src n0 \"https://cdn.example.com/a.png\"".to_string(),
            "update_image_src n0 \"logo.png\"".to_string(),
        ]
    );

    // No base: the path reaches the backend unchanged.
    set_base_url("");
    src.set("/assets/logo.svg".to_string());
    world.flush();
    assert_eq!(
        image_ops(&h.take_log()),
        vec!["update_image_src n0 \"/assets/logo.svg\"".to_string()]
    );
    drop(realized);
}
