//! Primitives — Typography, Icon, Image, Divider, Spacer, Surface.
//!
//! Body-only pages: the central frame renders the title, lead, overline,
//! status badge, and Usage panel.

use std::rc::Rc;

use runtime_core::{ui, Element};
use icons_lucide::{
    BELL, CHECK, COPY, DOWNLOAD, FILE, FOLDER, HEART, MAIL, PENCIL, PLUS, SEARCH, SETTINGS, STAR,
    TRASH_2, USER, X,
};
use idea_ui::{
    tone, typography_kind, variant, Button, Divider, DividerAxis, Grid, Icon, Image, Spacer,
    Stack, StackAlign, StackAxis, StackGap, StackPadding, Surface, SurfaceColor, ToneRef,
    Typography, TypographyKindRef,
};

use crate::pages::body;
use crate::shell::{CodePanel, DemoSurface, Prop, PropsTable, Section, P};

// =============================================================================
// Typography
// =============================================================================

pub fn typography() -> Element {
    body(vec![
        ui! {
            Section(title = "Type roles".to_string()) {
                P(content = "Every textual surface is a Typography. Pick a `kind` for the \
                    size + weight + spacing — the size scale is theme-tokenized \
                    (typography-{kind}-size).".to_string())
                DemoSurface { kind_gallery() }
            }
        },
        ui! {
            Section(title = "Tones".to_string()) {
                P(content = "Each tone reads a different theme color token. `muted = true` is \
                    shorthand for the muted text token.".to_string())
                DemoSurface { tone_gallery() }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "content", ty: "Reactive<String>",   desc: "Text. Literal, String, Signal<String>, or rx!(...)." },
                    Prop { name: "kind",    ty: "TypographyKindRef",  desc: "Display / H1..H3 / BodyXl..BodySm / Caption / Overline. Default: Body." },
                    Prop { name: "tone",    ty: "Option<ToneRef>",    desc: "Optional semantic color. Overrides the muted flag when Some." },
                    Prop { name: "muted",   ty: "bool",               desc: "Use the theme's muted text color when tone is None." },
                ])
            }
        },
    ])
}

fn kind_gallery() -> Element {
    let rows: Vec<Element> = vec![
        kind_row("Display", "The spark of a good idea", typography_kind::Display.into()),
        kind_row("H1", "The spark of a good idea", typography_kind::H1.into()),
        kind_row("H2", "The spark of a good idea", typography_kind::H2.into()),
        kind_row("H3", "The spark of a good idea", typography_kind::H3.into()),
        kind_row("BodyXl", "The quick brown fox jumps over the lazy dog.", typography_kind::BodyXl.into()),
        kind_row("BodyLg", "The quick brown fox jumps over the lazy dog.", typography_kind::BodyLg.into()),
        kind_row("Body", "The quick brown fox jumps over the lazy dog.", typography_kind::Body.into()),
        kind_row("BodySm", "The quick brown fox jumps over the lazy dog.", typography_kind::BodySm.into()),
        kind_row("Caption", "Helper text under a control", typography_kind::Caption.into()),
        kind_row("Overline", "Section label", typography_kind::Overline.into()),
    ];
    ui! { Stack(gap = StackGap::Md) { rows } }
}

fn kind_row(name: &str, sample: &str, kind: TypographyKindRef) -> Element {
    let label = ui! { Typography(content = name.to_string(), kind = typography_kind::Overline, muted = true) };
    let sample_line = ui! { Typography(content = sample.to_string(), kind = kind) };
    ui! { Stack(gap = StackGap::Xs) { label sample_line } }
}

fn tone_gallery() -> Element {
    let primary: ToneRef = tone::Primary.into();
    let danger: ToneRef = tone::Danger.into();
    let success: ToneRef = tone::Success.into();
    let warning: ToneRef = tone::Warning.into();
    let info: ToneRef = tone::Info.into();
    ui! {
        Stack(gap = StackGap::Sm) {
            Typography(content = "Default — readable on both surfaces.".to_string())
            Typography(content = "Muted — secondary text.".to_string(), muted = true)
            Typography(content = "Primary".to_string(), tone = Some(primary))
            Typography(content = "Success".to_string(), tone = Some(success))
            Typography(content = "Danger".to_string(), tone = Some(danger))
            Typography(content = "Warning".to_string(), tone = Some(warning))
            Typography(content = "Info".to_string(), tone = Some(info))
        }
    }
}

// =============================================================================
// Icon
// =============================================================================

