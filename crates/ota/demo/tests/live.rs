//! The demo's window, against the mock backend, reading the live release
//! location in Cargo.toml (the local MinIO from ./setup.sh, with a release
//! published by ./publish.sh). `#[ignore]`d, so a plain `cargo test` lists
//! it as ignored instead of passing it unrun: run ./setup.sh and
//! ./publish.sh, then `cargo test -p ota-demo --test live -- --ignored`.

use host_mock::{pump, Harness};

#[test]
#[ignore = "needs the demo's MinIO with a release: run ./setup.sh and ./publish.sh, then cargo test -p ota-demo --test live -- --ignored"]
fn the_window_shows_the_updater_and_the_downloaded_panel() {
    let cache = tempfile::tempdir().unwrap();
    pump::install_executor();
    pump::install_scheduler();
    let h = Harness::new();
    let tree = h.world.enter(|| ota_demo::app_with(Some(cache.path().into())));
    let _realized = h.mount(tree);
    let screen = || h.live_roots().iter().map(|n| h.live_tree(*n)).collect::<Vec<_>>().join("\n");
    h.flush();
    let first = screen();
    assert!(first.contains("NATIVE · the updater") && first.contains("Checking for updates"), "{first}");
    assert!(first.contains("Fetching the panel's bundle"), "before the download:\n{first}");

    // Timers carry the deferred fetch of the shown panel (and the 5 s
    // re-checks, harmless here); tasks carry the downloads. The panel's
    // copy is the part you edit, so the test waits on the updater's line.
    let arrived = || screen().contains(", downloaded)") && !screen().contains("Fetching the panel's bundle");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !arrived() && std::time::Instant::now() < deadline {
        pump::pump_timers();
        pump::pump_tasks();
        h.flush();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let after = screen();
    println!("{after}");
    assert!(arrived(), "{after}");
    assert!(after.contains("REMOTE · from the bundle") && after.contains("bundle `panel`"), "{after}");
    assert!(after.contains("Up to date"), "{after}");
    // How the check got its answer, and under which manifest.
    // Either line names the manifest; the service or the precomputed answer
    // says "answered by", the index "decided here from the index".
    assert!(after.contains("manifest ") && (after.contains("answered by") || after.contains("decided here from the index")), "{after}");
}
