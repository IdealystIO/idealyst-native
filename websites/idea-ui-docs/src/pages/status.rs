//! Status — Spinner, Skeleton, Progress, Badge, Tag, Chip.
//!
//! Each `pub fn` returns the page **body only** — a column of demo
//! `Section`s. The central frame (`shell::page_frame_content`, rendered
//! inside the page's lazy chunk) adds the group overline, title, status
//! badge, lead, and the `Usage` panel, so bodies never add their own
//! title/lead/scroll wrapper.

use std::rc::Rc;

use runtime_core::{signal, ui, Element};
use idea_ui::{
    Badge, Button, Chip, Progress, ProgressMode, Skeleton, SkeletonWidth, Spinner,
    SpinnerSize, Stack, StackAxis, StackGap, Tag, tone, variant,
};

use crate::pages::body;
use crate::shell::{CodePanel, DemoSurface, Prop, PropsTable, Section, P};

// =============================================================================
// Spinner
// =============================================================================

pub fn spinner() -> Element {
    body(vec![
        ui! {
            Section(title = "Sizes".to_string()) {
                P(content = "Two scales — Small (the default) for inline use and Large for \
                    prominent, full-region loading states.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Lg) {
                        Spinner(size = SpinnerSize::Small)
                        Spinner(size = SpinnerSize::Large)
                    }
                }
            }
        },
        ui! {
            Section(title = "Tones".to_string()) {
                P(content = "Spinner has no tone axis today — its color is platform-native. \
                    The framework's ActivityIndicator primitive draws the system spinner, so \
                    it matches the host look automatically. When the primitive grows a tint \
                    hook, a Tone axis lands here.".to_string())
            }
        },
        ui! {
            Section(title = "With label".to_string()) {
                P(content = "Pair a spinner with a short status line by laying them out in a \
                    row. The spinner signals that work is in progress; the label says what.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
                        Spinner(size = SpinnerSize::Small)
                        P(content = "Loading…".to_string())
                    }
                }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "size", ty: "SpinnerSize", desc: "Small or Large. No tone — the framework primitive's color is platform-native today. Default: Small." },
                ])
            }
        },
    ])
}

// =============================================================================
// Skeleton
// =============================================================================

pub fn skeleton() -> Element {
    body(vec![
        ui! {
            Section(title = "Text".to_string()) {
                P(content = "Stack a few skeleton lines to suggest a heading and body copy. \
                    Vary the width so the placeholder reads like real text, not a solid \
                    block.".to_string())
                DemoSurface {
                    // A definite-width frame so the percentage widths (Full /
                    // ThreeQuarter / Half) resolve — DemoSurface itself centers
                    // + shrink-wraps, which would collapse %-width children.
                    view(style = crate::styles::PercentWidthFrame()) {
                        Stack(gap = StackGap::Sm) {
                            Skeleton(height = 24.0, radius = 6.0, width = SkeletonWidth::Full)
                            Skeleton(height = 16.0, radius = 4.0, width = SkeletonWidth::Full)
                            Skeleton(height = 16.0, radius = 4.0, width = SkeletonWidth::ThreeQuarter)
                            Skeleton(height = 16.0, radius = 4.0, width = SkeletonWidth::Half)
                        }
                    }
                }
            }
        },
        ui! {
            Section(title = "Media card".to_string()) {
                P(content = "An avatar circle next to two text lines — the common \
                    list-item placeholder. A circular skeleton is just a square block with a \
                    radius of half its size.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Md) {
                        Skeleton(width = SkeletonWidth::Px(48.0), height = 48.0, radius = 24.0)
                        // Explicit px widths: the line column has no definite
                        // width here (the row shrink-wraps in the centered
                        // DemoSurface), so % presets would collapse to zero.
                        Stack(gap = StackGap::Xs) {
                            Skeleton(height = 16.0, width = SkeletonWidth::Px(220.0))
                            Skeleton(height = 14.0, width = SkeletonWidth::Px(150.0))
                        }
                    }
                }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "width",  ty: "SkeletonWidth", desc: "Full / Half / ThreeQuarter / Px(f32). Default: Full." },
                    Prop { name: "height", ty: "f32",           desc: "Height in pixels. Default: 16." },
                    Prop { name: "radius", ty: "f32",           desc: "Border radius in pixels. 0 for sharp, higher for pill/circle. Default: 4." },
                ])
            }
        },
        ui! {
            Section(title = "Recipe — list item placeholders".to_string()) {
                CodePanel(src = r##"if loading.get() {
    Stack(gap = StackGap::Md) {
        for _ in 0..6 {
            Stack(axis = StackAxis::Row, gap = StackGap::Md) {
                Skeleton(width = SkeletonWidth::Px(48.0), height = 48.0, radius = 24.0)
                Stack(gap = StackGap::Xs) {
                    Skeleton(height = 16.0, width = SkeletonWidth::ThreeQuarter)
                    Skeleton(height = 14.0, width = SkeletonWidth::Half)
                }
            }
        }
    }
}"##.to_string())
            }
        },
    ])
}

