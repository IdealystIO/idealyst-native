//! Build-time extraction of each component's source, for the docs page's
//! "View source".
//!
//! Shared by `build.rs` (which runs it, via `#[path]`) and this crate's
//! unit tests. The catalog records each component's file (absolute, from
//! `file!()`); this parses that file and returns the component fn's text
//! as written — doc comments and `#[component]` included, body comments
//! kept — by slicing the original lines the item spans. Parsing with `syn`
//! rather than scanning for braces is what makes a `'{'` or a format
//! string inside the body harmless.

use std::collections::BTreeMap;

/// The source of fn `name` in `text`: from its first attribute (or `fn`
/// line) through its closing brace, whole lines. Searches inline modules
/// too. `None` when the file doesn't parse or has no such fn.
pub fn fn_source(text: &str, name: &str) -> Option<String> {
    let file = syn::parse_file(text).ok()?;
    let (first, last) = find(&file.items, name)?;
    let lines: Vec<&str> = text.lines().collect();
    let body = lines.get(first - 1..last)?.join("\n");
    Some(dedent(&body))
}

/// `(first line, last line)`, 1-based, of the fn named `name`.
fn find(items: &[syn::Item], name: &str) -> Option<(usize, usize)> {
    for item in items {
        match item {
            syn::Item::Fn(f) if f.sig.ident == name => {
                let start = f
                    .attrs
                    .first()
                    .map(|a| a.pound_token.span.start().line)
                    .unwrap_or_else(|| f.sig.fn_token.span.start().line);
                let end = f.block.brace_token.span.close().end().line;
                return Some((start, end));
            }
            syn::Item::Mod(m) => {
                if let Some((_, items)) = &m.content {
                    if let Some(found) = find(items, name) {
                        return Some(found);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Strip the indentation every non-blank line shares (a fn inside a
/// module is indented).
fn dedent(block: &str) -> String {
    let indent = block
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    block
        .lines()
        .map(|l| if l.len() >= indent { &l[indent..] } else { l.trim_start() })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `module_path::Name` → source, for every catalog component whose file
/// can be read and parsed. Each file is read once.
pub fn component_sources(cat: &serde_json::Value) -> BTreeMap<String, String> {
    let mut files: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for c in cat["components"].as_array().into_iter().flatten() {
        let (Some(name), Some(module), Some(file)) = (c["name"].as_str(), c["module_path"].as_str(), c["file"].as_str())
        else {
            continue;
        };
        let text = files.entry(file.to_string()).or_insert_with(|| std::fs::read_to_string(file).ok());
        if let Some(src) = text.as_deref().and_then(|t| fn_source(t, name)) {
            out.insert(format!("{module}::{name}"), src);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"use x::y;

/// A badge.
/// Second line.
#[component]
pub fn Badge(props: &BadgeProps) -> Element {
    // A comment the reader should see.
    let s = format!("{}", '{');
    ui! { view() }
}

mod inner {
    /// Nested.
    #[component]
    fn Deep() -> Element {
        ui! { view() }
    }
}
"#;

    #[test]
    fn takes_the_fn_with_its_docs_and_comments() {
        let src = fn_source(FILE, "Badge").unwrap();
        assert!(src.starts_with("/// A badge.\n/// Second line.\n#[component]"), "{src}");
        assert!(src.contains("// A comment the reader should see."), "{src}");
        assert!(src.ends_with("ui! { view() }\n}"), "a brace in a char literal doesn't end it early: {src}");
    }

    #[test]
    fn finds_fns_in_inline_modules_and_dedents_them() {
        let src = fn_source(FILE, "Deep").unwrap();
        assert!(src.starts_with("/// Nested.\n#[component]\nfn Deep()"), "{src}");
    }

    #[test]
    fn component_sources_reads_each_catalog_file_and_skips_unreadable_ones() {
        let dir = std::env::temp_dir().join(format!("docs-app-sources-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("badge.rs");
        std::fs::write(&file, FILE).unwrap();
        let cat = serde_json::json!({ "components": [
            { "name": "Badge", "module_path": "lib::badge", "file": file.to_str().unwrap() },
            { "name": "Gone", "module_path": "lib::gone", "file": "/does/not/exist.rs" },
        ]});
        let sources = component_sources(&cat);
        assert!(sources["lib::badge::Badge"].contains("pub fn Badge"));
        assert!(!sources.contains_key("lib::gone::Gone"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_fn_or_unparseable_file_has_no_source() {
        assert!(fn_source(FILE, "Nope").is_none());
        assert!(fn_source("fn (", "Badge").is_none());
    }
}
