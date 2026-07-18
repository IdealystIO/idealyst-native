//! Input grammar parser for `docs!`.
//!
//! The parser is intentionally permissive at lex time and strict on
//! interpretation: any top-level identifier that isn't a known
//! field name (`slug`, `title`, `category`, `description`,
//! `related`, `concepts`) or a known block kind (`section`,
//! `compare`, `note`, `demo`) is rejected with a clear error.

use proc_macro2::Span;
use std::collections::HashSet;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{braced, bracketed, parenthesized, Ident, LitStr, Path, Result, Token};

// =============================================================================
// AST
// =============================================================================

pub struct DocPage {
    pub slug: LitStr,
    pub title: LitStr,
    pub category: Ident,
    pub description: Option<LitStr>,
    pub related: Vec<LitStr>,
    pub concepts: Vec<Ident>,
    pub blocks: Vec<TopBlock>,
}

pub enum TopBlock {
    Section(Section),
    Compare(Compare),
    Note(Note),
    Demo(Demo),
}

pub struct Section {
    pub heading: LitStr,
    pub slug_override: Option<LitStr>,
    pub blocks: Vec<Block>,
}

pub enum Block {
    Paragraph(Vec<Span_>),
    Code { language: Ident, source: LitStr },
    List(Vec<Vec<Span_>>),
    Comparison(Compare),
    Note(Note),
    Demo(Demo),
}

#[derive(Clone)]
pub enum Span_ {
    text(LitStr),
    Code(LitStr),
    link { text: LitStr, target: LitStr },
}

pub struct Compare {
    pub from: Ident,
    pub blocks: Vec<Block>,
}

pub struct Note {
    pub kind: Ident,
    pub blocks: Vec<Block>,
}

pub struct Demo {
    pub fn_path: Path,
    pub description: Option<LitStr>,
}

// =============================================================================
// Page-level parsing
// =============================================================================

impl Parse for DocPage {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut slug: Option<LitStr> = None;
        let mut title: Option<LitStr> = None;
        let mut category: Option<Ident> = None;
        let mut description: Option<LitStr> = None;
        let mut related: Vec<LitStr> = Vec::new();
        let mut concepts: Vec<Ident> = Vec::new();
        let mut blocks: Vec<TopBlock> = Vec::new();

        while !input.is_empty() {
            // Every top-level item starts with an ident — either a
            // field name (followed by `=`) or a block kind (followed
            // by `(` or `{`).
            let lookahead = input.fork();
            let kw: Ident = lookahead.parse()?;
            let name = kw.to_string();

            // Decide based on what follows the ident.
            let is_field = match name.as_str() {
                "slug" | "title" | "category" | "description" | "related"
                | "concepts" => true,
                _ => false,
            };

            if is_field {
                input.parse::<Ident>()?; // consume
                input.parse::<Token![=]>()?;

                match name.as_str() {
                    "slug" => slug = Some(input.parse()?),
                    "title" => title = Some(input.parse()?),
                    "category" => category = Some(input.parse()?),
                    "description" => description = Some(input.parse()?),
                    "related" => related = parse_string_array(input)?,
                    "concepts" => concepts = parse_ident_array(input)?,
                    _ => unreachable!(),
                }

                // Field separator: optional comma at EOF.
                if !input.is_empty() {
                    input.parse::<Token![,]>()?;
                }
            } else {
                // Block — section / compare / note / demo.
                let block = match name.as_str() {
                    "section" => TopBlock::Section(input.parse()?),
                    "compare" => TopBlock::Compare(input.parse()?),
                    "note" => TopBlock::Note(input.parse()?),
                    "demo" => TopBlock::Demo(input.parse()?),
                    other => {
                        return Err(syn::Error::new(
                            kw.span(),
                            format!(
                                "expected a field (slug/title/category/description/related/concepts) \
                                 or block (section/compare/note/demo); got `{}`",
                                other
                            ),
                        ));
                    }
                };
                blocks.push(block);

                // Block separator: optional comma.
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                }
            }
        }

        // Required fields.
        let slug = slug.ok_or_else(|| {
            syn::Error::new(Span::call_site(), "missing required field `slug`")
        })?;
        let title = title.ok_or_else(|| {
            syn::Error::new(Span::call_site(), "missing required field `title`")
        })?;
        let category = category.ok_or_else(|| {
            syn::Error::new(Span::call_site(), "missing required field `category`")
        })?;

        // Duplicate-slug check across sections.
        let mut seen = HashSet::new();
        for b in &blocks {
            if let TopBlock::Section(s) = b {
                let derived = s
                    .slug_override
                    .as_ref()
                    .map(|s| s.value())
                    .unwrap_or_else(|| heading_to_slug(&s.heading.value()));
                if !seen.insert(derived.clone()) {
                    return Err(syn::Error::new(
                        s.heading.span(),
                        format!(
                            "duplicate section slug `{}`. Two sections derive (or set) \
                             the same slug. Pass an explicit `slug = \"...\"` on one of them.",
                            derived
                        ),
                    ));
                }
            }
        }

        Ok(DocPage {
            slug,
            title,
            category,
            description,
            related,
            concepts,
            blocks,
        })
    }
}

