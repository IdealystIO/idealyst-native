//! Cross-platform LAYOUT parity for every screen in the idea-ui catalog.
//!
//! One author tree, two platforms, 49 routes: navigate both apps to the same
//! route, read each element's PLATFORM-RESOLVED geometry (the browser's
//! `getBoundingClientRect`, GTK's allocated bounds — never Taffy's own numbers,
//! which are the layout *input* and would make the check tautological), and diff
//! them. A divergence means the framework's core promise is broken for that
//! screen.
//!
//! # Running it
//!
//! ```bash
//! idealyst test --parity web,linux   websites/idea-ui-docs
//! idealyst test --parity web,macos   websites/idea-ui-docs   # on a mac
//! ```
//!
//! The runner builds and launches one app per platform, pins both to the same
//! viewport (`--viewport`, default 1280x800 — responsive layout legitimately
//! diverges at different sizes), exports `IDEALYST_<PLATFORM>_BRIDGE` plus
//! `IDEALYST_PARITY_PLATFORMS`, and runs this target. To drive it by hand:
//!
//! ```bash
//! IDEALYST_PARITY_PLATFORMS=web,linux \
//! IDEALYST_WEB_BRIDGE=127.0.0.1:7001 \
//! IDEALYST_LINUX_BRIDGE=127.0.0.1:7002 \
//!   cargo test -p idea-ui-docs --test parity -- --nocapture
//! ```
//!
//! With the bridges unset the test **skips** (prints a note and returns), so a
//! bare `cargo test` stays green — same posture as `#[robot_test]`.
//!
//! # Which platforms
//!
//! Read from `IDEALYST_PARITY_PLATFORMS` rather than hardcoded, so the same
//! file serves `web,linux`, `web,macos`, `linux,windows` — every pair the
//! framework grows, without a new test per combination.
//!
//! # What is compared, and what is deliberately not
//!
//! Scoped to the `page-content` anchor: the sidebar and header are the same
//! author tree on every backend, but re-diffing them on all 49 routes would
//! bury the page under test in noise. Alignment is structural (by `test_id`,
//! else `kind|label`), so an extra wrapper on one platform is reported as one
//! structural line rather than throwing off every later sibling.
//!
//! Geometry carries a px tolerance ([`GEOMETRY_TOLERANCE_PX`]) because two
//! platforms shape text with different engines and an intrinsically-sized label
//! genuinely differs by a pixel or two. The `frame.child_axis` check has no
//! tolerance at all: whether a node's children run in a row or a column is a
//! category, and a disagreement there is the "icon beside the label on web,
//! above it on native" class of bug.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use robot_test::parity::{self, compare, report, DiffOptions, Tolerance};
use robot_test::RobotClient;
use serde_json::json;

/// The anchor both platforms' comparisons are scoped to — set on the per-screen
/// content column in `shell::screen_frame`.
const CONTENT_ANCHOR: &str = "page-content";

/// Geometry budget in logical px. Generous enough to absorb text-shaping
/// differences between two engines, far tighter than any real layout bug.
const GEOMETRY_TOLERANCE_PX: f32 = 3.0;

/// How long to wait for a route's content to appear and settle. Web fetches a
/// lazy wasm chunk per page on first visit, so the first few routes are the slow
/// ones.
const ROUTE_SETTLE_TIMEOUT: Duration = Duration::from_secs(20);

/// Most mismatch lines to print per route.
///
/// A sweep over 49 screens can produce thousands of lines the first time a
/// divergence class appears, and a panic message that long is unreadable — the
/// interesting part is WHICH screens broke and what the first few findings look
/// like. The count is always reported in full so nothing is hidden.
const MAX_LINES_PER_ROUTE: usize = 12;

/// One route's STRUCTURAL findings — elements one platform renders and the other
/// does not.
///
/// Reported separately from the prop/geometry mismatches because they come from
/// the alignment, not the diff, and they were previously counted in the progress
/// line but omitted from the failure detail — so the single largest finding
/// category (166 items on the Table page) named no elements at all and was
/// impossible to act on.
fn summarize_structural(unmatched: &[parity::Unmatched], a: &str, b: &str) -> String {
    let shown = unmatched.len().min(MAX_LINES_PER_ROUTE);
    let mut out = format!("{} structural divergence(s):\n", unmatched.len());
    for u in &unmatched[..shown] {
        let side = if u.in_a { a } else { b };
        out.push_str(&format!("{}  <{}>  only in {side}\n", u.path, u.kind));
    }
    if unmatched.len() > shown {
        out.push_str(&format!("  … and {} more\n", unmatched.len() - shown));
    }
    out
}