// =============================================================================
// Progress
// =============================================================================

pub fn progress() -> Element {
    // Live value for the interactive Value-mode demo — each button snaps
    // the signal and the fill ANIMATES to the new width (the fill sheet's
    // width transition), which is the point of the demo.
    let pct = signal(0.35f32);
    let set10: Rc<dyn Fn()> = Rc::new(move || pct.set(0.10));
    let set45: Rc<dyn Fn()> = Rc::new(move || pct.set(0.45));
    let set80: Rc<dyn Fn()> = Rc::new(move || pct.set(0.80));
    let set100: Rc<dyn Fn()> = Rc::new(move || pct.set(1.0));
    body(vec![
        ui! {
            Section(title = "Value".to_string()) {
                P(content = "The default mode: the fill follows `value` (0.0..=1.0) \
                    reactively — pass a literal or a live Signal<f32>. Every change \
                    animates to the new width instead of snapping.".to_string())
                DemoSurface {
                    // Definite width so the full-width track resolves — DemoSurface
                    // centers + shrink-wraps, which would collapse the bar to zero.
                    view(style = crate::styles::PercentWidthFrame()) {
                        Stack(gap = StackGap::Md) {
                            Progress(value = pct, tone = tone::Primary)
                            Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
                                Button(label = "10%".to_string(),  variant = variant::Soft, on_click = set10)
                                Button(label = "45%".to_string(),  variant = variant::Soft, on_click = set45)
                                Button(label = "80%".to_string(),  variant = variant::Soft, on_click = set80)
                                Button(label = "100%".to_string(), variant = variant::Soft, on_click = set100)
                            }
                            Progress(value = 0.25f32, tone = tone::Success)
                            Progress(value = 0.85f32, tone = tone::Warning, cap = idea_ui::ProgressCap::Rounded)
                        }
                    }
                }
            }
        },
        ui! {
            Section(title = "Indeterminate".to_string()) {
                P(content = "For work of unknown duration: a fixed segment sweeps the track \
                    left to right, forever. The sweep is transform-driven over the uniform \
                    layout-measurement seam, so it behaves identically on every backend.".to_string())
                DemoSurface {
                    view(style = crate::styles::PercentWidthFrame()) {
                        Stack(gap = StackGap::Md) {
                            Progress(mode = ProgressMode::Indeterminate, tone = tone::Info)
                        }
                    }
                }
            }
        },
        ui! {
            Section(title = "Simulated".to_string()) {
                P(content = "A fake loader: starts empty and creeps toward (never reaching) \
                    full in irregular, shrinking steps — as if data were loading. Use it \
                    when there's no real progress to report but a sweeping bar feels too \
                    idle. When the work actually finishes, switch the same bar to \
                    ProgressMode::Value with value = 1.0 and it animates home.".to_string())
                DemoSurface {
                    view(style = crate::styles::PercentWidthFrame()) {
                        Stack(gap = StackGap::Md) {
                            Progress(mode = ProgressMode::Simulated, tone = tone::Primary)
                        }
                    }
                }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "value",   ty: "Reactive<f32>",          desc: "Completion in 0.0..=1.0, read in Value mode (ignored by the others). Static or a live Signal<f32>; changes animate." },
                    Prop { name: "mode",    ty: "Reactive<ProgressMode>", desc: "Value (default, value-driven), Indeterminate (endless sweep), or Simulated (fake loading creep)." },
                    Prop { name: "tone",    ty: "ToneRef",                desc: "Semantic palette for the fill. Default: Primary." },
                    Prop { name: "variant", ty: "VariantRef",             desc: "Surface skeleton for the fill. Default: Filled." },
                    Prop { name: "size",    ty: "ControlSize",            desc: "Sm / Md / Lg bar thickness. Default: Md." },
                    Prop { name: "cap",     ty: "ProgressCap",            desc: "End caps: None (square, default) or Rounded (pill track + fill)." },
                ])
                CodePanel(src = r##"// Live value-driven bar — changes animate to the new width
let pct = signal(0.0f32);
ui! { Progress(value = pct, tone = tone::Primary) }

// Indeterminate (unknown duration): endless sweep
ui! { Progress(mode = ProgressMode::Indeterminate, tone = tone::Info) }

// Simulated loading: creeps toward full, never arrives
ui! { Progress(mode = ProgressMode::Simulated) }"##.to_string())
            }
        },
    ])
}

