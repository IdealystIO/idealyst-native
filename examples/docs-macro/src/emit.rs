//! Emission for the `docs!` macro.
//!
//! Two outputs per page:
//!
//! 1. `pub static PAGE_META: crate::meta::PageMeta = ...` — the metadata
//!    blob. All `&'static` data, lives in `.rodata`.
//! 2. `pub fn page() -> ::framework_core::Primitive` — the
//!    renderable screen. v1 emits a minimal tree (title + sections
//!    + paragraphs as plain text); rendering rich layouts via shell
//!    components lands in a follow-up.

use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::Result;

use crate::parse::{Block, Compare, DocPage, Demo, Note, Span_, TopBlock};
use crate::parse::heading_to_slug;

pub fn emit(page: DocPage) -> Result<TokenStream2> {
    let meta = emit_page_meta(&page)?;
    let render = emit_page_fn(&page)?;

    Ok(quote! {
        #meta
        #render
    })
}

// =============================================================================
// PAGE_META emission
// =============================================================================

fn emit_page_meta(page: &DocPage) -> Result<TokenStream2> {
    let slug = &page.slug;
    let title = &page.title;
    let category_variant = &page.category;
    let category = quote! { crate::meta::PageCategory::#category_variant };

    let description = match &page.description {
        Some(s) => quote! { ::core::option::Option::Some(#s) },
        None => quote! { ::core::option::Option::None },
    };

    let related = &page.related;
    let related_tokens = quote! { &[ #( #related ),* ] };

    let concept_tokens: Vec<_> = page
        .concepts
        .iter()
        .map(|c| quote! { crate::meta::DocConcept::#c })
        .collect();
    let concepts_tokens = quote! { &[ #( #concept_tokens ),* ] };

    let section_tokens: Vec<_> = page
        .blocks
        .iter()
        .map(|b| emit_top_block_meta(b))
        .collect::<Result<Vec<_>>>()?;
    let sections_tokens = quote! { &[ #( #section_tokens ),* ] };

    Ok(quote! {
        pub static PAGE_META: crate::meta::PageMeta = crate::meta::PageMeta {
            slug: #slug,
            title: #title,
            category: #category,
            description: #description,
            related: #related_tokens,
            concepts: #concepts_tokens,
            sections: #sections_tokens,
        };
    })
}

/// Top-level blocks are conceptually "sections" — but compare/note/demo
/// don't have headings. The metadata model is "every page is a flat
/// list of sections," so we wrap each non-section top-block in a
/// pseudo-section with an empty heading. The UI side does the right
/// thing with each.
///
/// We *could* support free-standing compare/note/demo blocks at the
/// page level (no enclosing section), but in practice every example
/// nests them inside a `section`. If the author writes one at top
/// level, it ends up in a section whose heading is empty — visually
/// it'll render as an orphan, which is fine v1 behavior.
fn emit_top_block_meta(block: &TopBlock) -> Result<TokenStream2> {
    match block {
        TopBlock::Section(s) => {
            let heading = &s.heading;
            let slug = match &s.slug_override {
                Some(lit) => lit.value(),
                None => heading_to_slug(&s.heading.value()),
            };
            let blocks = s
                .blocks
                .iter()
                .map(emit_block_meta)
                .collect::<Result<Vec<_>>>()?;
            Ok(quote! {
                crate::meta::SectionMeta {
                    heading: #heading,
                    slug: #slug,
                    blocks: &[ #( #blocks ),* ],
                }
            })
        }
        TopBlock::Compare(c) => {
            let cmp = emit_compare_meta(c)?;
            // Wrap in a no-heading section.
            Ok(quote! {
                crate::meta::SectionMeta {
                    heading: "",
                    slug: "",
                    blocks: &[ #cmp ],
                }
            })
        }
        TopBlock::Note(n) => {
            let note = emit_note_meta(n)?;
            Ok(quote! {
                crate::meta::SectionMeta {
                    heading: "",
                    slug: "",
                    blocks: &[ #note ],
                }
            })
        }
        TopBlock::Demo(d) => {
            let demo = emit_demo_meta(d);
            Ok(quote! {
                crate::meta::SectionMeta {
                    heading: "",
                    slug: "",
                    blocks: &[ #demo ],
                }
            })
        }
    }
}

fn emit_block_meta(block: &Block) -> Result<TokenStream2> {
    match block {
        Block::Paragraph(spans) => {
            let span_tokens: Vec<_> = spans.iter().map(emit_span_meta).collect();
            Ok(quote! {
                crate::meta::BlockMeta::Paragraph(&[ #( #span_tokens ),* ])
            })
        }
        Block::Code { language, source } => {
            let lang = language.to_string();
            let trimmed = trim_indented_source(&source.value());
            Ok(quote! {
                crate::meta::BlockMeta::Code {
                    language: #lang,
                    source: #trimmed,
                }
            })
        }
        Block::List(items) => {
            let item_tokens: Vec<_> = items
                .iter()
                .map(|spans| {
                    let s: Vec<_> = spans.iter().map(emit_span_meta).collect();
                    quote! { &[ #( #s ),* ] as &[crate::meta::Span] }
                })
                .collect();
            Ok(quote! {
                crate::meta::BlockMeta::List(&[ #( #item_tokens ),* ])
            })
        }
        Block::Comparison(c) => emit_compare_meta(c),
        Block::Note(n) => emit_note_meta(n),
        Block::Demo(d) => Ok(emit_demo_meta(d)),
    }
}

fn emit_compare_meta(c: &Compare) -> Result<TokenStream2> {
    let from = &c.from;
    let blocks: Vec<_> = c.blocks.iter().map(emit_block_meta).collect::<Result<Vec<_>>>()?;
    Ok(quote! {
        crate::meta::BlockMeta::Comparison {
            from: crate::meta::ComparisonFramework::#from,
            blocks: &[ #( #blocks ),* ],
        }
    })
}

fn emit_note_meta(n: &Note) -> Result<TokenStream2> {
    let kind = &n.kind;
    let blocks: Vec<_> = n.blocks.iter().map(emit_block_meta).collect::<Result<Vec<_>>>()?;
    Ok(quote! {
        crate::meta::BlockMeta::Note {
            kind: crate::meta::NoteKind::#kind,
            blocks: &[ #( #blocks ),* ],
        }
    })
}

fn emit_demo_meta(d: &Demo) -> TokenStream2 {
    let name = path_to_string(&d.fn_path);
    let description = match &d.description {
        Some(lit) => quote! { ::core::option::Option::Some(#lit) },
        None => quote! { ::core::option::Option::None },
    };
    quote! {
        crate::meta::BlockMeta::Demo {
            name: #name,
            description: #description,
        }
    }
}

fn emit_span_meta(span: &Span_) -> TokenStream2 {
    match span {
        Span_::Text(s) => quote! { crate::meta::Span::Text(#s) },
        Span_::Code(s) => quote! { crate::meta::Span::Code(#s) },
        Span_::Link { text, target } => quote! {
            crate::meta::Span::Link { text: #text, target: #target }
        },
    }
}

// =============================================================================
// page() function emission
// =============================================================================
//
// Emits an `idealyst` UI tree using the shell components the docs
// site provides (`PageHeader`, `Card`, `CodeBlock`, plus
// `idea_ui::{Heading, Body, Stack, ScrollView, Text}`).
//
// Shape per page:
//
//   ScrollView {
//       Stack(gap = StackGap::Xl) {
//           PageHeader(title, description)
//           // Per section: a Card containing the section's blocks.
//           Card {
//               Heading(content = "...", kind = HeadingKind::H2)
//               Body(content = "paragraph text", tone = BodyTone::Muted)
//               CodeBlock(code = "...")
//               // Comparisons / notes / demos flatten inline for v1.
//           }
//       }
//   }
//
// Inline span structure (bold, code spans, links) is flattened to a
// plain string in this v1 — backticks around code spans, link text
// inlined. Per-span styling lands when the shell grows richer
// primitives.
//
// Demos are real function references emitted as inline component
// invocations; the compiler checks the symbol at the docs! call site.

fn emit_page_fn(page: &DocPage) -> Result<TokenStream2> {
    let title_lit = page.title.value();
    let description_lit = page
        .description
        .as_ref()
        .map(|d| d.value())
        .unwrap_or_default();

    let mut card_blocks: Vec<TokenStream2> = Vec::new();
    for block in &page.blocks {
        card_blocks.extend(emit_top_block_render(block));
    }

    Ok(quote! {
        pub fn page() -> ::framework_core::Primitive {
            ::framework_core::ui! {
                ScrollView {
                    Stack(gap = ::idea_ui::StackGap::Xl) {
                        PageHeader(
                            title = #title_lit.to_string(),
                            description = #description_lit.to_string(),
                        )
                        #( #card_blocks )*
                    }
                }
            }
        }
    })
}

/// Each top-level block in the docs! input produces zero or more
/// elements inside the page's Stack. For sections we emit one Card
/// per section. Free-floating compare/note/demo (without an enclosing
/// section) become inline Cards or direct component calls.
fn emit_top_block_render(block: &TopBlock) -> Vec<TokenStream2> {
    match block {
        TopBlock::Section(s) => {
            let heading = s.heading.value();
            let mut card_children: Vec<TokenStream2> = Vec::new();
            card_children.push(quote! {
                Heading(
                    content = #heading.to_string(),
                    kind = ::idea_ui::HeadingKind::H2,
                )
            });
            for b in &s.blocks {
                card_children.extend(emit_block_render(b));
            }
            vec![quote! {
                Card {
                    #( #card_children )*
                }
            }]
        }
        TopBlock::Compare(c) => {
            // Top-level comparison (no enclosing section). Emit a Card
            // labeled by the framework.
            vec![emit_compare_card(c)]
        }
        TopBlock::Note(n) => {
            vec![emit_note_card(n)]
        }
        TopBlock::Demo(d) => {
            // Direct component call.
            let path = &d.fn_path;
            vec![quote! { #path() }]
        }
    }
}

/// Renders one block *inside* a Card (the section body). Each return
/// value is a token stream consumable by `ui!`.
fn emit_block_render(block: &Block) -> Vec<TokenStream2> {
    match block {
        Block::Paragraph(spans) => {
            let text = spans_to_plain_text(spans);
            vec![quote! {
                Body(
                    content = #text.to_string(),
                    tone = ::idea_ui::BodyTone::Muted,
                )
            }]
        }
        Block::Code { source, .. } => {
            let text = trim_indented_source(&source.value());
            vec![quote! {
                CodeBlock(code = #text.to_string())
            }]
        }
        Block::List(items) => items
            .iter()
            .map(|spans| {
                let text = format!("• {}", spans_to_plain_text(spans));
                quote! {
                    Body(
                        content = #text.to_string(),
                        tone = ::idea_ui::BodyTone::Muted,
                    )
                }
            })
            .collect(),
        Block::Comparison(c) => {
            // Comparison inside a section. Emit a sub-heading + the
            // inner blocks.
            let framework_label = format!("From {}", c.from);
            let mut out = Vec::new();
            out.push(quote! {
                Heading(
                    content = #framework_label.to_string(),
                    kind = ::idea_ui::HeadingKind::H3,
                )
            });
            for b in &c.blocks {
                out.extend(emit_block_render(b));
            }
            out
        }
        Block::Note(n) => {
            // Note inside a section. Prefix with the kind label.
            let kind_label = format!("{}:", n.kind);
            let mut out = Vec::new();
            out.push(quote! {
                Heading(
                    content = #kind_label.to_string(),
                    kind = ::idea_ui::HeadingKind::H3,
                )
            });
            for b in &n.blocks {
                out.extend(emit_block_render(b));
            }
            out
        }
        Block::Demo(d) => {
            let path = &d.fn_path;
            vec![quote! { #path() }]
        }
    }
}

fn emit_compare_card(c: &Compare) -> TokenStream2 {
    let label = format!("From {}", c.from);
    let mut card_children: Vec<TokenStream2> = Vec::new();
    card_children.push(quote! {
        Heading(
            content = #label.to_string(),
            kind = ::idea_ui::HeadingKind::H3,
        )
    });
    for b in &c.blocks {
        card_children.extend(emit_block_render(b));
    }
    quote! {
        Card {
            #( #card_children )*
        }
    }
}

fn emit_note_card(n: &Note) -> TokenStream2 {
    let label = format!("{}:", n.kind);
    let mut card_children: Vec<TokenStream2> = Vec::new();
    card_children.push(quote! {
        Heading(
            content = #label.to_string(),
            kind = ::idea_ui::HeadingKind::H3,
        )
    });
    for b in &n.blocks {
        card_children.extend(emit_block_render(b));
    }
    quote! {
        Card {
            #( #card_children )*
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Concatenate a list of spans into a plain-text string for the
/// minimal renderer. Code spans are bracketed with backticks; link
/// spans use their display text.
fn spans_to_plain_text(spans: &[Span_]) -> String {
    let mut out = String::new();
    for s in spans {
        match s {
            Span_::Text(lit) => out.push_str(&lit.value()),
            Span_::Code(lit) => {
                out.push('`');
                out.push_str(&lit.value());
                out.push('`');
            }
            Span_::Link { text, .. } => out.push_str(&text.value()),
        }
    }
    out
}

fn path_to_string(path: &syn::Path) -> String {
    path.to_token_stream().to_string().replace(' ', "")
}

/// Trim a raw string literal's common leading indentation. Authors
/// indent code blocks naturally inside `docs! { ... }`, so the macro
/// strips the prefix shared by every non-empty line.
fn trim_indented_source(src: &str) -> String {
    // Trim leading/trailing blank lines.
    let lines: Vec<&str> = src.lines().collect();
    let first = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
    let last = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(lines.len());
    let body = &lines[first..last];

    // Compute the common leading indent across non-blank lines.
    let common: usize = body
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    let mut out = String::new();
    for (i, line) in body.iter().enumerate() {
        if !line.trim().is_empty() {
            out.push_str(&line[common..]);
        }
        if i + 1 != body.len() {
            out.push('\n');
        }
    }
    out
}
