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
    Config {
        url: Box::leak(format!("file://{}", dir.display()).into_boxed_str()),
        public_keys: &[],
        app: "remote-showcase",
        resolver: None,
        host_fns: None,
    }
}

struct Launch {
    h: Harness,
    ota: ota::Ota,
    _realized: runtime_scene::Realized<host_mock::Node>,
}

impl Launch {
    fn start(url_dir: &std::path::Path, cache: &std::path::Path, built_in: bool, apply: Apply) -> Launch {
        Launch::start_with(config(url_dir), cache, built_in, apply)
    }

    fn start_with(config: Config, cache: &std::path::Path, built_in: bool, apply: Apply) -> Launch {
        pump::install_executor();
        pump::install_scheduler();
        let h = Harness::new();
        let ota = h
            .world
            .enter(|| {
                ota::start(
                    config,
                    Options {
                        host_fns: host_fns(),
                        built_in: if built_in { vec![("showcase", BUILT_IN)] } else { vec![] },
                        apply,
                        cache_dir: Some(cache.to_path_buf()),
                        check_every: None,
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

    /// Let downloads over the platform's HTTP stack finish (real I/O, not
    /// the pumped executor alone): until `done`, or 30 s.
    fn settle_until(&self, done: impl Fn(&Launch) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !done(self) && std::time::Instant::now() < deadline {
            self.settle();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn answered_by(&self) -> Option<ota::AnsweredBy> {
        self.h.world.enter(|| runtime_world::untrack(|| self.ota.checks().get())).answered_by
    }

    /// The showcase bundle's state.
    fn bundle(&self) -> ota::BundleState {
        let all = self.h.world.enter(|| runtime_world::untrack(|| self.ota.bundles().get()));
        all.into_iter().find(|b| b.name == "showcase").expect("the showcase bundle is listed")
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
    publish(&target, &[upload(v1.clone())], 1, "test").unwrap();

    let first = Launch::start(releases.path(), cache.path(), false, Apply::NextLaunch);
    assert!(!first.screen().contains(FEED), "nothing to show before the download:\n{}", first.screen());
    first.settle();
    assert!(first.screen().contains(FEED), "{}", first.screen());
    assert_eq!(first.status(), Status::UpToDate);
    let state = first.bundle();
    assert_eq!(state.latest.map(|l| l.version).as_deref(), Some("1.0.0"));
    let running = state.running.expect("running");
    assert_eq!((running.version.as_deref(), running.from), (Some("1.0.0"), ota::Source::Download));
    assert_eq!(state.activity, ota::Activity::Idle);
    let checks = first.h.world.enter(|| runtime_world::untrack(|| first.ota.checks().get()));
    assert!(checks.last.is_some() && checks.next.is_none(), "{checks:?}");
    drop(first);

    let offline = tempfile::tempdir().unwrap(); // no index there
    let second = Launch::start(offline.path(), cache.path(), false, Apply::NextLaunch);
    assert!(second.screen().contains(FEED), "the cached bundle runs at launch:\n{}", second.screen());
    assert_eq!(second.bundle().running.map(|r| r.from), Some(ota::Source::Cache));
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
    publish(&target, &[upload(v1.clone())], 1, "test").unwrap();
    let planned = publish(&target, &[upload(needs_newer_app("2.0.0"))], 2, "test").unwrap();
    assert_eq!(planned[0].new_requirements, ["prop `idea_ui::components::button::Button.glow`"]);

    let launch = Launch::start(releases.path(), cache.path(), false, Apply::NextLaunch);
    launch.settle();
    assert!(launch.screen().contains(FEED), "{}", launch.screen());
    assert_eq!(launch.status(), Status::AppUpdateRequired);
    let state = launch.bundle();
    assert_eq!((state.latest.map(|l| l.version).as_deref(), state.newer_needs_app_update), (Some("1.0.0"), true));
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
    publish(&target, &[upload(v2.clone())], 1, "test").unwrap();

    let launch = Launch::start(releases.path(), cache.path(), true, Apply::NextLaunch);
    assert!(launch.screen().contains(FEED), "the built-in bundle runs at once");
    let remounts = launch.ota.remote().__generation();
    launch.settle();
    assert_eq!(launch.status(), Status::UpdateReady);
    let state = launch.bundle();
    assert_eq!(state.running.map(|r| r.from), Some(ota::Source::BuiltIn));
    assert_eq!(state.activity, ota::Activity::Ready("2.0.0".into()));
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
    publish(&Target::Dir(releases.path().into()), &[upload(release("2.0.0"))], 1, "test").unwrap();
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
    let config = Config { url: Box::leak(url.into_boxed_str()), public_keys: &[], app: "remote-showcase", resolver: None, host_fns: None };
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

/// With `check_every` and `Apply::Now`, a release published while the app
/// runs is picked up by the next scheduled check and swapped in: the
/// over-the-air demo's loop (`crates/ota/demo`).
#[test]
fn a_release_published_while_running_is_picked_up_by_the_next_check() {
    let releases = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let target = Target::Dir(releases.path().into());
    publish(&target, &[upload(release("1.0.0"))], 1, "test").unwrap();

    pump::install_executor();
    pump::install_scheduler();
    let h = Harness::new();
    let ota = h
        .world
        .enter(|| {
            ota::start(
                config(releases.path()),
                Options {
                    host_fns: host_fns(),
                    apply: Apply::Now,
                    cache_dir: Some(cache.path().into()),
                    check_every: Some(std::time::Duration::from_secs(5)),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let tree = h.world.enter(|| ui! { App() });
    let _realized = h.mount(tree);
    let settle = || {
        for _ in 0..4 {
            pump::pump_timers();
            pump::pump_tasks();
            h.flush();
        }
    };
    let running = || {
        let all = h.world.enter(|| runtime_world::untrack(|| ota.bundles().get()));
        all.into_iter().find(|b| b.name == "showcase").and_then(|b| b.running).and_then(|r| r.version)
    };
    settle();
    assert_eq!(running().as_deref(), Some("1.0.0"));
    let checks = h.world.enter(|| runtime_world::untrack(|| ota.checks().get()));
    assert_eq!(checks.next.zip(checks.last).map(|(n, l)| n - l), Some(5), "{checks:?}");

    publish(&target, &[upload(release("1.1.0"))], 2, "test").unwrap();
    let remounts = ota.remote().__generation();
    settle();
    assert_eq!(running().as_deref(), Some("1.1.0"), "the scheduled check swapped it in");
    assert!(ota.remote().__generation() > remounts);
    let screen = h.live_roots().iter().map(|n| h.live_tree(*n)).collect::<Vec<_>>().join("\n");
    assert!(screen.contains(FEED), "{screen}");
}

/// Taking the running release down: without the kill switch an app keeps
/// it until the next launch (the replacement waits, downloaded); with it,
/// the app swaps to the release it would now choose at once, whatever its
/// `apply` setting.
#[test]
fn a_take_down_waits_for_the_next_launch_unless_its_the_kill_switch() {
    let releases = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let target = Target::Dir(releases.path().into());
    let (v1, v2) = (release("1.0.0"), release("2.0.0"));
    publish(&target, &[upload(v1.clone())], 1, "test").unwrap();
    publish(&target, &[upload(v2.clone())], 2, "test").unwrap();
    let launch = Launch::start(releases.path(), cache.path(), false, Apply::NextLaunch);
    launch.settle();
    let version = |l: &Launch| l.bundle().running.and_then(|r| r.version);
    assert_eq!(version(&launch).as_deref(), Some("2.0.0"));

    let v2_sha = remote_bundle::content_hash(&v2);
    ota_publish::take_down(&target, "showcase", &v2_sha, ota_publish::TakeDown::default(), "test").unwrap();
    launch.ota.check();
    launch.settle();
    assert_eq!(version(&launch).as_deref(), Some("2.0.0"), "a plain take-down waits for the next launch");
    assert_eq!(launch.status(), Status::UpdateReady);
    assert_eq!(launch.bundle().activity, ota::Activity::Ready("1.0.0".into()));

    // Taken down again, urgently: restore, then kill.
    ota_publish::restore(&target, "showcase", &v2_sha, "test").unwrap();
    ota_publish::take_down(&target, "showcase", &v2_sha, ota_publish::TakeDown { urgent: true, reason: None }, "test").unwrap();
    let remounts = launch.ota.remote().__generation();
    launch.ota.check();
    launch.settle();
    assert_eq!(version(&launch).as_deref(), Some("1.0.0"), "the kill switch swapped it at once");
    assert!(launch.ota.remote().__generation() > remounts);
    assert!(launch.screen().contains(FEED), "{}", launch.screen());
}

/// The kill switch on the only release: back to the built-in copy if there
/// is one, else the bundle stops running and its screens hold an empty place.
#[test]
fn killing_the_only_release_falls_back_to_the_built_in_copy_or_nothing() {
    for built_in in [true, false] {
        let releases = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let target = Target::Dir(releases.path().into());
        let v2 = release("2.0.0");
        publish(&target, &[upload(v2.clone())], 1, "test").unwrap();
        let launch = Launch::start(releases.path(), cache.path(), built_in, Apply::Now);
        launch.settle();
        assert_eq!(launch.bundle().running.and_then(|r| r.version).as_deref(), Some("2.0.0"));

        let how = ota_publish::TakeDown { urgent: true, reason: Some("broken".into()) };
        ota_publish::take_down(&target, "showcase", &remote_bundle::content_hash(&v2), how, "test").unwrap();
        launch.ota.check();
        launch.settle();
        let state = launch.bundle();
        assert!(matches!(&state.activity, ota::Activity::Failed(e) if e.contains("kill switch")), "{state:?}");
        if built_in {
            assert_eq!(state.running.map(|r| r.from), Some(ota::Source::BuiltIn));
            assert!(launch.screen().contains(FEED), "the built-in copy runs:\n{}", launch.screen());
        } else {
            assert_eq!(state.running, None);
            assert!(!launch.screen().contains(FEED), "the killed bundle no longer runs:\n{}", launch.screen());
        }
    }
}

/// This build, registered from its build (`idealyst ota manifest`): the
/// app reads its precomputed answer and needs no index at all.
#[test]
fn a_registered_build_reads_its_precomputed_answer() {
    let releases = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let target = Target::Dir(releases.path().into());
    publish(&target, &[upload(release("1.0.0"))], 1, "test").unwrap();
    let manifest = ota::manifest(&host_fns());
    ota_publish::register(&target, &manifest, ota::index::ManifestSource::Build, Some("showcase".into()), None).unwrap();
    std::fs::remove_file(releases.path().join("index.json")).unwrap();

    let app = Launch::start(releases.path(), cache.path(), false, Apply::NextLaunch);
    app.settle();
    assert!(app.screen().contains(FEED), "{}", app.screen());
    assert_eq!(app.answered_by(), Some(ota::AnsweredBy::Precomputed));
    assert_eq!(app.ota.manifest_id(), manifest.id);
}

/// An answer stored under this build's id but made for another manifest
/// (a newer app that can run 2.0.0) is never acted on: the app decides
/// from the index, and keeps the release it can run.
#[test]
fn an_answer_made_for_another_build_is_never_used() {
    let releases = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let target = Target::Dir(releases.path().into());
    publish(&target, &[upload(release("1.0.0"))], 1, "test").unwrap();
    publish(&target, &[upload(needs_newer_app("2.0.0"))], 2, "test").unwrap();
    let mine = ota::manifest(&host_fns());
    let mut newer = mine.provides.clone();
    newer.components.get_mut("idea_ui::components::button::Button").unwrap().insert("glow".into(), "bool".into());
    let index = ota_publish::read_index(&target).unwrap();
    let theirs = ota::index::resolve(&index, &newer);
    assert_eq!(theirs.bundles["showcase"].release.as_ref().unwrap().version, "2.0.0");
    let path = releases.path().join(ota::index::resolved_path(&mine.id));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, theirs.to_json()).unwrap();

    let app = Launch::start(releases.path(), cache.path(), false, Apply::NextLaunch);
    app.settle();
    assert_eq!(app.answered_by(), Some(ota::AnsweredBy::Index));
    assert_eq!(app.bundle().running.and_then(|r| r.version).as_deref(), Some("1.0.0"));
    assert_eq!(app.status(), Status::AppUpdateRequired);
}

/// The resolution service (`ota-resolver`) on a free port, reading
/// `releases`, with each request's keys recorded on the way in: the real
/// router, behind a layer that only looks.
struct Service {
    url: String,
    requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Service {
    fn start(releases: std::path::PathBuf) -> Service {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = requests.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let settings = ota_resolver::Settings { index_ttl: std::time::Duration::ZERO, ..Default::default() };
                let resolver = ota_resolver::Resolver::new(Target::Dir(releases), settings);
                let record = move |req: axum::extract::Request, next: axum::middleware::Next| {
                    let seen = seen.clone();
                    async move {
                        let (parts, body) = req.into_parts();
                        let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
                        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
                        let mut keys: Vec<String> = json.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
                        keys.sort();
                        seen.lock().unwrap().push(keys.join("+"));
                        next.run(axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes))).await
                    }
                };
                let app = ota_resolver::router(std::sync::Arc::new(resolver)).layer(axum::middleware::from_fn(record));
                axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(), app).await.unwrap();
            });
        });
        Service { url, requests }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// With a resolution service configured, the app sends its manifest id;
/// only when the service doesn't know it does it send the manifest, once.
/// The next launch sends the id alone. With the service gone, the app
/// still updates, from the location.
#[test]
fn a_resolution_service_is_asked_by_id_and_sent_the_manifest_once() {
    let releases = tempfile::tempdir().unwrap();
    let target = Target::Dir(releases.path().into());
    publish(&target, &[upload(release("1.0.0"))], 1, "test").unwrap();
    let service = Service::start(releases.path().into());
    let with_service = Config { resolver: Some(Box::leak(service.url.clone().into_boxed_str())), ..config(releases.path()) };

    let cache = tempfile::tempdir().unwrap();
    let first = Launch::start_with(with_service, cache.path(), false, Apply::NextLaunch);
    first.settle_until(|l| l.screen().contains(FEED));
    assert!(first.screen().contains(FEED), "{:?}\n{}", first.status(), first.screen());
    assert_eq!(first.answered_by(), Some(ota::AnsweredBy::Service));
    assert_eq!(service.requests(), ["manifest", "manifest+provides"]);
    drop(first);

    let fresh = tempfile::tempdir().unwrap();
    let second = Launch::start_with(with_service, fresh.path(), false, Apply::NextLaunch);
    second.settle_until(|l| l.screen().contains(FEED));
    assert_eq!(second.answered_by(), Some(ota::AnsweredBy::Service));
    assert_eq!(service.requests(), ["manifest", "manifest+provides", "manifest"], "known now: the id alone");
    drop(second);

    // Nothing listens here: the check falls through to the location.
    let gone = Config { resolver: Some("http://127.0.0.1:9"), ..config(releases.path()) };
    let third = Launch::start_with(gone, tempfile::tempdir().unwrap().path(), false, Apply::NextLaunch);
    third.settle_until(|l| l.answered_by().is_some());
    assert_eq!(third.answered_by(), Some(ota::AnsweredBy::Index));
}
