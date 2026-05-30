//! Typography — the unified text component.

use runtime_core::{ui, Element};
use idea_ui::doc_controls::DocControls;
use idea_ui::{
    tone, typography_kind, Stack, StackGap, ToneRef, Typography, TypographyKindRef,
    TypographyProps,
};

use crate::shell::{
    self, Callout, CodePanel, ComponentPage, Demo, DemoSurface, H2, P, Prop, PropsTable, Section,
};

pub fn page() -> Element {
    let state = TypographyProps::init_state();
    state.content.set(
        "Twiddle the controls to see kind, tone, and align live.".to_string(),
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

    shell::layout(ui! {
        ComponentPage(
            title = "Typography".to_string(),
            lead = "Every textual surface on a page is a Typography. Pick a `kind` for the \
                size + weight + spacing; pick a `tone` for the color; pick an `align` for \
                horizontal alignment.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Kind gallery".to_string()) {
                P(content = "Every TypographyKind rendered at its native scale. The size scale \
                    is theme-tokenized (`typography-{kind}-size`); apps retune by overriding \
                    individual fields on the Typography theme struct.".to_string())
                DemoSurface {
                    kind_gallery()
                }
            }

            Section(title = "Tone gallery".to_string()) {
                P(content = "Each tone reads a different theme color token. `muted = true` is \
                    a shorthand for `tone = None` + the muted text token.".to_string())
                DemoSurface {
                    tone_gallery()
                }
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "content",
                        ty: "Reactive<String>",
                        desc: "Text content. Accepts a literal, String, Signal<String>, or rx!(...) computed binding.",
                    },
                    Prop {
                        name: "kind",
                        ty: "TypographyKindRef",
                        desc: "Display / H1..H3 / BodyXl..BodySm / Caption / Overline. Defaults to Body.",
                    },
                    Prop {
                        name: "tone",
                        ty: "Option<ToneRef>",
                        desc: "Optional semantic color. When Some, overrides the muted flag.",
                    },
                    Prop {
                        name: "muted",
                        ty: "bool",
                        desc: "When true and tone is None, use the theme's muted text color.",
                    },
                    Prop {
                        name: "align",
                        ty: "TextAlign",
                        desc: "Left (default) / Center / Right / Justify. Skipped from auto-controls (framework enum without VariantEnum).",
                    },
                ])
            }

            Section(title = "Reactive text".to_string()) {
                P(content = "Typography's `content` is `Reactive<String>` — a literal is static, \
                    a Signal updates the rendered node in place, and `rx!(...)` updates on every \
                    read signal change. No parent rebuild.".to_string())
                CodePanel(src = r##"// Static — String coerces to Reactive::Static.
Typography(content = "Welcome".to_string())

// Live — Signal<String> updates the rendered text in place.
let name = signal!("World".to_string());
Typography(content = name)

// Computed — rx! tracks every read signal and re-derives.
Typography(content = rx!(format!("{} clicks", count.get())))"##.to_string())
            }

            Callout(label = "Picking the right kind".to_string()) {
                P(content = "Use H1 / H2 / H3 for headings. Use Body* for paragraphs (Body is \
                    the default; BodyLg for lead paragraphs, BodySm for tight UI text). Use \
                    Caption for muted helper text under controls. Use Overline for SECTION-style \
                    labels above grouped content.".to_string())
            }
        }
    })
}

fn kind_gallery() -> Element {
    let rows: Vec<Element> = vec![
        kind_row("Display",  "The quick brown fox", typography_kind::Display.into()),
        kind_row("H1",       "The quick brown fox", typography_kind::H1.into()),
        kind_row("H2",       "The quick brown fox", typography_kind::H2.into()),
        kind_row("H3",       "The quick brown fox", typography_kind::H3.into()),
        kind_row("BodyXl",   "The quick brown fox jumps over the lazy dog.", typography_kind::BodyXl.into()),
        kind_row("BodyLg",   "The quick brown fox jumps over the lazy dog.", typography_kind::BodyLg.into()),
        kind_row("Body",     "The quick brown fox jumps over the lazy dog.", typography_kind::Body.into()),
        kind_row("BodySm",   "The quick brown fox jumps over the lazy dog.", typography_kind::BodySm.into()),
        kind_row("Caption",  "Helper text under a control",                  typography_kind::Caption.into()),
        kind_row("Overline", "Section label",                                typography_kind::Overline.into()),
    ];
    ui! { Stack(gap = StackGap::Md) { rows } }
}

fn kind_row(name: &str, sample: &str, kind: TypographyKindRef) -> Element {
    let name_text = name.to_string();
    let label = ui! {
        Typography(content = name_text, kind = typography_kind::Overline, muted = true)
    };
    let sample_line = ui! { Typography(content = sample.to_string(), kind = kind) };
    let children: Vec<Element> = vec![label, sample_line];
    ui! { Stack(gap = StackGap::Xs) { children } }
}

fn tone_gallery() -> Element {
    enum ToneCell {
        Default,
        Muted,
        Color(ToneRef),
    }
    let tones: Vec<(&'static str, ToneCell)> = vec![
        ("Default", ToneCell::Default),
        ("Muted",   ToneCell::Muted),
        ("Primary", ToneCell::Color(tone::Primary.into())),
        ("Danger",  ToneCell::Color(tone::Danger.into())),
        ("Success", ToneCell::Color(tone::Success.into())),
        ("Warning", ToneCell::Color(tone::Warning.into())),
        ("Info",    ToneCell::Color(tone::Info.into())),
    ];
    let mut rows: Vec<Element> = Vec::with_capacity(tones.len());
    for (label, cell) in tones {
        let sample = format!("{} — readable on both light and dark surfaces.", label);
        rows.push(match cell {
            ToneCell::Default => ui! { Typography(content = sample) },
            ToneCell::Muted => ui! { Typography(content = sample, muted = true) },
            ToneCell::Color(t) => ui! { Typography(content = sample, tone = Some(t)) },
        });
    }
    ui! { Stack(gap = StackGap::Sm) { rows } }
}
