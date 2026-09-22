//! The macro and the source scanner must name the same site.
//!
//! A tag carries `runtime_template::site_key(package, file, line, col)`
//! computed by the proc macro from its own call span. The build-time
//! producer computes the same key from the file on disk. If the two
//! disagree — by a column convention, by a path prefix, by a package
//! name — every descriptor addresses nothing, and NOTHING ELSE FAILS:
//! both halves stay internally consistent and patches simply never
//! match. There is no way to notice except by comparing them.
//!
//! So this test compiles a real `ui!`, reads its tag, and re-derives the
//! key from this file's own source. It is the only check of:
//!
//! - `proc_macro::Span::column()` being 1-based where `proc_macro2`'s is
//!   0-based;
//! - the position being the macro's NAME token;
//! - `CARGO_MANIFEST_DIR` stripping producing the same relative path
//!   rustc reports for an integration test (`tests/site_key.rs`).

#![cfg(feature = "ui-overlay")]

use runtime_macros::ui;
use runtime_vocabulary::glue::Element;

/// Build the tagged tree. The `ui!` below is the subject: its line and
/// column are what both halves must agree on, so moving it is fine and
/// hard-coding them would not be.
fn subject() -> Element {
    ui! {
        view() {
            text { "site key" }
        }
    }
}

#[test]
fn the_macro_and_the_scanner_name_the_same_site() {
    let harness = host_mock::Harness::new();
    let tags = harness.world.enter(|| runtime_vocabulary::overlay::tags(&subject()));
    assert!(!tags.is_empty(), "the subject must be tagged");
    let compiled = tags[0].site;
    assert!(
        tags.iter().all(|t| t.site == compiled),
        "one `ui!`, one site key"
    );

    let text = include_str!("site_key.rs");
    let sites = runtime_macros_parse::sites_in_file(
        env!("CARGO_PKG_NAME"),
        "tests/site_key.rs",
        text,
    )
    .expect("this file parses");

    // Exactly one `ui!` in this file, and it is the subject.
    assert_eq!(sites.len(), 1, "expected one site in this file");
    assert_eq!(
        sites[0].id.key(),
        compiled,
        "scanner says {} (key {}), the expansion tagged {}",
        sites[0].id,
        sites[0].id.key(),
        compiled
    );
}
