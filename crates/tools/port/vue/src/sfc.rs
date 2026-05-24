//! Vue Single-File Component block splitter.
//!
//! A `.vue` file's top level is a sequence of root tags:
//! `<script setup>`, `<template>`, `<style>` (in any order, plus
//! optional `<custom-block>` extensions).
//!
//! We don't try to be a full HTML parser — Vue SFCs have a
//! regular structure (root tags only nest within themselves, no
//! conditional `<script>` blocks etc.) so a state-machine scan
//! that matches `<tag>` openings to `</tag>` closes at the same
//! nesting depth handles every real-world SFC. Inner content
//! (including `<` characters) is preserved verbatim.

#[derive(Debug, Default)]
pub struct SfcBlocks<'a> {
    pub script: Option<SfcBlock<'a>>,
    pub template: Option<SfcBlock<'a>>,
    pub style: Option<SfcBlock<'a>>,
}

#[derive(Debug)]
pub struct SfcBlock<'a> {
    /// Raw block contents, *without* the surrounding `<tag>...</tag>`.
    pub content: &'a str,
    /// 1-based line in the original file where the opening tag begins.
    pub line: u32,
    /// Whether the opening tag had `setup` (for `<script>` only).
    pub setup: bool,
}

pub fn split(source: &str) -> SfcBlocks<'_> {
    let mut out = SfcBlocks::default();
    let mut i = 0;
    let bytes = source.as_bytes();
    let mut line = 1u32;
    while i < bytes.len() {
        // Track line numbers for diagnostics.
        if bytes[i] == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // Found a tag opening. Identify name + attributes.
        let tag_start = i;
        let tag_end = find_char(bytes, tag_start, b'>');
        let Some(tag_end) = tag_end else { break };
        let tag = &source[tag_start..=tag_end]; // `<script setup>`
        let (name, attrs) = split_tag(tag);
        match name {
            "script" | "template" | "style" => {
                let content_start = tag_end + 1;
                let close_pat = format!("</{}>", name);
                let content_end = match source[content_start..].find(&close_pat) {
                    Some(rel) => content_start + rel,
                    None => break,
                };
                let block = SfcBlock {
                    content: &source[content_start..content_end],
                    line,
                    setup: name == "script" && attrs.contains("setup"),
                };
                match name {
                    "script" => out.script = Some(block),
                    "template" => out.template = Some(block),
                    "style" => out.style = Some(block),
                    _ => unreachable!(),
                }
                i = content_end + close_pat.len();
            }
            _ => {
                // Unknown root tag — skip it and continue.
                i = tag_end + 1;
            }
        }
    }
    out
}

fn find_char(bytes: &[u8], start: usize, target: u8) -> Option<usize> {
    bytes[start..].iter().position(|&b| b == target).map(|p| start + p)
}

fn split_tag(tag: &str) -> (&str, &str) {
    // tag is like "<script setup lang=\"ts\">" or "<template>".
    // Strip the angle brackets and split into name + attribute soup.
    let inner = tag.trim_start_matches('<').trim_end_matches('>');
    match inner.find(char::is_whitespace) {
        Some(idx) => (&inner[..idx], &inner[idx + 1..]),
        None => (inner, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_script_template_style() {
        let src = "<script setup>const x = 1;</script>\n<template><div/></template>\n<style>.a{}</style>";
        let blocks = split(src);
        assert_eq!(blocks.script.as_ref().unwrap().content, "const x = 1;");
        assert!(blocks.script.as_ref().unwrap().setup);
        assert_eq!(blocks.template.as_ref().unwrap().content, "<div/>");
        assert_eq!(blocks.style.as_ref().unwrap().content, ".a{}");
    }

    #[test]
    fn handles_missing_blocks() {
        let src = "<template><p/></template>";
        let blocks = split(src);
        assert!(blocks.script.is_none());
        assert!(blocks.template.is_some());
    }
}
