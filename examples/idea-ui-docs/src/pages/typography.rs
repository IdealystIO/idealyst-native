//! Typography — the unified text component.
//!
//! Demo layout: a static gallery showing every `TypographyKind` at
//! its native size, plus one interactive demo where every prop is
//! twiddleable through the auto-generated controls panel.

use runtime_core::{ui, Primitive};
use idea_ui::doc_controls::DocControls;
use idea_ui::{typography, card, stack, TypographyKind, TypographyProps, TypographyTone, StackGap};

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
        variant_row("Display", "The quick brown fox", TypographyKind::Display),
        variant_row("H1", "The quick brown fox", TypographyKind::H1),
        variant_row("H2", "The quick brown fox", TypographyKind::H2),
        variant_row("H3", "The quick brown fox", TypographyKind::H3),
        variant_row("BodyXl", "The quick brown fox jumps over the lazy dog.", TypographyKind::BodyXl),
        variant_row("BodyLg", "The quick brown fox jumps over the lazy dog.", TypographyKind::BodyLg),
        variant_row("Body", "The quick brown fox jumps over the lazy dog.", TypographyKind::Body),
        variant_row("BodySm", "The quick brown fox jumps over the lazy dog.", TypographyKind::BodySm),
        variant_row("Caption", "Helper text under a control", TypographyKind::Caption),
        variant_row("Overline", "Section label", TypographyKind::Overline),
    ];

    let label = ui! {
        Typography(content = "Variant gallery".to_string(), kind = TypographyKind::H2)
    };
    let blurb = ui! {
        Typography(
            content = "Every `TypographyKind` rendered at its native size. The size scale \
                       is theme-tokenized (`typography-{kind}-size`); apps can retune by \
                       overriding individual fields on the `Typography` theme struct.".to_string(),
            tone = TypographyTone::Muted,
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

fn variant_row(name: &str, sample: &str, kind: TypographyKind) -> Primitive {
    let name_text = format!("{:?}", kind);
    // Render the kind name in muted Caption so it doesn't fight the
    // sample line above it.
    let _ = name;
    let label = ui! {
        Typography(content = name_text, kind = TypographyKind::Overline, tone = TypographyTone::Muted)
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
    let tones = [
        ("Default", TypographyTone::Default),
        ("Muted", TypographyTone::Muted),
        ("Primary", TypographyTone::Primary),
        ("Danger", TypographyTone::Danger),
        ("Success", TypographyTone::Success),
        ("Warning", TypographyTone::Warning),
        ("Info", TypographyTone::Info),
    ];
    let mut rows: Vec<Primitive> = Vec::with_capacity(tones.len());
    for (label, tone) in tones {
        let sample = format!("{} — readable on both light and dark surfaces.", label);
        rows.push(ui! {
            Typography(content = sample, tone = tone)
        });
    }
    let label = ui! {
        Typography(content = "Tone gallery".to_string(), kind = TypographyKind::H2)
    };
    let blurb = ui! {
        Typography(
            content = "Each tone reads a different theme color token. `Inverse` is omitted \
                       here because it's white-on-dark — visible only against a dark surface.".to_string(),
            tone = TypographyTone::Muted,
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
