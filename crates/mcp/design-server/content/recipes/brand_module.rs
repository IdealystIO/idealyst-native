//! # Brand module: the user's look as theme tokens + component restyles
//!
//! summary: src/brand.rs — one install() that turns the user's brand into a light/dark theme pair and app-wide component restyles.
//! tags: brand, theme, tokens, restyle, dark-mode
//! render: / @phone => Acme, Create project
//! render: / @desktop => Acme, Create project, Starter, Your brand drives every surface
//!
//! The ONLY place an idea-ui app expresses its look. Copy it to
//! `src/brand.rs`, replace every value in `spec` with what the user gave
//! you, and call `brand::install()` first thing in `app()`.
//!
//! Order of tools, strongest first (read_design_skill("brand") for why):
//! 1. **Theme tokens** — color, radius, type, font. Every component and
//!    every app `stylesheet!` follows them, and light/dark swaps for free.
//! 2. **Axis defaults** (`XStyles::set_default`) — the brand's shape
//!    language: pill buttons, filled fields, elevated cards.
//! 3. **Slot restyles** (`XStyles::restyle`) — the brand's details a token
//!    can't say: a heavier button label, a card that drops its border.
//!
//! Never brand per instance (`styles = …` at a call site) and never put a
//! color literal outside this file. If the brand doesn't specify
//! something, leave idea-ui's default — don't invent a brand.

use idea_ui::{
    install_idea_theme_schemes, tone, typography_kind, variant, Badge, Button, ButtonShape,
    ButtonStyles, Card, CardPadding, CardStyles, CardVariant, Field, FieldAppearance, FieldStyles,
    IdeaThemeDefaults, IntentColors, Stack, StackAxis, StackGap, StackPadding, Tokenized,
    Typography,
};
use runtime_core::{signal, ui, Color, Element, FontFamily, FontWeight, StyleRules};
use std::rc::Rc;

/// The brand, exactly as the user specified it. Every constant here is
/// THEIRS: a hex from their palette, their typeface, their corner style.
/// Placeholder values are marked; replace them, don't keep them.
mod spec {
    /// One brand hue, as the six shades the theme needs. Get these from
    /// the user's palette; when they give a single hex, it is `base`, and
    /// the others are the nearest shades of their own scale — ask rather
    /// than eyeball a tint.
    pub struct Ramp {
        /// The brand color itself: filled buttons, selected states.
        pub base: &'static str,
        /// Text on `base` — whichever of white/near-black passes 4.5:1.
        pub on_base: &'static str,
        /// Light-mode tint for soft surfaces (soft buttons, badges, alerts).
        pub light_tint: &'static str,
        /// Light-mode text/outline color — `base` darkened until it passes
        /// 4.5:1 on white.
        pub light_fg: &'static str,
        /// Dark-mode tint: `base` at low alpha over the dark surface.
        pub dark_tint: &'static str,
        /// Dark-mode text/outline color — `base` lightened until it passes
        /// 4.5:1 on the dark surface.
        pub dark_fg: &'static str,
    }

    /// PLACEHOLDER — the user's primary brand color.
    pub const PRIMARY: Ramp = Ramp {
        base: "#2f6bff",
        on_base: "#ffffff",
        light_tint: "#eaf0ff",
        light_fg: "#1d4fd8",
        dark_tint: "rgba(47, 107, 255, 0.18)",
        dark_fg: "#8fb0ff",
    };

    /// PLACEHOLDER — the user's body typeface, with system fallbacks. The
    /// font must also be bundled (or be a system font) on every target.
    pub const FONT: &str = "Inter, system-ui, -apple-system, Segoe UI, sans-serif";

    /// PLACEHOLDER — corner radii (sm, md, lg). Sharp brands ~(2, 4, 6),
    /// friendly ones ~(6, 10, 16). idea-ui's default is (4, 8, 12).
    pub const RADIUS: (f32, f32, f32) = (6.0, 10.0, 14.0);

    /// PLACEHOLDER — does the brand use pill-shaped buttons?
    pub const PILL_BUTTONS: bool = true;
}

/// Install the brand. Call once, first thing in `app()`, before any
/// component builds — restyles registered later only reach new elements.
pub fn install() {
    install_idea_theme_schemes(light(), dark());
    restyle_components();
}

fn lit(hex: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(hex.into()))
}

fn intent(r: &spec::Ramp, dark: bool) -> IntentColors {
    let (tint, fg) = if dark { (r.dark_tint, r.dark_fg) } else { (r.light_tint, r.light_fg) };
    IntentColors {
        solid_bg: lit(r.base),
        solid_text: lit(r.on_base),
        soft_bg: lit(tint),
        soft_text: lit(fg),
        fg: lit(fg),
        border: lit(fg),
    }
}

/// What both schemes share: shape and type.
fn shared(mut t: IdeaThemeDefaults, dark: bool) -> IdeaThemeDefaults {
    t.intents.primary = intent(&spec::PRIMARY, dark);
    // The focus ring is the brand's — users see it on every keyboard move.
    t.colors.focus_ring = lit(if dark { spec::PRIMARY.dark_fg } else { spec::PRIMARY.base });
    (t.radius.sm, t.radius.md, t.radius.lg) = spec::RADIUS;
    t.font = FontFamily::System(spec::FONT.into());
    t
}

/// Light scheme: idea-ui's light neutrals + the brand. Keep idea's
/// neutrals unless the brand defines its own grays.
fn light() -> IdeaThemeDefaults {
    shared(idea_ui::light_theme(), false)
}

/// Dark scheme. Always ship one: users choose it at the OS level, and a
/// brand that only exists in light mode looks broken in dark.
fn dark() -> IdeaThemeDefaults {
    shared(idea_ui::dark_theme(), true)
}

/// The brand's shape language, as axis defaults and slot restyles.
fn restyle_components() {
    if spec::PILL_BUTTONS {
        ButtonStyles::set_default(ButtonShape::Pill);
    }
    ButtonStyles::restyle(|s| {
        s.label().base(|_| StyleRules { font_weight: Some(FontWeight::SemiBold), ..Default::default() });
    });
    // Filled fields read as "type here" on a busy screen.
    FieldStyles::set_default(FieldAppearance::Contained);
    // Cards lift off the page instead of being outlined.
    CardStyles::set_default(CardVariant::Elevated);
    CardStyles::restyle(|s| {
        s.root().arm(CardVariant::Elevated, |t| StyleRules {
            background: Some(t.color.surface()),
            ..Default::default()
        });
    });
}

/// A demo of the installed brand.
pub fn app() -> Element {
    install();
    let name = signal(String::new());
    let on_name: Rc<dyn Fn(String)> = Rc::new(move |v| name.set(v));
    let create: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Xl) {
            Typography(content = "Acme", kind = typography_kind::H1)
            Card(padding = CardPadding::Md) {
                Stack(gap = StackGap::Md) {
                    Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                        Typography(content = "New project", kind = typography_kind::H3)
                        Badge(label = "Starter", tone = tone::Primary, variant = variant::Soft)
                    }
                    Field(label = Some("Project name".to_string()), value = name, on_change = on_name)
                    Typography(content = "Your brand drives every surface on this card.", muted = true)
                    Button(label = "Create project", on_click = create)
                }
            }
        }
    }
}
