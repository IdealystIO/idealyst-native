//! Frozen-golden corpus: the SSR backend's output must match the
//! **frozen bytes the OLD core produced** for the same logical app,
//! exactly — `html` AND `head_css`, with **zero normalization**.
//!
//! This is the hydration acceptance gate for the port: the web backend's
//! adopt-mode boot (`backend-web`'s `newcore_hydrate`) walks SSR DOM
//! cursor-style in creation order and adopted old-core SSR output, so
//! byte-identical output adopts identically. Any divergence found here is
//! a hydration bug, not a formatting nit; do NOT normalize it away, and
//! do NOT re-freeze to make it pass (`IDEALYST_FREEZE_GOLDENS=1` can now
//! only RE-BASELINE against the current renderer, permanently discarding
//! the old core's testimony — see `tests/goldens/README.md`).
//!
//! Corpus (mirrors scene-parity's paired scenarios, serialized to HTML
//! instead of op streams): static kitchen sink, sheet/token styling with
//! variants + state overlays, dyn branches (both committed initial
//! states), keyed lists, styled-text runs, a swap navigator with author
//! chrome rendered at two paths, the SSG crawl, and the default-font
//! fill contract.

use std::rc::Rc;

use backend_ssr::RenderedPage;
use runtime_shared::{
    Color, SafeAreaSides, StyleApplication, StyleRules, StyleSheet, TextRun, TokenEntry,
    TokenValue, Tokenized,
};

// ===========================================================================
// Harness
// ===========================================================================

fn render_new(
    path: &str,
    build: impl FnOnce() -> runtime_scene::Element,
) -> RenderedPage {
    backend_ssr::newcore::render_path(path, build)
}

// ---------------------------------------------------------------------------
// Frozen-artifact gate
// ---------------------------------------------------------------------------

fn goldens() -> parity_goldens::Goldens {
    parity_goldens::Goldens::new(env!("CARGO_MANIFEST_DIR"))
}

/// Corpus item names carry route separators (`swap_navigator@/about`), so
/// flatten them into one filename stem.
fn stem(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        out.push(if ch.is_ascii_alphanumeric() { ch } else { '_' });
    }
    out.trim_matches('_').to_string()
}

/// The gate: the rendered page must match the frozen old-core bytes
/// exactly (html + head_css), with no normalization. This is the
/// hydration-compatibility proof, so any difference is a bug.
fn assert_matches_frozen(name: &str, page: &RenderedPage) {
    let stem = stem(name);
    goldens().check_text(&format!("{stem}.html"), &page.html);
    goldens().check_text(&format!("{stem}.head.css"), &page.head_css);
}

// ===========================================================================
// Shared fixtures (the scene-parity shapes, local so this crate stays
// independent of the dev crate)
// ===========================================================================

fn test_rules(width: f32, background: &str) -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(runtime_shared::Length::Px(width))),
        background: Some(Tokenized::Literal(Color(background.to_string()))),
        ..Default::default()
    }
}

fn surface_token(value: &str) -> TokenEntry {
    TokenEntry {
        name: "color-surface",
        value: TokenValue::Color(Color(value.to_string())),
    }
}

/// A `stylesheet!`-shaped sheet: token-referencing base + a `size`
/// variant axis with a defaulted arm.
fn themed_sheet() -> Rc<StyleSheet> {
    fn base() -> StyleRules {
        StyleRules {
            width: Some(Tokenized::Literal(runtime_shared::Length::Px(100.0))),
            background: Some(Tokenized::token("color-surface", Color("#000".into()))),
            ..Default::default()
        }
    }
    Rc::new(
        StyleSheet::new(|_vs| base())
            .variant("size", "large", |_vs| StyleRules {
                width: Some(Tokenized::Literal(runtime_shared::Length::Px(400.0))),
                ..Default::default()
            })
            .variant("size", "medium", |_vs| StyleRules::default())
            .variant_default("size", "medium"),
    )
}

/// A sheet with a `state hovered { … }` overlay (the macro's reserved
/// `__state_hovered` axis) — exercises the `apply_styled_states` path,
/// which SSR lowers to a `:hover` pseudo rule in `head_css`.
fn hover_sheet() -> Rc<StyleSheet> {
    Rc::new(
        StyleSheet::new(|_vs| StyleRules {
            width: Some(Tokenized::Literal(runtime_shared::Length::Px(100.0))),
            ..Default::default()
        })
        .variant("__state_hovered", "on", |_vs| StyleRules {
            width: Some(Tokenized::Literal(runtime_shared::Length::Px(999.0))),
            ..Default::default()
        }),
    )
}

