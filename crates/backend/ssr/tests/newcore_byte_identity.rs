//! Byte-identity corpus: the SAME logical app rendered through the
//! old-core SSR path ([`backend_ssr::render_path`], walker + `Element`)
//! and the new-core path ([`backend_ssr::newcore::render_path`],
//! per-request `World` + vocabulary handlers) must emit **byte-identical**
//! `html` AND `head_css`.
//!
//! This is the hydration acceptance gate for the port: the web backend's
//! adopt-mode boot (`backend-web`'s `newcore_hydrate`) walks SSR DOM
//! cursor-style in creation order and already adopts old-core SSR
//! output — byte-identical output therefore adopts identically. Any
//! divergence found here is a hydration bug, not a formatting nit; do
//! NOT normalize differences away.
//!
//! Corpus (mirrors scene-parity's paired scenarios, serialized to HTML
//! instead of op streams): static kitchen sink, sheet/token styling with
//! variants + state overlays, dyn branches (both committed initial
//! states), keyed lists, styled-text runs, and a swap navigator with
//! author chrome rendered at two paths.

#![cfg(feature = "new-core")]

use std::rc::Rc;

use backend_ssr::RenderedPage;
use runtime_core::{
    Color, SafeAreaSides, StyleApplication, StyleRules, StyleSheet, TextRun, TokenEntry,
    TokenValue, Tokenized,
};

// ===========================================================================
// Harness
// ===========================================================================

/// Old-core render with the session-wide registration dedup reset first
/// (same reset `render_all` performs per page), so every corpus item
/// starts from a clean old-core registration table regardless of test
/// ordering within a thread.
fn render_old(
    path: &str,
    app: impl FnOnce() -> runtime_core::Element,
) -> RenderedPage {
    runtime_core::reset_for_ssg_render();
    backend_ssr::render_path(path, app)
}

fn render_new(
    path: &str,
    build: impl FnOnce() -> runtime_scene::Element,
) -> RenderedPage {
    backend_ssr::newcore::render_path(path, build)
}

/// Assert exact string equality with a byte-level first-divergence
/// report (index + surrounding context from both sides) so a corpus
/// failure is directly actionable.
fn assert_bytes(name: &str, part: &str, old: &str, new: &str) {
    if old == new {
        return;
    }
    let at = old
        .bytes()
        .zip(new.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| old.len().min(new.len()));
    let lo = at.saturating_sub(60);
    let old_hi = (at + 60).min(old.len());
    let new_hi = (at + 60).min(new.len());
    panic!(
        "byte divergence in corpus item `{name}` ({part}) at byte {at}\n\
         old (len {}): …{}…\n\
         new (len {}): …{}…\n\
         full old:\n{}\n\
         full new:\n{}",
        old.len(),
        &old[lo..old_hi],
        new.len(),
        &new[lo..new_hi],
        old,
        new,
    );
}

fn assert_pages_identical(name: &str, old: &RenderedPage, new: &RenderedPage) {
    assert_bytes(name, "html", &old.html, &new.html);
    assert_bytes(name, "head_css", &old.head_css, &new.head_css);
}

// ===========================================================================
// Shared fixtures (the scene-parity shapes, local so this crate stays
// independent of the dev crate)
// ===========================================================================

fn test_rules(width: f32, background: &str) -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(runtime_core::Length::Px(width))),
        background: Some(Tokenized::Literal(Color(background.to_string()))),
        ..Default::default()
    }
}

/// Old-core route to a resolved-rules apply: literal rules in a static
/// sheet resolve to themselves, digesting identically to the new side's
/// `StyleProp::Static` (the same equivalence scene-parity relies on).
fn static_style(width: f32, background: &str) -> StyleApplication {
    StyleApplication::new(Rc::new(StyleSheet::r#static(test_rules(width, background))))
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
            width: Some(Tokenized::Literal(runtime_core::Length::Px(100.0))),
            background: Some(Tokenized::token("color-surface", Color("#000".into()))),
            ..Default::default()
        }
    }
    Rc::new(
        StyleSheet::new(|_vs| base())
            .variant("size", "large", |_vs| StyleRules {
                width: Some(Tokenized::Literal(runtime_core::Length::Px(400.0))),
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
            width: Some(Tokenized::Literal(runtime_core::Length::Px(100.0))),
            ..Default::default()
        })
        .variant("__state_hovered", "on", |_vs| StyleRules {
            width: Some(Tokenized::Literal(runtime_core::Length::Px(999.0))),
            ..Default::default()
        }),
    )
}

