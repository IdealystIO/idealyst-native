//! Extending — custom tones, custom variants, building your own
//! component, the DocControls derive.

use runtime_core::{ui, Element};

use crate::shell::{self, Callout, CodePanel, ComponentPage, H2, H3, P, Prop, PropsTable};

// =============================================================================
// Custom tone
// =============================================================================

pub fn custom_tone() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "Adding a custom tone".to_string(),
            lead = "Tone is an open trait. Implement it on a marker struct and your \
                tone plugs into every intent-aware component — Button, Badge, Tag, \
                Alert — without modifying their props.".to_string(),
        ) {
            H2(content = "The Tone trait".to_string())
            P(content = "Every tone supplies six color slots — fill_bg, fill_fg, soft_bg, \
                soft_fg, fg, border. Slot completeness is compile-enforced; there's no \
                Option-returning fallback, no panic if a slot is missing. If your tone genuinely \
                has no value for a slot, pick a sensible reuse.".to_string())
            CodePanel(src = r##"use idea_theme::extensible::Tone;
use idea_ui::IdeaTheme;
use runtime_core::{Color, Tokenized};

#[derive(Copy, Clone, Default)]
pub struct Hype;

impl Tone for Hype {
    fn key(&self) -> &'static str { "hype" }

    fn fill_bg(&self, _theme: &dyn IdeaTheme) -> Tokenized<Color> {
        Tokenized::token("tone-hype-fill-bg", Color("#ff3ea5".into()))
    }
    fn fill_fg(&self, _theme: &dyn IdeaTheme) -> Tokenized<Color> {
        Tokenized::token("tone-hype-fill-fg", Color("#ffffff".into()))
    }
    fn soft_bg(&self, _theme: &dyn IdeaTheme) -> Tokenized<Color> {
        Tokenized::token("tone-hype-soft-bg", Color("rgba(255, 62, 165, 0.12)".into()))
    }
    fn soft_fg(&self, _theme: &dyn IdeaTheme) -> Tokenized<Color> {
        Tokenized::token("tone-hype-soft-fg", Color("#c1257f".into()))
    }
    fn fg(&self, _theme: &dyn IdeaTheme) -> Tokenized<Color> {
        Tokenized::token("tone-hype-fg", Color("#c1257f".into()))
    }
    fn border(&self, _theme: &dyn IdeaTheme) -> Tokenized<Color> {
        Tokenized::token("tone-hype-border", Color("#c1257f".into()))
    }
}"##.to_string())

            H2(content = "Register the tone with each component sheet".to_string())
            P(content = "Built-in component sheets (Button, Badge, Tag, Alert, IconButton, \
                Typography) pre-generate one CSS arm per built-in tone. To make your custom \
                tone work in those components, install an extended sheet that adds your arm. \
                This is a one-time setup at app boot.".to_string())
            CodePanel(src = r##"use idea_theme::extensible::{
    install_button_sheet, install_badge_sheet, install_tag_sheet, install_alert_sheet,
    ButtonSheetBuilder, BadgeSheetBuilder, TagSheetBuilder, AlertSheetBuilder,
    tone, variant,
};

pub fn install_hype() {
    // Helper that fluently adds every built-in tone + variant to a
    // builder, plus our custom tone, so each component sheet has a
    // pre-generated arm for every combination.
    install_button_sheet(
        ButtonSheetBuilder::new()
            .add_tone(tone::Primary).add_tone(tone::Secondary).add_tone(tone::Neutral)
            .add_tone(tone::Success).add_tone(tone::Danger)
            .add_tone(tone::Warning).add_tone(tone::Info)
            .add_tone(Hype)
            .add_variant(variant::Filled).add_variant(variant::Soft)
            .add_variant(variant::Outlined).add_variant(variant::Ghost)
            .build(),
    );
    install_badge_sheet(
        BadgeSheetBuilder::new()
            .add_tone(tone::Primary).add_tone(tone::Neutral).add_tone(tone::Success)
            .add_tone(tone::Danger).add_tone(tone::Warning).add_tone(tone::Info)
            .add_tone(Hype)
            .add_variant(variant::Filled).add_variant(variant::Soft)
            .add_variant(variant::Outlined)
            .build(),
    );
    install_tag_sheet(
        TagSheetBuilder::new()
            .add_tone(tone::Primary).add_tone(tone::Neutral).add_tone(Hype)
            .add_variant(variant::Filled).add_variant(variant::Soft)
            .add_variant(variant::Outlined)
            .build(),
    );
    install_alert_sheet(
        AlertSheetBuilder::new()
            .add_tone(tone::Info).add_tone(tone::Success).add_tone(tone::Warning)
            .add_tone(tone::Danger).add_tone(Hype)
            .add_variant(variant::Filled).add_variant(variant::Soft)
            .build(),
    );
}"##.to_string())

            H2(content = "Use it".to_string())
            CodePanel(src = r##"// Call this once at app boot (right after install_idea_theme).
install_hype();

// Then use it like any built-in tone.
ui! {
    Button(label = "Get it!".into(), on_click = cb, tone = Hype.into(), variant = variant::Filled)
    Badge(label = "Hype".into(), tone = Hype.into(), variant = variant::Soft)
    Alert(title = "New release".into(), tone = Hype.into(), variant = variant::Soft)
}"##.to_string())

            Callout(label = "Token names matter".to_string()) {
                P(content = "Pick a prefix (here `tone-hype-*`) and use it consistently across \
                    your tone's six methods. That makes future theme overrides easy: a custom \
                    theme can rebind every `tone-hype-*` name without recompiling components.".to_string())
            }
        }
    })
}

