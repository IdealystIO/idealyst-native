//! Theming pages — tokens, intents, light/dark, custom themes, modifiers.

use std::rc::Rc;

use runtime_core::{ui, Element, Signal};
use idea_ui::{
    dark_theme, light_theme, set_idea_theme, tone, variant, Badge, Button, Stack, StackAxis,
    StackGap, Switch, ToneRef,
};

use crate::shell::{
    self, Callout, CodePanel, ComponentPage, DemoSurface, H2, H3, P, Prop, PropsTable,
};

// =============================================================================
// Theme tokens
// =============================================================================

pub fn tokens() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "Theme tokens".to_string(),
            lead = "Tokens are the live, signal-backed values stylesheets read. \
                Components don't hardcode colors — they reference token names that \
                the active theme resolves.".to_string(),
        ) {
            H2(content = "How a token reaches the screen".to_string())
            P(content = "Every visible value in idea-ui flows through this pipeline:".to_string())
            CodePanel(src = r##"// 1. The component's stylesheet references a token by name + fallback.
stylesheet! {
    pub Button<()> {
        base(_t) {
            background: Tokenized::token("intent-primary-solid-bg", Color("#5b6cff".into())),
            color:      Tokenized::token("intent-primary-solid-text", Color("#fff".into())),
        }
    }
}

// 2. The installed theme binds that name to a real value.
//    `install_idea_theme(light_theme())` registers every "intent-*"
//    token from `light_theme()`'s IntentColors blocks.

// 3. At apply-style time the framework resolves the name against the
//    active theme's token map. The fallback only fires if the name is
//    not bound — useful for new components that read a token the
//    installed theme hasn't been updated to provide yet."##.to_string())

            H2(content = "Why this shape".to_string())
            P(content = "Tokens give you three things in one mechanism:".to_string())
            CodePanel(src = r##"// Theme swap: `set_theme(other_theme())` re-runs every component's
//    apply-style with no rebuild. The DOM nodes stay; only the resolved
//    values change. CSS variables on the web do the actual swap.

// Reference defaults: `light_theme()` and `dark_theme()` ship with a
//    full set of names — components can rely on the names existing
//    without baking in concrete values.

// Custom values: apps replace any subset of those names by installing
//    their own theme value or by writing their own `IdeaTheme` impl."##.to_string())

            H2(content = "The built-in token names".to_string())
            P(content = "Reference docs for every component show which tokens they read. The \
                conventions:".to_string())
            PropsTable(rows = vec![
                Prop {
                    name: "color-background",
                    ty: "Color",
                    desc: "Page background. Reads from theme.colors().background.",
                },
                Prop {
                    name: "color-surface",
                    ty: "Color",
                    desc: "Default raised surface (Card, Field background, …).",
                },
                Prop {
                    name: "color-surface-alt",
                    ty: "Color",
                    desc: "Elevated surface — Card variant::Elevated, code blocks.",
                },
                Prop {
                    name: "color-text",
                    ty: "Color",
                    desc: "Primary text color, used by Typography's default tone.",
                },
                Prop {
                    name: "color-text-muted",
                    ty: "Color",
                    desc: "Secondary text — Typography(muted = true), help text.",
                },
                Prop {
                    name: "color-border",
                    ty: "Color",
                    desc: "Hairline borders on Cards, dividers, props tables.",
                },
                Prop {
                    name: "intent-{name}-solid-bg",
                    ty: "Color",
                    desc: "Solid-variant background for the given intent (primary, danger, …).",
                },
                Prop {
                    name: "intent-{name}-solid-text",
                    ty: "Color",
                    desc: "Foreground rendered on the solid background.",
                },
                Prop {
                    name: "intent-{name}-soft-bg",
                    ty: "Color",
                    desc: "Tinted background for Soft-variant components.",
                },
                Prop {
                    name: "intent-{name}-soft-text",
                    ty: "Color",
                    desc: "Foreground rendered on the soft background.",
                },
                Prop {
                    name: "intent-{name}-fg",
                    ty: "Color",
                    desc: "The intent color used as text/border for Outlined / Ghost.",
                },
                Prop {
                    name: "spacing-{xs,sm,md,lg,xl,xxl}",
                    ty: "Length",
                    desc: "Padding/gap scale read by Stack, Card, and chrome stylesheets.",
                },
                Prop {
                    name: "radius-{sm,md,lg,pill}",
                    ty: "Length",
                    desc: "Corner-radius scale.",
                },
                Prop {
                    name: "typography-{kind}-size",
                    ty: "Length",
                    desc: "Font sizes — one token per TypographyKind variant.",
                },
            ])

            Callout(label = "Why fallbacks matter".to_string()) {
                P(content = "Every `Tokenized::token(name, fallback)` carries a literal fallback. \
                    If the name isn't bound (e.g. a custom theme that hasn't been updated for a \
                    new component yet), the fallback renders — the UI degrades visibly, not \
                    silently. Never write `unwrap()` against a missing token; the fallback is \
                    the contract.".to_string())
            }
        }
    })
}

