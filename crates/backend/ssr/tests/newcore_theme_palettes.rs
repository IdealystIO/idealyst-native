//! Static renders must paint the READER's theme on the first frame.
//!
//! A server-rendered page is written once, before the reader's system
//! preference or stored choice exists, so publishing only the active
//! palette guarantees a wrong first paint for half the audience: the
//! document ships light `:root` values, the browser paints them, and the
//! theme only corrects once the wasm bundle boots and re-publishes
//! tokens. That is the white-flash-then-dark most SSG sites exhibit.
//!
//! `install_theme_palettes` closes it by declaring EVERY palette, which
//! SSR turns into `prefers-color-scheme` rules (the zero-JavaScript
//! default) plus `[data-theme]` overrides (for an app that restores a
//! stored choice). These tests pin that emission end-to-end through a
//! real render rather than against the pure CSS helper, which is unit
//! tested in the `css` crate.

use std::rc::Rc;

use runtime_shared::{
    Color, ColorScheme, StyleApplication, StyleRules, StyleSheet, ThemePalette, TokenEntry,
    TokenValue, Tokenized,
};
use runtime_vocabulary::builders::{text, view};

/// A node styled through a registered STYLESHEET. The theme's pending
/// token/palette state reaches the backend on the first sheet
/// registration (`flush_pending_host_state`, driven by
/// `ensure_registered_with`), so a tree with no sheet emits no `:root`
/// block at all and would prove nothing.
fn styled() -> runtime_scene::Element {
    fn sheet() -> Rc<StyleSheet> {
        Rc::new(StyleSheet::new(|_vs| StyleRules {
            background: Some(Tokenized::token("color-background", Color("#FAF8F4".into()))),
            ..Default::default()
        }))
    }
    view()
        .style(StyleApplication::new(sheet()))
        .children(vec![text().content("hello").build()])
        .build()
}

fn color(name: &'static str, v: &str) -> TokenEntry {
    TokenEntry { name, value: TokenValue::Color(Color(v.into())) }
}

fn light() -> Vec<TokenEntry> {
    vec![color("color-background", "#FAF8F4"), color("color-text", "#1C1A17")]
}

fn dark() -> Vec<TokenEntry> {
    vec![color("color-background", "#171512"), color("color-text", "#F5F2EC")]
}

/// Render a page that installs the light palette as active and declares
/// both, exactly as an app's theme setup would.
fn render_themed() -> String {
    backend_ssr::newcore::render_path("/", || {
        runtime_vocabulary::glue::install_tokens(&light());
        runtime_vocabulary::glue::install_theme_palettes(&[
            ThemePalette { name: "light", scheme: Some(ColorScheme::Light), tokens: light() },
            ThemePalette { name: "dark", scheme: Some(ColorScheme::Dark), tokens: dark() },
        ]);
        styled()
    })
    .head_css
}

/// THE regression: a dark-preferring reader must receive dark values
/// from the static document itself.
#[test]
fn dark_reader_gets_dark_tokens_before_any_script_runs() {
    let head = render_themed();

    assert!(
        head.contains("@media (prefers-color-scheme: dark)"),
        "no prefers-color-scheme block ⇒ a dark reader paints the light palette \
         until the bundle boots (the flash). head_css: {head}"
    );
    let dark_block = head
        .split("@media (prefers-color-scheme: dark){")
        .nth(1)
        .expect("dark block present");
    assert!(
        dark_block.contains("--color-background:#171512"),
        "the dark block must carry the dark values; got {dark_block}"
    );
}

/// The browser paints its own canvas, form controls and scrollbars from
/// `color-scheme`. Without it a dark page still flashes a white canvas
/// no matter how correct the tokens are — the failure mode that survives
/// "I emitted the dark tokens".
#[test]
fn color_scheme_is_declared_so_browser_surfaces_match() {
    let head = render_themed();
    assert!(
        head.contains("color-scheme:light dark"),
        "both palettes declared ⇒ `color-scheme: light dark`; got {head}"
    );
}

/// The hook a stored choice needs. `:where()` keeps it specificity-inert
/// so the running app's own `:root` theme still wins after boot; the
/// bare attribute form would outrank and permanently pin it.
#[test]
fn stored_choice_hook_is_emitted_and_specificity_inert() {
    let head = render_themed();
    assert!(
        head.contains(":root:where([data-theme=\"dark\"])"),
        "an app restoring a stored choice needs an attribute hook; got {head}"
    );
    assert!(
        !head.contains(":root[data-theme"),
        "bare `:root[data-theme]` is (0,2,0) and would outrank every theme the \
         running app installs; got {head}"
    );
}

/// Apps that declare nothing must be byte-unaffected — this feature is
/// additive, and the frozen SSR corpus depends on that.
#[test]
fn app_without_palettes_emits_no_extra_css() {
    let head = backend_ssr::newcore::render_path("/", || {
        runtime_vocabulary::glue::install_tokens(&light());
        styled()
    })
    .head_css;

    assert!(head.contains(":root{--color-background:#FAF8F4"), "active tokens still emitted");
    for absent in ["prefers-color-scheme", "data-theme", "color-scheme:"] {
        assert!(
            !head.contains(absent),
            "an app that declares no palettes must emit no {absent:?} rules; got {head}"
        );
    }
}

/// Shared tokens must not be duplicated into every block: this CSS is
/// inlined into every page of the site.
#[test]
fn palette_blocks_carry_only_the_tokens_that_differ() {
    let head = backend_ssr::newcore::render_path("/", || {
        let mut light_with_shared = light();
        light_with_shared.push(TokenEntry {
            name: "spacing-md",
            value: TokenValue::Length(runtime_shared::Length::Px(16.0)),
        });
        let mut dark_with_shared = dark();
        dark_with_shared.push(TokenEntry {
            name: "spacing-md",
            value: TokenValue::Length(runtime_shared::Length::Px(16.0)),
        });
        runtime_vocabulary::glue::install_tokens(&light_with_shared);
        runtime_vocabulary::glue::install_theme_palettes(&[
            ThemePalette {
                name: "dark",
                scheme: Some(ColorScheme::Dark),
                tokens: dark_with_shared,
            },
        ]);
        styled()
    })
    .head_css;

    let dark_block = head
        .split("@media (prefers-color-scheme: dark){")
        .nth(1)
        .expect("dark block")
        .split("}}")
        .next()
        .expect("block body");
    assert!(
        !dark_block.contains("spacing-md"),
        "a token identical to the active palette must not be repeated; got {dark_block}"
    );
}

/// SSR renders the light palette, so it must SAY light. It previously
/// inherited the trait default `Auto` — every app spells the check
/// `matches!(color_scheme(), Dark)`, so the light branch was taken
/// anyway, but the backend claimed no preference was known while
/// emitting a specific one.
#[test]
fn ssr_reports_the_scheme_it_actually_renders() {
    use runtime_vocabulary::caps::AppEnvOps;
    let b = backend_ssr::SsrBackend::new();
    assert_eq!(b.color_scheme(), ColorScheme::Light);
}
