//! A `///` block documents the next item, however many blank lines and
//! plain `//` comments sit in between. So a doc comment left behind when
//! the item under it was moved or deleted silently becomes the opening of
//! the NEXT item's docs — in rustdoc, in editor hover, and in the MCP
//! catalog, whose component list shows a component's first doc line.
//!
//! That is how `Button` came to be summarised as "The text-typography
//! subset the label must carry ON ITS OWN NODE." (a note about a removed
//! helper). This test fails on any `///` block that is followed by blank
//! or `//` lines and then another `///` block: the first block is orphaned
//! and has been glued onto the second's item.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `(line, text)` of every orphaned `///` block in `source`.
fn orphaned_doc_blocks(source: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    let is_doc = |l: &str| l.starts_with("///");
    let is_gap = |l: &str| l.is_empty() || (l.starts_with("//") && !l.starts_with("///"));
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_doc(lines[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && is_doc(lines[i]) {
            i += 1;
        }
        let mut k = i;
        while k < lines.len() && is_gap(lines[k]) {
            k += 1;
        }
        if k > i && k < lines.len() && is_doc(lines[k]) {
            out.push((start + 1, lines[start].to_string()));
        }
    }
    out
}

#[test]
fn regression_no_doc_comment_is_glued_onto_the_next_items_docs() {
    let mut files = Vec::new();
    rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    files.sort();
    let mut found = Vec::new();
    for file in &files {
        for (line, text) in orphaned_doc_blocks(&std::fs::read_to_string(file).unwrap()) {
            found.push(format!("{}:{line}: {text}", file.display()));
        }
    }
    assert!(
        found.is_empty(),
        "orphaned `///` blocks — each documents the next item instead of its own; \
         move it onto its item or make it a `//` comment:\n{}",
        found.join("\n")
    );
}

/// The detector itself: Button's shape before the fix is caught; a doc
/// block directly over its item, or one with attributes in between, is not.
#[test]
fn the_detector_catches_the_button_shape_only() {
    let button_before_fix = "/// The text-typography subset the label must carry ON ITS OWN NODE.\n\
        // (Former `label_typography_style` …)\n\
        \n\
        /// Renders a styled, clickable button.\n\
        #[component]\n\
        pub fn Button() {}\n";
    assert_eq!(orphaned_doc_blocks(button_before_fix).len(), 1);
    let attached = "/// Docs.\n#[component]\n/// More docs.\npub fn A() {}\n";
    assert!(orphaned_doc_blocks(attached).is_empty());
}