// =============================================================================
// Block parsers
// =============================================================================

impl Parse for Section {
    fn parse(input: ParseStream) -> Result<Self> {
        input.parse::<Ident>()?; // "section"
        let args;
        parenthesized!(args in input);

        let mut heading: Option<LitStr> = None;
        let mut slug_override: Option<LitStr> = None;

        while !args.is_empty() {
            let key: Ident = args.parse()?;
            args.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "heading" => heading = Some(args.parse()?),
                "slug" => slug_override = Some(args.parse()?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("section: unexpected key `{}` (expected `heading` or `slug`)", other),
                    ));
                }
            }
            if !args.is_empty() {
                args.parse::<Token![,]>()?;
            }
        }

        let heading = heading.ok_or_else(|| {
            syn::Error::new(Span::call_site(), "section: missing required `heading`")
        })?;

        let body;
        braced!(body in input);
        let blocks = parse_block_list(&body)?;

        Ok(Section {
            heading,
            slug_override,
            blocks,
        })
    }
}

impl Parse for Compare {
    fn parse(input: ParseStream) -> Result<Self> {
        input.parse::<Ident>()?; // "compare"
        let args;
        parenthesized!(args in input);

        let from_key: Ident = args.parse()?;
        if from_key != "from" {
            return Err(syn::Error::new(
                from_key.span(),
                format!("compare: expected `from`, got `{}`", from_key),
            ));
        }
        args.parse::<Token![=]>()?;
        let from: Ident = args.parse()?;

        if !args.is_empty() {
            args.parse::<Token![,]>()?;
        }

        let body;
        braced!(body in input);
        let blocks = parse_block_list(&body)?;

        Ok(Compare { from, blocks })
    }
}

impl Parse for Note {
    fn parse(input: ParseStream) -> Result<Self> {
        input.parse::<Ident>()?; // "note"
        let args;
        parenthesized!(args in input);

        let kind_key: Ident = args.parse()?;
        if kind_key != "kind" {
            return Err(syn::Error::new(
                kind_key.span(),
                format!("note: expected `kind`, got `{}`", kind_key),
            ));
        }
        args.parse::<Token![=]>()?;
        let kind: Ident = args.parse()?;

        if !args.is_empty() {
            args.parse::<Token![,]>()?;
        }

        let body;
        braced!(body in input);
        let blocks = parse_block_list(&body)?;

        Ok(Note { kind, blocks })
    }
}

impl Parse for Demo {
    fn parse(input: ParseStream) -> Result<Self> {
        input.parse::<Ident>()?; // "demo"
        let args;
        parenthesized!(args in input);

        let fn_path: Path = args.parse()?;
        let mut description: Option<LitStr> = None;

        while !args.is_empty() {
            args.parse::<Token![,]>()?;
            if args.is_empty() {
                break; // trailing comma
            }
            let key: Ident = args.parse()?;
            args.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "description" => description = Some(args.parse()?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("demo: unexpected key `{}`", other),
                    ));
                }
            }
        }

        Ok(Demo { fn_path, description })
    }
}

// =============================================================================
// Block list inside section / compare / note / list
// =============================================================================

