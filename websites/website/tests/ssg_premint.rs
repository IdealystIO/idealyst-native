//! Premint × SSG end-to-end gate: `idealyst build --web --ssg
//! --premint-only` over the whole website, then structural assertions on
//! every exported page.
//!
//! What it proves (the composed contract from CHANGELOG "premint ×
//! SSR/SSG"):
//!
//! 1. every exported page links the content-addressed `premint.css`
//!    BEFORE its inline `<style>`s (cascade contract: engine `ui-*`
//!    fall-through/override rules must beat premint rules on source
//!    order at their shared (0,1,0) specificity);
//! 2. the server actually stamped preminted `iy-*` classes into the
//!    HTML (the whole point — the old bail existed because it couldn't);
//! 3. every `ui-*` class worn in a page's body has a matching rule in
//!    that page's inline `<style>` (fall-through completeness — the
//!    shared `css::hash_class_name` parity made server and client agree,
//!    this checks the server against itself);
//! 4. every `iy-*` class worn in a page's body has a rule in the linked
//!    `premint.css` (the dump's crawl covered what the server stamped).
//!
//! `#[ignore]` because it shells out to the installed `idealyst` CLI and
//! runs a full release web + SSG build (minutes). Run explicitly:
//!
//! ```text
//! cargo test -p website --test ssg_premint -- --ignored
//! ```
//!
//! (Requires `idealyst` on PATH — `cargo install --path crates/tools/cli
//! --force` after touching the build pipeline, same contract as the
//! prune-regression runner.)
//!
//! The frozen non-premint corpus gate (`ssg_parity.rs`) is deliberately
//! untouched by premint: it renders in-process without the premint cfgs.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn website_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn collect_pages(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(root) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip the bundle dir — only exported documents matter.
            if path.file_name().is_some_and(|n| n == "pkg") {
                continue;
            }
            collect_pages(&path, out);
        } else if path.file_name().is_some_and(|n| n == "index.html") {
            out.push(path);
        }
    }
}

/// Every `prefix`-classed token worn in `class="…"` attributes of `html`.
fn worn_classes(html: &str, prefix: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = html;
    while let Some(at) = rest.find("class=\"") {
        rest = &rest[at + 7..];
        let Some(end) = rest.find('"') else { break };
        for cls in rest[..end].split_whitespace() {
            if cls.starts_with(prefix) {
                out.insert(cls.to_string());
            }
        }
        rest = &rest[end + 1..];
    }
    out
}

#[test]
#[ignore = "shells out to the installed idealyst CLI; full release web+SSG build"]
fn regression_ssg_premint_only_pages_are_fully_styled() {
    let dir = website_dir();

    let status = std::process::Command::new("idealyst")
        .args(["build", "--web", "--ssg", "--premint-only", "--release"])
        .arg(&dir)
        .status()
        .expect("spawn idealyst — is it on PATH?");
    assert!(status.success(), "idealyst build --web --ssg --premint-only failed");

    let dist = dir.join("dist/web");
    let mut pages = Vec::new();
    collect_pages(&dist, &mut pages);
    assert!(
        pages.len() >= 30,
        "expected the full site export (33 routes); found {} page(s) under {}",
        pages.len(),
        dist.display()
    );

    // The linked premint asset, for assertion 4.
    let pkg = dist.join("pkg");
    let premint_css = std::fs::read_dir(&pkg)
        .expect("pkg dir")
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("premint.") && n.ends_with(".css"))
        })
        .expect("premint css staged in pkg/");
    let premint_text = std::fs::read_to_string(&premint_css).expect("read premint css");

    let mut stamped_any_iy = false;
    for page in &pages {
        let html = std::fs::read_to_string(page).expect("read page");
        let name = page.strip_prefix(&dist).unwrap_or(page).display();

        // 1. Link present, before the first inline <style>.
        let link_at = html
            .find("<link rel=\"stylesheet\" href=\"/pkg/premint.")
            .unwrap_or_else(|| panic!("{name}: premint <link> missing"));
        let style_at = html.find("<style>").unwrap_or(usize::MAX);
        assert!(
            link_at < style_at,
            "{name}: premint <link> must precede inline <style>s"
        );

        // 2/4. Server-stamped iy-* classes, each with CSS in the asset.
        let iy = worn_classes(&html, "iy-");
        stamped_any_iy |= !iy.is_empty();
        for cls in &iy {
            assert!(
                premint_text.contains(&format!(".{cls}")),
                "{name}: stamped class {cls} has no rule in {}",
                premint_css.display()
            );
        }

        // 3. Engine fall-through classes are self-contained per page.
        for cls in worn_classes(&html, "ui-") {
            assert!(
                html.contains(&format!(".{cls}")),
                "{name}: body wears {cls} but the page's inline CSS has no rule for it"
            );
        }
    }
    assert!(
        stamped_any_iy,
        "no page stamped a single iy-* class — the server is not running \
         in premint posture at all (cfg not injected?)"
    );
}