// =============================================================================
// Intents
// =============================================================================

pub fn intents() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "Intents".to_string(),
            lead = "Intent is the global semantic palette every component shares — \
                Primary, Secondary, Neutral, Success, Danger, Warning, Info.".to_string(),
        ) {
            H2(content = "One vocabulary, every component".to_string())
            P(content = "An intent isn't a color — it's a meaning. \"Danger\" reads as red on \
                light themes and a desaturated rose on dark themes; it's the right red on \
                Button (Filled), the right tint on Badge (Soft), and the right border color on \
                Alert (Outlined). You write the meaning once and let the theme + variant axes \
                produce the visual.".to_string())

            H2(content = "Built-in tones".to_string())
            P(content = "Every intent-aware component takes a `tone` prop. The seven built-ins, \
                rendered as Button (Filled) + Badge (Soft):".to_string())
            DemoSurface { intent_grid() }

            H2(content = "Where intent lives in the API".to_string())
            CodePanel(src = r##"use idea_ui::{tone, variant, ToneRef};

// Pass a tone built-in directly:
Button(label = "Save".into(), on_click = cb, tone = tone::Primary, variant = variant::Filled)

// Or pre-build a `ToneRef` for re-use:
let danger: ToneRef = tone::Danger.into();
Alert(title = "Failed".into(), tone = danger.clone(), variant = variant::Soft)
Badge(label = "Error".into(), tone = danger, variant = variant::Soft)"##.to_string())

            H2(content = "Tone-aware Typography".to_string())
            P(content = "Typography accepts an optional `tone` for inline emphasis — useful when \
                a single phrase carries semantic weight (\"3 failures\", \"\\u{2713} saved\").".to_string())
            CodePanel(src = r##"Typography(content = "Saved".into(), tone = Some(tone::Success.into()))
Typography(content = "3 errors".into(), tone = Some(tone::Danger.into()))"##.to_string())

            Callout(label = "Custom intents".to_string()) {
                P(content = "The `Tone` trait is open — implement it on a marker type and your \
                    custom intent works in every component that consumes a tone. See Extending \
                    → Adding a custom tone.".to_string())
            }
        }
    })
}

fn intent_grid() -> Element {
    let intents: Vec<(&'static str, ToneRef)> = vec![
        ("Primary", tone::Primary.into()),
        ("Secondary", tone::Secondary.into()),
        ("Neutral", tone::Neutral.into()),
        ("Success", tone::Success.into()),
        ("Danger", tone::Danger.into()),
        ("Warning", tone::Warning.into()),
        ("Info", tone::Info.into()),
    ];
    let mut rows: Vec<Element> = Vec::with_capacity(intents.len());
    for (name_str, t) in intents {
        let name = name_str.to_string();
        let on_click: Rc<dyn Fn()> = Rc::new(|| {});
        rows.push(ui! {
            Stack(axis = StackAxis::Row, gap = StackGap::Md) {
                Button(
                    label = name.clone(),
                    on_click = on_click,
                    tone = t.clone(),
                    variant = variant::Filled,
                )
                Badge(label = name, tone = t, variant = variant::Soft)
            }
        });
    }
    ui! { Stack(gap = StackGap::Sm) { rows } }
}

// =============================================================================
// Light & dark
// =============================================================================

