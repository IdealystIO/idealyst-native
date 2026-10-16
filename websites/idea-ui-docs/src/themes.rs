//! The docs' three themes — Light, Dark, and Cyber — and the header
//! toggle's choice between them.
//!
//! Light and Dark are idea-ui's built-ins. Cyber is this site's own: a
//! neon-on-black skin built from `dark_theme()` that sets every group a
//! theme owns — neutrals, intents, effects (hover glows, a tinted resting
//! shadow, the accent gradient), radius, type scale, and font — so the
//! component pages double as a demonstration of how far a theme alone
//! reaches. No component or stylesheet branches on it: the glows and the
//! gradient are token colors every theme defines (transparent glows in the
//! built-ins), which is what lets one build-time stylesheet serve all three.

use idea_ui::{dark_theme, light_theme, IdeaThemeDefaults, IntentColors, Tokenized};
use runtime_core::{Color, FontFamily};

/// Which theme the docs show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DocTheme {
    #[default]
    Light,
    Dark,
    Cyber,
}

impl DocTheme {
    /// Every theme, in toggle order.
    pub const ALL: [DocTheme; 3] = [DocTheme::Light, DocTheme::Dark, DocTheme::Cyber];

    /// The toggle label.
    pub fn label(self) -> &'static str {
        match self {
            DocTheme::Light => "Light",
            DocTheme::Dark => "Dark",
            DocTheme::Cyber => "Cyber",
        }
    }

    /// The idea theme to install for this choice.
    pub fn theme(self) -> IdeaThemeDefaults {
        match self {
            DocTheme::Light => light_theme(),
            DocTheme::Dark => dark_theme(),
            DocTheme::Cyber => cyber_theme(),
        }
    }
}

fn c(value: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(value.into()))
}

/// One intent block, slots in `IntentColors` order.
fn intent(solid_bg: &str, solid_text: &str, soft_bg: &str, soft_text: &str, fg: &str, border: &str) -> IntentColors {
    IntentColors {
        solid_bg: c(solid_bg),
        solid_text: c(solid_text),
        soft_bg: c(soft_bg),
        soft_text: c(soft_text),
        fg: c(fg),
        border: c(border),
    }
}