/// How far apart a mismatch's two values are, for ranking. `None` for
/// non-numeric kinds (text, flags, presence) — those sort after the measurable
/// ones since there is no magnitude to compare.
fn magnitude(m: &parity::Mismatch) -> Option<f32> {
    match &m.kind {
        parity::MismatchKind::ValueDiffers { a, b } => match (a, b) {
            (parity::PropValue::Length(x), parity::PropValue::Length(y))
            | (parity::PropValue::Number(x), parity::PropValue::Number(y)) => Some((x - y).abs()),
            _ => None,
        },
        _ => None,
    }
}

/// One route's findings, geometry first (that is what this sweep is for),
/// **largest divergence first**, truncated to [`MAX_LINES_PER_ROUTE`] with an
/// explicit remainder count.
///
/// The magnitude sort is the difference between a usable report and a useless
/// one. Ordered by path instead, the visible lines were whatever sorted first
/// alphabetically — the page container and the hero — while the actual bug on
/// that screen (a 38x38 icon box rendering 20x38, an 18px width divergence)
/// sat somewhere in the 431 truncated lines. A reader saw sub-pixel text drift
/// and concluded the sweep had found nothing.
fn summarize(mismatches: &[parity::Mismatch], a: &str, b: &str) -> String {
    let (mut geometry, mut props): (Vec<_>, Vec<_>) = (Vec::new(), Vec::new());
    for m in mismatches {
        if m.key.starts_with("frame.") {
            geometry.push(m.clone());
        } else {
            props.push(m.clone());
        }
    }
    // Biggest first within each group; unmeasurable kinds last.
    let by_magnitude_desc = |x: &parity::Mismatch, y: &parity::Mismatch| {
        magnitude(y)
            .unwrap_or(f32::MIN)
            .partial_cmp(&magnitude(x).unwrap_or(f32::MIN))
            .unwrap_or(std::cmp::Ordering::Equal)
    };
    geometry.sort_by(by_magnitude_desc);
    props.sort_by(by_magnitude_desc);
    let total = mismatches.len();
    let ordered: Vec<parity::Mismatch> =
        geometry.iter().chain(props.iter()).cloned().collect();
    let shown = ordered.len().min(MAX_LINES_PER_ROUTE);
    let mut out = format!(
        "{total} divergence(s): {} geometry, {} prop\n{}",
        geometry.len(),
        props.len(),
        report(&ordered[..shown], a, b),
    );
    if total > shown {
        out.push_str(&format!("\n  … and {} more", total - shown));
    }
    out
}

/// The widest root element's frame width — a proxy for the layout viewport,
/// since the app shell fills it on every backend.
fn viewport_width(client: &mut RobotClient) -> anyhow::Result<f32> {
    let snap = client.call("get_snapshot", json!({}))?;
    let roots = parity::parse_snapshot(&snap);
    let mut widest = 0.0f32;
    for r in &roots {
        let native = client.call("introspect_native", json!({ "element_id": r.id }))?;
        if let Some(node) = parity::parse_native(native).ok().flatten() {
            if let Some(f) = node.frame {
                widest = widest.max(f.width);
            }
        }
    }
    Ok(widest)
}

/// Fail immediately when the two platforms are not laid out at the same width.
///
/// Every geometry comparison is meaningless across different viewports —
/// responsive breakpoints resolve differently, text wraps at different points,
/// and every element ends up a different size in a different place. Without this
/// check that shows up as thousands of plausible-looking divergences spread over
/// every screen, which reads exactly like "the framework has no parity" and is
/// actually "the two windows are different sizes".
///
/// Seen live: `idealyst dev --web` opened the user's DEFAULT browser alongside
/// the runner's pinned headless one, both dialled the relay, and the visible
/// window (1912x890) answered the verbs while the native app was at 1280x800 —
/// so all 49 screens "failed". One clear error beats 3000 bogus findings.
fn assert_same_viewport(
    a: &mut RobotClient,
    b: &mut RobotClient,
    name_a: &str,
    name_b: &str,
) -> anyhow::Result<()> {
    let (wa, wb) = (viewport_width(a)?, viewport_width(b)?);
    anyhow::ensure!(
        (wa - wb).abs() <= 1.0,
        "{name_a} is laid out {wa}px wide and {name_b} {wb}px — a parity \
         comparison across different viewports is meaningless (responsive \
         breakpoints and text wrapping both change). Check that only ONE client \
         is attached per app, and that the runner's `--viewport` actually \
         applied: a headless browser that ignores `--window-size`, or a second \
         visible browser window answering the relay, both produce this.",
    );
    eprintln!("[parity] both platforms laid out {wa}px wide");
    Ok(())
}

