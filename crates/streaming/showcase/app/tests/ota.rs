//! The showcase updated over the air, end to end: its real bundle built as
//! a release (`build_remote::finish`, what `idealyst build --remote` runs),
//! published to a directory (`ota-publish`, what `idealyst ota publish`
//! runs), and read by the app's client (`ota`) through a `file://` URL —
//! the same files a CDN would serve.

#![cfg(not(feature = "inline"))]

use host_mock::{pump, Harness};
use ota::{Apply, Config, Options, Status};
use ota_publish::{publish, Target, Upload};
use runtime_core::ui;
use remote_showcase::{host_fns, App, BUILT_IN};

const FEED: &str = "Feed — rendered by the bundle";

/// A release build of the showcase bundle, at `version`.
fn release(version: &str) -> Vec<u8> {
    let spec = build_remote::BundleSpec { name: "showcase".into(), package: "remote-showcase".into() };
    build_remote::finish(BUILT_IN, &spec, version, None).expect("a release").0
}

/// `release`, claiming to need a prop this app's `Button` doesn't have:
/// what a bundle built against a newer app looks like.
fn needs_newer_app(version: &str) -> Vec<u8> {
    let wasm = release(version);
    let mut requires = remote_bundle::requires(&wasm).unwrap().unwrap();
    requires.components.get_mut("idea_ui::components::button::Button").unwrap().insert("glow".into(), "bool".into());
    remote_bundle::with_requires(&wasm, &requires).unwrap()
}

fn config(dir: &std::path::Path) -> Config {
    Config { url: Box::leak(format!("file://{}", dir.display()).into_boxed_str()), public_keys: &[], app: "remote-showcase" }
}

struct Launch {
    h: Harness,
    ota: ota::Ota,
    _realized: runtime_scene::Realized<host_mock::Node>,
}

impl Launch {
    fn start(url_dir: &std::path::Path, cache: &std::path::Path, built_in: bool, apply: Apply) -> Launch {
        pump::install_executor();
        pump::install_scheduler();
        let h = Harness::new();
        let ota = h
            .world
            .enter(|| {
                ota::start(
                    config(url_dir),
                    Options {
                        host_fns: host_fns(),
                        built_in: if built_in { vec![("showcase", BUILT_IN)] } else { vec![] },
                        apply,
                        cache_dir: Some(cache.to_path_buf()),
                    },
                )
            })
            .expect("starts");
        let tree = h.world.enter(|| ui! { App() });
        let realized = h.mount(tree);
        h.flush();
        Launch { h, ota, _realized: realized }
    }

    /// Let downloads and deferred work run.
    fn settle(&self) {
        for _ in 0..4 {
            pump::pump_timers();
            pump::pump_tasks();
            self.h.flush();
        }
    }

    fn screen(&self) -> String {
        self.h.live_roots().iter().map(|n| self.h.live_tree(*n)).collect::<Vec<_>>().join("\n")
    }

    fn status(&self) -> Status {
        self.h.world.enter(|| runtime_world::untrack(|| self.ota.status().get()))
    }
}

/// `ota::config!()` reads the app's own Cargo.toml.
#[test]
fn the_config_comes_from_cargo_toml() {
    let c = ota::config!();
    assert_eq!(c.app, "remote-showcase");
    assert_eq!(c.url, option_env!("IDEALYST_OTA_URL").unwrap_or("http://127.0.0.1:7878/ota"));
    assert!(c.public_keys.is_empty());
}

fn upload(wasm: Vec<u8>) -> Upload {
    Upload { name: "showcase".into(), wasm }
}

/// First launch, nothing built in: the screens show an empty place, their
/// bundle is found in the index and downloaded, and they mount from it.
/// The next launch runs it from the cache at once — offline.
#[test]
fn a_first_launch_downloads_the_bundle_and_the_next_runs_it_offline() {
    let releases = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let target = Target::Dir(releases.path().into());
    let v1 = release("1.0.0");
    publish(&target, &[upload(v1.clone())], 1).unwrap();

    let first = Launch::start(releases.path(), cache.path(), false, Apply::NextLaunch);
    assert!(!first.screen().contains(FEED), "nothing to show before the download:\n{}", first.screen());
    first.settle();
    assert!(first.screen().contains(FEED), "{}", first.screen());
    assert_eq!(first.status(), Status::UpToDate);
    drop(first);

    let offline = tempfile::tempdir().unwrap(); // no index there
    let second = Launch::start(offline.path(), cache.path(), false, Apply::NextLaunch);
    assert!(second.screen().contains(FEED), "the cached bundle runs at launch:\n{}", second.screen());
    second.settle();
    assert!(matches!(second.status(), Status::Failed(_)), "{:?}", second.status());
    assert!(second.screen().contains(FEED), "and keeps running offline");
}

/// A release that needs more than this app has is never run: the app runs
/// the newest one it can, and says an app update would bring the rest.
#[test]
fn a_release_needing_a_newer_app_is_skipped_and_reported() {
    let releases = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let target = Target::Dir(releases.path().into());
    let v1 = release("1.0.0");
    publish(&target, &[upload(v1.clone())], 1).unwrap();
    let planned = publish(&target, &[upload(needs_newer_app("2.0.0"))], 2).unwrap();
    assert_eq!(planned[0].new_requirements, ["prop `idea_ui::components::button::Button.glow`"]);

    let launch = Launch::start(releases.path(), cache.path(), false, Apply::NextLaunch);
    launch.settle();
    assert!(launch.screen().contains(FEED), "{}", launch.screen());
    assert_eq!(launch.status(), Status::AppUpdateRequired);
    let state = std::fs::read_to_string(cache.path().join("state.json")).unwrap();
    assert!(state.contains(&remote_bundle::content_hash(&v1)), "runs 1.0.0: {state}");
}