pub fn light_dark(is_dark: Signal<bool>) -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "Light & dark".to_string(),
            lead = "Theme switching is a single function call. Every component \
                re-resolves through CSS variables — no rebuild, no per-component \
                flicker.".to_string(),
        ) {
            H2(content = "Swap themes at runtime".to_string())
            P(content = "Two reference themes ship with idea-ui: `light_theme()` and \
                `dark_theme()`. Pass either one to `install_idea_theme(...)` at startup, and \
                call `set_idea_theme(...)` to swap later.".to_string())
            CodePanel(src = r##"use idea_ui::{install_idea_theme, set_idea_theme, light_theme, dark_theme};

// At startup:
install_idea_theme(light_theme());

// Later, in response to a toggle:
set_idea_theme(dark_theme());"##.to_string())

            H2(content = "Live".to_string())
            P(content = "Flip the switch in the sidebar (or this one) to toggle the active \
                theme. Every visible token updates with a smooth color transition because the \
                shell stylesheets declare per-property transitions.".to_string())
            DemoSurface { dark_switch(is_dark) }

            H2(content = "Why transitions, not rebuilds".to_string())
            P(content = "Theme tokens compile to CSS custom properties on the web backend; \
                changing a property's value triggers the property-level transitions declared \
                in each stylesheet. The DOM doesn't change, no apply-style runs — the only \
                cost is the transition itself. On native backends the same set_theme call \
                triggers a tokens update which the backend translates to the platform's \
                color-update path (UIColor.dynamicProvider on iOS, etc.).".to_string())

            Callout(label = "Detecting the active theme".to_string()) {
                P(content = "If you need to branch on \"is the current theme dark?\" — for example \
                    to pick a palette in a code-block highlighter — read a `color-background` \
                    token and check the luminance. See `shell::theme_is_dark` in the docs source.".to_string())
            }
        }
    })
}

fn dark_switch(is_dark: Signal<bool>) -> Element {
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |dark| {
        is_dark.set(dark);
        if dark {
            set_idea_theme(dark_theme());
        } else {
            set_idea_theme(light_theme());
        }
    });
    ui! {
        Switch(
            label = Some("Dark mode".to_string()),
            value = is_dark,
            on_change = on_change,
        )
    }
}

// =============================================================================
// Custom themes
// =============================================================================

pub fn custom_theme() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "Custom themes".to_string(),
            lead = "Override the built-in palette to brand your app — or replace the \
                whole theme struct when the built-in shape isn't enough.".to_string(),
        ) {
            H2(content = "Path 1: override tokens".to_string())
            P(content = "The simplest path keeps the built-in `IdeaTheme` struct and only \
                changes the colors you care about. Build a theme via the `idea_color!` and \
                `idea_header!` macros, install it once, and ship.".to_string())
            CodePanel(src = r##"use idea_ui::{install_idea_theme, light_theme, idea_color};

// Start from the built-in light theme and replace primary.
let mut theme = light_theme();
theme.intents.primary.solid_bg = idea_color!("#7c5cff");
theme.intents.primary.solid_text = idea_color!("#ffffff");
theme.intents.primary.soft_bg = idea_color!("rgba(124, 92, 255, 0.12)");
theme.intents.primary.soft_text = idea_color!("#5a3fdb");
theme.intents.primary.fg = idea_color!("#5a3fdb");
theme.intents.primary.border = idea_color!("#5a3fdb");

install_idea_theme(theme);"##.to_string())

            H2(content = "Path 2: implement `IdeaTheme` on a new struct".to_string())
            P(content = "When you want fields the built-in theme doesn't have (an app-specific \
                accent slot, a richer typography scale, custom spacing tokens), implement \
                `IdeaTheme` on your own struct.".to_string())
            CodePanel(src = r##"use idea_ui::{install_idea_theme, IdeaTheme, IdeaThemeDefaults, Colors, Intents};

#[derive(Clone)]
pub struct AppTheme {
    base: idea_ui::IdeaThemeDefaults,
    // App-specific tokens live alongside the base shape.
    pub hype_color: runtime_core::Color,
}

impl IdeaTheme for AppTheme {
    fn colors(&self) -> &Colors    { self.base.colors() }
    fn intents(&self) -> &Intents  { self.base.intents() }
    fn spacing(&self) -> &idea_ui::Spacing { self.base.spacing() }
    fn radius(&self)  -> &idea_ui::Radius  { self.base.radius() }
    fn typography(&self) -> &idea_theme::theme::Typography { self.base.typography() }
}

let theme = AppTheme {
    base: IdeaThemeDefaults::light(),
    hype_color: runtime_core::Color("#ff3ea5".into()),
};
install_idea_theme(theme);"##.to_string())

            Callout(label = "Per-stylesheet token overrides".to_string()) {
                P(content = "If you don't want a whole theme struct, individual stylesheets can \
                    override any `Tokenized::token(name, fallback)` reference by binding their \
                    own name. The token namespace is per-process-thread; the last-installed \
                    value wins. Useful for design-system experiments without recompiling the \
                    theme.".to_string())
            }

            H2(content = "Where to look in the source".to_string())
            P(content = "The reference themes live in `crates/ui/idea-theme/src/theme.rs`. Read \
                `light_theme()` and `dark_theme()` to see what fields exist and what intent \
                slots they fill — these are also the canonical fallback values every component \
                references.".to_string())
        }
    })
}