pub fn icon() -> Element {
    let names: [(runtime_core::IconData, &str); 16] = [
        (SEARCH, "search"), (PLUS, "plus"), (CHECK, "check"), (X, "x"),
        (HEART, "heart"), (STAR, "star"), (SETTINGS, "settings"), (DOWNLOAD, "download"),
        (TRASH_2, "trash"), (COPY, "copy"), (BELL, "bell"), (MAIL, "mail"),
        (USER, "user"), (FOLDER, "folder"), (FILE, "file"), (PENCIL, "pencil"),
    ];
    let tiles: Vec<Element> = names
        .iter()
        .map(|&(data, label)| {
            ui! {
                Stack(gap = StackGap::Xs) {
                    Icon(data = data, size = 22.0)
                    Typography(content = label.to_string(), kind = typography_kind::Caption, muted = true)
                }
            }
        })
        .collect();

    body(vec![
        ui! {
            Section(title = "Library".to_string()) {
                P(content = "Line icons on a 24px grid. They inherit currentColor and take an \
                    explicit pixel size.".to_string())
                Grid(columns = 6u32, gap = StackGap::Md) { tiles }
            }
        },
        ui! {
            Section(title = "Sizes".to_string()) {
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Lg, align = StackAlign::Center) {
                        Icon(data = STAR, size = 14.0)
                        Icon(data = STAR, size = 18.0)
                        Icon(data = STAR, size = 24.0)
                        Icon(data = STAR, size = 32.0)
                        Icon(data = STAR, size = 40.0)
                    }
                }
            }
        },
        ui! {
            Section(title = "Tones".to_string()) {
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Lg, align = StackAlign::Center) {
                        Icon(data = HEART, size = 24.0, tone = Some(tone::Primary.into()))
                        Icon(data = HEART, size = 24.0, tone = Some(tone::Success.into()))
                        Icon(data = HEART, size = 24.0, tone = Some(tone::Danger.into()))
                        Icon(data = HEART, size = 24.0, tone = Some(tone::Warning.into()))
                        Icon(data = HEART, size = 24.0, tone = Some(tone::Info.into()))
                    }
                }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "data", ty: "IconData",         desc: "The glyph — typically an icons_lucide::* constant." },
                    Prop { name: "size", ty: "f32",              desc: "Square size in pixels. Default: 24." },
                    Prop { name: "tone", ty: "Option<ToneRef>",  desc: "Optional semantic tint (intent-{tone}-fg)." },
                    Prop { name: "color", ty: "Option<Color>",   desc: "Explicit color override, wins over tone." },
                ])
            }
        },
    ])
}

// =============================================================================
// Image
// =============================================================================

pub fn image() -> Element {
    body(vec![
        ui! {
            Section(title = "Rounded".to_string()) {
                P(content = "Responsive media with an explicit size and an optional rounded \
                    treatment. Falls back to a placeholder if the source can't load.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Lg, align = StackAlign::Center) {
                        Image(src = "https://picsum.photos/seed/idea1/160/120".to_string(), width = Some(160.0), height = Some(120.0))
                        Image(src = "https://picsum.photos/seed/idea2/120/120".to_string(), width = Some(120.0), height = Some(120.0), rounded = true)
                    }
                }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "src",     ty: "String",       desc: "Image URL or asset path." },
                    Prop { name: "alt",     ty: "Option<String>", desc: "Accessible description." },
                    Prop { name: "width",   ty: "Option<f32>",  desc: "Explicit width in pixels." },
                    Prop { name: "height",  ty: "Option<f32>",  desc: "Explicit height in pixels." },
                    Prop { name: "rounded", ty: "bool",         desc: "Apply a radius-md corner. Default: false." },
                ])
            }
        },
    ])
}

// =============================================================================
// Divider
// =============================================================================

pub fn divider() -> Element {
    body(vec![
        ui! {
            Section(title = "Horizontal".to_string()) {
                P(content = "A 1px hairline reading color-border. The default axis stacks \
                    content above and below.".to_string())
                DemoSurface {
                    Stack(gap = StackGap::Md) {
                        Typography(content = "Above the line".to_string())
                        Divider(axis = DividerAxis::Horizontal)
                        Typography(content = "Below the line".to_string())
                    }
                }
            }
        },
        ui! {
            Section(title = "Vertical".to_string()) {
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Md) {
                        Typography(content = "Edit".to_string())
                        Divider(axis = DividerAxis::Vertical)
                        Typography(content = "Duplicate".to_string())
                        Divider(axis = DividerAxis::Vertical)
                        Typography(content = "Delete".to_string())
                    }
                }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "axis", ty: "DividerAxis", desc: "Horizontal (1px-tall border-bottom) or Vertical (1px-wide border-left)." },
                ])
            }
        },
    ])
}

// =============================================================================
// Spacer
// =============================================================================

pub fn spacer() -> Element {
    let noop: Rc<dyn Fn()> = Rc::new(|| {});
    body(vec![
        ui! {
            Section(title = "Push siblings apart".to_string()) {
                P(content = "An empty flex item with flex-grow: 1. Drop one between siblings in \
                    a row to push them to opposite ends without computing margins.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
                        Typography(content = "Title".to_string(), kind = typography_kind::H3)
                        Spacer()
                        Button(label = "Save".to_string(), on_click = noop, tone = tone::Primary, variant = variant::Filled)
                    }
                }
                CodePanel(src = r##"Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
    Typography(content = "Title".into(), kind = typography_kind::H3)
    Spacer()
    Button(label = "Save".into(), on_click = save, tone = tone::Primary, variant = variant::Filled)
}"##.to_string())
            }
        },
    ])
}

// =============================================================================
// Surface
// =============================================================================

pub fn surface() -> Element {
    body(vec![
        ui! {
            Section(title = "Surface levels".to_string()) {
                P(content = "The base container. Its background is drawn from a neutral token — \
                    Background, Surface, or SurfaceAlt — so nested surfaces read as distinct \
                    layers on both light and dark themes.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Md) {
                        Surface(background = SurfaceColor::Surface, padding = StackPadding::Lg) {
                            Typography(content = "surface".to_string())
                        }
                        Surface(background = SurfaceColor::SurfaceAlt, padding = StackPadding::Lg) {
                            Typography(content = "surface-alt".to_string())
                        }
                    }
                }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "background", ty: "SurfaceColor",  desc: "Background / Surface / SurfaceAlt neutral token." },
                    Prop { name: "padding",    ty: "StackPadding",  desc: "Inner padding from the spacing scale." },
                    Prop { name: "grow",       ty: "f32",           desc: "flex-grow factor. Default: 0." },
                    Prop { name: "children",   ty: "Vec<Element>",  desc: "Surface content." },
                ])
            }
        },
    ])
}
