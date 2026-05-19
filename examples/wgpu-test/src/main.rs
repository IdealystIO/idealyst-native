//! Smoke test for the wgpu desktop-preview backend.
//!
//! Opens a phone-sized window (or tablet / tv via `--tablet` / `--tv`
//! CLI args) and renders a small UI that exercises every primitive
//! the MVP backend implements, plus the framework's theme system.
//!
//! Tapping the "Dark mode" toggle flips a `Signal<bool>`; an
//! `Effect` watches it and calls `set_theme(light_theme() |
//! dark_theme())`. The framework re-fires every styled Effect and
//! the backend's `apply_style` pushes new resolved colors through
//! to the renderer — so the whole UI repaints with the new palette.
//!
//! Run:
//! ```
//! cargo run -p wgpu-test
//! cargo run -p wgpu-test -- --tablet
//! cargo run -p wgpu-test -- --tv
//! ```

use std::rc::Rc;

use framework_core::primitives::scroll_view::scroll_view;
use framework_core::primitives::slider::slider;
use framework_core::primitives::text_input::text_input;
use framework_core::primitives::toggle::toggle;
use framework_core::{
    button, install_theme, pressable, set_theme, signal, text, view, AlignItems, Color, Easing,
    FlexDirection, JustifyContent, Length, Primitive, Signal, StyleRules, StyleSheet,
    ThemeTokens, TokenEntry, Tokenized, Transition,
};

/// Duration of color crossfades for the theme swap. iOS uses
/// ~300ms for cross-screen palette transitions.
const THEME_FADE_MS: u32 = 300;

fn theme_transition() -> Transition {
    Transition::new(THEME_FADE_MS, Easing::EaseInOut)
}

// =============================================================================
// Theme
// =============================================================================

/// App theme. Stylesheets read from this struct; swapping the
/// theme (via `set_theme`) re-fires every styled `Effect` so the
/// backend's `apply_style` is called again with the new resolved
/// colors. Tokens are empty: we don't need runtime CSS variable
/// indirection on the wgpu backend, so all values are resolved
/// at apply-time.
#[derive(Clone)]
struct AppTheme {
    background: Color,
    surface: Color,
    surface_alt: Color,
    text: Color,
    muted: Color,
    accent: Color,
    accent_pressed: Color,
    pressable_bg: Color,
    pressable_bg_pressed: Color,
    border: Color,
    input_bg: Color,
    input_text: Color,
}

impl ThemeTokens for AppTheme {
    fn tokens(&self) -> Vec<TokenEntry> {
        Vec::new()
    }
}

fn light_theme() -> AppTheme {
    AppTheme {
        background: Color("#f2f2f7".into()),
        surface: Color("#ffffff".into()),
        surface_alt: Color("#e9e9ef".into()),
        text: Color("#0a0a0a".into()),
        muted: Color("#6c707a".into()),
        accent: Color("#007aff".into()),
        accent_pressed: Color("#0062cc".into()),
        pressable_bg: Color("#34c759".into()),
        pressable_bg_pressed: Color("#28a046".into()),
        border: Color("#d8d8dd".into()),
        input_bg: Color("#ffffff".into()),
        input_text: Color("#0a0a0a".into()),
    }
}

fn dark_theme() -> AppTheme {
    AppTheme {
        background: Color("#0f1115".into()),
        surface: Color("#1e2330".into()),
        surface_alt: Color("#2a3142".into()),
        text: Color("#ffffff".into()),
        muted: Color("#9ba3b4".into()),
        accent: Color("#7c4dff".into()),
        accent_pressed: Color("#5d35cc".into()),
        pressable_bg: Color("#2d6e3e".into()),
        pressable_bg_pressed: Color("#1f5530".into()),
        border: Color("#2a3142".into()),
        input_bg: Color("#ffffff".into()),
        input_text: Color("#1c1c1e".into()),
    }
}

// =============================================================================
// App
// =============================================================================

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let result = match mode.as_str() {
        "--tablet" => backend_wgpu_tablet::run(app),
        "--tv" => backend_wgpu_tv::run(app),
        _ => backend_wgpu_phone::run(app),
    };
    if let Err(e) = result {
        eprintln!("wgpu-test exited with error: {e}");
        std::process::exit(1);
    }
}

