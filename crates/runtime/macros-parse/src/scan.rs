//! Finding `ui!` sites in a source file.
//!
//! The proc macro learns where it is from its own call span. A
//! build-time producer has to work it out from the file, and it has to
//! arrive at the SAME [`SiteId`] or the descriptor it writes addresses
//! nothing. This module is that half, and it lives here rather than in
//! the CLI so the two conventions are decided once:
//!
//! - **line** is 1-based on both sides.
//! - **column** is 1-based on both sides — `proc_macro::Span::column()`
//!   is 1-based, `proc_macro2`'s `LineColumn::column` is 0-based, and
//!   [`site_of`] adds the one. Getting this wrong is invisible (every
//!   key is simply different) which is exactly why it is pinned by a
//!   test that compiles a real `ui!` and compares its tag against this
//!   scanner's answer.
//! - **the position is the macro's NAME token**, `ui` / `jsx`, because
//!   that is where a function-like proc macro's `call_site()` starts.
//!
//! A file that does not parse yields an error and, in the CLI, is
//! skipped with a warning: one half-typed file must not take down the
//! descriptor set for the rest of the crate.
//!
//! # `jsx!` is not scanned
//!
//! It has its own grammar and its own node type, so the split pass
//! never runs on it and its nodes carry no tag (see `runtime_macros`'
//! `jsx` module). Leaving it out here is what makes that safe: a `jsx!`
//! body is part of [`file_skeleton`], so an edit inside one MOVES the
//! skeleton and the dev loop falls back to a rebuild. Scanning it and
//! silently failing to parse would reach the same place by accident;
//! not scanning it says so.

use syn::visit::Visit;

use runtime_template::SiteId;

use crate::ast::Ui;

/// One `ui!` invocation found in a file.
pub struct Site {
    pub id: SiteId,
    /// Always `"ui"` today. Kept as data rather than assumed, so a
    /// second scannable macro is an addition here and not a rewrite.
    pub macro_name: String,
    /// The body, parsed. Not yet numbered — [`crate::describe`] does
    /// that.
    pub ui: Ui,
    /// Byte range of the macro's delimited body within the file,
    /// braces included.
    ///
    /// What [`file_skeleton`] blanks out. A dev loop has to answer "did
    /// anything OUTSIDE the `ui!` sites change?", and a descriptor
    /// cannot answer it: a `let x = 1;` becoming `let x = 2;` moves no
    /// site and changes no descriptor, but it is compiled code and it
    /// needs a rebuild.
    pub body_range: std::ops::Range<usize>,
}

/// Every `ui!` site in one source file, in source order.
///
/// `file` must be the path RELATIVE to the package root, `/`-separated —
/// the same normalization the macro applies to its span's file. `text`
/// is the file's contents.
///
/// A site whose body does not parse as a `ui!` body is skipped rather
/// than failing the file: it is either mid-edit or uses a construct this
/// parser rejects, and in both cases the right answer is "no descriptor
/// for that site", not "no descriptors at all".
pub fn sites_in_file(package: &str, file: &str, text: &str) -> syn::Result<Vec<Site>> {
    let ast = syn::parse_file(text)?;
    let mut finder = Finder { package, file, out: Vec::new() };
    finder.visit_file(&ast);
    Ok(finder.out)
}

/// The [`SiteId`] for a macro invocation, given where the file sits.
///
/// Public because the same conversion is what a test needs to check the
/// scanner against a real expansion's tag.
pub fn site_of(package: &str, file: &str, mac: &syn::Macro) -> Option<SiteId> {
    let name = mac.path.segments.last()?.ident.clone();
    let start = name.span().start();
    Some(SiteId {
        package: package.to_string().into(),
        file: file.to_string().into(),
        line: start.line as u32,
        // `proc_macro2` counts columns from 0; rustc's
        // `proc_macro::Span::column()`, which the macro half reads,
        // counts from 1.
        col: start.column as u32 + 1,
    })
}

/// The file with every scanned `ui!` body blanked out.
///
/// The counterpart of the descriptor set: together they partition a
/// source file into "the part an overlay can patch" and "the part that
/// needs a compiler". A save is patchable only when this is byte-for-byte
/// unchanged — otherwise something outside the sites moved, and no
/// amount of descriptor diffing can tell you what it does.
///
/// Blanked rather than deleted so a `ui!` body gaining a line does not
/// shift the skeleton around it; what is left is the exact surrounding
/// text with a fixed-width hole where each body was.
pub fn file_skeleton(package: &str, file: &str, text: &str) -> syn::Result<String> {
    let sites = sites_in_file(package, file, text)?;
    Ok(skeleton_of(text, &sites))
}

