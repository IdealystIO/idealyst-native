//! Vue `<template>` → porter `JsxNode` tree.
//!
//! Note on handler_fns: parsed templates pass the script walker's
//! `handler_fns` map in so `@click="increment"` substitutes the
//! captured function body inline instead of leaving an undefined
//! `increment()` call.
//!
//! A focused HTML-with-Vue-directives walker. Covers what the
//! bundled fixtures need plus the obvious extensions:
//!
//! - elements: `<View>...</View>`, `<Tag />` (self-closing),
//! - text content: `Count:` between tags,
//! - `{{ expr }}` interpolations,
//! - directives: `@click="fn"`, `:value="x"`, `v-if="cond"`,
//!   `v-for="item in items"`, `v-model="x"`.
//!
//! Anything unrecognized becomes a `JsxNode::Hole` with the
//! original markup preserved. This isn't a full HTML5 parser —
//! Vue templates are well-formed (closing tags required, no
//! `<script>`-style content gotchas), so the simple tag matcher
//! suffices for real-world SFCs.

use port_core::ir::*;
use std::collections::HashMap;

pub fn parse(template: &str, handler_fns: &HashMap<String, String>) -> JsxNode {
    let mut p = Parser::new(template, handler_fns);
    let nodes = p.parse_children();
    // The template root is implicitly one node; if there are
    // multiple top-level children, wrap in a synthetic `<>`.
    match nodes.len() {
        0 => JsxNode::Hole(Hole {
            kind: HoleKind::Unsupported,
            reason: "empty <template>".into(),
            original: SourceSnippet::new(template),
        }),
        1 => nodes.into_iter().next().unwrap(),
        _ => JsxNode::Element {
            name: "View".into(),
            attrs: vec![],
            children: Some(nodes),
        },
    }
}

