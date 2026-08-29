//! Foundations — Color, Intents, Spacing & Radius, Theme editor.
//!
//! Body-only pages: the central frame renders the title, lead, overline,
//! status badge, and Usage panel.

use std::rc::Rc;

use runtime_core::{signal, ui, view, Color, Element, IntoElement, StyleApplication, Tokenized};
use idea_ui::{
    tone, typography_kind, variant, Badge, Button, Grid, Stack, StackAxis, StackGap, ToneRef,
    Typography,
};
use idea_theme_editor::{ThemeDraft, ThemeEditor};

use crate::pages::body;
use crate::shell::{Callout, CodePanel, DemoSurface, P, Section};
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

    // Component-scoped tokens: not part of the neutral canvas, but
    // still theme-bound. One entry per component that owns a surface
    // the neutrals can't express without collateral damage.
    let component_tokens: [(&str, &str, &str); 1] =
        [("Table header", "color-table-header", "#f1f5f9")];
    let component_cards: Vec<Element> = component_tokens
        .iter()
        .map(|&(name, token, fallback)| swatch(name, token, fallback))
        .collect();

    body(vec![
        ui! {
            Section(title = "Neutral tokens".to_string()) {
                P(content = "The non-intent canvas every component paints on. Stylesheets \
                    reference these by name; the active theme binds a value to each at install \
                    time, so a Light/Dark swap rebinds the values without regenerating a class.".to_string())
                Grid(columns = 3u32, gap = StackGap::Md) { cards }
            }
        },
        ui! {
            Section(title = "Component tokens".to_string()) {
                P(content = "A few surfaces get a token of their own because sharing a neutral \
                    would make them un-retintable. Table headers ship with the surface-alt value, \
                    so the default look is identical — but overriding color-table-header repaints \
                    the header band alone, leaving cards, field wells and row hover untouched.".to_string())
                Grid(columns = 3u32, gap = StackGap::Md) { component_cards }
            }
        },
    ])
}

fn swatch(name: &str, token: &'static str, fallback: &'static str) -> Element {
    let fb = fallback.to_string();
    // The per-token fill rides the INLINE layer, not an override: an
    // override disqualified every swatch from preminting (11 live-engine
    // fall-throughs on this page under `--premint-report`). Inline still
    // carries the token reference, so it re-tints on theme swap.
    let block = view(vec![])
        .with_style(move || {
            StyleApplication::new(SwatchBlock::sheet()).with_inline(
                runtime_core::StyleRules {
                    background: Some(Tokenized::token(token, Color(fb.clone().into()))),
                    ..Default::default()
                },
            )
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
                Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Md) {
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
                P(content = "An intent names a meaning rather than a color. \"Danger\" reads as the \
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
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Lg) {
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
        Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Lg) {
            Typography(content = label.to_string(), kind = typography_kind::Caption, muted = true)
            Stack(axis = StackAxis::Row, wrap = true, gap = gap) { a b }
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


// =============================================================================
// Theme editor — the live token panel, editing THIS site
// =============================================================================

/// The `idea-theme-editor` panel, pointed at the docs site's own theme.
///
/// Deliberately not a sandboxed preview: the panel edits the tokens this
/// page is painted with, so a change to `color-surface` re-tints the
/// sidebar, the header, and the panel itself at the same time. That IS
/// the demonstration — every component reads its colors through named
/// tokens, so one write reaches all of them.
pub fn theme_editor() -> Element {
    // One draft for the page. Created here (a component scope) rather
    // than at module level: `from_live` makes a signal per token, and
    // it has to read the theme this app installed at boot.
    let draft = ThemeDraft::from_live();

    // What the output panel shows. Filled by the buttons rather than
    // recomputed on every keystroke — `to_rust()` reads all 74 token
    // signals, and re-highlighting a code block on each character typed
    // would make the panel above it feel sticky.
    let output = signal(String::new());

    let export_rust: Rc<dyn Fn()> = {
        let draft = draft.clone();
        Rc::new(move || {
            output.set(draft.to_rust().unwrap_or_else(|| {
                "// Nothing has changed yet — edit a token above, then press this again.".to_string()
            }));
        })
    };
    let export_json: Rc<dyn Fn()> = {
        let draft = draft.clone();
        Rc::new(move || output.set(draft.to_json()))
    };
    let revert: Rc<dyn Fn()> = {
        let draft = draft.clone();
        Rc::new(move || {
            draft.revert();
            output.set(String::new());
        })
    };

    body(vec![
        ui! {
            Section(title = "Live editing".to_string()) {
                P(content = "Every control below is one token of the theme this page is \
                    painted with. Editing one applies it immediately, so the whole site — \
                    sidebar, header, and the panel itself — re-tints as you type. Nothing is \
                    sandboxed: this is the same install the app booted with.".to_string())
                P(content = "A value that does not parse marks its own row and is not \
                    applied, so half-typed text never reaches the app. Revert puts every \
                    control back to the value the page opened with; a browser reload brings \
                    back the stock palette.".to_string())
                Callout(label = "What the editor cannot reach".to_string()) {
                    P(content = "Only tokenized values move. Stylesheets also carry \
                        hand-written literals — a 1px border width, a fixed opacity — and \
                        those are not tokens, so no editor can reach them. Editing a theme \
                        is not editing every rule.".to_string())
                }
                DemoSurface {
                    ThemeEditor(draft = draft.clone())
                }
            }
        },
        ui! {
            Section(title = "Taking it with you".to_string()) {
                P(content = "The panel renders controls and nothing else — file dialogs and \
                    clipboards belong to the app around it. Everything else is a plain method \
                    on the draft, which is what these buttons call.".to_string())
                Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
                    Button(label = "Export as Rust".to_string(), on_click = export_rust)
                    Button(
                        label = "Save as JSON".to_string(),
                        on_click = export_json,
                        variant = variant::Soft,
                    )
                    Button(
                        label = "Revert".to_string(),
                        on_click = revert,
                        variant = variant::Soft,
                        tone = tone::Danger,
                    )
                }
                view {
                    if output.get().is_empty() {
                        P(content = "Press a button to see the output.".to_string())
                    } else {
                        CodePanel(src = output.get())
                    }
                }
                P(content = "Rust export is the EDITS only, assigned onto a theme binding you \
                    already have — this crate cannot know which palette your app installed. \
                    Tokens with no theme field behind them (radius-pill is Length::Full by \
                    design, and extension tokens have no accessor at all) come out as an \
                    update_tokens call instead, so an edit is never silently dropped."
                    .to_string())
                P(content = "JSON is the save format: every token as name and text, in the \
                    order the panel lays them out. Loading one is all-or-nothing — a file \
                    with a single bad value applies nothing rather than leaving the app half \
                    themed.".to_string())
            }
        },
    ])
}