/// [`file_skeleton`] over sites already scanned.
pub fn skeleton_of(text: &str, sites: &[Site]) -> String {
    let mut ranges: Vec<&std::ops::Range<usize>> = sites.iter().map(|s| &s.body_range).collect();
    ranges.sort_by_key(|r| r.start);

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for range in ranges {
        // Nested sites (a `ui!` inside a `ui!` body) are already inside
        // an outer hole; skip anything the cursor has passed.
        if range.start < cursor {
            continue;
        }
        out.push_str(&text[cursor..range.start]);
        out.push_str("{\u{0}ui\u{0}}");
        cursor = range.end.min(text.len());
    }
    out.push_str(&text[cursor.min(text.len())..]);
    out
}

struct Finder<'a> {
    package: &'a str,
    file: &'a str,
    out: Vec<Site>,
}

impl<'a, 'ast> Visit<'ast> for Finder<'a> {
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let Some(name) = mac.path.segments.last().map(|s| s.ident.to_string()) else {
            return;
        };
        // `jsx!` deliberately absent — see the module docs.
        if name != "ui" {
            return;
        }
        let Some(id) = site_of(self.package, self.file, mac) else { return };
        let body_range = mac.delimiter.span().join().byte_range();
        if let Ok(ui) = syn::parse2::<Ui>(mac.tokens.clone()) {
            self.out.push(Site { id, macro_name: name, ui, body_range });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
fn a() -> Element {
    let n = 1;
    ui! { view() { text { "hi" } } }
}

fn b() -> Element {
    jsx! { <View><Text>x</Text></View> }
}
"#;

    #[test]
    fn a_scan_finds_the_ui_sites_and_leaves_jsx_alone() {
        let sites = sites_in_file("pkg", "src/a.rs", SRC).expect("parses");
        assert_eq!(sites.len(), 1, "the `jsx!` is not a scannable site");
        assert_eq!(sites[0].macro_name, "ui");
        // 1-based, matching `proc_macro::Span::column()` — the whole
        // reason the scanner adds one to proc-macro2's 0-based column.
        assert!(sites[0].id.col >= 1);
    }

    /// A `jsx!` body is part of the skeleton, so editing one moves it
    /// and the dev loop rebuilds. That is the correct outcome — `jsx!`
    /// nodes carry no tag, so there is nothing a patch could address —
    /// and it has to be a property, not a coincidence.
    #[test]
    fn editing_a_jsx_body_moves_the_skeleton() {
        let base = file_skeleton("pkg", "src/a.rs", SRC).expect("parses");
        let edited = SRC.replace("<Text>x</Text>", "<Text>y</Text>");
        assert_ne!(file_skeleton("pkg", "src/a.rs", &edited).expect("parses"), base);
    }

    /// The skeleton is what answers "did anything outside the `ui!`
    /// sites change?". A body edit must leave it alone; a logic edit
    /// must move it. Both directions, because a skeleton that never
    /// changed would make every save look patchable.
    #[test]
    fn the_skeleton_hides_bodies_and_keeps_everything_else() {
        let base = file_skeleton("pkg", "src/a.rs", SRC).expect("parses");
        assert!(base.contains("let n = 1;"), "{base}");
        assert!(!base.contains("\"hi\""), "the body must be hidden:\n{base}");

        let body_edit = SRC.replace("\"hi\"", "\"bye\"");
        assert_eq!(
            file_skeleton("pkg", "src/a.rs", &body_edit).expect("parses"),
            base,
            "an edit inside a `ui!` body must not move the skeleton"
        );

        let logic_edit = SRC.replace("let n = 1;", "let n = 2;");
        assert_ne!(
            file_skeleton("pkg", "src/a.rs", &logic_edit).expect("parses"),
            base,
            "an edit outside every body must move the skeleton"
        );
    }

    /// A body that gains a LINE must still leave the skeleton alone —
    /// the hole is fixed-width, so the text after it does not shift.
    #[test]
    fn a_body_growing_a_line_does_not_move_the_skeleton() {
        let base = file_skeleton("pkg", "src/a.rs", SRC).expect("parses");
        let grown = SRC.replace(
            r#"ui! { view() { text { "hi" } } }"#,
            "ui! {\n        view() {\n            text { \"hi\" }\n            text { \"more\" }\n        }\n    }",
        );
        assert_eq!(file_skeleton("pkg", "src/a.rs", &grown).expect("parses"), base);
    }
}
