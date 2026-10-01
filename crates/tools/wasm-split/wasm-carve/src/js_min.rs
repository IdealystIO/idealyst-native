//! The conservative JS minifier backend-web runs over its embedded shims
//! (`crates/backend/web/build_support/js_min.rs`), shared by source so the
//! glue file's own JS is stripped by exactly the same transforms: comments
//! and per-line whitespace only, every newline in code kept (ASI is
//! untouched), no renaming. Its one blind spot — a regex literal containing
//! `//` or `/*` — is why [`crate::glue_js`] applies it only to JS this
//! repository writes (web-glue's runtime and the loader boilerplate), never
//! to a crate's import snippets or `js_module!` sources; the tests pin that
//! those inputs contain no regex literals.

include!("../../../../backend/web/build_support/js_min.rs");
