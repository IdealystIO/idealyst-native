use std::fs;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("fixtures");
    p.push(name);
    p
}

#[test]
fn counter_fixture_ports_to_expected_rust() {
    let source = fs::read_to_string(fixture("counter.svelte")).expect("read counter.svelte");
    let (rendered, report) = port_svelte::port(&source).expect("port should succeed");

    let expected_path = fixture("counter.expected.rs");
    if std::env::var("UPDATE_SNAPSHOT").is_ok() {
        fs::write(&expected_path, &rendered).expect("update snapshot");
        eprintln!("snapshot updated: {}", expected_path.display());
    }

    let expected = fs::read_to_string(&expected_path)
        .expect("counter.expected.rs missing — run with UPDATE_SNAPSHOT=1");
    assert_eq!(rendered, expected, "snapshot drift");

    assert!(report.holes.iter().any(|h| h.original.text.contains("console.log")));
}
