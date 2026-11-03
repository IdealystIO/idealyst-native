//! Regression: `dev-events` sat in mcp-server's `[target.'cfg(unix)'.dependencies]`
//! while `dev_sessions`, `build_wait`, `dev_status` and `dev_runner` use it on
//! every host, so mcp-server — and with it the whole `idealyst` CLI — stopped
//! compiling for Windows. A host-only test can't cross-compile, so this checks
//! the manifest: the crates those always-compiled modules import must be plain
//! `[dependencies]`, not in any target-gated table.

const MANIFEST: &str = include_str!("../Cargo.toml");

/// The table a `name = …` dependency line sits in.
fn table_of(dep: &str) -> Option<String> {
    let mut table = String::new();
    for line in MANIFEST.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            table = t.to_string();
        } else if t.starts_with(&format!("{dep} ")) || t.starts_with(&format!("{dep}=")) {
            return Some(table.clone());
        }
    }
    None
}

#[test]
fn regression_dev_events_is_a_dependency_on_every_host() {
    assert_eq!(table_of("dev-events").as_deref(), Some("[dependencies]"));
}