/// The Cyber theme: electric violet primary, electric blue secondary,
/// cyan info, acid green, toxic yellow, on a near-black violet ground, with
/// magenta kept to the accent gradient's first stop (a magenta primary fill
/// was too harsh to look at). Neon fills carry dark text
/// (white on a neon fill fails contrast); every text/fill pair meets WCAG AA
/// (pinned by `cyber_text_pairs_meet_wcag_aa`).
///
/// Values are plain literals: installing wraps the theme in `IdeaThemeRef`,
/// which keys each field under its canonical token name, so this needs no
/// token names of its own.
pub fn cyber_theme() -> IdeaThemeDefaults {
    let mut t = dark_theme();

    let colors = &mut t.colors;
    colors.background = c("#07060f");
    colors.surface = c("#0e0b1d");
    colors.surface_alt = c("#161230");
    colors.text = c("#e8fbff");
    colors.text_muted = c("#9497c9");
    colors.text_inverse = c("#07060f");
    colors.border = c("#2b2250");
    // Borders light up violet under the pointer.
    colors.border_hover = c("#a259ff");
    colors.border_strong = c("#5d4aa8");
    colors.focus_ring = c("#00f0ff");
    colors.overlay = c("rgba(3, 2, 10, 0.78)");
    colors.table_header = c("#161230");
    // Row tones stay opaque (a pinned column slides over them): the
    // intent tints precomposited over `surface`.
    colors.table_row_highlight = c("#1f1238");
    colors.table_row_highlight_hover = c("#2b1650");
    colors.table_row_warning = c("#24260f");
    colors.table_row_warning_hover = c("#323512");
    colors.table_row_danger = c("#2c0f1d");
    colors.table_row_danger_hover = c("#3f1427");

    let i = &mut t.intents;
    i.primary = intent(
        "#a259ff", "#0e0019",
        "rgba(162, 89, 255, 0.16)", "#d4b0ff",
        "#b98aff", "rgba(162, 89, 255, 0.50)",
    );
    i.secondary = intent(
        "#4d8dff", "#00081f",
        "rgba(77, 141, 255, 0.16)", "#a8c8ff",
        "#79a8ff", "rgba(77, 141, 255, 0.50)",
    );
    i.neutral = intent(
        "#2b2250", "#e8fbff",
        "rgba(148, 151, 201, 0.14)", "#d3d5f5",
        "#bfc1e8", "rgba(148, 151, 201, 0.32)",
    );
    i.success = intent(
        "#39ff14", "#021400",
        "rgba(57, 255, 20, 0.12)", "#a3ff8f",
        "#5dff3f", "rgba(57, 255, 20, 0.45)",
    );
    i.danger = intent(
        "#ff3864", "#1a0007",
        "rgba(255, 56, 100, 0.14)", "#ffa3b7",
        "#ff6688", "rgba(255, 56, 100, 0.50)",
    );
    i.warning = intent(
        "#f5ff00", "#191a00",
        "rgba(245, 255, 0, 0.12)", "#f9ff8c",
        "#f2ff47", "rgba(245, 255, 0, 0.45)",
    );
    i.info = intent(
        "#00f0ff", "#001b1d",
        "rgba(0, 240, 255, 0.12)", "#8cf8ff",
        "#3df3ff", "rgba(0, 240, 255, 0.50)",
    );

    let e = &mut t.effects;
    // Raised surfaces cast violet instead of black.
    e.shadow = c("rgba(162, 89, 255, 0.30)");
    e.glow_primary = c("rgba(162, 89, 255, 0.95)");
    e.glow_secondary = c("rgba(77, 141, 255, 0.90)");
    e.glow_neutral = c("rgba(0, 240, 255, 0.60)");
    e.glow_success = c("rgba(57, 255, 20, 0.85)");
    e.glow_danger = c("rgba(255, 56, 100, 0.90)");
    e.glow_warning = c("rgba(245, 255, 0, 0.80)");
    e.glow_info = c("rgba(0, 240, 255, 0.90)");
    e.gradient_start = c("#c04dff");
    e.gradient_mid = c("#7b61ff");
    e.gradient_end = c("#00f0ff");
    e.gradient_text = c("#07060f");

    // Hard edges.
    t.radius.sm = 2.0;
    t.radius.md = 3.0;
    t.radius.lg = 4.0;

    // A terminal face. A system stack, so it needs no font asset on any
    // backend: the first family the platform has wins.
    t.font = FontFamily::System(
        "\"JetBrains Mono\", \"SF Mono\", ui-monospace, Menlo, Consolas, \"Liberation Mono\", monospace"
            .to_string(),
    );
    // Monospace runs wide; pull the body sizes in a step.
    t.typography.body_size = 13.0;
    t.typography.body_sm_size = 12.0;
    t.typography.body_lg_size = 15.0;

    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::color::{parse, Rgba};

    fn rgba(t: &Tokenized<Color>) -> Rgba {
        parse(&t.value().0).unwrap_or_else(|e| panic!("{:?}: {e:?}", t.value().0))
    }

    /// `fg` alpha-composited over an opaque `bg`.
    fn over(fg: Rgba, bg: Rgba) -> Rgba {
        let a = f64::from(fg.a) / 255.0;
        let mix = |f: u8, b: u8| (f64::from(f) * a + f64::from(b) * (1.0 - a)).round() as u8;
        Rgba::new(mix(fg.r, bg.r), mix(fg.g, bg.g), mix(fg.b, bg.b), 255)
    }

    fn contrast(a: Rgba, b: Rgba) -> f64 {
        let lum = |c: Rgba| {
            let lin = |v: u8| {
                let s = f64::from(v) / 255.0;
                if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
        };
        let (la, lb) = (lum(a), lum(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// Every text/fill pair the Cyber theme hands a component meets WCAG AA
    /// (4.5:1): body text on the page and surfaces, each intent's solid and
    /// soft pairs (soft fills composited over `surface`, which is what the
    /// eye sees), and the gradient text on all three stops.
    #[test]
    fn cyber_text_pairs_meet_wcag_aa() {
        let t = cyber_theme();
        let surface = rgba(&t.colors.surface);
        let mut pairs: Vec<(String, Rgba, Rgba)> = vec![
            ("text on background".into(), rgba(&t.colors.text), rgba(&t.colors.background)),
            ("text on surface".into(), rgba(&t.colors.text), surface),
            ("muted on surface".into(), rgba(&t.colors.text_muted), surface),
            ("muted on surface_alt".into(), rgba(&t.colors.text_muted), rgba(&t.colors.surface_alt)),
        ];
        let i = &t.intents;
        for (name, block) in [
            ("primary", &i.primary), ("secondary", &i.secondary), ("neutral", &i.neutral),
            ("success", &i.success), ("danger", &i.danger), ("warning", &i.warning), ("info", &i.info),
        ] {
            pairs.push((format!("{name} solid"), rgba(&block.solid_text), rgba(&block.solid_bg)));
            let soft = over(rgba(&block.soft_bg), surface);
            pairs.push((format!("{name} soft"), over(rgba(&block.soft_text), soft), soft));
            pairs.push((format!("{name} fg on surface"), rgba(&block.fg), surface));
        }
        let e = &t.effects;
        for (stop, fill) in [("start", &e.gradient_start), ("mid", &e.gradient_mid), ("end", &e.gradient_end)] {
            pairs.push((format!("gradient text on {stop}"), rgba(&e.gradient_text), rgba(fill)));
        }
        let failures: Vec<String> = pairs
            .iter()
            .map(|(label, fg, bg)| (label, contrast(*fg, *bg)))
            .filter(|(_, r)| *r < 4.5)
            .map(|(label, r)| format!("{label}: {r:.2}"))
            .collect();
        assert!(failures.is_empty(), "below WCAG AA 4.5:1 — {failures:#?}");
    }

    /// Cyber lights the glows the built-ins leave transparent — the one
    /// difference the hover glow keys on.
    #[test]
    fn cyber_lights_every_intent_glow() {
        let e = cyber_theme().effects;
        for glow in [
            &e.glow_primary, &e.glow_secondary, &e.glow_neutral, &e.glow_success,
            &e.glow_danger, &e.glow_warning, &e.glow_info,
        ] {
            assert!(rgba(glow).a > 0, "{:?} must be visible", glow.value().0);
        }
        assert_eq!(rgba(&light_theme().effects.glow_primary).a, 0, "the built-ins stay unlit");
    }
}
