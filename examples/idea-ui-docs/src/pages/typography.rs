//! Typography — the unified text component.
//!
//! Demo layout: a static gallery showing every `TypographyKind` at
//! its native size, plus one interactive demo where every prop is
//! twiddleable through the auto-generated controls panel.

use runtime_core::{ui, Primitive};
use idea_ui::doc_controls::DocControls;
use idea_ui::{typography, card, stack, TypographyKindRef, TypographyProps, StackGap};

use crate::shell::{demo_card, page_header};

pub fn page() -> Primitive {
    ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Typography",
                "Every kind of text on a page is a `Typography` component. Pick a `kind` for \
                 the size + weight + spacing; pick a `tone` for the color; pick an `align` for \
                 horizontal alignment."
            ) }

            { variant_gallery() }
            { interactive_demo() }
            { tone_gallery() }
        }
    }
}

// =============================================================================
// Static gallery — every kind shown at its native scale
// =============================================================================

fn variant_gallery() -> Primitive {
    let rows: Vec<Primitive> = vec![
        variant_row("Display", "The quick brown fox", idea_ui::typography_kind::Display.into()),
        variant_row("H1", "The quick brown fox", idea_ui::typography_kind::H1.into()),
        variant_row("H2", "The quick brown fox", idea_ui::typography_kind::H2.into()),
        variant_row("H3", "The quick brown fox", idea_ui::typography_kind::H3.into()),
        variant_row("BodyXl", "The quick brown fox jumps over the lazy dog.", idea_ui::typography_kind::BodyXl.into()),
        variant_row("BodyLg", "The quick brown fox jumps over the lazy dog.", idea_ui::typography_kind::BodyLg.into()),
        variant_row("Body", "The quick brown fox jumps over the lazy dog.", idea_ui::typography_kind::Body.into()),
        variant_row("BodySm", "The quick brown fox jumps over the lazy dog.", idea_ui::typography_kind::BodySm.into()),
        variant_row("Caption", "Helper text under a control", idea_ui::typography_kind::Caption.into()),
        variant_row("Overline", "Section label", idea_ui::typography_kind::Overline.into()),
    ];

    let label = ui! {
        Typography(content = "Variant gallery".to_string(), kind = idea_ui::typography_kind::H2.into())
    };
    let blurb = ui! {
        Typography(
            content = "Every `TypographyKind` rendered at its native size. The size scale \
                       is theme-tokenized (`typography-{kind}-size`); apps can retune by \
                       overriding individual fields on the `Typography` theme struct.".to_string(),
            muted = true,
        )
    };
    let mut children: Vec<Primitive> = Vec::with_capacity(rows.len() + 2);
    children.push(label);
    children.push(blurb);
    for r in rows {
        children.push(r);
    }
    ui! {
        Card { children }
    }
}

fn variant_row(name: &str, sample: &str, kind: TypographyKindRef) -> Primitive {
    let name_text = name.to_string();
    // Render the kind name in muted Overline so it doesn't fight the
    // sample line above it.
    let label = ui! {
        Typography(content = name_text, kind = idea_ui::typography_kind::Overline.into(), muted = true)
    };
    let sample_line = ui! {
        Typography(content = sample.to_string(), kind = kind)
    };
    let children: Vec<Primitive> = vec![label, sample_line];
    ui! {
        Stack(gap = StackGap::Xs) { children }
    }
}

// =============================================================================
// Interactive demo — auto-generated controls for every Typography prop
// =============================================================================

fn interactive_demo() -> Primitive {
    let state = TypographyProps::init_state();
    state.content.set(
        "Twiddle the controls to see the variant, tone, and align axes live.".to_string(),
    );
    let preview = TypographyProps::reactive_preview(&state, |props| {
        let content = props.content;
        let kind = props.kind;
        let tone = props.tone;
        let align = props.align;
        ui! {
            Typography(content = content, kind = kind, tone = tone, align = align)
        }
    });
    let controls = TypographyProps::render_controls(&state);
    demo_card(
        "Interactive",
        "Every Typography prop wired to a control. The `DocControls` derive walks the props \
         struct, builds a Signal per field, and rebuilds the preview tree on every signal flip.",
        preview,
        controls,
    )
}

// =============================================================================
// Tone gallery — one Body sample per tone
// =============================================================================

fn tone_gallery() -> Primitive {
    use idea_ui::ToneRef;
    enum ToneCell {
        Default,
        Muted,
        Color(ToneRef),
    }
    let tones: Vec<(&'static str, ToneCell)> = vec![
        ("Default", ToneCell::Default),
        ("Muted", ToneCell::Muted),
        ("Primary", ToneCell::Color(idea_ui::tone::Primary.into())),
        ("Danger", ToneCell::Color(idea_ui::tone::Danger.into())),
        ("Success", ToneCell::Color(idea_ui::tone::Success.into())),
        ("Warning", ToneCell::Color(idea_ui::tone::Warning.into())),
        ("Info", ToneCell::Color(idea_ui::tone::Info.into())),
    ];
    let mut rows: Vec<Primitive> = Vec::with_capacity(tones.len());
    for (label, cell) in tones {
        let sample = format!("{} — readable on both light and dark surfaces.", label);
        rows.push(match cell {
            ToneCell::Default => ui! {
                Typography(content = sample)
            },
            ToneCell::Muted => ui! {
                Typography(content = sample, muted = true)
            },
            ToneCell::Color(t) => ui! {
                Typography(content = sample, tone = Some(t))
            },
        });
    }
    let label = ui! {
        Typography(content = "Tone gallery".to_string(), kind = idea_ui::typography_kind::H2.into())
    };
    let blurb = ui! {
        Typography(
            content = "Each tone reads a different theme color token. `Inverse` is omitted \
                       here because it's white-on-dark — visible only against a dark surface.".to_string(),
            muted = true,
        )
    };
    let mut children: Vec<Primitive> = Vec::with_capacity(rows.len() + 2);
    children.push(label);
    children.push(blurb);
    for r in rows {
        children.push(r);
    }
    ui! {
        Card { children }
    }
}