fn app() -> Primitive {
    install_theme(light_theme());
    let count: Signal<i32> = signal!(0);
    let dark_mode: Signal<bool> = signal!(false);
    let volume: Signal<f32> = signal!(0.4);
    let name: Signal<String> = signal!(String::new());

    // Long list rendered into the scrollview so total content
    // height exceeds the phone viewport — that's what makes the
    // scroll behavior visible.
    let mut list_rows: Vec<Primitive> = Vec::new();
    for i in 1..=20 {
        list_rows.push(
            view(vec![
                text(format!("Row {i}")).with_style(row_label_sheet()).into(),
                text(format!("#{i:02}"))
                    .with_style(row_value_sheet())
                    .into(),
            ])
            .with_style(form_row_sheet())
            .into(),
        );
    }

    // Theme swap: handled inline in the toggle's `on_change`
    // callback below.
    //
    // Why not an `Effect::new(...)` here? `app()` runs *before*
    // `render(...)` sets up the framework's surrounding scope.
    // Effects created outside a scope have `owns: true`, meaning
    // their slot is freed when the handle drops — and the handle
    // we bind to `_theme_effect` drops at the end of `app()`,
    // before the effect can subscribe to anything useful.
    //
    // The toggle's `on_change` closure is captured by the
    // framework's Toggle primitive (which lives inside the
    // build-walker scope), so it persists for the toggle's
    // lifetime and is the safe place to call `set_theme`.

    // ScrollView is the OUTERMOST node so it fills the window
    // edge-to-edge — that puts the scrollbar right at the
    // screen's right edge instead of inset behind a padded root.
    // The padding + gap that used to live on `root_sheet` now
    // live on an inner content view that the scrollview wraps.
    scroll_view(vec![view(vec![
        text("wgpu preview").with_style(title_sheet()).into(),
        text("themed iOS form inputs")
            .with_style(subtitle_sheet())
            .into(),

        // Tap count card.
        view(vec![
            text("tap count").with_style(card_label_sheet()).into(),
            text({
                let count = count;
                move || format!("{}", count.get())
            })
            .with_style(count_value_sheet())
            .into(),
        ])
        .with_style(count_card_sheet())
        .into(),

        // Button + pressable.
        pressable(
            vec![text("Pressable area").with_style(card_label_sheet()).into()],
            {
                let count = count;
                move || count.update(|n| *n += 1)
            },
        )
        .with_style(pressable_sheet())
        .into(),
        button("Tap me", {
            let count = count;
            move || count.update(|n| *n += 1)
        })
        .with_style(button_sheet())
        .into(),

        // Toggle row — flips the dark_mode signal AND swaps the
        // theme inline. (See note in app() about why an Effect
        // here doesn't work.)
        view(vec![
            text("Dark mode").with_style(row_label_sheet()).into(),
            toggle(dark_mode, move |v| {
                dark_mode.set(v);
                if v {
                    set_theme(dark_theme());
                } else {
                    set_theme(light_theme());
                }
            })
            .into(),
        ])
        .with_style(form_row_sheet())
        .into(),

        // Slider row.
        view(vec![
            view(vec![
                text("Volume").with_style(row_label_sheet()).into(),
                text({
                    let volume = volume;
                    move || format!("{:.0}%", volume.get() * 100.0)
                })
                .with_style(row_value_sheet())
                .into(),
            ])
            .with_style(row_label_pair_sheet())
            .into(),
            slider(volume, move |v| volume.set(v))
                .range(0.0, 1.0)
                .with_style(slider_sheet())
                .into(),
        ])
        .with_style(form_col_sheet())
        .into(),

        // Text input.
        view(vec![
            text("Your name").with_style(row_label_sheet()).into(),
            text_input(name, move |v| name.set(v))
                .placeholder("Type here…".to_string())
                .with_style(text_input_sheet())
                .into(),
        ])
        .with_style(form_col_sheet())
        .into(),
        // Section header for the list — anchors the eye so the
        // scrolling boundary is obvious.
        text("Long list").with_style(subtitle_sheet()).into(),
        // The overflowing list. Each row is a themed form_row.
        view(list_rows)
            .with_style(form_col_sheet())
            .into(),
    ])
    .with_style(inner_content_sheet())
    .into()])
    .with_style(scroll_view_sheet())
    .into()
}

// =============================================================================
// Style helpers — every sheet now reads from `AppTheme`.
// =============================================================================

