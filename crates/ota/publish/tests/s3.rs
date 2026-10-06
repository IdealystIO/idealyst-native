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

fn target() -> Option<Target> {
    let Ok(prefix) = std::env::var("IDEALYST_OTA_TEST_S3") else {
        eprintln!("skipped: set IDEALYST_OTA_TEST_S3 to an s3:// prefix to run");
        return None;
    };
    let run = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    Some(Target::parse(&format!("{}/{run}", prefix.trim_end_matches('/'))).unwrap())
}

/// Eight publishes racing to one index: every release lands. The index is
/// only written if no one changed it since it was read; a loser re-reads
/// and re-applies its change.
#[test]
fn racing_publishes_all_land() {
    let Some(target) = target() else { return };
    let threads: Vec<_> = (0..8)
        .map(|n| {
            let target = target.clone();
            std::thread::spawn(move || publish(&target, &[Upload { name: format!("b{n}"), wasm: bundle(n) }], 1).unwrap())
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
    let Some(target) = target() else { return };
    publish(&target, &[Upload { name: "shop".into(), wasm: bundle(1) }], 1).unwrap();
    let again = publish(&target, &[Upload { name: "shop".into(), wasm: bundle(1) }], 2).unwrap();
    assert!(again[0].unchanged);
    publish(&target, &[Upload { name: "shop".into(), wasm: bundle(2) }], 3).unwrap();
    assert_eq!(read_index(&target).unwrap().bundles["shop"].releases[0].version, "1.0.2");
    assert_eq!(rollback(&target, "shop").unwrap().unwrap().version, "1.0.1");
    let shop = &read_index(&target).unwrap().bundles["shop"];
    assert_eq!((shop.releases.len(), shop.withdrawn[0].version.as_str()), (1, "1.0.2"));
}