// =============================================================================
// Badge
// =============================================================================

pub fn badge() -> Element {
    body(vec![
        ui! {
            Section(title = "Soft (default)".to_string()) {
                P(content = "A muted tint with a tone-colored label — the everyday status \
                    pill. Every tone is available.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
                        Badge(label = "Neutral".to_string(),   tone = tone::Neutral,   variant = variant::Soft)
                        Badge(label = "Primary".to_string(),   tone = tone::Primary,   variant = variant::Soft)
                        Badge(label = "Secondary".to_string(), tone = tone::Secondary, variant = variant::Soft)
                        Badge(label = "Success".to_string(),   tone = tone::Success,   variant = variant::Soft)
                        Badge(label = "Danger".to_string(),    tone = tone::Danger,    variant = variant::Soft)
                        Badge(label = "Warning".to_string(),   tone = tone::Warning,   variant = variant::Soft)
                        Badge(label = "Info".to_string(),      tone = tone::Info,      variant = variant::Soft)
                    }
                }
            }
        },
        ui! {
            Section(title = "Solid".to_string()) {
                P(content = "The Filled variant paints a solid tone fill with a contrasting \
                    label — use it for counts and high-emphasis statuses.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
                        Badge(label = "Neutral".to_string(),   tone = tone::Neutral,   variant = variant::Filled)
                        Badge(label = "Primary".to_string(),   tone = tone::Primary,   variant = variant::Filled)
                        Badge(label = "Secondary".to_string(), tone = tone::Secondary, variant = variant::Filled)
                        Badge(label = "Success".to_string(),   tone = tone::Success,   variant = variant::Filled)
                        Badge(label = "Danger".to_string(),    tone = tone::Danger,    variant = variant::Filled)
                        Badge(label = "Warning".to_string(),   tone = tone::Warning,   variant = variant::Filled)
                        Badge(label = "Info".to_string(),      tone = tone::Info,      variant = variant::Filled)
                    }
                }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label",   ty: "Reactive<String>", desc: "Pill text. Static or reactive." },
                    Prop { name: "tone",    ty: "ToneRef",          desc: "Semantic palette. Default: Neutral." },
                    Prop { name: "variant", ty: "VariantRef",       desc: "Filled / Soft / Outlined. No Ghost (a transparent badge would be invisible). Default: Soft." },
                ])
                CodePanel(src = r##"// Status indicator in a header
Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
    Typography(content = "User settings".into(), kind = typography_kind::H2)
    Badge(label = "Beta".into(), tone = tone::Info, variant = variant::Soft)
}

// Count badge next to a label
Stack(axis = StackAxis::Row, gap = StackGap::Xs) {
    Typography(content = "Inbox".into())
    Badge(label = "3".into(), tone = tone::Primary, variant = variant::Filled)
}"##.to_string())
            }
        },
    ])
}

// =============================================================================
// Tag
// =============================================================================