const TEST_ICON: runtime_core::primitives::icon::IconData =
    runtime_core::primitives::icon::IconData {
        view_box: (24, 24),
        paths: &["M0 0h24v24H0z"],
        fill_rule: runtime_core::primitives::icon::FillRule::NonZero,
        filled: false,
    };

// ===========================================================================
// 1. Static kitchen sink — every core primitive, static props
// ===========================================================================

fn kitchen_old() -> runtime_core::Element {
    use runtime_core::primitives::activity_indicator::activity_indicator;
    use runtime_core::primitives::slider::slider;
    use runtime_core::{
        button, external_link, icon, image, pressable, scroll_view, signal, styled_text, text,
        text_area, text_input, toggle, view, IntoElement,
    };
    let toggle_on = signal(true);
    let slider_val = signal(2.5f32);
    let input_val = signal(String::from("hi"));
    let area_val = signal(String::from("hello\nworld"));
    view(vec![
        text("hello").into_element(),
        styled_text(vec![TextRun::plain("styled"), TextRun::plain(" runs")]).into_element(),
        text("leaf")
            .with_style(static_style(80.0, "#445566"))
            .into_element(),
        button("Go", || {}).into_element(),
        pressable(vec![text("press me").into_element()], || {}).into_element(),
        image("logo.png").alt("Logo".to_string()).into_element(),
        icon(TEST_ICON).into_element(),
        toggle(toggle_on, |_| {}).into_element(),
        slider(slider_val, |_| {})
            .range(0.0, 10.0)
            .step(0.5)
            .into_element(),
        activity_indicator().into_element(),
        external_link("https://example.com", vec![text("docs").into_element()]).into_element(),
        scroll_view(vec![text("scrollable").into_element()])
            .safe_area(SafeAreaSides::TOP)
            .into_element(),
        text_input(input_val, |_| {})
            .placeholder("Type here".to_string())
            .into_element(),
        text_area(area_val, |_| {})
            .placeholder("notes".to_string())
            .min_rows(2)
            .max_rows(6)
            .into_element(),
    ])
    .with_style(static_style(120.0, "#112233"))
    .into_element()
}

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
    let old = render_old("/", kitchen_old);
    let new = render_new("/", kitchen_new);
    assert_pages_identical("static_kitchen_sink", &old, &new);
    // Sanity: the corpus item actually rendered something substantial.
    assert!(old.html.contains("<button>Go</button>"), "corpus is live: {}", old.html);
}

// ===========================================================================
// 2. Sheet + token styling: install_tokens, variant selection, hover
//    state overlay (the stylesheet!-equivalent surface without the macro)
// ===========================================================================

#[test]
fn corpus_styled_sheet_tokens_and_state_overlay() {
    let old = render_old("/", || {
        use runtime_core::{text, view, IntoElement};
        runtime_core::install_tokens(&[surface_token("#101010")]);
        view(vec![
            view(vec![])
                .with_style(StyleApplication::new(themed_sheet()))
                .into_element(),
            view(vec![])
                .with_style(StyleApplication::new(themed_sheet()).with("size", "large"))
                .into_element(),
            view(vec![])
                .with_style(StyleApplication::new(hover_sheet()))
                .into_element(),
            text("themed").into_element(),
        ])
        .into_element()
    });
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
    assert_pages_identical("styled_sheet_tokens", &old, &new);
    // The token block and the hover overlay actually made it to <head>.
    assert!(old.head_css.contains("--color-surface:#101010;"), "tokens: {}", old.head_css);
    assert!(old.head_css.contains(":hover{"), "hover overlay: {}", old.head_css);
}

// ===========================================================================
// 3. Dyn branch — committed initial state, both branches
// ===========================================================================

