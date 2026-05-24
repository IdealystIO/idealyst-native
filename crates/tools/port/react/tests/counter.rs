//! End-to-end snapshot of the Counter fixture.
//!
//! Goals:
//! - prove the parser → IR → emitter pipeline wires up,
//! - lock in the shape of the emitted Rust (changes here are
//!   visible diffs in CI, intentional or otherwise),
//! - exercise the hole reporting path.
//!
//! Run with `UPDATE_SNAPSHOT=1 cargo test -p port-react` to
//! regenerate `fixtures/counter.expected.rs` after intentional
//! emitter changes.

use std::fs;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("fixtures");
    p.push(name);
    p
}

#[test]
fn counter_fixture_ports_to_expected_rust() {
    let source = fs::read_to_string(fixture_path("counter.tsx")).expect("read counter.tsx");
    let (rendered, report) = port_react::port(&source).expect("port should succeed");

    let expected_path = fixture_path("counter.expected.rs");
    if std::env::var("UPDATE_SNAPSHOT").is_ok() {
        fs::write(&expected_path, &rendered).expect("update snapshot");
        eprintln!("snapshot updated: {}", expected_path.display());
    }

    let expected = fs::read_to_string(&expected_path)
        .expect("counter.expected.rs missing — run with UPDATE_SNAPSHOT=1 to seed it");

    assert_eq!(
        rendered, expected,
        "emitter output drifted from snapshot. \
         Re-run with UPDATE_SNAPSHOT=1 if the change is intentional."
    );

    // Sanity-check that at least the console.log hole is reported.
    assert!(
        report.holes.iter().any(|h| h.original.text.contains("console.log")),
        "expected console.log hole in report, got: {:?}",
        report.holes,
    );
}