pub fn tag() -> Element {
    // A removable tag with a live close callback for the "Removable" demo.
    let removed = signal(false);
    let on_remove: Rc<dyn Fn()> = Rc::new(move || removed.set(true));

    body(vec![
        ui! {
            Section(title = "Removable".to_string()) {
                P(content = "Pass `on_remove` and a close (×) affordance appears to the right \
                    of the label. The host owns the removal — the callback flips its own \
                    state (here a signal).".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
                        Tag(label = "Rust".to_string(), tone = tone::Primary, variant = variant::Soft, on_remove = Some(on_remove.clone()))
                        Tag(label = "Go".to_string(),   tone = tone::Neutral, variant = variant::Soft, on_remove = Some(on_remove.clone()))
                    }
                }
            }
        },
        ui! {
            Section(title = "With icon".to_string()) {
                P(content = "Tag's label is plain text today — there's no icon slot. To pair a \
                    glyph with the label, put the glyph in the label string, or lay an Icon \
                    primitive next to the Tag inside a row.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
                        Tag(label = "★ Featured".to_string(), tone = tone::Warning, variant = variant::Soft)
                        Tag(label = "✓ Verified".to_string(), tone = tone::Success, variant = variant::Soft)
                    }
                }
            }
        },
        ui! {
            Section(title = "Intents".to_string()) {
                P(content = "Tag carries the same tone × variant axes as Badge. Pick the tone \
                    that matches the label's meaning.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
                        Tag(label = "Primary".to_string(),   tone = tone::Primary,   variant = variant::Soft)
                        Tag(label = "Success".to_string(),   tone = tone::Success,   variant = variant::Soft)
                        Tag(label = "Danger".to_string(),    tone = tone::Danger,    variant = variant::Soft)
                        Tag(label = "Outlined".to_string(),  tone = tone::Neutral,   variant = variant::Outlined)
                    }
                }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label",     ty: "Reactive<String>",     desc: "Tag text." },
                    Prop { name: "tone",      ty: "ToneRef",              desc: "Semantic palette. Default: Neutral." },
                    Prop { name: "variant",   ty: "VariantRef",           desc: "Filled / Soft / Outlined. Default: Soft." },
                    Prop { name: "on_remove", ty: "Option<Rc<dyn Fn()>>", desc: "When Some, a close (×) button renders to the right of the label." },
                ])
                CodePanel(src = r##"let langs = signal(vec!["Rust".to_string(), "Go".to_string()]);

ui! {
    Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
        for lang in langs.get() {
            Tag(
                label = lang.clone(),
                tone = tone::Primary,
                variant = variant::Soft,
                on_remove = Some(Rc::new(move || {
                    langs.update(|v| v.retain(|s| *s != lang));
                })),
            )
        }
    }
}"##.to_string())
            }
        },
    ])
}

// =============================================================================
// Chip
// =============================================================================

pub fn chip() -> Element {
    // Filter row: each chip is controlled — the host owns the selected flag
    // and flips it in `on_select`. Independent signals = multi-select.
    let rust = signal(true);
    let go = signal(false);
    let swift = signal(false);

    let toggle_rust: Rc<dyn Fn()> = Rc::new(move || rust.set(!rust.get()));
    let toggle_go: Rc<dyn Fn()> = Rc::new(move || go.set(!go.get()));
    let toggle_swift: Rc<dyn Fn()> = Rc::new(move || swift.set(!swift.get()));

    body(vec![
        ui! {
            Section(title = "Filter (selectable)".to_string()) {
                P(content = "A Chip is the selectable member of the pill family. Tapping it \
                    reports a select; its `selected` flag drives a lit (chosen variant) vs \
                    muted (Ghost) appearance. Chips are controlled — the host keeps the \
                    state and flips it in `on_select`. Independent signals make this a \
                    multi-select row.".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
                        Chip(label = "Rust".to_string(),  selected = rust.get(),  on_select = Some(toggle_rust.clone()),  tone = tone::Primary)
                        Chip(label = "Go".to_string(),    selected = go.get(),    on_select = Some(toggle_go.clone()),    tone = tone::Primary)
                        Chip(label = "Swift".to_string(), selected = swift.get(), on_select = Some(toggle_swift.clone()), tone = tone::Primary)
                    }
                }
            }
        },
        ui! {
            Section(title = "Intents".to_string()) {
                P(content = "Selected chips paint with the tone you choose; unselected chips \
                    drop to the quieter Ghost variant of the same tone, so a row reads as \
                    \"one lit, the rest muted\".".to_string())
                DemoSurface {
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
                        Chip(label = "Primary".to_string(), selected = true,  tone = tone::Primary)
                        Chip(label = "Success".to_string(), selected = true,  tone = tone::Success)
                        Chip(label = "Danger".to_string(),  selected = true,  tone = tone::Danger)
                        Chip(label = "Muted".to_string(),   selected = false, tone = tone::Neutral)
                    }
                }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label",     ty: "Reactive<String>",     desc: "Chip text." },
                    Prop { name: "selected",  ty: "bool",                 desc: "Controlled selected state — the host owns it. Drives the lit/muted appearance." },
                    Prop { name: "on_select", ty: "Option<Rc<dyn Fn()>>", desc: "Fires when tapped. The host flips its selected source here. When unset, the chip is inert." },
                    Prop { name: "tone",      ty: "ToneRef",              desc: "Semantic palette. Default: Neutral." },
                    Prop { name: "variant",   ty: "VariantRef",           desc: "Surface treatment for the selected state. Unselected always uses Ghost. Default: Soft." },
                    Prop { name: "size",      ty: "ControlSize",          desc: "Sm / Md / Lg. Default: Md." },
                ])
            }
        },
    ])
}