// =============================================================================
// Modifiers
// =============================================================================

pub fn modifiers() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "Modifiers".to_string(),
            lead = "Tone, Variant, ButtonSize, Shape: the four orthogonal axes every \
                themed component composes against.".to_string(),
        ) {
            H2(content = "Why four axes".to_string())
            P(content = "A component picks the axes it needs and ignores the rest. Button \
                consumes all four: tone for the palette, variant for the visual treatment, \
                size for padding/text-size, shape for the corner radius. Badge consumes only \
                tone + variant. Typography consumes its own TypographyKind axis instead. \
                Composing them as separate traits lets each component pick its own set without \
                an explosion of bespoke enums.".to_string())

            H2(content = "Tone — semantic palette".to_string())
            P(content = "`Tone` is the trait every intent shares. Built-in markers live in \
                `idea_ui::tone`. Each tone returns six color slots (fill_bg, fill_fg, soft_bg, \
                soft_fg, fg, border) that downstream components map to their visual.".to_string())
            CodePanel(src = r##"// Built-ins (zero-sized markers):
idea_ui::tone::{Primary, Secondary, Neutral, Success, Danger, Warning, Info}

// Coerce to the type-erased `ToneRef` for prop passing:
let t: idea_ui::ToneRef = idea_ui::tone::Danger.into();"##.to_string())

            H2(content = "Variant — visual treatment".to_string())
            P(content = "`Variant` answers \"how is the surface drawn?\": Filled (solid \
                background), Soft (tinted background), Outlined (border only, transparent fill), \
                Ghost (transparent, color on hover). Not every component supports every variant \
                — Badge omits Ghost (it'd be invisible), Card has its own variant set.".to_string())
            CodePanel(src = r##"idea_ui::variant::{Filled, Soft, Outlined, Ghost}

// Variants compose with tones at apply-style time. The pair
// (tone, variant) becomes a cache key — 1000 buttons sharing
// (Filled, Primary) materialize one resolved StyleRules.
Button(label = "OK".into(), tone = tone::Primary, variant = variant::Filled)
Button(label = "OK".into(), tone = tone::Primary, variant = variant::Soft)"##.to_string())

            H2(content = "ButtonSize — padding + text size".to_string())
            P(content = "Three sizes for the Button family (Button, IconButton): Sm, Md, Lg. \
                Each binds to a different set of spacing + typography tokens; the relationship \
                between the three is set by the theme, so an app can dial the scale by \
                installing a tighter or looser theme without touching call sites.".to_string())
            CodePanel(src = r##"idea_ui::size::{Sm, Md, Lg}

Button(label = "Tiny".into(), tone = tone::Neutral, variant = variant::Soft, size = size::Sm)
Button(label = "Big".into(),  tone = tone::Neutral, variant = variant::Soft, size = size::Lg)"##.to_string())

            H2(content = "Shape — corner radius".to_string())
            P(content = "Four shape options: Sharp, Sm, Md, Pill. Shape is independent of size \
                — a Pill-shaped Lg button and a Pill-shaped Sm button both look like pills, \
                just at different scales.".to_string())
            CodePanel(src = r##"idea_ui::shape::{Sharp, Sm, Md, Pill}

Button(label = "Pill".into(), tone = tone::Primary, variant = variant::Filled, shape = shape::Pill)"##.to_string())

            H2(content = "Typography uses its own axis".to_string())
            P(content = "Typography has too many shape concerns (size + weight + line-height + \
                letter-spacing) to share Button's size axis cleanly. It uses `TypographyKind`: \
                Display, H1..H3, BodyXl..BodySm, Caption, Overline. Same trait surface — \
                the markers live in `idea_ui::typography_kind`.".to_string())
            CodePanel(src = r##"Typography(content = "Hello".into(), kind = typography_kind::H1)
Typography(content = "small print".into(), kind = typography_kind::Caption)"##.to_string())

            H3(content = "How a (tone, variant) pair is resolved".to_string())
            P(content = "Each component owns a stylesheet whose `variant_axis(\"appearance\", ...)` \
                pre-generates one CSS rule per (tone, variant) pair at install time. Apply-style \
                is a className lookup — no string allocation, no per-node Effect.".to_string())

            Callout(label = "Extending these axes".to_string()) {
                P(content = "Every trait above is open. You add a marker, impl the trait, and \
                    install a stylesheet that carries the new arm. See Extending → Adding a \
                    custom tone / Adding a custom variant for end-to-end walkthroughs.".to_string())
            }
        }
    })
}