const TEST_ICON: runtime_shared::primitives::icon::IconData =
    runtime_shared::primitives::icon::IconData {
        view_box: (24, 24),
        paths: &["M0 0h24v24H0z"],
        fill_rule: runtime_shared::primitives::icon::FillRule::NonZero,
        filled: false,
    };

// ===========================================================================
// 1. Static kitchen sink — every core primitive, static props
// ===========================================================================

fn kitchen_new() -> runtime_scene::Element {
    use runtime_vocabulary::builders::{
        activity_indicator, button, icon, image, link, pressable, scroll_view, slider, text,
        text_area, text_input, toggle, view,
    };
    use runtime_world::signal;
    let toggle_on = signal(true);
    let slider_val = signal(2.5f32);
    let input_val = signal(String::from("hi"));
    let area_val = signal(String::from("hello\nworld"));
    view()
        .style(test_rules(120.0, "#112233"))
        .child(text().content("hello"))
        .child(text().runs(vec![TextRun::plain("styled"), TextRun::plain(" runs")]))
        .child(text().content("leaf").style(test_rules(80.0, "#445566")))
        .child(button().label("Go").on_press(|| {}))
        .child(pressable(|| {}).child(text().content("press me")))
        .child(image().src("logo.png").alt("Logo"))
        .child(icon().data(TEST_ICON))
        .child(toggle().value(toggle_on).on_change(|_| {}))
        .child(
            slider()
                .value(slider_val)
                .on_change(|_| {})
                .range(0.0, 10.0)
                .step(0.5),
        )
        .child(activity_indicator())
        .child(
            link()
                .url("https://example.com")
                .external(true)
                .child(text().content("docs")),
        )
        .child(
            scroll_view()
                .safe_area(SafeAreaSides::TOP)
                .child(text().content("scrollable")),
        )
        .child(
            text_input()
                .value(input_val)
                .on_change(|_| {})
                .placeholder("Type here"),
        )
        .child(
            text_area()
                .value(area_val)
                .on_change(|_| {})
                .placeholder("notes")
                .min_rows(2)
                .max_rows(6),
        )
        .build()
}

#[test]
fn corpus_static_kitchen_sink() {
    let new = render_new("/", kitchen_new);
    assert_matches_frozen("static_kitchen_sink", &new);
    // Sanity: the corpus item actually rendered something substantial.
    assert!(new.html.contains("<button>Go</button>"), "corpus is live: {}", new.html);
}

// ===========================================================================
// 2. Sheet + token styling: install_tokens, variant selection, hover
//    state overlay (the stylesheet!-equivalent surface without the macro)
// ===========================================================================

#[test]
fn corpus_styled_sheet_tokens_and_state_overlay() {
    let new = render_new("/", || {
        use runtime_vocabulary::builders::{text, view};
        runtime_vocabulary::theme::install_tokens(&[surface_token("#101010")]);
        view()
            .child(view().style(StyleApplication::new(themed_sheet())))
            .child(view().style(StyleApplication::new(themed_sheet()).with("size", "large")))
            .child(view().style(StyleApplication::new(hover_sheet())))
            .child(text().content("themed"))
            .build()
    });
    assert_matches_frozen("styled_sheet_tokens", &new);
    // The token block and the hover overlay actually made it to <head>.
    assert!(new.head_css.contains("--color-surface:#101010;"), "tokens: {}", new.head_css);
    assert!(new.head_css.contains(":hover{"), "hover overlay: {}", new.head_css);
}

// ===========================================================================
// 3. Dyn branch — committed initial state, both branches
// ===========================================================================

fn dyn_new(initial: bool) -> runtime_scene::Element {
    use runtime_scene::dyn_keyed;
    use runtime_vocabulary::builders::{text, view};
    use runtime_world::signal;
    let show = signal(initial);
    view()
        .child(text().content("before"))
        .child(dyn_keyed(
            move || show.get(),
            move |&on| {
                if on {
                    view()
                        .style(test_rules(60.0, "#606060"))
                        .child(text().content("shown"))
                        .build()
                } else {
                    text().content("hidden").build()
                }
            },
        ))
        .child(text().content("after"))
        .build()
}

#[test]
fn corpus_dyn_branch_both_initial_states() {
    for initial in [true, false] {
        let new = render_new("/", move || dyn_new(initial));
        assert_matches_frozen(
            if initial { "dyn_branch_then" } else { "dyn_branch_else" },
            &new,
        );
        let want = if initial { "shown" } else { "hidden" };
        assert!(new.html.contains(want), "committed initial branch rendered: {}", new.html);
    }
}

// ===========================================================================
// 4. Keyed list — header + reactive keyed rows + footer
// ===========================================================================

