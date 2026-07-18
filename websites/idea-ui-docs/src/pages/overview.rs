//! Overview — the landing screen (the design's `D.home`).
//!
//! Unlike the component pages, the Overview is rendered **full-bleed** by
//! `shell::landing_frame` (no group overline / title / status / Usage
//! panel), so this module owns its whole layout: the hero, the stat row,
//! the Principles feature grid, the token-resolution strip, and the
//! Catalog grid. The hero CTAs and catalog cards are in-app navigation
//! `link`s (the idiomatic route-jump primitive) wrapping styled views —
//! never nested interactive `Button`s.

use runtime_core::{ui, Color, Element, IconData};
use idea_ui::{tone, typography_kind, Grid, Icon, StackGap, ToneRef, Typography};
use icons_lucide::{ARROW_RIGHT, CHECK, FOLDER, SETTINGS, STAR};

use crate::routes::{CATALOG, BUTTON_ROUTE, INTENTS_ROUTE};
use crate::styles::{
    CatCard, CatChip, CatChips, CatCount, CatGroupLabel, CatHead, CtaOutline, CtaOutlineText,
    CtaPrimary, CtaPrimaryText, FeatureBody, FeatureCard, FeatureIconBox, FeatureIconBoxTone,
    FeatureTextCol, FeatureTitle, HeroBadge, HeroBadgeText, HeroCard, HeroCtaRow, HeroDot,
    LandingPad, SectionLabel, StatCard, StatLabel, StatNumber, StatNumberTone, TokenStrip,
    TokenStripCode, TokenStripCodeAccent, TokenStripCol, TokenStripLabel,
};

// The sidebar group that holds the Overview itself — excluded from the
// Catalog grid, which only enumerates the component groups.
const LANDING_GROUP: &str = "Get started";

pub fn overview() -> Element {
    let hero = hero();
    let stats = ui! {
        Grid(columns = 4u32, gap = StackGap::Md) {
            stat("40", "Components", StatNumberTone::Primary)
            stat("7", "Intent palettes", StatNumberTone::Success)
            stat("2", "Built-in themes", StatNumberTone::Info)
            stat("1", "Token vocabulary", StatNumberTone::Warning)
        }
    };
    let features = ui! {
        Grid(columns = 2u32, gap = StackGap::Md) {
            feature(
                STAR, FeatureIconBoxTone::Primary, tone::Primary.into(),
                "Tone × variant drives color",
                "A component asks for intent-primary-solid-bg rather than a literal indigo. \
                 Tones and variants stay theme-agnostic.",
            )
            feature(
                SETTINGS, FeatureIconBoxTone::Info, tone::Info.into(),
                "One canonical token vocabulary",
                "Both sides agree on a fixed set of names. Stylesheets reference them; the active \
                 theme binds values at install time.",
            )
            feature(
                CHECK, FeatureIconBoxTone::Success, tone::Success.into(),
                "Swap a theme, re-skin everything",
                "Class names never change — only the values behind the tokens do. Flip the toggle \
                 above to watch it happen live.",
            )
            feature(
                FOLDER, FeatureIconBoxTone::Warning, tone::Warning.into(),
                "Built for design ↔ code alignment",
                "Every preview is a debugging surface: compare what is specced here against what \
                 your components actually render.",
            )
        }
    };
    let token_strip = token_strip();
    let catalog = catalog();

    ui! {
        view(style = LandingPad()) {
            hero
            stats
            text(style = SectionLabel()) { "Principles".to_string() }
            features
            text(style = SectionLabel()) { "The token model".to_string() }
            token_strip
            text(style = SectionLabel()) { "Catalog".to_string() }
            catalog
        }
    }
}

// ---- Hero --------------------------------------------------------------

