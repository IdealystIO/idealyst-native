//! Cross-platform render-parity check for idea-ui-docs.
//!
//! Drives both apps to the **All Components** page (`/all`) — every component
//! on one page, each section anchored with a `test_id` — then captures each
//! element's platform-native render state (resolved geometry + visual props,
//! read from the live `CALayer` / `getComputedStyle`, not the authored styles)
//! and diffs the canonical props with cross-platform normalization. One capture
//! covers the whole library.
//!
//! # Run it
//!
//! ```bash
//! idealyst test --parity web,macos examples/idea-ui-docs
//! ```
//!
//! Launches both apps at a matched viewport, points `IDEALYST_WEB_BRIDGE` /
//! `IDEALYST_MACOS_BRIDGE` at each, and runs this. With those unset (a bare
//! `cargo test`) it **skips**.

use std::time::Duration;

use robot_test::parity::{self, compare, report, DiffOptions};
use robot_test::RobotClient;
use serde_json::json;

/// Flip to `true` to make any prop divergence fail the test.
const STRICT: bool = false;

/// Navigate an app to the All Components page by clicking its sidebar entry.
/// No-op if already there / the link isn't found.
fn goto_all_components(c: &mut RobotClient) {
    if let Ok(v) = c.call("find_element", json!({ "label": "All Components" })) {
        if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
            let _ = c.call("click", json!({ "element_id": id }));
            std::thread::sleep(Duration::from_millis(800)); // let the screen mount + lay out
        }
    }
}

#[test]
fn web_and_macos_render_with_parity() {
    let (Some(mut web), Some(mut mac)) = (parity::connect("web"), parity::connect("macos")) else {
        eprintln!(
            "SKIP web_and_macos_render_with_parity: no bridges provisioned. \
             Run `idealyst test --parity web,macos examples/idea-ui-docs`."
        );
        return;
    };

    // Both apps onto the All Components fixture, so one capture covers the
    // whole library and every section is test_id-anchored.
    goto_all_components(&mut web);
    goto_all_components(&mut mac);

    // Scope to the page's content anchor so the (per-platform) navigator chrome
    // is excluded — only the component demos are compared. `"all-components"`
    // matches `pages::all::PARITY_ROOT`.
    let (alignment, mismatches) = compare(&mut web, &mut mac, DiffOptions::default(), Some("all-components"))
        .expect("parity compare");

    let only_web = alignment.unmatched.iter().filter(|u| u.in_a).count();
    let only_mac = alignment.unmatched.iter().filter(|u| !u.in_a).count();
    eprintln!(
        "\n[parity] {} aligned elements | structural: {only_web} only-web, {only_mac} only-macos \
         | {} prop divergence(s)",
        alignment.pairs.len(),
        mismatches.len(),
    );
    assert!(!alignment.pairs.is_empty(), "nothing aligned — introspection failed on a platform");

    if mismatches.is_empty() && alignment.unmatched.is_empty() {
        eprintln!("[parity] full parity. 🎉");
        return;
    }
    if !mismatches.is_empty() {
        eprintln!("\n[parity] prop divergences:\n{}\n", report(&mismatches, "web", "macos"));
    }
    if STRICT {
        panic!(
            "render parity broken: {} prop divergence(s), {} structural",
            mismatches.len(),
            alignment.unmatched.len()
        );
    }
}
