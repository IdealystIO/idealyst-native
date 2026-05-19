//! Svelte markup walker.
//!
//! Svelte's markup is HTML with these extensions:
//!
//! - `{expr}` — interpolation.
//! - `{#if cond}…{:else if cond}…{:else}…{/if}` — conditional.
//! - `{#each items as item}…{/each}` — iteration.
//! - `on:click={handler}` — event binding. Handler is an
//!   identifier (function ref) or an inline expression.
//! - `bind:value={x}` — two-way binding.
//! - `class:active={cond}` — conditional class.
//!
//! For the Counter fixture, only the basics are needed (text +
//! `{count}` interpolation + `on:click={increment}`). The walker
//! covers a slightly broader subset and falls back to a `Hole`
//! for anything unrecognized.

use port_core::ir::*;
use std::collections::HashMap;

pub fn parse(markup: &str, handler_fns: &HashMap<String, String>) -> JsxNode {
    let mut p = Parser::new(markup, handler_fns);
    let nodes = p.parse_children();
    match nodes.len() {
        0 => JsxNode::Hole(Hole {
            kind: HoleKind::Unsupported,
            reason: "empty Svelte markup".into(),
            original: SourceSnippet::new(markup),
        }),
        1 => nodes.into_iter().next().unwrap(),
        _ => JsxNode::Element { name: "View".into(), attrs: vec![], children: Some(nodes) },
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

    fn rest(&self) -> &'a str { &self.src[self.cur..] }

    fn parse_children(&mut self) -> Vec<JsxNode> {
        let mut out: Vec<JsxNode> = Vec::new();
        loop {
            self.skip_whitespace();
            let rest = self.rest();
            if rest.is_empty() { break; }
            if rest.starts_with("</") { break; }
            if rest.starts_with("{/") || rest.starts_with("{:") { break; }
            if rest.starts_with("<!--") {
                if let Some(end) = rest.find("-->") {
                    self.cur += end + 3;
                } else { break; }
                continue;
            }
            if rest.starts_with("{#") {
                out.push(self.parse_block());
                continue;
            }
            if rest.starts_with('{') {
                out.push(self.parse_interpolation());
                continue;
            }
            if rest.starts_with('<') {
                out.push(self.parse_element());
                continue;
            }
            // Text up to next `<` or `{`.
            let next_lt = rest.find('<').unwrap_or(rest.len());
            let next_mu = rest.find('{').unwrap_or(rest.len());
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

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.rest().chars().next() {
            if c.is_whitespace() {
                self.cur += c.len_utf8();
            } else { break; }
        }
    }

    fn parse_element(&mut self) -> JsxNode {
        let rest = self.rest();
        let close_idx = match rest.find('>') {
            Some(i) => i,
            None => return JsxNode::Hole(Hole {
                kind: HoleKind::Unsupported,
                reason: "unterminated tag".into(),
                original: SourceSnippet::new(rest),
            }),
        };
        let opening = &rest[..=close_idx];
        let self_closing = opening.ends_with("/>");
        let inner = &opening[1..opening.len() - if self_closing { 2 } else { 1 }];
        let (name, attr_soup) = match inner.find(char::is_whitespace) {
            Some(i) => (&inner[..i], inner[i..].trim()),
            None => (inner, ""),
        };
        let name = name.to_string();
        let attrs = self.parse_attrs(attr_soup);
        self.cur += close_idx + 1;
        if self_closing {
            return JsxNode::Element { name, attrs, children: None };
        }
        let children = self.parse_children();
        let close = format!("</{}>", name);
        if self.rest().starts_with(&close) {
            self.cur += close.len();
        }
        JsxNode::Element { name, attrs, children: Some(children) }
    }

    fn parse_attrs(&self, soup: &str) -> Vec<JsxAttr> {
        let mut out = Vec::new();
        let mut s = soup;
        while !s.trim().is_empty() {
            s = s.trim_start();
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
                let (val, consumed) = read_attr_value(s);
                s = &s[consumed..];
                out.push(self.translate_attr(key, &val));
            } else {
                out.push(JsxAttr { name: key.into(), value: JsxAttrValue::StringLit(String::new()) });
            }
        }
        out
    }

    fn parse_interpolation(&mut self) -> JsxNode {
        // We're at `{`. Find matching `}`.
        let rest = self.rest();
        let Some(end) = rest.find('}') else {
            return JsxNode::Hole(Hole {
                kind: HoleKind::Unsupported,
                reason: "unterminated {}".into(),
                original: SourceSnippet::new(rest),
            });
        };
        let raw = rest[1..end].trim();
        self.cur += end + 1;
        JsxNode::Expr(rewrite_svelte_expr(raw))
    }

    fn parse_block(&mut self) -> JsxNode {
        // `{#if cond}` / `{#each items as item}` / etc. Skip the
        // opening, walk children, expect `{/tag}`.
        let rest = self.rest();
        let Some(end) = rest.find('}') else {
            return JsxNode::Hole(Hole {
                kind: HoleKind::Unsupported,
                reason: "unterminated block opening".into(),
                original: SourceSnippet::new(rest),
            });
        };
        let header = &rest[2..end]; // skip `{#`
        self.cur += end + 1;
        // Walk inner children.
        let children = self.parse_children();
        // Expect `{/<tag>}` — skip whatever the close is.
        if let Some(idx) = self.rest().find('}') {
            self.cur += idx + 1;
        }
        // Lower as a hole that carries the original construct;
        // structural lowering ({#if}→if-block etc.) belongs to a
        // future pass. The children are preserved.
        JsxNode::Hole(Hole {
            kind: HoleKind::Unsupported,
            reason: format!("Svelte block `{{#{}}}` — needs structural lowering", header.trim()),
            original: SourceSnippet::new(format!("{{#{}}} … ({} child nodes) …", header.trim(), children.len())),
        })
    }

    fn translate_attr(&self, key: &str, val: &str) -> JsxAttr {
        if let Some(event) = key.strip_prefix("on:") {
            let name = format!("on_{}", event);
            // Strip surrounding `{}` if present — `read_attr_value`
            // preserves them as a brace-marker.
            let inner = strip_braces(val.trim());
            let body = if let Some(fn_body) = self.handler_fns.get(inner) {
                fn_body.clone()
            } else {
                rewrite_svelte_expr(inner)
            };
            return JsxAttr {
                name,
                value: JsxAttrValue::Expr(format!("move || {}", body)),
            };
        }
        if let Some(prop) = key.strip_prefix("bind:") {
            return JsxAttr {
                name: prop.into(),
                value: JsxAttrValue::Hole(Hole {
                    kind: HoleKind::AttributeValue,
                    reason: format!("two-way binding `bind:{}` needs shim", prop),
                    original: SourceSnippet::new(format!("bind:{}={{{}}}", prop, val)),
                }),
            };
        }
        if let Some(_class_name) = key.strip_prefix("class:") {
            return JsxAttr {
                name: key.into(),
                value: JsxAttrValue::Hole(Hole {
                    kind: HoleKind::AttributeValue,
                    reason: "conditional class directive".into(),
                    original: SourceSnippet::new(format!("{}={{{}}}", key, val)),
                }),
            };
        }
        // `{value}` braced expression or `"literal"` string.
        let stripped = val.trim();
        if stripped.starts_with('{') && stripped.ends_with('}') {
            let inner = stripped[1..stripped.len() - 1].trim();
            return JsxAttr {
                name: key.into(),
                value: JsxAttrValue::Expr(rewrite_svelte_expr(inner)),
            };
        }
        JsxAttr { name: key.into(), value: JsxAttrValue::StringLit(stripped.into()) }
    }
}

/// Read either a `"…"` / `'…'` string or a `{…}` braced
/// expression value. Returns (value, bytes_consumed).
fn read_attr_value(s: &str) -> (String, usize) {
    let first = s.chars().next();
    match first {
        Some('"') | Some('\'') => {
            let q = first.unwrap();
            let s_rest = &s[1..];
            if let Some(end) = s_rest.find(q) {
                return (s_rest[..end].to_string(), end + 2);
            }
        }
        Some('{') => {
            let s_rest = &s[1..];
            if let Some(end) = s_rest.find('}') {
                // Return the *whole* braced form so downstream
                // can detect the brace markers.
                return (format!("{{{}}}", &s_rest[..end]), end + 2);
            }
        }
        _ => {}
    }
    (String::new(), 0)
}

/// Strip a single leading `{` and trailing `}` if both are present.
fn strip_braces(s: &str) -> &str {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        inner.trim()
    } else {
        s
    }
}

/// Rewrite a Svelte expression. Bare identifiers that look like
/// reactive lets get `.get()`. Other expressions pass through.
fn rewrite_svelte_expr(expr: &str) -> String {
    let e = expr.trim();
    if e.chars().all(|c| c.is_alphanumeric() || c == '_') && !e.is_empty() {
        return format!("{}.get()", e);
    }
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
    if out.chars().all(|c| c.is_whitespace()) { String::new() } else { out }
}

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