fn hero() -> Element {
    ui! {
        view(style = HeroCard()) {
            view(style = HeroBadge()) {
                view(style = HeroDot()) {}
                text(style = HeroBadgeText()) { "idea-ui · v0.1.0".to_string() }
            }
            Typography(
                content = "The idea-ui component library".to_string(),
                kind = typography_kind::Display,
            )
            Typography(
                content = "A token-driven UI kit where every component composes from a shared, \
                    swappable design vocabulary. This reference keeps the design and the \
                    implementation honest with each other.".to_string(),
                kind = typography_kind::BodyXl,
                muted = true,
            )
            view(style = HeroCtaRow()) {
                link(route = &BUTTON_ROUTE, params = ()) {
                    view(style = CtaPrimary()) {
                        text(style = CtaPrimaryText()) { "Browse components".to_string() }
                        Icon(data = ARROW_RIGHT, size = 18.0, color = Some(Color("#ffffff".into())))
                    }
                }
                link(route = &INTENTS_ROUTE, params = ()) {
                    view(style = CtaOutline()) {
                        Icon(data = STAR, size = 18.0, tone = Some(tone::Neutral.into()))
                        text(style = CtaOutlineText()) { "View the tokens".to_string() }
                    }
                }
            }
        }
    }
}

// ---- Stat card ---------------------------------------------------------

fn stat(num: &str, label: &str, tone: StatNumberTone) -> Element {
    let num = num.to_string();
    let label = label.to_string();
    ui! {
        view(style = StatCard()) {
            text(style = StatNumber().tone(tone)) { num }
            text(style = StatLabel()) { label }
        }
    }
}

// ---- Principle feature card --------------------------------------------

fn feature(
    icon: IconData,
    box_tone: FeatureIconBoxTone,
    icon_tone: ToneRef,
    title: &str,
    body: &str,
) -> Element {
    let title = title.to_string();
    let body = body.to_string();
    ui! {
        view(style = FeatureCard()) {
            view(style = FeatureIconBox().tone(box_tone)) {
                Icon(data = icon, size = 19.0, tone = Some(icon_tone))
            }
            view(style = FeatureTextCol()) {
                text(style = FeatureTitle()) { title }
                text(style = FeatureBody()) { body }
            }
        }
    }
}

// ---- Token-resolution strip --------------------------------------------

fn token_strip() -> Element {
    ui! {
        view(style = TokenStrip()) {
            view(style = TokenStripCol()) {
                text(style = TokenStripLabel()) { "How a value resolves".to_string() }
                text(style = TokenStripCode()) { "theme field → TokenEntry { name } → registry".to_string() }
                text(style = TokenStripCode()) { "stylesheet → token(\"intent-primary-solid-bg\")".to_string() }
                text(style = TokenStripCodeAccent()) { "set_idea_theme(dark) // rebinds, no reclass".to_string() }
            }
            link(route = &INTENTS_ROUTE, params = ()) {
                view(style = CtaPrimary()) {
                    text(style = CtaPrimaryText()) { "Explore intents".to_string() }
                    Icon(data = ARROW_RIGHT, size = 18.0, color = Some(Color("#ffffff".into())))
                }
            }
        }
    }
}

// ---- Catalog grid ------------------------------------------------------

fn catalog() -> Element {
    // One card per component group (the landing's own "Get started" group
    // is excluded). Each card navigates to the group's first entry.
    let cards: Vec<Element> = CATALOG
        .iter()
        .filter(|g| g.label != LANDING_GROUP)
        .map(catalog_card)
        .collect();
    ui! {
        Grid(columns = 2u32, gap = StackGap::Md) { cards }
    }
}

fn catalog_card(group: &'static crate::routes::Group) -> Element {
    let first = group.entries[0].route;
    let label = group.label.to_string();
    let count = group.entries.len().to_string();
    let chips: Vec<Element> = group
        .entries
        .iter()
        .map(|e| {
            let name = e.name.to_string();
            ui! { text(style = CatChip()) { name } }
        })
        .collect();
    ui! {
        link(route = first, params = ()) {
            view(style = CatCard()) {
                view(style = CatHead()) {
                    text(style = CatGroupLabel()) { label }
                    text(style = CatCount()) { count }
                }
                view(style = CatChips()) { chips }
            }
        }
    }
}
