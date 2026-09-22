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
//!
//! # Sites nested in other macros
//!
//! `syn` does not parse a macro's tokens, so a visitor never descends
//! into one — and `pressable(vec![ui! { text { "x" } }], …)` is a shape
//! real apps use constantly. Those sites DO expand and DO carry tags, so
//! a scanner that missed them would leave every edit inside one to a
//! rebuild forever.
//!
//! So every macro's token tree is walked for `ui !` / `jsx !` groups,
//! recursively. That is the same trick the dynamic-split harvester used
//! to find `lazy!` nested inside `ui!`. The results are merged with the
//! visitor's and sorted by byte offset, because DOCUMENT ORDER is what
//! the ordinal numbering downstream depends on.

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
    ///
    /// `None` when the body did not parse. The site is still RECORDED,
    /// because the sites of a file are numbered by position and a
    /// silently dropped one would renumber every site after it — which
    /// is how an ordinal-matched patch reaches the wrong site. A caller
    /// that meets a `None` should rebuild; it cannot be described.
    pub ui: Option<Ui>,
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

/// Every `ui!` site in one source file, in DOCUMENT ORDER.
///
/// `file` must be the path RELATIVE to the package root, `/`-separated —
/// the same normalization the macro applies to its span's file. `text`
/// is the file's contents.
///
/// Includes sites nested inside other macros' tokens (see the module
/// docs), and includes sites whose body did not parse — with
/// [`Site::ui`] `None`. Order is by byte offset and a site appears
/// exactly once however it was found.
pub fn sites_in_file(package: &str, file: &str, text: &str) -> syn::Result<Vec<Site>> {
    let ast = syn::parse_file(text)?;
    let mut finder = Finder { package, file, out: Vec::new() };
    finder.visit_file(&ast);
    let mut out = finder.out;
    // One site can be reached both ways — the visitor sees a top-level
    // `ui!`, and a token walk of an enclosing macro would see it again
    // if that macro also contained it. Dedupe by body position, which is
    // the site's identity within the file.
    out.sort_by_key(|s| (s.body_range.start, s.body_range.end));
    out.dedup_by_key(|s| s.body_range.start);
    Ok(out)
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
    // Only DESCRIBABLE bodies are blanked. A site whose body did not
    // parse cannot be diffed, so leaving its text in the skeleton is
    // what makes an edit inside it rebuild — which is the only correct
    // answer for a construct nothing here understands.
    let mut ranges: Vec<&std::ops::Range<usize>> = sites
        .iter()
        .filter(|s| s.ui.is_some())
        .map(|s| &s.body_range)
        .collect();
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

impl<'a> Finder<'a> {
    /// Record one site, from the macro's NAME ident span and its
    /// delimited body.
    fn record(
        &mut self,
        name: String,
        name_span: proc_macro2::Span,
        body_range: std::ops::Range<usize>,
        tokens: proc_macro2::TokenStream,
    ) {
        let start = name_span.start();
        let id = SiteId {
            package: self.package.to_string().into(),
            file: self.file.to_string().into(),
            line: start.line as u32,
            col: start.column as u32 + 1,
        };
        self.out.push(Site {
            id,
            macro_name: name,
            ui: syn::parse2::<Ui>(tokens).ok(),
            body_range,
        });
    }

    /// Walk a token stream for `ui !` groups, recursing into every
    /// group. See the module docs on why this exists.
    fn walk_tokens(&mut self, tokens: proc_macro2::TokenStream) {
        use proc_macro2::{Delimiter, Spacing, TokenTree};

        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        let mut i = 0;
        while i < trees.len() {
            // `ui` `!` `{ … }` — and the `!` must be Alone, so `ui != x`
            // is not mistaken for an invocation. Requiring a group after
            // it would be enough on its own; checking both says why.
            if let TokenTree::Ident(ident) = &trees[i] {
                if ident == "ui" {
                    if let (Some(TokenTree::Punct(bang)), Some(TokenTree::Group(group))) =
                        (trees.get(i + 1), trees.get(i + 2))
                    {
                        if bang.as_char() == '!' && bang.spacing() == Spacing::Alone {
                            let range = group.delim_span().join().byte_range();
                            self.record(
                                "ui".to_string(),
                                ident.span(),
                                range,
                                group.stream(),
                            );
                            // Recurse into the body too: a `ui!` inside a
                            // `ui!` body is its own site, with its own
                            // tag, and the macro expands it as one.
                            self.walk_tokens(group.stream());
                            i += 3;
                            continue;
                        }
                    }
                }
            }
            if let TokenTree::Group(group) = &trees[i] {
                let _ = Delimiter::Brace;
                self.walk_tokens(group.stream());
            }
            i += 1;
        }
    }
}

impl<'a, 'ast> Visit<'ast> for Finder<'a> {
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let Some(name) = mac.path.segments.last().map(|s| s.ident.to_string()) else {
            return;
        };
        // `jsx!` deliberately absent from the RECORDED set — see the
        // module docs. Its tokens are still walked, because a `ui!`
        // nested inside a `jsx!` is an ordinary site.
        if name == "ui" {
            let body_range = mac.delimiter.span().join().byte_range();
            self.record(
                name,
                mac.path.segments.last().expect("checked").ident.span(),
                body_range,
                mac.tokens.clone(),
            );
        }
        // Every macro's tokens, including this one's: `syn` stops at the
        // delimiter and a site inside is invisible to the visitor.
        self.walk_tokens(mac.tokens.clone());
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

    const NESTED: &str = r#"
fn a() -> Element {
    ui! { text { "first" } }
}

fn b() -> Element {
    pressable(vec![ui! { text { "nested" } }], || {})
}

fn c() -> Element {
    ui! { text { "third" } }
}
"#;

    /// A `ui!` inside another macro's tokens is a real site: it expands,
    /// it carries tags, and an edit inside it must be patchable. `syn`
    /// never descends into a macro's tokens, so the visitor alone would
    /// miss it — and `pressable(vec![ui!{…}], …)` is a shape real apps
    /// use constantly.
    ///
    /// The ordering assertion is the load-bearing half: the nested site
    /// sits BETWEEN two top-level ones, and downstream numbering matches
    /// archived sites to current ones by position in this list. A
    /// nested site appended at the end instead of slotted in would
    /// renumber the sites after it and address every later patch to the
    /// wrong site.
    #[test]
    fn a_site_nested_in_another_macro_is_found_in_document_order() {
        let sites = sites_in_file("pkg", "src/a.rs", NESTED).expect("parses");
        assert_eq!(sites.len(), 3, "the nested site counts");

        let contents: Vec<String> = sites
            .iter()
            .map(|s| NESTED[s.body_range.clone()].to_string())
            .collect();
        assert!(contents[0].contains("first"), "{contents:?}");
        assert!(contents[1].contains("nested"), "{contents:?}");
        assert!(contents[2].contains("third"), "{contents:?}");

        assert!(
            sites.windows(2).all(|w| w[0].body_range.start < w[1].body_range.start),
            "sites must come out in byte order"
        );
        assert!(sites.iter().all(|s| s.ui.is_some()), "all three bodies parse");
    }

    /// A site found by BOTH routes — the visitor sees a top-level `ui!`,
    /// and the token walk of the same macro sees it too — must appear
    /// once. A duplicate would shift every ordinal after it.
    #[test]
    fn a_site_reachable_two_ways_is_recorded_once() {
        let src = "fn a() -> Element { ui! { text { \"x\" } } }\n";
        assert_eq!(sites_in_file("pkg", "src/a.rs", src).expect("parses").len(), 1);
    }

    /// A `ui!` inside another `ui!`'s body is its own site, with its own
    /// tag, because the macro expands it as one.
    #[test]
    fn a_site_inside_a_ui_body_is_its_own_site() {
        let src = r#"
fn a() -> Element {
    ui! {
        view() {
            presence(visible = flag) { ui! { text { "inner" } } }
        }
    }
}
"#;
        let sites = sites_in_file("pkg", "src/a.rs", src).expect("parses");
        assert_eq!(sites.len(), 2);
        assert!(sites[0].body_range.start < sites[1].body_range.start);
    }

    /// An unparseable body is still RECORDED — dropping it would
    /// renumber every site after it — and stays in the skeleton, so any
    /// edit inside it rebuilds.
    #[test]
    fn an_unparseable_body_is_recorded_and_left_in_the_skeleton() {
        let src = "fn a() { ui! { !!! } ; ui! { text { \"ok\" } } }\n";
        let sites = sites_in_file("pkg", "src/a.rs", src).expect("parses");
        assert_eq!(sites.len(), 2, "both are recorded");
        assert!(sites[0].ui.is_none(), "the first does not parse");
        assert!(sites[1].ui.is_some());

        let skeleton = skeleton_of(src, &sites);
        assert!(skeleton.contains("!!!"), "an undescribable body stays visible:\n{skeleton}");
        assert!(!skeleton.contains("\"ok\""), "a describable one is blanked:\n{skeleton}");
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