struct Parser<'a> {
    src: &'a str,
    cur: usize,
    handler_fns: &'a HashMap<String, String>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, handler_fns: &'a HashMap<String, String>) -> Self {
        Self { src, cur: 0, handler_fns }
    }

    fn rest(&self) -> &'a str {
        &self.src[self.cur..]
    }

    /// Walk forward producing siblings until we hit a closing tag
    /// or end-of-input. Closing-tag recognition belongs to the
    /// caller which already consumed the opening.
    fn parse_children(&mut self) -> Vec<JsxNode> {
        let mut out: Vec<JsxNode> = Vec::new();
        loop {
            self.skip_whitespace_lines();
            let rest = self.rest();
            if rest.is_empty() {
                break;
            }
            if rest.starts_with("</") {
                break;
            }
            if rest.starts_with("<!--") {
                // Comment — skip.
                if let Some(end) = rest.find("-->") {
                    self.cur += end + 3;
                } else {
                    break;
                }
                continue;
            }
            if rest.starts_with('<') {
                let node = self.parse_element();
                out.push(node);
                continue;
            }
            if rest.starts_with("{{") {
                let node = self.parse_interpolation();
                out.push(node);
                continue;
            }
            // Plain text up to the next `<` or `{{`.
            let next_lt = rest.find('<').unwrap_or(rest.len());
            let next_mu = rest.find("{{").unwrap_or(rest.len());
            let end = next_lt.min(next_mu);
            let text = &rest[..end];
            let normalized = collapse_text(text);
            if !normalized.is_empty() {
                out.push(JsxNode::Text(normalized));
            }
            self.cur += end;
        }
        coalesce(out)
    }

    fn skip_whitespace_lines(&mut self) {
        // Vue templates allow free whitespace + newlines between
        // tags; consume runs of whitespace at the start of a node
        // position. Internal whitespace is preserved by
        // `collapse_text` when it appears between tags and text.
        while let Some(c) = self.rest().chars().next() {
            if c.is_whitespace() {
                self.cur += c.len_utf8();
            } else {
                break;
            }
        }
    }

    fn parse_element(&mut self) -> JsxNode {
        // We're at '<'. Find the tag-name end + attribute soup.
        let rest = self.rest();
        let close_idx = match rest.find('>') {
            Some(i) => i,
            None => {
                return JsxNode::Hole(Hole {
                    kind: HoleKind::Unsupported,
                    reason: "unterminated tag".into(),
                    original: SourceSnippet::new(rest),
                });
            }
        };
        let opening = &rest[..=close_idx];
        let self_closing = opening.ends_with("/>");
        let inner = &opening[1..opening.len() - if self_closing { 2 } else { 1 }];
        let (name, attr_soup) = match inner.find(char::is_whitespace) {
            Some(i) => (&inner[..i], inner[i..].trim()),
            None => (inner, ""),
        };
        let name = name.to_string();
        let attrs = parse_attrs(attr_soup, self.handler_fns);
        self.cur += close_idx + 1;

        if self_closing {
            return JsxNode::Element { name, attrs, children: None };
        }

        // Walk children until the matching close tag.
        let children = self.parse_children();
        // Expect `</name>`.
        let close = format!("</{}>", name);
        if self.rest().starts_with(&close) {
            self.cur += close.len();
        }
        // If we hit something else, leave it — the rest of the
        // template walks past it naturally.

        JsxNode::Element { name, attrs, children: Some(children) }
    }

    fn parse_interpolation(&mut self) -> JsxNode {
        // We're at `{{`. Find `}}`.
        let rest = self.rest();
        let Some(end) = rest.find("}}") else {
            return JsxNode::Hole(Hole {
                kind: HoleKind::Unsupported,
                reason: "unterminated {{ }}".into(),
                original: SourceSnippet::new(rest),
            });
        };
        let raw = rest[2..end].trim();
        // The expression is a Vue expression; render it as a Rust
        // expression assuming `count` style refs map to `count.get()`.
        // The mechanical port can't know what's reactive, so we
        // leave the expression verbatim. The AI/manual pass adds
        // `.get()` where appropriate.
        let expr = rewrite_vue_expr(raw);
        self.cur += end + 2;
        JsxNode::Expr(expr)
    }
}

fn parse_attrs(soup: &str, handler_fns: &HashMap<String, String>) -> Vec<JsxAttr> {
    let mut out = Vec::new();
    let mut s = soup;
    while !s.trim().is_empty() {
        s = s.trim_start();
        // Find the first `=` or whitespace to get the attr name.
        let eq = s.find('=');
        let ws = s.find(char::is_whitespace);
        let key_end = match (eq, ws) {
            (Some(e), Some(w)) => e.min(w),
            (Some(e), None) => e,
            (None, Some(w)) => w,
            (None, None) => s.len(),
        };
        let key = &s[..key_end];
        s = &s[key_end..];
        if s.starts_with('=') {
            s = &s[1..];
            // Quoted value
            let quote = match s.chars().next() {
                Some('"') => '"',
                Some('\'') => '\'',
                _ => {
                    out.push(JsxAttr { name: key.into(), value: JsxAttrValue::StringLit(String::new()) });
                    continue;
                }
            };
            s = &s[1..];
            let Some(end) = s.find(quote) else { break };
            let val = &s[..end];
            s = &s[end + 1..];
            out.push(translate_vue_attr(key, val, handler_fns));
        } else {
            // Bare attribute (no value).
            out.push(JsxAttr { name: key.into(), value: JsxAttrValue::StringLit(String::new()) });
        }
    }
    out
}

