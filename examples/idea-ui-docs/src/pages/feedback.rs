//! Feedback — Alert, Spinner, Skeleton, Avatar (one page each).

use runtime_core::{ui, Element};
use idea_ui::doc_controls::DocControls;
use idea_ui::{
    Alert, AlertProps, Avatar, AvatarProps, Skeleton, SkeletonProps, Spinner, SpinnerProps, Stack,
    StackGap,
};

use crate::shell::{
    self, Callout, CodePanel, ComponentPage, Demo, DemoSurface, H2, P, Prop, PropsTable, Section,
};

// =============================================================================
// Alert
// =============================================================================

pub fn alert() -> Element {
    let state = AlertProps::init_state();
    state.title.set("Heads up".to_string());

    let preview = AlertProps::reactive_preview(&state, |props| {
        let title = props.title;
        let body = props.body;
        let tone = props.tone;
        let variant = props.variant;
        ui! {
            Alert(title = title, body = body, tone = tone, variant = variant)
        }
    });
    let controls = AlertProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "Alert".to_string(),
            lead = "Inline status banner. Tone drives the surface color; body is an \
                optional second line of detail.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Tone × variant matrix".to_string()) {
                DemoSurface { tone_grid() }
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "title",      ty: "Reactive<String>",         desc: "Headline text." },
                    Prop { name: "body",       ty: "Reactive<Option<String>>", desc: "Optional second-line detail." },
                    Prop { name: "tone",       ty: "ToneRef",                  desc: "Semantic palette. Default: Info." },
                    Prop { name: "variant",    ty: "VariantRef",               desc: "Filled / Soft / Outlined. Default: Soft." },
                    Prop { name: "on_dismiss", ty: "Option<Rc<dyn Fn()>>",     desc: "When Some, a close affordance appears in the top-right." },
                ])
            }

            Callout(label = "Pairing with toasts".to_string()) {
                P(content = "Alert is in-flow — it pushes content. For a transient overlay use a \
                    Modal-or-Popover with an Alert inside; for stacking notifications, build a \
                    toast region above your shell.".to_string())
            }
        }
    })
}

fn tone_grid() -> Element {
    use idea_ui::{tone, variant};
    let rows: Vec<Element> = vec![
        ui! { Alert(title = "Info: cache refreshed".to_string(),    tone = tone::Info,    variant = variant::Soft) },
        ui! { Alert(title = "Success: saved 12 rows".to_string(),   tone = tone::Success, variant = variant::Soft) },
        ui! { Alert(title = "Warning: quota at 80%".to_string(),    tone = tone::Warning, variant = variant::Soft) },
        ui! { Alert(title = "Danger: payment failed".to_string(),   tone = tone::Danger,  variant = variant::Filled) },
    ];
    ui! { Stack(gap = StackGap::Sm) { rows } }
}

// =============================================================================
// Spinner
// =============================================================================

pub fn spinner() -> Element {
    let state = SpinnerProps::init_state();
    let preview = SpinnerProps::reactive_preview(&state, |props| {
        let size = props.size;
        ui! { Spinner(size = size) }
    });
    let controls = SpinnerProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "Spinner".to_string(),
            lead = "Indeterminate loading indicator. Wraps the framework's \
                ActivityIndicator with a size knob.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "size", ty: "SpinnerSize", desc: "Small or Large. No tone — the framework primitive's color is platform-native today." },
                ])
            }

            Section(title = "When to reach for it vs Skeleton".to_string()) {
                P(content = "Spinner says \"something is happening, I don't know how long\". \
                    Skeleton says \"content will arrive shaped roughly like this\". Prefer \
                    Skeleton when the layout is known — it avoids layout shift when the real \
                    content lands.".to_string())
            }
        }
    })
}

// =============================================================================
// Skeleton
// =============================================================================

pub fn skeleton() -> Element {
    let state = SkeletonProps::init_state();
    let preview = ui! {
        Stack(gap = StackGap::Sm) {
            Skeleton(height = 24.0, radius = 6.0)
            Skeleton(height = 16.0, radius = 4.0)
            Skeleton(height = 16.0, radius = 4.0)
        }
    };
    let controls = SkeletonProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "Skeleton".to_string(),
            lead = "Muted placeholder block. Communicates the shape of upcoming content \
                without rendering anything real.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "width",  ty: "SkeletonWidth", desc: "Full / Half / ThreeQuarter / Px(f32). Default: Full." },
                    Prop { name: "height", ty: "f32",           desc: "Height in pixels. Default: 16." },
                    Prop { name: "radius", ty: "f32",           desc: "Border radius in pixels. 0 for sharp, higher for pill/circle. Default: 4." },
                ])
            }

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
        }
    })
}

// =============================================================================
// Avatar
// =============================================================================

pub fn avatar() -> Element {
    let state = AvatarProps::init_state();
    state.initials.set("AB".to_string());

    let preview = AvatarProps::reactive_preview(&state, |props| {
        let initials = props.initials;
        let color = props.color;
        let size = props.size;
        ui! {
            Avatar(initials = initials, color = color, size = size)
        }
    });
    let controls = AvatarProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "Avatar".to_string(),
            lead = "Circular identity element. Renders an image when `src` is set, \
                otherwise falls back to initials on a `color`-tinted background.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "src",      ty: "Option<String>",   desc: "Optional image URL. When Some, an Image primitive renders and initials are hidden." },
                    Prop { name: "initials", ty: "Reactive<String>", desc: "Fallback text rendered when src is None." },
                    Prop { name: "color",    ty: "AvatarColor",      desc: "Placeholder tint. Distinct from Tone because an avatar doesn't represent a semantic action." },
                    Prop { name: "size",     ty: "AvatarSize",       desc: "Sm / Md / Lg / Xl." },
                ])
            }

            Callout(label = "Why a separate `AvatarColor` axis".to_string()) {
                P(content = "Tone carries semantic meaning (Primary action, Danger banner, …). \
                    An avatar isn't an action — it's a person/entity stand-in, and assigning it \
                    \"Primary\" or \"Danger\" would mis-signal. AvatarColor is a stable set of \
                    decorative tints intended for hash-to-color avatar grids.".to_string())
            }
        }
    })
}