// =============================================================================
// Custom variant
// =============================================================================

pub fn custom_variant() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "Adding a custom variant".to_string(),
            lead = "Variant is the \"how is the surface drawn?\" axis. Built-ins are \
                Filled, Soft, Outlined, Ghost; add your own by implementing the \
                Variant trait.".to_string(),
        ) {
            H2(content = "The Variant trait".to_string())
            P(content = "A Variant has a stable key (for the resolution cache) and a render \
                function that produces a StyleRules. The function receives a ResolutionCtx \
                carrying the theme and the active tone, so a variant can read \
                `ctx.theme.intents()` to pick its colors.".to_string())
            CodePanel(src = r##"use idea_theme::extensible::{ResolutionCtx, Variant};
use runtime_core::{Color, Shadow, StyleRules};

#[derive(Copy, Clone, Default)]
pub struct Glow;

impl Variant for Glow {
    fn key(&self) -> &'static str { "glow" }

    fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
        // Use the active tone's `fill_bg` as the background, plus a
        // glow shadow tinted with the tone's `fg` color.
        StyleRules {
            background: Some(ctx.tone.fill_bg(ctx.theme)),
            shadow: Some(Shadow {
                x: 0.0,
                y: 0.0,
                blur: 18.0,
                color: Color("rgba(91, 108, 255, 0.45)".into()),
            }),
            ..Default::default()
        }
    }
}"##.to_string())

            H2(content = "Register the variant alongside built-ins".to_string())
            CodePanel(src = r##"install_button_sheet(
    ButtonSheetBuilder::new()
        .add_tone(tone::Primary).add_tone(tone::Danger) // … and any others
        .add_variant(variant::Filled).add_variant(variant::Soft)
        .add_variant(variant::Outlined).add_variant(variant::Ghost)
        .add_variant(Glow)
        .build(),
);"##.to_string())

            H2(content = "Use it".to_string())
            CodePanel(src = r##"Button(label = "Glow".into(), on_click = cb, tone = tone::Primary, variant = Glow.into())"##.to_string())

            Callout(label = "Card's variants are different".to_string()) {
                P(content = "Card uses Variant too, but its built-ins (Flat, Elevated) don't read \
                    a tone — they pull surface colors directly. When writing a custom Card \
                    variant, follow the same pattern: ignore `ctx.tone`, read `ctx.theme.colors()` \
                    for surface tokens.".to_string())
            }
        }
    })
}

