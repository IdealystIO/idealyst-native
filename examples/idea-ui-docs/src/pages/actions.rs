//! Actions — Button, IconButton, Link, Avatar.
//!
//! Each `pub fn` returns the page **body only** — a column of demo
//! `Section`s. The central page frame renders the title, lead, group
//! overline, status badge, and the `Usage` panel, so these bodies never
//! render their own title/lead/scroll wrapper.

use std::rc::Rc;

use runtime_core::{ui, Element};
use icons_lucide::{DOWNLOAD, HEART, PENCIL, PLUS, SEARCH, SETTINGS, STAR, TRASH_2, X};
use idea_ui::{
    Avatar, AvatarColor, AvatarSize, Button, IconButton, IconButtonSize, Link, Stack, StackAlign,
    StackAxis, StackGap, tone, variant, size,
};

use crate::pages::body;
use crate::shell::{Callout, CodePanel, DemoSurface, Prop, PropsTable, Section, H3, P};

// =============================================================================
// Button
// =============================================================================

pub fn button() -> Element {
    body(vec![
        ui! {
            Section(title = "Variants × tones".to_string()) {
                P(content = "Four surface treatments — Filled, Soft, Outlined, Ghost — each \
                    pairing with any of the seven tones. Here are the variants on the \
                    Primary tone:".to_string())
                DemoSurface { button_variant_row() }
                P(content = "And a row of all seven tones, Filled:".to_string())
                DemoSurface { button_tone_row() }
            }
        },
        ui! {
            Section(title = "Sizes".to_string()) {
                P(content = "Sm / Md / Lg drive padding and font scale together.".to_string())
                DemoSurface { button_size_row() }
            }
        },
        ui! {
            Section(title = "With icon · loading · disabled".to_string()) {
                P(content = "A leading or trailing icon sits inline with the label; \
                    `disabled` dims the surface and blocks the press.".to_string())
                DemoSurface { button_state_row() }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label",         ty: "Reactive<String>",      desc: "Button text. Static or reactive." },
                    Prop { name: "on_click",      ty: "Rc<dyn Fn()>",          desc: "Callback fired on press." },
                    Prop { name: "tone",          ty: "ToneRef",               desc: "Semantic palette. Default: Primary." },
                    Prop { name: "variant",       ty: "VariantRef",            desc: "Filled / Soft / Outlined / Ghost. Default: Filled." },
                    Prop { name: "size",          ty: "ButtonSizeRef",         desc: "Sm / Md / Lg. Default: Md." },
                    Prop { name: "shape",         ty: "ShapeRef",              desc: "Sharp / Sm / Md / Pill. Default: Md." },
                    Prop { name: "disabled",      ty: "bool",                  desc: "Blocks the press and dims the surface. Default: false." },
                    Prop { name: "loading",       ty: "bool",                  desc: "Swaps the leading slot for a spinner and blocks the press (reads as busy, not off). Default: false." },
                    Prop { name: "leading_icon",  ty: "Option<IconData>",      desc: "Vector icon rendered before the label. Inherits the label color." },
                    Prop { name: "trailing_icon", ty: "Option<IconData>",      desc: "Vector icon rendered after the label." },
                    Prop { name: "block",         ty: "bool",                  desc: "Stretch to fill the container width (full-bleed CTA). Default: false." },
                    Prop { name: "bind_to",       ty: "Option<Ref<PressableHandle>>", desc: "When Some, fills the given Ref on mount — anchor an overlay/popover to this button." },
                ])
            }
        },
        ui! {
            Section(title = "Recipes".to_string()) {
                H3(content = "Primary action".to_string())
                CodePanel(src = r##"Button(
    label = "Save".into(),
    on_click = save,
    tone = tone::Primary,
    variant = variant::Filled,
)"##.to_string())
                H3(content = "Destructive action".to_string())
                CodePanel(src = r##"Button(
    label = "Delete".into(),
    on_click = confirm_delete,
    tone = tone::Danger,
    variant = variant::Filled,
    leading_icon = Some(icons_lucide::TRASH_2),
)"##.to_string())
            }
        },
        ui! {
            Callout(label = "Static styles, no flicker".to_string()) {
                P(content = "Every (tone, variant, size, shape) tuple is pre-generated as a \
                    className lookup against the installed stylesheet — there's no per-button \
                    apply-style closure. Theme swaps update CSS variables in bulk, and 1000 \
                    buttons sharing a tuple share one class.".to_string())
            }
        },
    ])
}

