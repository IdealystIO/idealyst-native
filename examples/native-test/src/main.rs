//! Smoke test for the wgpu desktop-preview backend.
//!
//! Opens a phone-sized window (or tablet / tv via `--tablet` /
//! `--tv`) skinned for iOS by default — pass `--android` to flip
//! to the Material 3 skin. Renders a small UI exercising every
//! primitive the MVP backend implements, plus the framework's
//! theme system.
//!
//! Tapping the "Dark mode" toggle flips a `Signal<bool>`; an
//! `Effect` watches it and calls `set_theme(light_theme() |
//! dark_theme())`. The framework re-fires every styled Effect and
//! the backend's `apply_style` pushes new resolved colors through
//! to the renderer — so the whole UI repaints with the new palette.
//!
//! Run:
//! ```
//! cargo run -p native-test                       # phone, iOS
//! cargo run -p native-test -- --android          # phone, Android (M3)
//! cargo run -p native-test -- --tablet           # tablet, iOS
//! cargo run -p native-test -- --tablet --android # tablet, Android
//! cargo run -p native-test -- --tv --android     # TV, Android
//! ```

use std::rc::Rc;

use framework_core::primitives::activity_indicator::{
    activity_indicator, ActivityIndicatorSize,
};
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

/// Form factor selected by `--phone` (default) / `--tablet` / `--tv`.
#[derive(Clone, Copy)]
enum FormFactor {
    Phone,
    Tablet,
    Tv,
}

impl FormFactor {
    fn flag(self) -> &'static str {
        match self {
            FormFactor::Phone => "--phone",
            FormFactor::Tablet => "--tablet",
            FormFactor::Tv => "--tv",
        }
    }

    /// Logical width of the variant's window. Used by the
    /// dual-window spawn path to compute where to place the
    /// child window so it doesn't overlap the parent.
    fn logical_width(self) -> i32 {
        match self {
            FormFactor::Phone => native_phone::WIDTH as i32,
            FormFactor::Tablet => native_tablet::WIDTH as i32,
            FormFactor::Tv => native_tv::WIDTH as i32,
        }
    }
}

/// Which skin the test app should mount. The CLI maps
/// `--ios`/`--android` to the right concrete `Skin` impl.
#[derive(Clone, Copy)]
enum SkinChoice {
    Ios,
    Android,
}

/// Horizontal anchor used when spawning the parent of a
/// dual-window run. Picked so the two windows fit on a typical
/// desktop without overlapping.
const DUAL_WINDOW_LEFT_X: i32 = 60;
const DUAL_WINDOW_TOP_Y: i32 = 80;
/// Gap between the parent and child windows.
const DUAL_WINDOW_GAP: i32 = 24;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut form = FormFactor::Phone;
    let mut wants_ios = false;
    let mut wants_android = false;
    let mut explicit_position: Option<(i32, i32)> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--phone" => form = FormFactor::Phone,
            "--tablet" => form = FormFactor::Tablet,
            "--tv" => form = FormFactor::Tv,
            "--ios" => wants_ios = true,
            "--android" => wants_android = true,
            "--at" => match iter.next() {
                Some(coord) => {
                    explicit_position = parse_position(coord).or_else(|| {
                        eprintln!("native-test: --at expects 'X,Y' (got {coord:?})");
                        std::process::exit(2);
                    });
                }
                None => {
                    eprintln!("native-test: --at expects a 'X,Y' argument");
                    std::process::exit(2);
                }
            },
            other => {
                eprintln!("native-test: unknown flag {other:?}");
                eprintln!(
                    "usage: cargo run -p native-test -- \
                     [--phone|--tablet|--tv] [--ios|--android] [--at X,Y]"
                );
                std::process::exit(2);
            }
        }
    }
    // Default to iOS when no skin flag is given.
    if !wants_ios && !wants_android {
        wants_ios = true;
    }

    // Winit's event loop is per-process — to open both skins in
    // parallel we re-exec ourself for the second one and run the
    // first in this process. Both windows get explicit positions
    // so they sit side by side.
    let (parent_pos, child_pos) = if wants_ios && wants_android {
        let pw = form.logical_width();
        let parent = (DUAL_WINDOW_LEFT_X, DUAL_WINDOW_TOP_Y);
        let child = (DUAL_WINDOW_LEFT_X + pw + DUAL_WINDOW_GAP, DUAL_WINDOW_TOP_Y);
        (Some(parent), Some(child))
    } else {
        (explicit_position, None)
    };
    let child = if let Some((cx, cy)) = child_pos {
        let exe = std::env::current_exe().expect("current_exe");
        Some(
            std::process::Command::new(exe)
                .arg(form.flag())
                .arg("--android")
                .arg("--at")
                .arg(format!("{cx},{cy}"))
                .spawn()
                .expect("spawn second native-test instance"),
        )
    } else {
        None
    };

    let skin_choice = if wants_ios { SkinChoice::Ios } else { SkinChoice::Android };
    let skin: Rc<dyn render_wgpu::Skin> = match skin_choice {
        SkinChoice::Ios => Rc::new(ios_sim::IosSim::new()),
        SkinChoice::Android => Rc::new(android_sim::AndroidSim::new()),
    };
    let result = match form {
        FormFactor::Phone => native_phone::run_at(skin, parent_pos, app),
        FormFactor::Tablet => native_tablet::run_at(skin, parent_pos, app),
        FormFactor::Tv => native_tv::run_at(skin, parent_pos, app),
    };

    if let Some(mut c) = child {
        let _ = c.wait();
    }
    if let Err(e) = result {
        eprintln!("native-test exited with error: {e}");
        std::process::exit(1);
    }
}

fn parse_position(s: &str) -> Option<(i32, i32)> {
    let (xs, ys) = s.split_once(',')?;
    Some((xs.trim().parse().ok()?, ys.trim().parse().ok()?))
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
        // Spinner row — exercises ActivityIndicator (no state,
        // just continuous animation). Two sizes side-by-side.
        view(vec![
            text("Loading").with_style(row_label_sheet()).into(),
            view(vec![
                activity_indicator()
                    .size(ActivityIndicatorSize::Small)
                    .into(),
                activity_indicator()
                    .size(ActivityIndicatorSize::Large)
                    .into(),
            ])
            .with_style(spinner_row_sheet())
            .into(),
        ])
        .with_style(form_row_sheet())
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

fn spinner_row_sheet() -> Rc<StyleSheet> {
    themed(|_t| StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(px(12.0)),
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