#[test]
fn corpus_keyed_list() {
    let new = render_new("/", || {
        use runtime_scene::keyed;
        use runtime_vocabulary::builders::{text, view};
        use runtime_world::signal;
        let items = signal(vec![1u32, 2, 3]);
        view()
            .child(text().content("header"))
            .child(keyed(
                move || items.get(),
                |n| *n,
                |n| text().content(format!("row-{n}")).build(),
            ))
            .child(text().content("footer"))
            .build()
    });
    assert_matches_frozen("keyed_list", &new);
    assert!(new.html.contains("row-1") && new.html.contains("row-3"), "rows: {}", new.html);
}

// ===========================================================================
// 5. Styled-text runs — per-run inline styles with token colors
// ===========================================================================

#[test]
fn corpus_styled_text_runs() {
    fn runs() -> Vec<TextRun> {
        use runtime_shared::TextRunStyle;
        vec![
            TextRun::plain("the "),
            TextRun::styled(
                "ui!",
                TextRunStyle {
                    font_family: Some(runtime_shared::FontFamily::System(
                        "ui-monospace, monospace".into(),
                    )),
                    background: Some(Tokenized::token("color-surface-alt", Color("#eee".into()))),
                    ..Default::default()
                },
            ),
            TextRun::plain(" macro"),
        ]
    }
    let new = render_new("/", || {
        use runtime_vocabulary::builders::{text, view};
        view().child(text().runs(runs())).build()
    });
    assert_matches_frozen("styled_text_runs", &new);
    assert!(
        new.html.contains("var(--color-surface-alt, #eee)"),
        "token-colored run style inlined: {}",
        new.html
    );
}

/// Underline + italic runs — the deltas a `code_editor` decoration
/// lowers to. Pins the LONGHAND emission: `text-decoration` as a
/// shorthand would reset `text-decoration-color` to `currentColor` and
/// silently drop a diagnostic's red mark under blue syntax-colored
/// text.
#[test]
fn corpus_styled_text_underline_runs() {
    fn runs() -> Vec<TextRun> {
        use runtime_shared::{FontStyle, RunUnderline, TextRunStyle, UnderlineStyle};
        vec![
            TextRun::plain("let "),
            TextRun::styled(
                "x",
                TextRunStyle {
                    color: Some(Tokenized::Literal(Color("#00f".into()))),
                    underline: Some(RunUnderline {
                        style: UnderlineStyle::Dotted,
                        color: Some(Tokenized::Literal(Color("#c00".into()))),
                    }),
                    ..Default::default()
                },
            ),
            TextRun::styled(
                " // note",
                TextRunStyle {
                    font_style: Some(FontStyle::Italic),
                    ..Default::default()
                },
            ),
        ]
    }
    let new = render_new("/", || {
        use runtime_vocabulary::builders::{text, view};
        view().child(text().runs(runs())).build()
    });
    // No frozen golden: underline/italic runs postdate the old core, so
    // there is no byte-identity artifact to compare against (see
    // tests/goldens/README.md — these files cannot be re-derived). The
    // assertions below are the fence instead.
    assert!(
        new.html.contains("text-decoration-style: dotted")
            && new.html.contains("text-decoration-color: #c00"),
        "the underline's own pattern and color must survive to the span: {}",
        new.html
    );
    assert!(new.html.contains("font-style: italic"), "italic run: {}", new.html);
}

// ===========================================================================
// 6. Swap navigator with author chrome, rendered at two paths
// ===========================================================================

use runtime_shared::primitives::navigator::Route;

const NAV_HOME: Route<()> = Route::new("home", "/");
const NAV_ABOUT: Route<()> = Route::new("about", "/about");

fn nav_new() -> runtime_scene::Element {
    use runtime_vocabulary::builders::{navigator_outlet, swap_navigator, text, view};
    swap_navigator(&NAV_HOME)
        .screen(NAV_HOME, |_| {
            view()
                .style(test_rules(10.0, "#111111"))
                .child(text().content("home"))
                .build()
        })
        .screen(NAV_ABOUT, |_| {
            view()
                .style(test_rules(20.0, "#222222"))
                .child(text().content("about"))
                .build()
        })
        .layout(|| {
            view()
                .child(text().content("chrome"))
                .child(navigator_outlet())
                .build()
        })
        .build()
}

#[test]
fn corpus_swap_navigator_at_path() {
    for (path, want) in [("/", "home"), ("/about", "about")] {
        let new = render_new(path, nav_new);
        assert_matches_frozen(&format!("swap_navigator@{path}"), &new);
        assert!(
            new.html.contains(want) && new.html.contains("chrome"),
            "path-matched screen + chrome rendered at {path}: {}",
            new.html
        );
    }
}

// ===========================================================================
// 7. SSG crawl (`render_all` old ↔ `newcore::render_all` new): route
//    discovery through the shared collector + per-page byte identity
// ===========================================================================