fn button_variant_row() -> Element {
    let on_click: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Md) {
            Button(label = "Filled".to_string(),   on_click = on_click.clone(), tone = tone::Primary, variant = variant::Filled)
            Button(label = "Soft".to_string(),     on_click = on_click.clone(), tone = tone::Primary, variant = variant::Soft)
            Button(label = "Outlined".to_string(), on_click = on_click.clone(), tone = tone::Primary, variant = variant::Outlined)
            Button(label = "Ghost".to_string(),    on_click = on_click,         tone = tone::Primary, variant = variant::Ghost)
        }
    }
}

fn button_tone_row() -> Element {
    let on_click: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
            Button(label = "Primary".to_string(),   on_click = on_click.clone(), tone = tone::Primary)
            Button(label = "Secondary".to_string(), on_click = on_click.clone(), tone = tone::Secondary)
            Button(label = "Neutral".to_string(),   on_click = on_click.clone(), tone = tone::Neutral)
            Button(label = "Success".to_string(),   on_click = on_click.clone(), tone = tone::Success)
            Button(label = "Danger".to_string(),    on_click = on_click.clone(), tone = tone::Danger)
            Button(label = "Warning".to_string(),   on_click = on_click.clone(), tone = tone::Warning)
            Button(label = "Info".to_string(),      on_click = on_click,         tone = tone::Info)
        }
    }
}

fn button_size_row() -> Element {
    let on_click: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Md) {
            Button(label = "Small".to_string(),  on_click = on_click.clone(), size = size::Sm)
            Button(label = "Medium".to_string(), on_click = on_click.clone(), size = size::Md)
            Button(label = "Large".to_string(),  on_click = on_click,         size = size::Lg)
        }
    }
}

fn button_state_row() -> Element {
    let on_click: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Md) {
            Button(label = "Download".to_string(), on_click = on_click.clone(), leading_icon = Some(DOWNLOAD))
            Button(label = "Saving".to_string(),   on_click = on_click.clone(), loading = true)
            Button(label = "Disabled".to_string(), on_click = on_click,         disabled = true)
        }
    }
}

// =============================================================================
// IconButton
// =============================================================================

pub fn icon_button() -> Element {
    body(vec![
        ui! {
            Section(title = "Variants".to_string()) {
                P(content = "Square sibling of Button — same four surface treatments, sized \
                    to a glyph or vector icon instead of a label.".to_string())
                DemoSurface { icon_button_variant_row() }
            }
        },
        ui! {
            Section(title = "Sizes".to_string()) {
                P(content = "Sm / Md / Lg map to 24 / 32 / 48 px squares — a closed enum \
                    because the size controls the square footprint.".to_string())
                DemoSurface { icon_button_size_row() }
            }
        },
        ui! {
            Section(title = "Tones".to_string()) {
                P(content = "The same seven-tone palette as Button, here Filled.".to_string())
                DemoSurface { icon_button_tone_row() }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "glyph",    ty: "String",          desc: "Single-character glyph rendered centered. Used when `icon` is None." },
                    Prop { name: "icon",     ty: "Option<IconData>", desc: "Vector (Lucide) icon. Wins over `glyph` when Some." },
                    Prop { name: "on_click", ty: "Rc<dyn Fn()>",    desc: "Callback fired on press." },
                    Prop { name: "tone",     ty: "ToneRef",         desc: "Semantic palette. Default: Neutral." },
                    Prop { name: "variant",  ty: "VariantRef",      desc: "Filled / Soft / Outlined / Ghost. Default: Filled." },
                    Prop { name: "size",     ty: "IconButtonSize",  desc: "Sm / Md / Lg — closed enum (square footprint)." },
                    Prop { name: "selected", ty: "bool",            desc: "Paints the tone's accent fill (the active-toggle / selected-tool look). Default: false." },
                    Prop { name: "disabled", ty: "bool",            desc: "Blocks the press and dims the button. Default: false." },
                ])
            }
        },
    ])
}

