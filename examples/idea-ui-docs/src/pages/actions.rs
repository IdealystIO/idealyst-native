//! Actions — Button, IconButton, Badge, Tag (one page each).

use std::rc::Rc;

use runtime_core::{ui, Element};
use idea_ui::doc_controls::DocControls;
use idea_ui::{
    Badge, BadgeProps, Button, ButtonProps, IconButton, IconButtonProps, Stack, StackAxis, StackGap,
    Tag, TagProps, tone, variant,
};

use crate::shell::{
    self, Callout, CodePanel, ComponentPage, Demo, DemoSurface, H2, P, Prop, PropsTable, Section,
};

// =============================================================================
// Button
// =============================================================================

pub fn button() -> Element {
    let state = ButtonProps::init_state();
    state.label.set("Click me".to_string());

    let preview = ButtonProps::reactive_preview(&state, |props| {
        let label = props.label;
        let tone = props.tone;
        let variant = props.variant;
        let size = props.size;
        let shape = props.shape;
        let on_click: Rc<dyn Fn()> = Rc::new(|| {});
        ui! {
            Button(
                label = label,
                on_click = on_click,
                tone = tone,
                variant = variant,
                size = size,
                shape = shape,
            )
        }
    });
    let controls = ButtonProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "Button".to_string(),
            lead = "The themed clickable. Picks a palette via `tone`, a visual treatment \
                via `variant`, and a size + corner radius via `size` and `shape`.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Variants × tones".to_string()) {
                P(content = "Every variant pairs with every tone. Here's a single row showing \
                    each variant on the Primary tone:".to_string())
                DemoSurface {
                    variant_row()
                }
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "label",
                        ty: "Reactive<String>",
                        desc: "Button text. Static or reactive.",
                    },
                    Prop {
                        name: "on_click",
                        ty: "Rc<dyn Fn()>",
                        desc: "Callback fired on press.",
                    },
                    Prop {
                        name: "tone",
                        ty: "ToneRef",
                        desc: "Semantic palette. Default: Primary.",
                    },
                    Prop {
                        name: "variant",
                        ty: "VariantRef",
                        desc: "Filled / Soft / Outlined / Ghost. Default: Filled.",
                    },
                    Prop {
                        name: "size",
                        ty: "ButtonSizeRef",
                        desc: "Sm / Md / Lg. Default: Md.",
                    },
                    Prop {
                        name: "shape",
                        ty: "ShapeRef",
                        desc: "Sharp / Sm / Md / Pill. Default: Md.",
                    },
                    Prop {
                        name: "disabled",
                        ty: "Option<Rc<dyn Fn() -> bool>>",
                        desc: "Optional reactive disabled flag. Returning true disables the press handler and dims the surface.",
                    },
                    Prop {
                        name: "bind_to",
                        ty: "Option<Ref<PressableHandle>>",
                        desc: "When Some, fills the given Ref on mount. Anchor an overlay/popover to this button.",
                    },
                ])
            }

            Section(title = "Recipes".to_string()) {
                H2(content = "Primary action".to_string())
                CodePanel(src = r##"Button(
    label = "Save".into(),
    on_click = save,
    tone = tone::Primary,
    variant = variant::Filled,
)"##.to_string())

                H2(content = "Destructive action".to_string())
                CodePanel(src = r##"Button(
    label = "Delete".into(),
    on_click = confirm_delete,
    tone = tone::Danger,
    variant = variant::Filled,
)"##.to_string())

                H2(content = "Ghost button (toolbar)".to_string())
                CodePanel(src = r##"Button(
    label = "Cancel".into(),
    on_click = close,
    tone = tone::Neutral,
    variant = variant::Ghost,
)"##.to_string())

                H2(content = "Anchor a Popover to the button".to_string())
                CodePanel(src = r##"let trigger: Ref<PressableHandle> = Ref::new();
let open = signal!(false);

ui! {
    Button(
        label = "More".into(),
        on_click = move || open.update(|v| *v = !*v),
        tone = tone::Neutral,
        variant = variant::Soft,
        bind_to = Some(trigger),
    )
    if open.get() {
        Popover(target = Some(AnchorTarget::from(trigger))) {
            // menu items
        }
    }
}"##.to_string())
            }

            Callout(label = "Static styles, no flicker".to_string()) {
                P(content = "Every (tone, variant, size, shape) tuple is a className lookup against \
                    a pre-generated stylesheet. There's no per-button apply-style closure — theme \
                    swaps update CSS variables in bulk, and 1000 buttons sharing a tuple share \
                    one class.".to_string())
            }
        }
    })
}