fn themed<F>(f: F) -> Rc<StyleSheet>
where
    F: Fn(&AppTheme) -> StyleRules + 'static,
{
    Rc::new(StyleSheet::new::<AppTheme, _>(f))
}

/// Build a themed stylesheet with a `state pressed` overlay. Both
/// the base and pressed closures receive `&AppTheme` so the
/// pressed color tracks the active theme too.
fn themed_with_pressed<B, P>(base: B, pressed: P) -> Rc<StyleSheet>
where
    B: Fn(&AppTheme) -> StyleRules + 'static,
    P: Fn(&AppTheme) -> StyleRules + 'static,
{
    Rc::new(
        StyleSheet::new::<AppTheme, _>(base).variant("__state_pressed", "on", pressed),
    )
}

/// Inner content view that lives *inside* the outermost
/// `ScrollView`. The scrollview owns the background + viewport
/// dimensions so the scrollbar can sit flush against the
/// window's right edge; the padding + gap that used to live on
/// a separate root view now live here.
fn inner_content_sheet() -> Rc<StyleSheet> {
    themed(|_t| StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Stretch),
        justify_content: Some(JustifyContent::FlexStart),
        padding_top: Some(px(48.0)),
        padding_right: Some(px(24.0)),
        padding_bottom: Some(px(48.0)),
        padding_left: Some(px(24.0)),
        gap: Some(px(16.0)),
        ..Default::default()
    })
}

fn title_sheet() -> Rc<StyleSheet> {
    themed(|t| StyleRules {
        color: Some(literal_color(&t.text)),
        color_transition: Some(theme_transition()),
        font_size: Some(px(28.0)),
        ..Default::default()
    })
}

fn subtitle_sheet() -> Rc<StyleSheet> {
    themed(|t| StyleRules {
        color: Some(literal_color(&t.muted)),
        color_transition: Some(theme_transition()),
        font_size: Some(px(14.0)),
        ..Default::default()
    })
}

fn card_label_sheet() -> Rc<StyleSheet> {
    themed(|t| StyleRules {
        color: Some(literal_color(&t.text)),
        color_transition: Some(theme_transition()),
        font_size: Some(px(14.0)),
        ..Default::default()
    })
}

fn count_card_sheet() -> Rc<StyleSheet> {
    themed(|t| StyleRules {
        background: Some(literal_color(&t.surface)),
        background_transition: Some(theme_transition()),
        padding_top: Some(px(16.0)),
        padding_right: Some(px(20.0)),
        padding_bottom: Some(px(16.0)),
        padding_left: Some(px(20.0)),
        border_top_left_radius: Some(px(12.0)),
        border_top_right_radius: Some(px(12.0)),
        border_bottom_right_radius: Some(px(12.0)),
        border_bottom_left_radius: Some(px(12.0)),
        border_top_width: Some(f32_literal(1.0)),
        border_right_width: Some(f32_literal(1.0)),
        border_bottom_width: Some(f32_literal(1.0)),
        border_left_width: Some(f32_literal(1.0)),
        border_top_color: Some(literal_color(&t.border)),
        border_right_color: Some(literal_color(&t.border)),
        border_bottom_color: Some(literal_color(&t.border)),
        border_left_color: Some(literal_color(&t.border)),
        border_top_color_transition: Some(theme_transition()),
        gap: Some(px(4.0)),
        ..Default::default()
    })
}

fn count_value_sheet() -> Rc<StyleSheet> {
    themed(|t| StyleRules {
        color: Some(literal_color(&t.accent)),
        color_transition: Some(theme_transition()),
        font_size: Some(px(36.0)),
        ..Default::default()
    })
}

fn pressable_sheet() -> Rc<StyleSheet> {
    themed_with_pressed(
        |t| StyleRules {
            background: Some(literal_color(&t.pressable_bg)),
            // Press feedback should be snappy; theme swap should
            // crossfade. Same property, different durations — the
            // base style declares the theme transition; the
            // pressed overlay omits the spec so it snaps.
            background_transition: Some(theme_transition()),
            padding_top: Some(px(20.0)),
            padding_right: Some(px(20.0)),
            padding_bottom: Some(px(20.0)),
            padding_left: Some(px(20.0)),
            border_top_left_radius: Some(px(12.0)),
            border_top_right_radius: Some(px(12.0)),
            border_bottom_right_radius: Some(px(12.0)),
            border_bottom_left_radius: Some(px(12.0)),
            ..Default::default()
        },
        |t| StyleRules {
            background: Some(literal_color(&t.pressable_bg_pressed)),
            ..Default::default()
        },
    )
}

