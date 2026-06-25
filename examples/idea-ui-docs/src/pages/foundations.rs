//! Foundations — Color, Intents, Spacing & Radius.
//!
//! Body-only pages: the central frame renders the title, lead, overline,
//! status badge, and Usage panel.

use std::rc::Rc;

use runtime_core::{ui, view, Color, Element, IntoElement, StyleApplication, Tokenized};
use idea_ui::{
    tone, typography_kind, variant, Badge, Button, Grid, Stack, StackAxis, StackGap, ToneRef,
    Typography,
};

use crate::pages::body;
use crate::shell::{Callout, DemoSurface, P, Section};
use crate::styles::{GapBlock, RadiusBox, RadiusBoxR, SwatchBlock};

// =============================================================================
// Color — the neutral token swatches
// =============================================================================

pub fn colors() -> Element {
    // (display name, canonical token, fallback) — the 11 neutral tokens.
    let tokens: [(&str, &str, &str); 11] = [
        ("Background", "color-background", "#f6f7f9"),
        ("Surface", "color-surface", "#ffffff"),
        ("Surface alt", "color-surface-alt", "#f1f5f9"),
        ("Text", "color-text", "#0f172a"),
        ("Text muted", "color-text-muted", "#64748b"),
        ("Text inverse", "color-text-inverse", "#ffffff"),
        ("Border", "color-border", "#e3e8ef"),
        ("Border hover", "color-border-hover", "#cdd5e0"),
        ("Border strong", "color-border-strong", "#94a3b8"),
        ("Focus ring", "color-focus-ring", "#6366f1"),
        ("Overlay", "color-overlay", "rgba(15,23,42,0.45)"),
    ];
    let cards: Vec<Element> = tokens.iter().map(|&(name, token, fallback)| swatch(name, token, fallback)).collect();

    body(vec![
        ui! {
            Section(title = "Neutral tokens".to_string()) {
                P(content = "The non-intent canvas every component paints on. Stylesheets \
                    reference these by name; the active theme binds a value to each at install \
                    time, so a Light/Dark swap rebinds the values without regenerating a class.".to_string())
                Grid(columns = 3u32, gap = StackGap::Md) { cards }
            }
        },
    ])
}

fn swatch(name: &str, token: &'static str, fallback: &'static str) -> Element {
    let fb = fallback.to_string();
    let block = view(vec![])
        .with_style(move || {
            StyleApplication::new(SwatchBlock::sheet())
                .override_background(Tokenized::token(token, Color(fb.clone().into())))
        })
        .into_element();
    let name = name.to_string();
    let token_s = token.to_string();
    ui! {
        Stack(gap = StackGap::Xs) {
            block
            Typography(content = name, kind = typography_kind::BodySm)
            Typography(content = token_s, kind = typography_kind::Caption, muted = true)
        }
    }
}

// =============================================================================
// Intents — the seven semantic palettes
// =============================================================================

pub fn intents() -> Element {
    let tones: Vec<(&'static str, ToneRef)> = vec![
        ("Primary", tone::Primary.into()),
        ("Secondary", tone::Secondary.into()),
        ("Neutral", tone::Neutral.into()),
        ("Success", tone::Success.into()),
        ("Danger", tone::Danger.into()),
        ("Warning", tone::Warning.into()),
        ("Info", tone::Info.into()),
    ];
    let rows: Vec<Element> = tones
        .into_iter()
        .map(|(name, t)| {
            let label = name.to_string();
            let on_click: Rc<dyn Fn()> = Rc::new(|| {});
            ui! {
                Stack(axis = StackAxis::Row, gap = StackGap::Md) {
                    Button(label = label.clone(), on_click = on_click, tone = t.clone(), variant = variant::Filled)
                    Button(label = "Soft".to_string(), on_click = Rc::new(|| {}) as Rc<dyn Fn()>, tone = t.clone(), variant = variant::Soft)
                    Badge(label = label, tone = t, variant = variant::Soft)
                }
            }
        })
        .collect();

    body(vec![
        ui! {
            Section(title = "Semantic palettes".to_string()) {
                P(content = "An intent isn't a color — it's a meaning. \"Danger\" reads as the \
                    right red on Button (Filled), the right tint on Badge (Soft), and the right \
                    border on Alert (Outlined). You write the meaning once; the theme + variant \
                    axes produce the visual. Each tone exposes six slots: solid-bg, solid-text, \
                    soft-bg, soft-text, fg, border.".to_string())
                DemoSurface {
                    Stack(gap = StackGap::Sm) { rows }
                }
            }
        },
        ui! {
            Callout(label = "Custom intents".to_string()) {
                P(content = "The Tone trait is open — implement it on a marker type and your \
                    custom intent works in every component that takes a tone.".to_string())
            }
        },
    ])
}

// =============================================================================
// Spacing & Radius
// =============================================================================

pub fn scale() -> Element {
    body(vec![
        ui! {
            Section(title = "Spacing scale".to_string()) {
                P(content = "Six steps, shared by every gap and pad. Each row shows the real \
                    StackGap between two blocks.".to_string())
                DemoSurface {
                    Stack(gap = StackGap::Md) {
                        spacing_row("xs · 4px", StackGap::Xs)
                        spacing_row("sm · 8px", StackGap::Sm)
                        spacing_row("md · 12px", StackGap::Md)
                        spacing_row("lg · 16px", StackGap::Lg)
                        spacing_row("xl · 24px", StackGap::Xl)
                    }
                }
            }
        },
        ui! {
            Section(title = "Radius scale".to_string()) {
                P(content = "Four corner radii, from a 4px nick to a full pill.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, gap = StackGap::Lg) {
                        radius_box("sm · 4px", RadiusBoxR::Sm)
                        radius_box("md · 8px", RadiusBoxR::Md)
                        radius_box("lg · 12px", RadiusBoxR::Lg)
                        radius_box("pill", RadiusBoxR::Pill)
                    }
                }
            }
        },
    ])
}

fn spacing_row(label: &'static str, gap: StackGap) -> Element {
    let a = view(vec![]).with_style(GapBlock()).into_element();
    let b = view(vec![]).with_style(GapBlock()).into_element();
    ui! {
        Stack(axis = StackAxis::Row, gap = StackGap::Lg) {
            Typography(content = label.to_string(), kind = typography_kind::Caption, muted = true)
            Stack(axis = StackAxis::Row, gap = gap) { a b }
        }
    }
}

fn radius_box(label: &'static str, r: RadiusBoxR) -> Element {
    let boxed = view(vec![]).with_style(RadiusBox().r(r)).into_element();
    ui! {
        Stack(gap = StackGap::Xs) {
            boxed
            Typography(content = label.to_string(), kind = typography_kind::Caption, muted = true)
        }
    }
}