fn variant_row() -> Element {
    let on_click: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Stack(axis = StackAxis::Row, gap = StackGap::Md) {
            Button(label = "Filled".to_string(),   on_click = on_click.clone(), tone = tone::Primary, variant = variant::Filled)
            Button(label = "Soft".to_string(),     on_click = on_click.clone(), tone = tone::Primary, variant = variant::Soft)
            Button(label = "Outlined".to_string(), on_click = on_click.clone(), tone = tone::Primary, variant = variant::Outlined)
            Button(label = "Ghost".to_string(),    on_click = on_click,         tone = tone::Primary, variant = variant::Ghost)
        }
    }
}

// =============================================================================
// IconButton
// =============================================================================

pub fn icon_button() -> Element {
    let state = IconButtonProps::init_state();
    state.glyph.set("+".to_string());

    let preview = IconButtonProps::reactive_preview(&state, |props| {
        let glyph = props.glyph;
        let tone = props.tone;
        let variant = props.variant;
        let size = props.size;
        let on_click: Rc<dyn Fn()> = Rc::new(|| {});
        ui! {
            IconButton(
                glyph = glyph,
                on_click = on_click,
                tone = tone,
                variant = variant,
                size = size,
            )
        }
    });
    let controls = IconButtonProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "IconButton".to_string(),
            lead = "Square sibling of Button — takes a glyph string instead of a label. \
                Same tone / variant vocabulary; size is a closed enum.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "glyph",    ty: "String", desc: "Single-character glyph or short text rendered centered." },
                    Prop { name: "on_click", ty: "Rc<dyn Fn()>", desc: "Callback fired on press." },
                    Prop { name: "tone",     ty: "ToneRef", desc: "Semantic palette. Default: Neutral." },
                    Prop { name: "variant",  ty: "VariantRef", desc: "Filled / Soft / Outlined / Ghost. Default: Filled." },
                    Prop { name: "size",     ty: "IconButtonSize", desc: "Sm / Md / Lg — closed enum (square footprint)." },
                    Prop { name: "disabled", ty: "Option<Rc<dyn Fn() -> bool>>", desc: "Optional reactive disabled flag." },
                ])
            }

            Section(title = "When to use it".to_string()) {
                P(content = "Toolbar buttons, close affordances on cards, and inline actions \
                    inside tables. For decorated icons that need an SVG payload (not a glyph \
                    string), wrap an `Icon` primitive in a `Pressable`.".to_string())
            }
        }
    })
}

// =============================================================================
// Badge
// =============================================================================

pub fn badge() -> Element {
    let state = BadgeProps::init_state();
    state.label.set("New".to_string());

    let preview = BadgeProps::reactive_preview(&state, |props| {
        let label = props.label;
        let tone = props.tone;
        let variant = props.variant;
        ui! { Badge(label = label, tone = tone, variant = variant) }
    });
    let controls = BadgeProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "Badge".to_string(),
            lead = "Small pill for status indicators — \"New\", \"3\", \"Beta\". Tone + \
                variant axes; no shape axis (badges are always pill-radius).".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label",   ty: "Reactive<String>", desc: "Pill text. Static or reactive." },
                    Prop { name: "tone",    ty: "ToneRef",          desc: "Semantic palette. Default: Neutral." },
                    Prop { name: "variant", ty: "VariantRef",       desc: "Filled / Soft / Outlined. No Ghost (a transparent badge would be invisible). Default: Soft." },
                ])
            }

            Section(title = "Common patterns".to_string()) {
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
        }
    })
}

// =============================================================================
// Tag
// =============================================================================

pub fn tag() -> Element {
    let state = TagProps::init_state();
    state.label.set("Rust".to_string());

    let preview = TagProps::reactive_preview(&state, |props| {
        let label = props.label;
        let tone = props.tone;
        let variant = props.variant;
        ui! { Tag(label = label, tone = tone, variant = variant) }
    });
    let controls = TagProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "Tag".to_string(),
            lead = "Like Badge, with an optional close affordance. Use for chip-style \
                multi-select inputs, filter chips, or removable labels.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label",     ty: "Reactive<String>",         desc: "Tag text." },
                    Prop { name: "tone",      ty: "ToneRef",                  desc: "Semantic palette. Default: Neutral." },
                    Prop { name: "variant",   ty: "VariantRef",               desc: "Filled / Soft / Outlined." },
                    Prop { name: "on_remove", ty: "Option<Rc<dyn Fn()>>",     desc: "When Some, a close (×) button renders to the right of the label." },
                ])
            }

            Section(title = "Removable tag list".to_string()) {
                CodePanel(src = r##"let langs = signal!(vec!["Rust".to_string(), "Go".to_string()]);

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
        }
    })
}
