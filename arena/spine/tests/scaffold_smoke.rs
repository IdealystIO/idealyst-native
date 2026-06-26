//! Smoke test for the isolation scaffold. Runs the REAL `idealyst new`, so it's
//! `#[ignore]` by default — run explicitly:
//!
//!   cargo test -p arena-spine --test scaffold_smoke -- --ignored --nocapture
//!
//! Verifies the two isolation invariants: the project's framework dep points at
//! the local tree (a `path = …`), and its `.mcp.json` exposes only idealyst.

use arena_spine::harness::scaffold;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // arena/spine → arena → repo root
    std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
        .expect("repo root")
}

#[test]
#[ignore = "runs the real idealyst CLI"]
fn scaffolds_a_path_dep_isolated_project() {
    let tmp = std::env::temp_dir().join("arena_scaffold_smoke");
    std::fs::remove_dir_all(&tmp).ok();
    std::fs::create_dir_all(&tmp).unwrap();

    let scaffold = scaffold::create("todo_smoke", &tmp, &repo_root()).expect("scaffold");

    let cargo = std::fs::read_to_string(scaffold.project_dir.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("runtime-core"), "must depend on runtime-core");
    assert!(
        cargo.contains("path ="),
        "framework dep must be a local path dep for current-code testing; got:\n{cargo}"
    );

    let mcp = std::fs::read_to_string(scaffold.project_dir.join(".mcp.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&mcp).unwrap();
    let servers = v["mcpServers"].as_object().expect("mcpServers");
    assert_eq!(servers.len(), 1, "exactly one MCP server");
    assert!(servers.contains_key("idealyst"), "only idealyst is exposed");

    std::fs::remove_dir_all(&tmp).ok();
}