fn icon_button_variant_row() -> Element {
    let on_click: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Md) {
            IconButton(icon = Some(HEART),    on_click = on_click.clone(), tone = tone::Primary, variant = variant::Filled)
            IconButton(icon = Some(STAR),     on_click = on_click.clone(), tone = tone::Primary, variant = variant::Soft)
            IconButton(icon = Some(PENCIL),   on_click = on_click.clone(), tone = tone::Primary, variant = variant::Outlined)
            IconButton(icon = Some(SETTINGS), on_click = on_click,         tone = tone::Primary, variant = variant::Ghost)
        }
    }
}

fn icon_button_size_row() -> Element {
    let on_click: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Md) {
            IconButton(icon = Some(SEARCH), on_click = on_click.clone(), size = IconButtonSize::Sm)
            IconButton(icon = Some(SEARCH), on_click = on_click.clone(), size = IconButtonSize::Md)
            IconButton(icon = Some(SEARCH), on_click = on_click,         size = IconButtonSize::Lg)
        }
    }
}

fn icon_button_tone_row() -> Element {
    let on_click: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Sm) {
            IconButton(icon = Some(PLUS),    on_click = on_click.clone(), tone = tone::Primary)
            IconButton(icon = Some(STAR),    on_click = on_click.clone(), tone = tone::Secondary)
            IconButton(icon = Some(PENCIL),  on_click = on_click.clone(), tone = tone::Neutral)
            IconButton(icon = Some(HEART),   on_click = on_click.clone(), tone = tone::Success)
            IconButton(icon = Some(TRASH_2), on_click = on_click.clone(), tone = tone::Danger)
            IconButton(icon = Some(X),       on_click = on_click,         tone = tone::Warning)
        }
    }
}

// =============================================================================
// Link
// =============================================================================

pub fn link() -> Element {
    body(vec![
        ui! {
            Section(title = "Standalone".to_string()) {
                P(content = "A styled external/inline navigational link. On web it renders a \
                    real `<a href target=\"_blank\" rel=\"noopener\">`; on native it hands the \
                    URL to the platform opener.".to_string())
                DemoSurface {
                    Stack(gap = StackGap::Sm) {
                        Link(label = "Read the docs".to_string(),    url = "https://example.com/docs".to_string())
                        Link(label = "lucide.dev/icons".to_string(), url = "https://lucide.dev/icons".to_string())
                        Link(label = "Email support".to_string(),    url = "mailto:support@example.com".to_string())
                    }
                }
            }
        },
        ui! {
            Section(title = "Inline".to_string()) {
                P(content = "Drop a Link inside a row of text to weave it into a sentence.".to_string())
                DemoSurface {
                    // Baseline-align so the Link sits on the prose's text
                    // baseline instead of being centered/top-aligned in the row.
                    Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Xs, align = StackAlign::Baseline) {
                        P(content = "By continuing you agree to the".to_string())
                        Link(label = "terms of service".to_string(), url = "https://example.com/terms".to_string())
                        P(content = ".".to_string())
                    }
                }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label", ty: "Reactive<String>", desc: "Link text. Static or reactive." },
                    Prop { name: "url",   ty: "String",           desc: "Destination URL (https:, mailto:, tel:, …)." },
                ])
            }
        },
        ui! {
            Callout(label = "In-app route navigation".to_string()) {
                P(content = "`Link` is for external/opener URLs. For in-app route navigation use \
                    the framework's lowercase `link(&route, params, children)` primitive directly \
                    — it needs a typed Route, which is app-specific.".to_string())
            }
        },
    ])
}