// =============================================================================
// Build a component
// =============================================================================

pub fn build_component() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "Building a component".to_string(),
            lead = "How to write a new themed component on top of idea-ui's modifier \
                system. The recipe is short, the trait surface does the work.".to_string(),
        ) {
            H2(content = "The shape".to_string())
            P(content = "Every idea-ui component follows the same shape:".to_string())
            CodePanel(src = r##"1. A `*Props` struct holding the component's props.
2. A `Default` impl (manual when callbacks/Signals are present).
3. A `#[component]` fn that consumes the props and returns an Element.
4. Optional: a stylesheet (either declared with `stylesheet!` for fixed
   shape components, or programmatically built for components that
   compose multiple modifier axes)."##.to_string())

            H2(content = "Example — a Pill component (tone + variant + an icon)".to_string())
            P(content = "Suppose we want a Pill: like Badge, but with a leading glyph. We'll \
                lean on Badge's existing stylesheet for the surface, then drop a glyph inside.".to_string())
            CodePanel(src = r##"use runtime_core::{component, ui, IntoElement, Element, Reactive, StyleApplication};
use idea_ui::{Stack, StackAxis, StackGap, ToneRef, VariantRef, tone, variant, Typography};
use idea_theme::extensible::installed_badge_sheet;

#[derive(Default)]
pub struct PillProps {
    pub label:   Reactive<String>,
    pub glyph:   String,
    pub tone:    ToneRef,
    pub variant: VariantRef,
}

#[component]
pub fn Pill(props: PillProps) -> Element {
    let appearance_key = format!("{}_{}", props.tone.key(), props.variant.key());
    let style = StyleApplication::new(installed_badge_sheet())
        .with("appearance", appearance_key);

    ui! {
        view(style = style) {
            Stack(axis = StackAxis::Row, gap = StackGap::Xs) {
                Typography(content = props.glyph.clone())
                Typography(content = props.label)
            }
        }
    }
}"##.to_string())

            H2(content = "Call site".to_string())
            CodePanel(src = r##"ui! {
    Pill(
        label = "Online".into(),
        glyph = "\u{25CF}".into(),
        tone = tone::Success,
        variant = variant::Soft,
    )
}"##.to_string())

            H3(content = "When to write your own stylesheet".to_string())
            P(content = "If your component has a fixed shape (no tone/variant axes — think Stack, \
                Divider, Spacer), reach for the `stylesheet!` macro from `runtime_core` for \
                static styles. The macro produces a stylesheet you instantiate at the call site \
                via the builder methods it generates.".to_string())
            CodePanel(src = r##"use runtime_core::{stylesheet, Length, Tokenized};

stylesheet! {
    pub MyCard<idea_ui::IdeaThemeRef> {
        base(_t) {
            background: Tokenized::token("color-surface", runtime_core::Color("#fff".into())),
            border_radius: Tokenized::token("radius-lg", Length::Px(12.0)),
            padding: 24.0,
        }
    }
}

// At call site:
ui! { view(style = MyCard()) { /* children */ } }"##.to_string())

            H3(content = "When to build a programmatic sheet".to_string())
            P(content = "When the component composes multiple modifier axes (Button's tone × \
                variant × size × shape), the `stylesheet!` macro doesn't fit — its variant axis \
                count is small and fixed. Look at `idea_theme::extensible::sheets` and the \
                `*SheetBuilder` types (`ButtonSheetBuilder`, `BadgeSheetBuilder`, …) for the \
                builder pattern that pre-generates one arm per (tone, variant) tuple.".to_string())

            Callout(label = "Keep the trait surface small".to_string()) {
                P(content = "Don't reach for a new modifier trait unless you need it on multiple \
                    components. A one-off enum on a single component (like Card's CardPadding) \
                    is fine. Traits matter when the axis is shared vocabulary (Tone is, because \
                    Button + Badge + Tag + Alert all consume it).".to_string())
            }
        }
    })
}

