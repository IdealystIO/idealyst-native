//! Finding `ui!` / `jsx!` sites in a source file.
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

use syn::visit::Visit;

use runtime_template::SiteId;

use crate::ast::Ui;

/// One `ui!` / `jsx!` invocation found in a file.
pub struct Site {
    pub id: SiteId,
    /// `"ui"` or `"jsx"`.
    pub macro_name: String,
    /// The body, parsed. Not yet numbered — [`crate::describe`] does
    /// that.
    pub ui: Ui,
}

/// Every `ui!` / `jsx!` site in one source file, in source order.
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
        if name != "ui" && name != "jsx" {
            return;
        }
        let Some(id) = site_of(self.package, self.file, mac) else { return };
        if let Ok(ui) = syn::parse2::<Ui>(mac.tokens.clone()) {
            self.out.push(Site { id, macro_name: name, ui });
        }
    }
}
