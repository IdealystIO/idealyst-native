//! Against a real S3 API: set `IDEALYST_OTA_TEST_S3` to a writable prefix
//! (`s3://ota-test/race`), with the `aws` CLI's credentials and, for
//! MinIO, `AWS_ENDPOINT_URL`. Skipped otherwise. See docs/ota.md,
//! "Testing against S3".

use ota_publish::{publish, read_index, rollback, Target, Upload};

/// A release build as far as publishing reads one: metadata and
/// requirements on a minimal module, made distinct by `n`.
fn bundle(n: u32) -> Vec<u8> {
    let module = b"\0asm\x01\0\0\0".to_vec();
    let meta = remote_bundle::Metadata { name: format!("b{n}"), package: "p".into(), version: format!("1.0.{n}"), codec: 2 };
    let wasm = remote_bundle::with_metadata(&module, &meta).unwrap();
    remote_bundle::with_requires(&wasm, &remote_bundle::Requires::default()).unwrap()
}

/// A fresh prefix under `IDEALYST_OTA_TEST_S3`, reached through the `aws`
/// CLI — or, with `http`, the console's own signed requests (`S3Http`,
/// which reads the same AWS_* variables).
fn target_via(test: &str, http: bool) -> Option<Target> {
    let Ok(prefix) = std::env::var("IDEALYST_OTA_TEST_S3") else {
        eprintln!("skipped: set IDEALYST_OTA_TEST_S3 to an s3:// prefix to run");
        return None;
    };
    let run = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let location = format!("{}/{test}-{run}", prefix.trim_end_matches('/'));
    if http {
        #[cfg(feature = "s3-http")]
        return Some(Target::S3Http(ota_publish::S3Http::from_env(&location).unwrap()));
        #[cfg(not(feature = "s3-http"))]
        {
            eprintln!("skipped: build with --features s3-http");
            return None;
        }
    }
    Some(Target::parse(&location).unwrap())
}

fn target(test: &str) -> Option<Target> {
    target_via(test, false)
}

/// Eight publishes racing to one index: every release lands. The index is
/// only written if no one changed it since it was read; a loser re-reads
/// and re-applies its change.
#[test]
fn racing_publishes_all_land() {
    let Some(target) = target("race") else { return };
    let threads: Vec<_> = (0..8)
        .map(|n| {
            let target = target.clone();
            std::thread::spawn(move || publish(&target, &[Upload { name: format!("b{n}"), wasm: bundle(n) }], 1, "test").unwrap())
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let index = read_index(&target).unwrap();
    assert_eq!(index.bundles.len(), 8, "{:?}", index.bundles.keys().collect::<Vec<_>>());
}

#[test]
fn publish_twice_and_roll_back() {
    let Some(target) = target("rollback") else { return };
    publish_twice_and_roll_back_at(target);
}

/// The same, through the console's signed HTTP requests.
#[test]
fn over_signed_http_too() {
    let Some(target) = target_via("http", true) else { return };
    publish_twice_and_roll_back_at(target.clone());
    // And the take-down, pin and audit trail the console drives.
    publish(&target, &[Upload { name: "shop".into(), wasm: bundle(3) }], 4, "test").unwrap();
    ota_publish::pin(&target, "shop", &remote_bundle::content_hash(&bundle(1)), "console").unwrap();
    let index = read_index(&target).unwrap();
    assert_eq!(index.bundles["shop"].releases[0].version, "1.0.1", "pinned ahead of 1.0.3");
    let audit = ota_publish::read_audit(&target).unwrap();
    assert_eq!((audit[0].action, audit[0].actor.as_str()), (ota_publish::Action::Pin, "console"));
}

fn publish_twice_and_roll_back_at(target: Target) {
    publish(&target, &[Upload { name: "shop".into(), wasm: bundle(1) }], 1, "test").unwrap();
    let again = publish(&target, &[Upload { name: "shop".into(), wasm: bundle(1) }], 2, "test").unwrap();
    assert!(again[0].unchanged);
    publish(&target, &[Upload { name: "shop".into(), wasm: bundle(2) }], 3, "test").unwrap();
    assert_eq!(read_index(&target).unwrap().bundles["shop"].releases[0].version, "1.0.2");
    assert_eq!(rollback(&target, "shop", "test").unwrap().unwrap().version, "1.0.1");
    let shop = &read_index(&target).unwrap().bundles["shop"];
    assert_eq!((shop.releases.len(), shop.withdrawn[0].release.version.as_str()), (1, "1.0.2"));
}

/// Registering a build and keeping its answer current, against the real
/// conditional writes: through the `aws` CLI and through signed HTTP.
fn registered_answers_follow_the_index_at(target: Target) {
    use ota_index::{Manifest, ManifestSource};
    publish(&target, &[Upload { name: "shop".into(), wasm: bundle(1) }], 1, "test").unwrap();
    let manifest = Manifest::new(remote_bundle::Provides { codec: 2, ..Default::default() });
    let added = ota_publish::register(&target, &manifest, ManifestSource::Build, Some("app 1.0".into()), None).unwrap();
    assert_eq!(added, ota_publish::Registration::Added);
    assert_eq!(ota_publish::read_manifest(&target, &manifest.id).unwrap().unwrap(), manifest);

    publish(&target, &[Upload { name: "shop".into(), wasm: bundle(2) }], 2, "test").unwrap();
    let stored = ota_publish::read_answer(&target, &manifest.id).unwrap().unwrap();
    assert_eq!(stored, ota_index::resolve(&read_index(&target).unwrap(), &manifest.provides));
    assert_eq!(stored.bundles["shop"].release.as_ref().unwrap().version, "1.0.2");
    // The publish already rewrote it: nothing left to do.
    let r = ota_publish::resolve_registered(&target).unwrap();
    assert_eq!((r.written, r.current, r.failed.len()), (0, 1, 0));
}

#[test]
fn registered_answers_follow_the_index() {
    let Some(target) = target("answers") else { return };
    registered_answers_follow_the_index_at(target);
}

#[test]
fn registered_answers_follow_the_index_over_signed_http() {
    let Some(target) = target_via("answers-http", true) else { return };
    registered_answers_follow_the_index_at(target);
}

/// Builds reported from the field are markers of their own, found by
/// listing: a burst of them all lands, through the real store.
fn reports_are_listed_at(target: Target) {
    use ota_index::{Manifest, ManifestSource};
    let threads: Vec<_> = (0..16u32)
        .map(|n| {
            let target = target.clone();
            std::thread::spawn(move || {
                let mut p = remote_bundle::Provides { codec: n, ..Default::default() };
                p.components.insert("ui::Card".into(), Default::default());
                ota_publish::register(&target, &Manifest::new(p), ManifestSource::Reported, None, None).unwrap()
            })
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), ota_publish::Registration::Added);
    }
    let registry = ota_publish::read_registry(&target).unwrap();
    assert_eq!(registry.manifests.len(), 16);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    assert!(registry.manifests.iter().all(|m| m.source == ManifestSource::Reported && now.abs_diff(m.registered_at) < 600), "{registry:?}");
}

#[test]
fn reports_are_listed() {
    let Some(target) = target("reports") else { return };
    reports_are_listed_at(target);
}

#[test]
fn reports_are_listed_over_signed_http() {
    let Some(target) = target_via("reports-http", true) else { return };
    reports_are_listed_at(target);
}
