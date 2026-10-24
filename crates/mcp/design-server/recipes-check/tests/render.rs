//! Server-render every recipe the design MCP serves and check its declared
//! `//! render: <path> => <text>, !<absent text>, …` expectations.
//!
//! SSR runs the recipe's real `app()` — theme install, navigators, layout,
//! idea-ui components — through the same build + realize path a web page
//! takes, so "compiles" is not the bar: a recipe whose shell doesn't mount
//! its screen, or whose screen panics at realize, fails here.

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::newcore::render_path_with;
use mcp_design_recipes_check::RECIPES;

/// HTML text with tags stripped and entities decoded enough to match
/// plain expectations.
fn visible_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&#39;", "'").replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">")
}

#[test]
fn every_served_recipe_is_compiled_here() {
    let served: Vec<&str> = mcp_design_server::content::recipes().iter().map(|r| r.slug).collect();
    let compiled: Vec<&str> = RECIPES.iter().map(|(n, _)| *n).collect();
    assert_eq!(served, compiled);
}

#[test]
fn recipes_render_their_declared_content() {
    let mut failures = Vec::new();
    for recipe in mcp_design_server::content::recipes() {
        let app = RECIPES.iter().find(|(n, _)| *n == recipe.slug).expect("compiled").1;
        for check in &recipe.renders {
            // Each render is its own thread: a fresh reactive world and
            // theme, like a fresh SSR request.
            let path = check.path.clone();
            let (w, h) = check.viewport.size();
            let html = std::thread::Builder::new()
                .stack_size(16 << 20)
                .spawn(move || {
                    render_path_with(&path, |_| {}, move || {
                        // SSR seeds its default (desktop) viewport before
                        // `build`; overriding it here, first thing in the
                        // build, is what `current_breakpoint()` and every
                        // breakpoint-keyed branch then read.
                        runtime_shared::set_viewport_size(runtime_shared::ViewportSize::new(w, h));
                        app()
                    })
                    .html
                })
                .unwrap()
                .join();
            let html = match html {
                Ok(h) => h,
                Err(_) => {
                    failures.push(format!("{} @ {} ({}): render panicked", recipe.slug, check.path, check.viewport.name()));
                    continue;
                }
            };
            let text = visible_text(&html);
            for want in &check.expect {
                // `!text` asserts absence — e.g. that a swap navigator
                // rendered only the active screen.
                match want.strip_prefix('!') {
                    Some(absent) if text.contains(absent) => {
                        failures.push(format!("{} @ {} ({}): unexpected `{absent}`", recipe.slug, check.path, check.viewport.name()))
                    }
                    Some(_) => {}
                    None if !text.contains(want.as_str()) => {
                        failures.push(format!("{} @ {} ({}): missing `{want}`", recipe.slug, check.path, check.viewport.name()))
                    }
                    None => {}
                }
            }
        }
    }
    assert!(failures.is_empty(), "recipe render checks failed:\n{}", failures.join("\n"));
}

/// The server advertises which idea-ui the recipes were checked against;
/// this crate is what checks them, so the two must agree.
#[test]
fn advertised_idea_ui_version_matches_the_checked_one() {
    let manifest = include_str!("../../../../ui/idea-ui/Cargo.toml");
    let version = manifest
        .lines()
        .find_map(|l| l.strip_prefix("version = \""))
        .and_then(|v| v.split('"').next())
        .expect("idea-ui version");
    let major = version.split('.').next().unwrap();
    assert_eq!(
        mcp_design_server::content::CHECKED_AGAINST_IDEA_UI,
        major,
        "bump CHECKED_AGAINST_IDEA_UI in crates/mcp/design-server/src/content.rs after re-checking the recipes"
    );
}

/// `crates/mcp/README.md` lists every skill by name; a new or renamed skill
/// must show up there too.
#[test]
fn mcp_readme_names_every_skill() {
    let readme = include_str!("../../../README.md");
    for s in mcp_design_server::content::skills() {
        assert!(readme.contains(&format!("`{}`", s.slug)), "crates/mcp/README.md doesn't name skill `{}`", s.slug);
    }
}

/// Recipes are copied verbatim into apps, so they carry no idiom drift:
/// zero findings from `idealyst lint`'s default rule set, warnings included.
#[test]
fn recipes_are_lint_clean() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../content/recipes");
    let run = lint::lint_path(&dir, &lint::Config::default());
    assert!(run.parse_errors.is_empty(), "{:?}", run.parse_errors);
    assert_eq!(run.files_scanned, RECIPES.len());
    let findings: Vec<String> = run
        .diagnostics
        .iter()
        .map(|d| format!("{d:?}"))
        .collect();
    assert!(findings.is_empty(), "recipes have lint findings:\n{}", findings.join("\n"));
}