/// Switch an app to the dark theme by clicking its header toggle.
///
/// The docs app's `Dark` control is a `Pressable` wrapping a `Text`, so this
/// finds the label and walks up until something accepts a click — the same path
/// the bridge's `click` verb takes, which runs the author's handler rather than
/// synthesizing a platform gesture.
fn set_dark(client: &mut RobotClient) -> anyhow::Result<()> {
    let found = client.call("find_element", json!({ "label": "Dark" }))?;
    let mut id = found
        .get("id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow::anyhow!("no `Dark` control in the header"))?;
    for _ in 0..4 {
        if client.call("click", json!({ "element_id": id })).is_ok() {
            return Ok(());
        }
        let parent = client.call("get_parent", json!({ "element_id": id }))?;
        match parent.get("id").and_then(|v| v.as_u64()) {
            Some(p) => id = p,
            None => break,
        }
    }
    anyhow::bail!("found the `Dark` label but nothing up its ancestry accepted a click")
}

/// Platforms to compare, from the runner's `IDEALYST_PARITY_PLATFORMS`.
fn platforms() -> Option<(String, String)> {
    let raw = std::env::var("IDEALYST_PARITY_PLATFORMS").ok()?;
    let mut it = raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let a = it.next()?;
    let b = it.next()?;
    Some((a, b))
}

/// Number of elements in the subtree under the content anchor — the signal used
/// to tell "this route has finished rendering" from "the chunk is still
/// loading".
fn content_size(client: &mut RobotClient) -> anyhow::Result<usize> {
    let snap = client.call("get_snapshot", json!({}))?;
    let roots = parity::parse_snapshot(&snap);
    let Some(anchor) = parity::subtree_by_test_id(&roots, CONTENT_ANCHOR) else {
        return Ok(0);
    };
    fn count(n: &parity::SnapNode) -> usize {
        1 + n.children.iter().map(count).sum::<usize>()
    }
    Ok(count(anchor))
}

/// Navigate `client` to `route` by clicking that route's sidebar link.
///
/// The link's `test_id` IS the route name (`shell::route_link_anchored`), which
/// is why this works identically on both platforms without any per-backend
/// navigation path.
fn navigate(client: &mut RobotClient, route: &str) -> anyhow::Result<()> {
    let found = client.call("find_element", json!({ "test_id": route }))?;
    let id = found
        .get("id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow::anyhow!("no sidebar link with test_id {route:?}"))?;
    client.call("click", json!({ "element_id": id }))?;
    Ok(())
}

/// Wait until the content subtree is non-trivial AND unchanged across two
/// consecutive polls.
///
/// Both halves matter. "Non-trivial" rules out the loading fallback (web's lazy
/// chunk shows a progress bar — a handful of elements — before the real page
/// arrives). "Unchanged" rules out capturing a page mid-build, which would
/// report phantom structural divergence against a platform that had finished.
fn wait_settled(client: &mut RobotClient, route: &str) -> anyhow::Result<usize> {
    let deadline = Instant::now() + ROUTE_SETTLE_TIMEOUT;
    let mut last = 0usize;
    let mut stable_for = 0u32;
    while Instant::now() < deadline {
        let n = content_size(client)?;
        if n > 12 && n == last {
            stable_for += 1;
            if stable_for >= 2 {
                return Ok(n);
            }
        } else {
            stable_for = 0;
        }
        last = n;
        std::thread::sleep(Duration::from_millis(150));
    }
    anyhow::bail!(
        "route {route:?} never settled — the `{CONTENT_ANCHOR}` subtree held \
         {last} element(s) after {}s. A subtree stuck at 1 means the anchor \
         exists but nothing is parented under it (see the lazy-boundary \
         registry-parenting regression in `handlers/lazy.rs`); a size that keeps \
         changing means the page never stops rebuilding.",
        ROUTE_SETTLE_TIMEOUT.as_secs(),
    )
}

#[test]
fn every_idea_ui_screen_has_layout_parity() {
    let Some((name_a, name_b)) = platforms() else {
        eprintln!(
            "SKIP every_idea_ui_screen_has_layout_parity: set \
             IDEALYST_PARITY_PLATFORMS (plus each IDEALYST_<PLATFORM>_BRIDGE), \
             or run `idealyst test --parity web,linux`."
        );
        return;
    };
    let (Some(mut a), Some(mut b)) = (parity::connect(&name_a), parity::connect(&name_b)) else {
        eprintln!(
            "SKIP every_idea_ui_screen_has_layout_parity: no bridge for {name_a} \
             and/or {name_b} (set IDEALYST_{}_BRIDGE / IDEALYST_{}_BRIDGE).",
            name_a.to_uppercase(),
            name_b.to_uppercase(),
        );
        return;
    };
    eprintln!("[parity] {name_a} vs {name_b} — sweeping {} routes", idea_ui_docs::route_ids().len());
    // Before anything is compared: same viewport, or the whole run is noise.
    if let Err(e) = assert_same_viewport(&mut a, &mut b, &name_a, &name_b) {
        panic!("{e}");
    }

    // `IDEALYST_PARITY_THEME=dark` sweeps the dark theme instead of the light
    // one. A theme is not cosmetic to this comparison: it re-resolves every
    // colour token in the tree, and a backend that fails to re-apply one only
    // diverges in the theme it got wrong. Sweeping light alone leaves the other
    // half of every stylesheet untested.
    if std::env::var("IDEALYST_PARITY_THEME").as_deref() == Ok("dark") {
        for (label, client) in [(&name_a, &mut a), (&name_b, &mut b)] {
            set_dark(client)
                .unwrap_or_else(|e| panic!("could not switch {label} to dark: {e}"));
        }
        // The swap rebinds tokens and re-runs dependent effects on both sides.
        std::thread::sleep(Duration::from_millis(1500));
        eprintln!("[parity] both platforms switched to the dark theme");
    }

    let opts = DiffOptions {
        tol: Tolerance { geometry: GEOMETRY_TOLERANCE_PX, ..Default::default() },
        ..Default::default()
    };

    // Per-route findings, kept as a map so the failure report reads in catalog
    // order and names every broken screen rather than only the first.
    let mut broken: BTreeMap<&'static str, String> = BTreeMap::new();
    let mut swept = 0usize;

    for route in idea_ui_docs::route_ids() {
        // Drive both platforms to the same route before either is read.
        for (label, client) in [(&name_a, &mut a), (&name_b, &mut b)] {
            if let Err(e) = navigate(client, route) {
                // Printed, not just recorded: a sweep whose every route fails
                // this way used to run for half an hour in COMPLETE silence,
                // because only the compare path logged. Progress output has to
                // cover the failure paths or a stuck run is indistinguishable
                // from a slow one.
                eprintln!("  {route:<18} NAVIGATION FAILED on {label}: {e}");
                broken.insert(route, format!("navigation failed on {label}: {e}"));
                continue;
            }
        }
        if broken.contains_key(route) {
            continue;
        }
        let _sizes = match (wait_settled(&mut a, route), wait_settled(&mut b, route)) {
            (Ok(sa), Ok(sb)) => (sa, sb),
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("  {route:<18} DID NOT SETTLE: {e}");
                broken.insert(route, format!("{e}"));
                continue;
            }
        };

        match compare(&mut a, &mut b, opts, Some(CONTENT_ANCHOR)) {
            Ok((alignment, mismatches)) => {
                swept += 1;
                let geometry = mismatches
                    .iter()
                    .filter(|m| m.key.starts_with("frame."))
                    .count();
                eprintln!(
                    "  {route:<18} {}/{} elements  {} structural  {} prop  {} geometry",
                    alignment.pairs.len(),
                    alignment.pairs.len() + alignment.unmatched.len(),
                    alignment.unmatched.len(),
                    mismatches.len() - geometry,
                    geometry,
                );
                if !mismatches.is_empty() || !alignment.unmatched.is_empty() {
                    let mut report = String::new();
                    if !alignment.unmatched.is_empty() {
                        report.push_str(&summarize_structural(
                            &alignment.unmatched,
                            &name_a,
                            &name_b,
                        ));
                    }
                    if !mismatches.is_empty() {
                        report.push_str(&summarize(&mismatches, &name_a, &name_b));
                    }
                    broken.insert(route, report);
                }
            }
            Err(e) => {
                eprintln!("  {route:<18} COMPARE FAILED: {e}");
                broken.insert(route, format!("compare failed: {e}"));
            }
        }
    }

    // Report the findings BEFORE asserting that anything was swept: when every
    // route fails to navigate, the per-route reasons are the whole diagnosis,
    // and a bare "nothing was compared" throws them away.
    if !broken.is_empty() {
        let detail = broken
            .iter()
            .map(|(route, report)| format!("── {route} ──\n{report}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        panic!(
            "layout parity broken between {name_a} and {name_b} on {}/{} screens \
             ({swept} compared):\n\n{detail}",
            broken.len(),
            idea_ui_docs::route_ids().len(),
        );
    }
    assert!(
        swept > 0,
        "no route was compared and nothing failed either — the sweep proved \
         nothing (is the catalog empty?)"
    );
    eprintln!("[parity] all {swept} screens match");
}