const NAV_DOCS: Route<()> = Route::new("docs", "/docs");
/// Parameterized pattern — the crawl must SKIP it (no param values),
/// reporting it in `skipped_parameterized` on both cores.
const NAV_ITEM: Route<()> = Route::new("item", "/item/:id");

fn crawl_new() -> runtime_scene::Element {
    use runtime_vocabulary::builders::{navigator_outlet, swap_navigator, text, view};
    swap_navigator(&NAV_HOME)
        .screen(NAV_HOME, |_| view().child(text().content("home")).build())
        .screen(NAV_ABOUT, |_| view().child(text().content("about")).build())
        .screen(NAV_DOCS, |_| view().child(text().content("docs")).build())
        .screen(NAV_ITEM, |_| view().child(text().content("item")).build())
        .layout(|| {
            view()
                .child(text().content("chrome"))
                .child(navigator_outlet())
                .build()
        })
        .build()
}

/// The new-core SSG crawl mirrors the old one end-to-end: same route
/// discovery (the vocabulary navigator mounts publish their screen
/// paths into the SAME collector `dispatch_navigator` feeds), same
/// parameterized-skip behavior, and byte-identical rendered pages.
#[test]
fn corpus_render_all_crawl_discovers_and_matches() {
    let new = backend_ssr::newcore::render_all(|_| {}, crawl_new);

    let mut new_routes: Vec<&str> = new.pages.keys().map(|s| s.as_str()).collect();
    new_routes.sort_unstable();
    assert_eq!(
        new_routes,
        vec!["/", "/about", "/docs"],
        "the crawl discovers exactly the literal routes"
    );
    assert_eq!(
        new.skipped_parameterized,
        vec!["/item/:id"],
        "parameterized patterns are skipped, not rendered"
    );

    for route in new_routes {
        let new_page = &new.pages[route];
        assert_matches_frozen(&format!("render_all@{route}"), new_page);
        assert!(!new_page.html.is_empty(), "crawled page {route} rendered");
    }
}

// ===========================================================================
// 8. Default-text-font fill contract: STATIC applications fold the
//    theme default into font-less rules; DYNAMIC (reactive) ones don't
// ===========================================================================

/// Pins the old walker's asymmetric fill contract on BOTH cores —
/// `apply_one` (static) runs `with_default_text_font`, while
/// `attach_style_reactive` never does (reactive nodes ride the
/// `apply_default_text_font` document channel). The new core's
/// `style_attach` briefly folded the default into the DYNAMIC path too,
/// minting class hashes old-core SSR never mints — which broke website
/// SSG byte-parity site-wide (every reactive-styled node hashed
/// differently). Byte-comparing a static + dynamic pair with a default
/// font installed is the regression net for that whole class of drift.
///
/// AMENDED GOLDEN (the one deliberate re-baseline in this corpus, see
/// `goldens/README.md`): `default_font_fill.head.css` now carries the
/// `:root` default-font block. The old core never emitted it because
/// the document publication was gated on premint use — which was
/// itself the bug: the "document channel" the dynamic path rides was
/// switched off on live builds, so reactive nodes rendered in the
/// browser serif. The `.html` golden is untouched old-core bytes (no
/// class hash moved), and the `folds == 2` assertion below pins the
/// fill asymmetry independent of the frozen artifact.
#[test]
fn corpus_default_font_fill_static_folds_dynamic_does_not() {
    let test_font = || runtime_shared::FontFamily::System("TestFont, serif".to_string());

    let new = render_new("/", move || {
        use runtime_vocabulary::builders::view;
        runtime_vocabulary::theme::set_default_text_font(Some(test_font()));
        let sheet = themed_sheet();
        let sheet_for_dyn = sheet.clone();
        view()
            .child(view().style(test_rules(40.0, "#123123")))
            .child(view().style(StyleApplication::new(sheet)))
            .child(view().style(move || StyleApplication::new(sheet_for_dyn.clone())))
            .build()
    });

    assert_matches_frozen("default_font_fill", &new);
    // Sanity on the contract itself (not just old==new): the static
    // nodes' rules carry the folded font, the dynamic node's rule
    // doesn't. Count the FOLDED declaration form — the `:root` document
    // publication also names the font, but as the variable's value
    // (`--iy-default-font: TestFont, serif`) with `font-family:
    // var(--iy-default-font)`, deliberately not a fold.
    let folds = new.head_css.matches("font-family: TestFont, serif").count();
    assert_eq!(
        folds, 2,
        "exactly the two STATIC applications fold the default font: {}",
        new.head_css
    );
    // And the document publication is present for the dynamic node to
    // inherit — the amended half of the golden.
    assert!(
        new.head_css.contains("font-family: var(--iy-default-font)"),
        "the :root block must declare the inheritable font-family: {}",
        new.head_css
    );
}