// =============================================================================
// DocControls derive
// =============================================================================

pub fn doc_controls() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "DocControls derive".to_string(),
            lead = "The reflective macro powering every interactive demo on this site. \
                Enable the `docs` feature and add `#[derive(DocControls)]` to any *Props \
                struct — you get a state container, a from_state builder, and a control \
                panel for free.".to_string(),
        ) {
            H2(content = "What the macro generates".to_string())
            P(content = "For a struct `FooProps`, the derive emits:".to_string())
            PropsTable(rows = vec![
                Prop { name: "FooPropsState",        ty: "Copy struct",                desc: "One Signal<T> per controllable field." },
                Prop { name: "init_state()",         ty: "fn() -> FooPropsState",      desc: "Builds default field values." },
                Prop { name: "from_state(state)",    ty: "fn(&State) -> Props",        desc: "Reads the state signals into a fresh Props." },
                Prop { name: "render_controls(s)",   ty: "fn(&State) -> Element",      desc: "Renders a controls panel from idea-ui components." },
                Prop { name: "reactive_preview(s,f)",ty: "fn(&State, F) -> Element",   desc: "Wraps your build closure in a switch keyed on every controllable signal — the preview rebuilds whenever any control changes." },
            ])

            H2(content = "Type → control dispatch".to_string())
            P(content = "Field types map to controls by syntactic match in the proc-macro. \
                Anything unrecognized falls through to Default::default() and gets no control.".to_string())
            PropsTable(rows = vec![
                Prop { name: "String / Reactive<String>",          ty: "Field",                  desc: "Text input." },
                Prop { name: "bool",                                ty: "Switch",                 desc: "Toggle." },
                Prop { name: "Option<String> / Reactive<...>",     ty: "Switch + Field",         desc: "Switch toggles presence; field is the value when on." },
                Prop { name: "T: VariantEnum",                     ty: "Select",                 desc: "One row per variant; round-trips through a String-keyed shadow signal." },
                Prop { name: "*Ref (ToneRef, VariantRef, …)",      ty: "Select",                 desc: "Built-in modifiers listed via the RefBuiltins trait." },
                Prop { name: "Other (Rc<dyn Fn…>, Signal<T>, Vec, …)", ty: "skipped",             desc: "No control; field falls back to Default::default()." },
            ])

            H2(content = "Per-field attributes".to_string())
            CodePanel(src = r##"pub struct MyProps {
    pub label: Reactive<String>,

    // Omit this field from the controls panel — falls through to Default.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub on_click: Rc<dyn Fn()>,

    // Override the label rendered above the control.
    #[cfg_attr(feature = "docs", doc_control(label = "Custom title"))]
    pub title: Reactive<String>,
}"##.to_string())

            H2(content = "Wiring it on your own component".to_string())
            CodePanel(src = r##"#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct MyProps { /* ... */ }

#[component]
pub fn My(props: &MyProps) -> Element { /* ... */ }"##.to_string())

            H3(content = "Building a docs page".to_string())
            CodePanel(src = r##"use idea_ui::doc_controls::DocControls;

let state = MyProps::init_state();
state.label.set("Hello".to_string());

let preview = MyProps::reactive_preview(&state, |props| {
    ui! { My(label = props.label) }
});
let controls = MyProps::render_controls(&state);

// Compose preview + controls however you like. The shell::Demo
// component on this site puts them in a side-by-side wrapping row.
ui! {
    Demo(preview = preview, controls = controls)
}"##.to_string())

            Callout(label = "Why state mirrors the props".to_string()) {
                P(content = "The state struct holds Signal<T> per field — Signals are Copy, so \
                    capturing state into the preview closure is free. `reactive_preview` reads \
                    every signal in its switch key, so any control change triggers a rebuild \
                    of the preview subtree; the rest of the page stays mounted.".to_string())
            }
        }
    })
}