fn button_sheet() -> Rc<StyleSheet> {
    themed_with_pressed(
        |t| StyleRules {
            background: Some(literal_color(&t.accent)),
            background_transition: Some(theme_transition()),
            color: Some(literal_color(&Color("#ffffff".into()))),
            padding_top: Some(px(14.0)),
            padding_right: Some(px(20.0)),
            padding_bottom: Some(px(14.0)),
            padding_left: Some(px(20.0)),
            border_top_left_radius: Some(px(8.0)),
            border_top_right_radius: Some(px(8.0)),
            border_bottom_right_radius: Some(px(8.0)),
            border_bottom_left_radius: Some(px(8.0)),
            font_size: Some(px(16.0)),
            ..Default::default()
        },
        |t| StyleRules {
            background: Some(literal_color(&t.accent_pressed)),
            ..Default::default()
        },
    )
}

// ---------- Form input rows ----------

fn form_row_sheet() -> Rc<StyleSheet> {
    themed(|t| StyleRules {
        background: Some(literal_color(&t.surface)),
        background_transition: Some(theme_transition()),
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::SpaceBetween),
        padding_top: Some(px(12.0)),
        padding_right: Some(px(16.0)),
        padding_bottom: Some(px(12.0)),
        padding_left: Some(px(16.0)),
        border_top_left_radius: Some(px(12.0)),
        border_top_right_radius: Some(px(12.0)),
        border_bottom_right_radius: Some(px(12.0)),
        border_bottom_left_radius: Some(px(12.0)),
        ..Default::default()
    })
}

fn form_col_sheet() -> Rc<StyleSheet> {
    themed(|t| StyleRules {
        background: Some(literal_color(&t.surface)),
        background_transition: Some(theme_transition()),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Stretch),
        padding_top: Some(px(12.0)),
        padding_right: Some(px(16.0)),
        padding_bottom: Some(px(12.0)),
        padding_left: Some(px(16.0)),
        border_top_left_radius: Some(px(12.0)),
        border_top_right_radius: Some(px(12.0)),
        border_bottom_right_radius: Some(px(12.0)),
        border_bottom_left_radius: Some(px(12.0)),
        gap: Some(px(8.0)),
        ..Default::default()
    })
}

fn row_label_sheet() -> Rc<StyleSheet> {
    themed(|t| StyleRules {
        color: Some(literal_color(&t.text)),
        color_transition: Some(theme_transition()),
        font_size: Some(px(15.0)),
        ..Default::default()
    })
}

fn row_value_sheet() -> Rc<StyleSheet> {
    themed(|t| StyleRules {
        color: Some(literal_color(&t.muted)),
        color_transition: Some(theme_transition()),
        font_size: Some(px(13.0)),
        ..Default::default()
    })
}

fn row_label_pair_sheet() -> Rc<StyleSheet> {
    themed(|_t| StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::SpaceBetween),
        ..Default::default()
    })
}

fn slider_sheet() -> Rc<StyleSheet> {
    themed(|_t| StyleRules {
        flex_grow: Some(f32_literal(1.0)),
        ..Default::default()
    })
}

fn text_input_sheet() -> Rc<StyleSheet> {
    themed(|t| StyleRules {
        color: Some(literal_color(&t.input_text)),
        font_size: Some(px(17.0)),
        ..Default::default()
    })
}

/// Outermost ScrollView. Fills the window edge-to-edge so the
/// overlay scrollbar lives against the screen's right edge.
/// Owns the themed background; the inner content view handles
/// padding + gap so the bg keeps painting under any scroll
/// overshoot.
fn scroll_view_sheet() -> Rc<StyleSheet> {
    themed(|t| StyleRules {
        background: Some(literal_color(&t.background)),
        background_transition: Some(theme_transition()),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Stretch),
        width: Some(pct(100.0)),
        height: Some(pct(100.0)),
        ..Default::default()
    })
}

// =============================================================================
// Tokenized<T> literal helpers
// =============================================================================

fn literal_color(c: &Color) -> Tokenized<Color> {
    Tokenized::Literal(c.clone())
}

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

fn pct(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Percent(v))
}

fn f32_literal(v: f32) -> Tokenized<f32> {
    Tokenized::Literal(v)
}