fn dyn_old(initial: bool) -> runtime_core::Element {
    use runtime_core::{signal, text, view, when, IntoElement};
    let show = signal(initial);
    view(vec![
        text("before").into_element(),
        when(
            move || show.get(),
            || {
                view(vec![text("shown").into_element()])
                    .with_style(static_style(60.0, "#606060"))
                    .into_element()
            },
            || text("hidden").into_element(),
        ),
        text("after").into_element(),
    ])
    .into_element()
}

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
        let old = render_old("/", move || dyn_old(initial));
        let new = render_new("/", move || dyn_new(initial));
        assert_pages_identical(
            if initial { "dyn_branch_then" } else { "dyn_branch_else" },
            &old,
            &new,
        );
        let want = if initial { "shown" } else { "hidden" };
        assert!(old.html.contains(want), "committed initial branch rendered: {}", old.html);
    }
}

// ===========================================================================
// 4. Keyed list — header + reactive keyed rows + footer
// ===========================================================================

#[test]
fn corpus_keyed_list() {
    let old = render_old("/", || {
        use runtime_core::{each_keyed, signal, text, view, EachKey, EachRowBuild, IntoElement};
        let items = signal(vec![1u32, 2, 3]);
        view(vec![
            text("header").into_element(),
            each_keyed(move || {
                items
                    .get()
                    .into_iter()
                    .map(|n| {
                        let build: EachRowBuild =
                            Box::new(move || vec![text(format!("row-{n}")).into_element()]);
                        (EachKey::new(n), build)
                    })
                    .collect()
            }),
            text("footer").into_element(),
        ])
        .into_element()
    });
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
    assert_pages_identical("keyed_list", &old, &new);
    assert!(old.html.contains("row-1") && old.html.contains("row-3"), "rows: {}", old.html);
}

// ===========================================================================
// 5. Styled-text runs — per-run inline styles with token colors
// ===========================================================================

#[test]
fn corpus_styled_text_runs() {
    fn runs() -> Vec<TextRun> {
        use runtime_core::TextRunStyle;
        vec![
            TextRun::plain("the "),
            TextRun::styled(
                "ui!",
                TextRunStyle {
                    font_family: Some(runtime_core::FontFamily::System(
                        "ui-monospace, monospace".into(),
                    )),
                    background: Some(Tokenized::token("color-surface-alt", Color("#eee".into()))),
                    ..Default::default()
                },
            ),
            TextRun::plain(" macro"),
        ]
    }
    let old = render_old("/", || {
        use runtime_core::{styled_text, view, IntoElement};
        view(vec![styled_text(runs()).into_element()]).into_element()
    });
    let new = render_new("/", || {
        use runtime_vocabulary::builders::{text, view};
        view().child(text().runs(runs())).build()
    });
    assert_pages_identical("styled_text_runs", &old, &new);
    assert!(
        old.html.contains("var(--color-surface-alt, #eee)"),
        "token-colored run style inlined: {}",
        old.html
    );
}

// ===========================================================================
// 6. Swap navigator with author chrome, rendered at two paths
// ===========================================================================

use runtime_core::primitives::navigator::Route;

const NAV_HOME: Route<()> = Route::new("home", "/");
const NAV_ABOUT: Route<()> = Route::new("about", "/about");

fn nav_old() -> runtime_core::Element {
    use runtime_core::{text, view, IntoElement};
    use swap_navigator::{SwapBuilder, SwapNavigator};
    SwapNavigator::new(&NAV_HOME)
        .screen(NAV_HOME, |_| {
            view(vec![text("home").into_element()])
                .with_style(static_style(10.0, "#111111"))
                .into_element()
        })
        .screen(NAV_ABOUT, |_| {
            view(vec![text("about").into_element()])
                .with_style(static_style(20.0, "#222222"))
                .into_element()
        })
        .layout(|ctx| {
            view(vec![text("chrome").into_element(), ctx.outlet]).into_element()
        })
        .into_element()
}

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
        let old = backend_ssr::render_path_with(
            path,
            |b| {
                runtime_core::reset_for_ssg_render();
                swap_navigator::register_generic(b);
            },
            nav_old,
        );
        let new = render_new(path, nav_new);
        assert_pages_identical(&format!("swap_navigator@{path}"), &old, &new);
        assert!(
            old.html.contains(want) && old.html.contains("chrome"),
            "path-matched screen + chrome rendered at {path}: {}",
            old.html
        );
    }
}