// =============================================================================
// Avatar
// =============================================================================

pub fn avatar() -> Element {
    body(vec![
        ui! {
            Section(title = "Sizes".to_string()) {
                P(content = "Xs / Sm / Md / Lg / Xl map to 24 / 32 / 40 / 56 / 80 px \
                    diameters.".to_string())
                DemoSurface { avatar_size_row() }
            }
        },
        ui! {
            Section(title = "Content".to_string()) {
                P(content = "Renders the `src` image when set, otherwise the `initials` on a \
                    `color`-tinted placeholder background.".to_string())
                DemoSurface { avatar_content_row() }
            }
        },
        ui! {
            Section(title = "Group".to_string()) {
                P(content = "Place avatars in a row to form a stacked group — e.g. \
                    collaborators on a document.".to_string())
                DemoSurface { avatar_group_row() }
            }
        },
        ui! {
            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "src",      ty: "Option<String>",   desc: "Optional image URL. When Some, an Image renders and initials are hidden." },
                    Prop { name: "initials", ty: "Reactive<String>", desc: "Fallback text rendered when src is None." },
                    Prop { name: "color",    ty: "AvatarColor",      desc: "Placeholder tint. Distinct from Tone because an avatar is a person/object placeholder, not a semantic action." },
                    Prop { name: "size",     ty: "AvatarSize",       desc: "Xs / Sm / Md / Lg / Xl." },
                ])
            }
        },
        ui! {
            Callout(label = "Why a separate AvatarColor axis".to_string()) {
                P(content = "An avatar isn't a semantic action, so it doesn't take a Tone. The \
                    `color` prop picks a named placeholder tint (the intent's soft background \
                    + matching soft text) so a no-prop avatar reads as a generic \
                    placeholder.".to_string())
            }
        },
    ])
}

fn avatar_size_row() -> Element {
    ui! {
        Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Md) {
            Avatar(initials = "XS".to_string(), color = AvatarColor::Primary, size = AvatarSize::Xs)
            Avatar(initials = "SM".to_string(), color = AvatarColor::Primary, size = AvatarSize::Sm)
            Avatar(initials = "MD".to_string(), color = AvatarColor::Primary, size = AvatarSize::Md)
            Avatar(initials = "LG".to_string(), color = AvatarColor::Primary, size = AvatarSize::Lg)
            Avatar(initials = "XL".to_string(), color = AvatarColor::Primary, size = AvatarSize::Xl)
        }
    }
}

fn avatar_content_row() -> Element {
    ui! {
        Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Md) {
            Avatar(initials = "AB".to_string(), color = AvatarColor::Primary, size = AvatarSize::Lg)
            Avatar(initials = "CD".to_string(), color = AvatarColor::Success, size = AvatarSize::Lg)
            Avatar(
                src = Some("https://i.pravatar.cc/120?img=12".into()),
                size = AvatarSize::Lg,
            )
        }
    }
}

fn avatar_group_row() -> Element {
    ui! {
        Stack(axis = StackAxis::Row, wrap = true, gap = StackGap::Xs) {
            Avatar(initials = "AL".to_string(), color = AvatarColor::Primary,   size = AvatarSize::Md)
            Avatar(initials = "BR".to_string(), color = AvatarColor::Secondary, size = AvatarSize::Md)
            Avatar(initials = "CJ".to_string(), color = AvatarColor::Success,   size = AvatarSize::Md)
            Avatar(initials = "DM".to_string(), color = AvatarColor::Warning,   size = AvatarSize::Md)
        }
    }
}