fn parse_block_list(input: ParseStream) -> Result<Vec<Block>> {
    let mut out = Vec::new();
    while !input.is_empty() {
        out.push(parse_block(input)?);
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
    }
    Ok(out)
}

fn parse_block(input: ParseStream) -> Result<Block> {
    let kw: Ident = input.fork().parse()?;
    let name = kw.to_string();

    match name.as_str() {
        "p" => {
            input.parse::<Ident>()?; // "p"
            let args;
            parenthesized!(args in input);
            let spans = parse_span_list(&args)?;
            Ok(Block::Paragraph(spans))
        }
        "code" => {
            input.parse::<Ident>()?; // "code"
            let args;
            parenthesized!(args in input);
            // code(rust, r#"..."#) — language ident, then source string
            let language: Ident = args.parse()?;
            args.parse::<Token![,]>()?;
            let source: LitStr = args.parse()?;
            Ok(Block::Code { language, source })
        }
        "list" => {
            input.parse::<Ident>()?; // "list"
            let args;
            parenthesized!(args in input);
            let mut items = Vec::new();
            while !args.is_empty() {
                let item_buf;
                bracketed!(item_buf in args);
                let spans = parse_span_list(&item_buf)?;
                items.push(spans);
                if args.peek(Token![,]) {
                    args.parse::<Token![,]>()?;
                }
            }
            Ok(Block::List(items))
        }
        "compare" => Ok(Block::Comparison(input.parse()?)),
        "note" => Ok(Block::Note(input.parse()?)),
        "demo" => Ok(Block::Demo(input.parse()?)),
        other => Err(syn::Error::new(
            kw.span(),
            format!(
                "expected a block (`p(...)`, `code(...)`, `list(...)`, \
                 `compare(...)`, `note(...)`, `demo(...)`); got `{}`",
                other
            ),
        )),
    }
}

// =============================================================================
// Spans
// =============================================================================

fn parse_span_list(input: ParseStream) -> Result<Vec<Span_>> {
    let mut out = Vec::new();
    while !input.is_empty() {
        out.push(parse_span(input)?);
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
    }
    Ok(out)
}

fn parse_span(input: ParseStream) -> Result<Span_> {
    // Plain string literal → Text. Otherwise it's a function-call-shaped
    // span (`code(...)` or `link(...)`).
    if input.peek(LitStr) {
        let s: LitStr = input.parse()?;
        return Ok(Span_::text(s));
    }

    let kw: Ident = input.parse()?;
    let args;
    parenthesized!(args in input);

    match kw.to_string().as_str() {
        "code" => {
            let s: LitStr = args.parse()?;
            Ok(Span_::Code(s))
        }
        "link" => {
            let text: LitStr = args.parse()?;
            args.parse::<Token![,]>()?;
            let to_key: Ident = args.parse()?;
            if to_key != "to" {
                return Err(syn::Error::new(
                    to_key.span(),
                    format!("link: expected `to`, got `{}`", to_key),
                ));
            }
            args.parse::<Token![=]>()?;
            let target: LitStr = args.parse()?;
            Ok(Span_::link { text, target })
        }
        other => Err(syn::Error::new(
            kw.span(),
            format!(
                "expected a span (string literal, `code(...)`, or `link(...)`); got `{}`",
                other
            ),
        )),
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn parse_string_array(input: ParseStream) -> Result<Vec<LitStr>> {
    let buf;
    bracketed!(buf in input);
    let pairs: Punctuated<LitStr, Token![,]> =
        Punctuated::parse_terminated(&buf)?;
    Ok(pairs.into_iter().collect())
}

fn parse_ident_array(input: ParseStream) -> Result<Vec<Ident>> {
    let buf;
    bracketed!(buf in input);
    let pairs: Punctuated<Ident, Token![,]> =
        Punctuated::parse_terminated(&buf)?;
    Ok(pairs.into_iter().collect())
}

/// Lowercase + kebab-case heading text into a slug. Strips
/// non-alphanumeric runs and collapses them to single hyphens.
pub fn heading_to_slug(heading: &str) -> String {
    let lower = heading.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_was_dash = true; // collapse leading non-alnum
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    // Trim trailing dash.
    while out.ends_with('-') {
        out.pop();
    }
    out
}
