//! Cross-platform render-parity checking from a `#[robot_test]` / sync client.
//!
//! The shared model + diff live in the [`native_parity`] crate (so the MCP
//! server reuses the exact same comparison logic). This module adds the
//! `RobotClient`-driven pieces: [`capture_native`] (walk an app's tree over the
//! bridge and read each element's native node) and [`connect`] (dial the bridge
//! the CLI provisioned for a platform).
//!
//! See [`crate::parity`] re-exports for [`diff`](native_parity::diff),
//! [`Tolerance`](native_parity::Tolerance), etc.

use std::net::SocketAddr;
use std::time::Duration;

use serde_json::json;

pub use native_parity::{
    align, diff, diff_with, element_paths, parse_native, parse_snapshot, report,
    subtree_by_test_id, Alignment, AlignedPair, Capture, DiffOptions, Mismatch, MismatchKind,
    NativeNode, PropValue, SnapNode, Tolerance, Unmatched,
};

use crate::client::RobotClient;

/// The env var `idealyst test --parity` sets per platform, holding that app's
/// bridge address (`host:port`). E.g. `IDEALYST_WEB_BRIDGE`,
/// `IDEALYST_MACOS_BRIDGE`.
pub fn bridge_env_var(platform: &str) -> String {
    format!("IDEALYST_{}_BRIDGE", platform.to_uppercase())
}

/// Address of the bridge the CLI provisioned for `platform` (`"web"`,
/// `"macos"`, `"ios"`, `"android"`), or `None` when unset/unparseable — a
/// parity test treats `None` as "skip" so a bare `cargo test` stays green.
pub fn bridge_addr(platform: &str) -> Option<SocketAddr> {
    std::env::var(bridge_env_var(platform)).ok()?.parse().ok()
}

/// Connect to the bridge the CLI provisioned for `platform` and wait until the
/// app answers. `None` when the env var is unset or the app never came up —
/// the test should skip.
pub fn connect(platform: &str) -> Option<RobotClient> {
    let addr = bridge_addr(platform)?;
    let mut client = RobotClient::connect(addr).ok()?;
    client.wait_ready(Duration::from_secs(10)).ok()?;
    Some(client)
}

/// Walk `client`'s element tree and read each element's platform-native node.
///
/// Skips elements the backend can't introspect yet (`introspect_native`
/// returns `null`). Keyed by the stable positional element path (see
/// [`native_parity::element_paths`]). Prefer [`compare`] for a cross-platform
/// diff — it aligns structurally, which positional paths can't.
pub fn capture_native(client: &mut RobotClient) -> anyhow::Result<Capture> {
    let snapshot = client.call("get_snapshot", json!({}))?;
    let mut out = Capture::new();
    for (path, id) in element_paths(&snapshot) {
        let native = client.call("introspect_native", json!({ "element_id": id }))?;
        if let Some(node) = parse_native(native)
            .map_err(|e| anyhow::anyhow!("introspect_native for {path}: bad payload: {e}"))?
        {
            out.insert(path, node);
        }
    }
    Ok(out)
}

/// Read one element's native node over `client`.
fn introspect_one(client: &mut RobotClient, id: u64) -> anyhow::Result<Option<NativeNode>> {
    let v = client.call("introspect_native", json!({ "element_id": id }))?;
    parse_native(v).map_err(|e| anyhow::anyhow!("bad introspect_native payload: {e}"))
}

/// Cross-platform render-parity comparison of two running apps. This is the
/// right entry point for a parity test: it **structurally aligns** the two
/// element trees (by `test_id`/`kind`+`label`, tolerating wrapper/order
/// differences), introspects each aligned element on its own platform, and
/// diffs the canonical props with cross-platform normalization.
///
/// Returns the [`Alignment`] (so the caller can report structurally-unmatched
/// elements — things one platform renders and the other doesn't) and the prop
/// [`Mismatch`]es on aligned elements.
///
/// `root` optionally scopes the comparison to the subtree rooted at that
/// `test_id` — pass the content anchor to **exclude the navigator chrome**
/// (which is built per-platform with different native structure and so can't be
/// diffed element-by-element). `None` compares the whole tree.
///
/// For a meaningful result, both apps must show the **same route at the same
/// viewport size** — otherwise responsive layout legitimately diverges them.
pub fn compare(
    a: &mut RobotClient,
    b: &mut RobotClient,
    opts: DiffOptions,
    root: Option<&str>,
) -> anyhow::Result<(Alignment, Vec<Mismatch>)> {
    let snap_a = a.call("get_snapshot", json!({}))?;
    let snap_b = b.call("get_snapshot", json!({}))?;
    let roots_a = parse_snapshot(&snap_a);
    let roots_b = parse_snapshot(&snap_b);

    // Scope to the content anchor when given, else the whole tree.
    let (list_a, list_b): (Vec<SnapNode>, Vec<SnapNode>) = match root {
        Some(tid) => {
            let ra = subtree_by_test_id(&roots_a, tid)
                .ok_or_else(|| anyhow::anyhow!("root test_id {tid:?} not found in app A"))?;
            let rb = subtree_by_test_id(&roots_b, tid)
                .ok_or_else(|| anyhow::anyhow!("root test_id {tid:?} not found in app B"))?;
            (vec![ra.clone()], vec![rb.clone()])
        }
        None => (roots_a, roots_b),
    };
    let alignment = align(&list_a, &list_b);

    let mut cap_a = Capture::new();
    let mut cap_b = Capture::new();
    for pair in &alignment.pairs {
        if let Some(n) = introspect_one(a, pair.id_a)? {
            cap_a.insert(pair.path.clone(), n);
        }
        if let Some(n) = introspect_one(b, pair.id_b)? {
            cap_b.insert(pair.path.clone(), n);
        }
    }
    let mismatches = diff_with(&cap_a, &cap_b, opts);
    Ok((alignment, mismatches))
}
