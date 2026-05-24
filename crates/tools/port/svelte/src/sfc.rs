//! Svelte file block splitter.
//!
//! A `.svelte` file is conceptually:
//!
//!   <script lang="ts"> ...script... </script>
//!   <style> ...css... </style>      (optional)
//!   ...markup at the file's top level...
//!
//! Unlike Vue, there is no `<template>` wrapper — markup is the
//! file's top-level after extracting the script/style blocks.

#[derive(Debug, Default)]
pub struct SvelteBlocks<'a> {
    pub script: Option<&'a str>,
    pub style: Option<&'a str>,
    /// The remaining source after stripping `<script>` and
    /// `<style>` — Svelte's markup.
    pub markup: String,
}

pub fn split(source: &str) -> SvelteBlocks<'_> {
    let mut out = SvelteBlocks::default();
    let mut markup = source.to_string();

    for tag in ["script", "style"] {
        let open_pat = format!("<{}", tag);
        if let Some(start) = markup.find(&open_pat) {
            // Find the end of the opening tag.
            if let Some(open_end_rel) = markup[start..].find('>') {
                let content_start = start + open_end_rel + 1;
                let close = format!("</{}>", tag);
                if let Some(close_rel) = markup[content_start..].find(&close) {
                    let content_end = content_start + close_rel;
                    let after = content_end + close.len();
                    let content = &source[content_start..content_end];
                    match tag {
                        "script" => out.script = Some(content),
                        "style" => out.style = Some(content),
                        _ => unreachable!(),
                    }
                    // Remove this block from the markup string.
                    let removed = &markup[..start];
                    let kept = &markup[after..];
                    markup = format!("{}{}", removed, kept);
                }
            }
        }
    }

    out.markup = markup.trim().to_string();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_script_and_markup() {
        let src = "<script>let x = 0;</script>\n<div>hi</div>";
        let blocks = split(src);
        assert_eq!(blocks.script, Some("let x = 0;"));
        assert_eq!(blocks.markup, "<div>hi</div>");
    }
}