/// Map a Vue attr to its idealyst-jsx shape.
///
/// - `@click="fn"`    → `on_click={move || fn()}`
/// - `:foo="expr"`    → `foo={expr}`
/// - `v-if="cond"`    → AttributeValue hole (needs structural lowering, not attr)
/// - `foo="bar"`      → `foo="bar"` (verbatim string)
fn translate_vue_attr(
    key: &str,
    val: &str,
    handler_fns: &HashMap<String, String>,
) -> JsxAttr {
    if let Some(event) = key.strip_prefix('@') {
        let name = format!("on_{}", event);
        // Vue handler value is either a fn reference (`increment`)
        // — try to substitute its body from script::handler_fns —
        // or an inline expression (`count++`).
        let trimmed = val.trim();
        let expr = if let Some(body) = handler_fns.get(trimmed) {
            format!("move || {}", body)
        } else if val.contains('(') || val.contains('=') || val.contains('+') {
            format!("move || {}", rewrite_vue_expr(val))
        } else {
            // Unknown bare-ident handler — preserve as call.
            format!("move || {}()", val)
        };
        return JsxAttr { name, value: JsxAttrValue::Expr(expr) };
    }
    if let Some(prop) = key.strip_prefix(':') {
        return JsxAttr {
            name: prop.into(),
            value: JsxAttrValue::Expr(rewrite_vue_expr(val)),
        };
    }
    if key.starts_with("v-") {
        // Structural directives belong in element-position lowering,
        // not as attributes. Surface as a hole so the user sees it.
        return JsxAttr {
            name: key.into(),
            value: JsxAttrValue::Hole(Hole {
                kind: HoleKind::AttributeValue,
                reason: format!("Vue directive `{}` needs structural lowering", key),
                original: SourceSnippet::new(format!("{}=\"{}\"", key, val)),
            }),
        };
    }
    JsxAttr { name: key.into(), value: JsxAttrValue::StringLit(val.into()) }
}

/// Rewrite a Vue expression to a Rust one. The mechanical port
/// doesn't know what's reactive (Vue refs read via `.value`); we
/// emit the expression verbatim and let the AI pass handle the
/// `.value` → `.get()` translation. Special-case the obvious
/// `count++` etc. mutations.
fn rewrite_vue_expr(expr: &str) -> String {
    let e = expr.trim();
    // `count++` → `count.set(count.get() + 1)`
    if let Some(name) = e.strip_suffix("++") {
        let n = name.trim();
        return format!("{}.set({}.get() + 1)", n, n);
    }
    if let Some(name) = e.strip_suffix("--") {
        let n = name.trim();
        return format!("{}.set({}.get() - 1)", n, n);
    }
    // Bare ident reads → `.get()`. We can't easily know which idents
    // are reactive; assume single-token lowercase idents are.
    if e.chars().all(|c| c.is_alphanumeric() || c == '_') && !e.is_empty() {
        return format!("{}.get()", e);
    }
    // Otherwise pass through.
    e.to_string()
}

fn collapse_text(s: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    if out.chars().all(|c| c.is_whitespace()) {
        String::new()
    } else {
        out
    }
}

/// Same as the TSX walker's coalesce: text + expr siblings inside
/// a Text element collapse to one `format!()` expr.
fn coalesce(children: Vec<JsxNode>) -> Vec<JsxNode> {
    let cleaned: Vec<JsxNode> = children
        .into_iter()
        .filter(|c| !matches!(c, JsxNode::Text(s) if s.is_empty()))
        .collect();
    let textlike = cleaned.iter().all(|c| matches!(c, JsxNode::Text(_) | JsxNode::Expr(_)));
    let any_expr = cleaned.iter().any(|c| matches!(c, JsxNode::Expr(_)));
    if textlike && any_expr && !cleaned.is_empty() {
        let mut fmt = String::new();
        let mut args: Vec<String> = Vec::new();
        for c in &cleaned {
            match c {
                JsxNode::Text(s) => fmt.push_str(s),
                JsxNode::Expr(e) => {
                    fmt.push_str("{}");
                    args.push(e.clone());
                }
                _ => unreachable!(),
            }
        }
        let combined = format!("format!(\"{}\", {})", fmt, args.join(", "));
        return vec![JsxNode::Expr(combined)];
    }
    cleaned
}