/// With built-in bundles, an update downloads in the background and runs
/// from the next launch: the screen doesn't change under the user.
#[test]
fn an_update_downloads_and_waits_for_the_next_launch() {
    let releases = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let target = Target::Dir(releases.path().into());
    let v2 = release("2.0.0");
    publish(&target, &[upload(v2.clone())], 1).unwrap();

    let launch = Launch::start(releases.path(), cache.path(), true, Apply::NextLaunch);
    assert!(launch.screen().contains(FEED), "the built-in bundle runs at once");
    let remounts = launch.ota.remote().__generation();
    launch.settle();
    assert_eq!(launch.status(), Status::UpdateReady);
    assert_eq!(launch.ota.remote().__generation(), remounts, "nothing remounted");
    let state = std::fs::read_to_string(cache.path().join("state.json")).unwrap();
    assert!(state.contains(&remote_bundle::content_hash(&v2)), "2.0.0 runs from the next launch: {state}");
    assert!(cache.path().join("bundles").join(format!("{}.wasm", remote_bundle::content_hash(&v2))).exists());

    // `Apply::Now` swaps it in at once instead.
    drop(launch);
    let cache_now = tempfile::tempdir().unwrap();
    let now = Launch::start(releases.path(), cache_now.path(), true, Apply::Now);
    let remounts = now.ota.remote().__generation();
    now.settle();
    assert_eq!(now.status(), Status::UpToDate);
    assert!(now.ota.remote().__generation() > remounts, "remounted from the new release");
    assert!(now.screen().contains(FEED));
}

/// An app that requires signatures neither runs nor caches an unsigned
/// release; it keeps its built-in bundle and reports why.
#[test]
fn an_unsigned_release_is_refused_when_signatures_are_required() {
    let releases = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    publish(&Target::Dir(releases.path().into()), &[upload(release("2.0.0"))], 1).unwrap();
    let key = remote_bundle::SigningKey::generate().unwrap();
    let public: &'static str = Box::leak(key.public().to_hex().into_boxed_str());
    let keys: &'static [&'static str] = Box::leak(vec![public].into_boxed_slice());

    pump::install_executor();
    pump::install_scheduler();
    let h = Harness::new();
    // The built-in copy would need signing too: none, so the first screen
    // waits for a download that is refused.
    let ota = h
        .world
        .enter(|| {
            ota::start(
                Config { public_keys: keys, ..config(releases.path()) },
                Options { host_fns: host_fns(), cache_dir: Some(cache.path().into()), ..Default::default() },
            )
        })
        .unwrap();
    let tree = h.world.enter(|| ui! { App() });
    let _realized = h.mount(tree);
    for _ in 0..4 {
        pump::pump_timers();
        pump::pump_tasks();
        h.flush();
    }
    let status = h.world.enter(|| runtime_world::untrack(|| ota.status().get()));
    assert!(matches!(&status, Status::Failed(e) if e.contains("not signed")), "{status:?}");
    assert_eq!(std::fs::read_dir(cache.path().join("bundles")).unwrap().count(), 0, "nothing cached");
}

/// Over real HTTP, from an S3-compatible store: set `IDEALYST_OTA_TEST_URL`
/// to a release location `idealyst ota publish` wrote the showcase to (a
/// MinIO bucket, say — `docs/ota.md`, "Testing against S3"). Skipped
/// otherwise: it needs the network and a published release.
#[test]
fn over_http_from_s3() {
    let Ok(url) = std::env::var("IDEALYST_OTA_TEST_URL") else {
        eprintln!("skipped: set IDEALYST_OTA_TEST_URL to run against a published release");
        return;
    };
    let cache = tempfile::tempdir().unwrap();
    pump::install_executor();
    pump::install_scheduler();
    let h = Harness::new();
    let config = Config { url: Box::leak(url.into_boxed_str()), public_keys: &[], app: "remote-showcase" };
    let ota = h
        .world
        .enter(|| ota::start(config, Options { host_fns: host_fns(), cache_dir: Some(cache.path().into()), ..Default::default() }))
        .unwrap();
    let tree = h.world.enter(|| ui! { App() });
    let _realized = h.mount(tree);
    let screen = || h.live_roots().iter().map(|n| h.live_tree(*n)).collect::<Vec<_>>().join("\n");
    // The downloads complete on the platform's HTTP stack: wait for them.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !screen().contains(FEED) && std::time::Instant::now() < deadline {
        pump::pump_timers();
        pump::pump_tasks();
        h.flush();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let status = h.world.enter(|| runtime_world::untrack(|| ota.status().get()));
    assert!(screen().contains(FEED), "status {status:?}:\n{}", screen());
    assert_eq!(status, Status::UpToDate);
    assert_eq!(std::fs::read_dir(cache.path().join("bundles")).unwrap().count(), 1, "the download is cached");
}
