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

mod mandelbrot;

use std::rc::Rc;

use framework_core::primitives::activity_indicator::{
    activity_indicator, ActivityIndicatorSize,
};
use framework_core::primitives::icon::icon;
use framework_core::primitives::image::image;
use framework_core::primitives::navigator::{
    DrawerHandle, DrawerNavigator, HeaderButton, Navigator, NavigatorHandle, Route, Screen,
};
use framework_core::primitives::overlay::{overlay, BackdropMode};
use framework_core::primitives::portal::ViewportPlacement;
use framework_core::primitives::scroll_view::scroll_view;
use framework_core::primitives::slider::slider;
use framework_core::primitives::text_input::text_input;
use framework_core::primitives::toggle::toggle;
use framework_core::primitives::video::video;
use framework_core::primitives::web_view::web_view;
use framework_core::{
    button, pressable, signal, text, view, when, AlignItems, Color,
    Easing, FlexDirection, JustifyContent, Length, Primitive, Ref, SafeAreaSides, Shadow, Signal,
    StyleRules, StyleSheet, TokenEntry, TokenValue, Tokenized, Transition,
};
use idea_ui::{install_theme, set_theme, ThemeTokens};

/// Routes for the outer navigator (default `SlideFromRight`).
/// `Home` is the primitives showcase + nav-demo launchers;
/// `Detail` exercises a basic push/pop;
/// `ModalNavDemo` / `InstantNavDemo` each host their own inner
/// navigator with a non-default animator (slide-up + instant
/// snap) so multiple `ScreenTransition` impls can be observed
/// side-by-side in a single app run.
const HOME_ROUTE: Route<()> = Route::<()>::new("home", "/");
const DETAIL_ROUTE: Route<()> = Route::<()>::new("detail", "/detail");
const MODAL_NAV_DEMO_ROUTE: Route<()> = Route::<()>::new("modal-nav-demo", "/modal-nav-demo");
const INSTANT_NAV_DEMO_ROUTE: Route<()> =
    Route::<()>::new("instant-nav-demo", "/instant-nav-demo");
const DRAWER_DEMO_ROUTE: Route<()> = Route::<()>::new("drawer-demo", "/drawer-demo");
const DRAWER_BODY_ROUTE: Route<()> = Route::<()>::new("drawer-body", "/drawer-body");

/// Routes for the inner navigator inside the `ModalNavDemo`
/// screen. Inner nav uses `SlideFromBottom` — pushes appear
/// from below like a presented modal.
const MODAL_INNER_HOME_ROUTE: Route<()> =
    Route::<()>::new("modal-inner-home", "/modal-inner-home");
const MODAL_INNER_PUSHED_ROUTE: Route<()> =
    Route::<()>::new("modal-inner-pushed", "/modal-inner-pushed");

/// Routes for the inner navigator inside the `InstantNavDemo`
/// screen. Inner nav uses `InstantTransition` — pushes snap
/// without animation, useful for comparing against the slide
/// animators above.
const INSTANT_INNER_HOME_ROUTE: Route<()> =
    Route::<()>::new("instant-inner-home", "/instant-inner-home");
const INSTANT_INNER_PUSHED_ROUTE: Route<()> =
    Route::<()>::new("instant-inner-pushed", "/instant-inner-pushed");

/// Duration of color crossfades for the theme swap. iOS uses
/// ~300ms for cross-screen palette transitions.
const THEME_FADE_MS: u32 = 300;

fn theme_transition() -> Transition {
    Transition::new(THEME_FADE_MS, Easing::EaseInOut)
}

// =============================================================================
// Theme
// =============================================================================

/// App theme. Stylesheets reference tokens by name via
/// `Tokenized::token("app-X", fallback)`; `tokens()` emits one
/// `TokenEntry` per field so swapping the theme (via `set_theme`)
/// pushes new values through the framework's token pipeline,
/// which re-fires every styled `Effect` and updates the resolved
/// values on the backend.
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

// Token names emitted by `AppTheme::tokens()` and referenced by
// every stylesheet in this crate. Centralized so a typo at the
// emit site or the read site fails to compile rather than
// silently returning the fallback color.
const TOK_BACKGROUND: &str = "app-background";
const TOK_SURFACE: &str = "app-surface";
const TOK_SURFACE_ALT: &str = "app-surface-alt";
const TOK_TEXT: &str = "app-text";
const TOK_MUTED: &str = "app-muted";
const TOK_ACCENT: &str = "app-accent";
const TOK_ACCENT_PRESSED: &str = "app-accent-pressed";
const TOK_PRESSABLE_BG: &str = "app-pressable-bg";
const TOK_PRESSABLE_BG_PRESSED: &str = "app-pressable-bg-pressed";
const TOK_BORDER: &str = "app-border";
const TOK_INPUT_BG: &str = "app-input-bg";
const TOK_INPUT_TEXT: &str = "app-input-text";

impl ThemeTokens for AppTheme {
    fn tokens(&self) -> Vec<TokenEntry> {
        fn color_entry(name: &'static str, c: &Color) -> TokenEntry {
            TokenEntry {
                name,
                value: TokenValue::Color(c.clone()),
            }
        }
        vec![
            color_entry(TOK_BACKGROUND, &self.background),
            color_entry(TOK_SURFACE, &self.surface),
            color_entry(TOK_SURFACE_ALT, &self.surface_alt),
            color_entry(TOK_TEXT, &self.text),
            color_entry(TOK_MUTED, &self.muted),
            color_entry(TOK_ACCENT, &self.accent),
            color_entry(TOK_ACCENT_PRESSED, &self.accent_pressed),
            color_entry(TOK_PRESSABLE_BG, &self.pressable_bg),
            color_entry(TOK_PRESSABLE_BG_PRESSED, &self.pressable_bg_pressed),
            color_entry(TOK_BORDER, &self.border),
            color_entry(TOK_INPUT_BG, &self.input_bg),
            color_entry(TOK_INPUT_TEXT, &self.input_text),
        ]
    }
}

/// Fallback palette used by every `Tokenized::token(..., fallback)`
/// site below. Token-resolution at apply time picks up the
/// currently-installed `AppTheme`'s value; the fallback is what
/// renders if no theme is installed (or in unit tests).
fn default_palette() -> AppTheme {
    light_theme()
}

/// Build a `Tokenized<Color>` reference to one of `AppTheme`'s
/// named tokens. The fallback is sourced from the default light
/// palette so unstyled rendering still produces sensible colors.
fn color_token(name: &'static str, fallback: Color) -> Tokenized<Color> {
    Tokenized::token(name, fallback)
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
    // Device-model overrides. Each skin defaults to its premium
    // current-gen preset (iPhone 15 Pro / Pixel 8); these flags
    // pick a different bundle of notch / corner / bezel.
    let mut ios_device: Option<ios_sim::DeviceModel> = None;
    let mut android_device: Option<android_sim::DeviceModel> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--phone" => form = FormFactor::Phone,
            "--tablet" => form = FormFactor::Tablet,
            "--tv" => form = FormFactor::Tv,
            "--ios" => wants_ios = true,
            "--android" => wants_android = true,
            "--ios-device" => match iter.next().map(|s| s.as_str()) {
                Some("iphone-15-pro") | Some("15-pro") => {
                    ios_device = Some(ios_sim::DeviceModel::IPhone15Pro);
                }
                Some("iphone-13") | Some("13") => {
                    ios_device = Some(ios_sim::DeviceModel::IPhone13);
                }
                Some("iphone-se") | Some("se") => {
                    ios_device = Some(ios_sim::DeviceModel::IPhoneSE);
                }
                other => {
                    eprintln!(
                        "native-test: --ios-device expects \
                         iphone-15-pro | iphone-13 | iphone-se \
                         (got {other:?})"
                    );
                    std::process::exit(2);
                }
            },
            "--android-device" => match iter.next().map(|s| s.as_str()) {
                Some("pixel-8") | Some("pixel") => {
                    android_device = Some(android_sim::DeviceModel::Pixel8);
                }
                Some("galaxy-s") | Some("galaxy") => {
                    android_device = Some(android_sim::DeviceModel::GalaxyS);
                }
                Some("midrange") => {
                    android_device = Some(android_sim::DeviceModel::Midrange);
                }
                Some("tablet") => {
                    android_device = Some(android_sim::DeviceModel::Tablet);
                }
                other => {
                    eprintln!(
                        "native-test: --android-device expects \
                         pixel-8 | galaxy-s | midrange | tablet \
                         (got {other:?})"
                    );
                    std::process::exit(2);
                }
            },
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
                     [--phone|--tablet|--tv] [--ios|--android] \
                     [--ios-device <iphone-15-pro|iphone-13|iphone-se>] \
                     [--android-device <pixel-8|galaxy-s|midrange|tablet>] \
                     [--at X,Y]"
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
        SkinChoice::Ios => {
            let mut s = ios_sim::IosSim::new();
            if let Some(m) = ios_device {
                s = s.with_device(m);
            }
            Rc::new(s)
        }
        SkinChoice::Android => {
            let mut s = android_sim::AndroidSim::new();
            if let Some(m) = android_device {
                s = s.with_device(m);
            }
            Rc::new(s)
        }
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
    let nav: Ref<NavigatorHandle> = Ref::new();
    // Outer navigator uses the crate-default `SlideFromRight`.
    // No `with_transition` wrapper here — the demo navigators
    // inside `modal_nav_demo_screen` / `instant_nav_demo_screen`
    // install their own animators when their inner Navigators
    // are constructed.
    Navigator::new(&HOME_ROUTE)
        .screen(HOME_ROUTE, move |_| {
            Screen::new(home_screen(nav)).header_shown(false)
        })
        .screen(DETAIL_ROUTE, move |_| {
            Screen::new(detail_screen(nav)).title("Detail")
        })
        .screen(MODAL_NAV_DEMO_ROUTE, move |_| {
            // Outer-level title — appears in the outer
            // navigator's header. The inner navigator below
            // paints its own header for its own screens.
            Screen::new(modal_nav_demo_screen(nav)).title("Modal-nav demo")
        })
        .screen(INSTANT_NAV_DEMO_ROUTE, move |_| {
            Screen::new(instant_nav_demo_screen(nav)).title("Instant-nav demo")
        })
        .screen(DRAWER_DEMO_ROUTE, move |_| {
            // Drawer demo hides its outer header so the inner
            // drawer's hamburger header isn't double-stacked.
            Screen::new(drawer_demo_screen(nav)).header_shown(false)
        })
        .bind(nav)
        .into()
}

/// The original primitives showcase, now mounted as the
/// navigator's `home` screen. A "Push detail" button at the top
/// of the form fires `handle.push(&DETAIL_ROUTE, ())` — the
/// dispatcher mounts the detail subtree, adds it as a Taffy
/// child of the navigator, and the renderer's last-child walk
/// paints it on top of the home stack.
fn home_screen(nav: Ref<NavigatorHandle>) -> Primitive {
    let count: Signal<i32> = signal!(0);
    let dark_mode: Signal<bool> = signal!(false);
    let volume: Signal<f32> = signal!(0.4);
    let name: Signal<String> = signal!(String::new());
    let overlay_open: Signal<bool> = signal!(false);

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

        // Navigator launchers — three buttons, each pushing
        // into a route whose subtree mounts a navigator with a
        // distinct `ScreenTransition` animator. The push from
        // home itself always uses the outer navigator's slide
        // (default `SlideFromRight`); the contrast comes from
        // the inner navigators' own animations once you've
        // landed on each demo screen.
        button("Push detail (slide-right)", move || {
            if let Some(h) = nav.get() {
                h.push(&DETAIL_ROUTE, ());
            }
        })
        .into(),
        button("Open modal-nav demo (slide-up inside)", move || {
            if let Some(h) = nav.get() {
                h.push(&MODAL_NAV_DEMO_ROUTE, ());
            }
        })
        .into(),
        button("Open instant-nav demo (snap inside)", move || {
            if let Some(h) = nav.get() {
                h.push(&INSTANT_NAV_DEMO_ROUTE, ());
            }
        })
        .into(),
        button("Open drawer demo", move || {
            if let Some(h) = nav.get() {
                h.push(&DRAWER_DEMO_ROUTE, ());
            }
        })
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
        .into(),

        // Embedded WebView, backed by Blitz (pure-Rust HTML/CSS).
        // Renders the URL into an offscreen texture and composites
        // through the image pipeline alongside the rest of the UI.
        text("Embedded WebView").with_style(subtitle_sheet()).into(),
        web_view("https://example.com")
            .with_style(themed(|| StyleRules {
                width: Some(Tokenized::Literal(Length::Px(280.0))),
                height: Some(Tokenized::Literal(Length::Px(420.0))),
                border_top_left_radius: Some(px(12.0)),
                border_top_right_radius: Some(px(12.0)),
                border_bottom_right_radius: Some(px(12.0)),
                border_bottom_left_radius: Some(px(12.0)),
                overflow: Some(framework_core::Overflow::Hidden),
                ..Default::default()
            }))
            .into(),

        // Embedded wgpu surface demo. `mandelbrot_demo()` returns
        // a `Bound<GraphicsHandle>` whose drawer renders the
        // mandelbrot animation into the node's offscreen texture
        // each frame; the renderer composites it through the
        // image pipeline alongside the rest of the UI.
        view(vec![
            text("Embedded wgpu")
                .with_style(themed(|| StyleRules {
                    color: Some(color_token(TOK_TEXT, default_palette().text)),
                    font_size: Some(px(16.0)),
                    ..Default::default()
                }))
                .into(),
            mandelbrot::mandelbrot_demo()
                .with_style(themed(|| StyleRules {
                    width: Some(Tokenized::Literal(Length::Px(280.0))),
                    height: Some(Tokenized::Literal(Length::Px(200.0))),
                    border_top_left_radius: Some(px(12.0)),
                    border_top_right_radius: Some(px(12.0)),
                    border_bottom_right_radius: Some(px(12.0)),
                    border_bottom_left_radius: Some(px(12.0)),
                    overflow: Some(framework_core::Overflow::Hidden),
                    ..Default::default()
                }))
                .into(),
        ])
        .with_style(themed(|| StyleRules {
            background: Some(color_token(TOK_SURFACE, default_palette().surface)),
            padding_top: Some(px(14.0)),
            padding_right: Some(px(14.0)),
            padding_bottom: Some(px(14.0)),
            padding_left: Some(px(14.0)),
            border_top_left_radius: Some(px(16.0)),
            border_top_right_radius: Some(px(16.0)),
            border_bottom_right_radius: Some(px(16.0)),
            border_bottom_left_radius: Some(px(16.0)),
            gap: Some(Tokenized::Literal(Length::Px(10.0))),
            align_items: Some(AlignItems::Center),
            ..Default::default()
        }))
        .into(),

        // Shadow demo: a plain view with an author-defined
        // `shadow: Some(Shadow { ... })`. The wgpu backend's
        // rect pipeline picks it up via `RenderStyle.shadow`
        // and stages a soft-falloff shadow instance under the
        // view's rect. Nothing platform-specific — same code
        // works on any skin.
        view(vec![
            text("Custom shadow")
                .with_style(themed(|| StyleRules {
                    color: Some(color_token(TOK_TEXT, default_palette().text)),
                    font_size: Some(px(16.0)),
                    ..Default::default()
                }))
                .into(),
        ])
        .with_style(themed(|| StyleRules {
            background: Some(color_token(TOK_SURFACE, default_palette().surface)),
            padding_top: Some(px(18.0)),
            padding_right: Some(px(18.0)),
            padding_bottom: Some(px(18.0)),
            padding_left: Some(px(18.0)),
            border_top_left_radius: Some(px(16.0)),
            border_top_right_radius: Some(px(16.0)),
            border_bottom_right_radius: Some(px(16.0)),
            border_bottom_left_radius: Some(px(16.0)),
            shadow: Some(Shadow {
                x: 0.0,
                y: 6.0,
                blur: 16.0,
                color: Color("#1f2a4422".into()),
            }),
            ..Default::default()
        }))
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

        // Image row — points at a real PNG so the textured-quad
        // pipeline actually decodes and renders it. Failed
        // loads (file missing / unsupported format) fall back
        // to the placeholder rect inline.
        view(vec![
            text("Image").with_style(row_label_sheet()).into(),
            image("examples/hello-world/assets/logo.png")
                .with_style(media_thumb_sheet())
                .into(),
        ])
        .with_style(form_row_sheet())
        .into(),

        // Icon row — three icons from icons-lucide stamped in
        // the row label color. The wgpu backend renders each
        // as a colored placeholder square; the SVG path
        // rasterizer lands later.
        view(vec![
            text("Icons").with_style(row_label_sheet()).into(),
            view(vec![
                icon(icons_lucide::HOME).with_style(icon_sheet()).into(),
                icon(icons_lucide::SEARCH).with_style(icon_sheet()).into(),
                icon(icons_lucide::SETTINGS).with_style(icon_sheet()).into(),
            ])
            .with_style(icon_row_sheet())
            .into(),
        ])
        .with_style(form_row_sheet())
        .into(),

        // Overlay row — a button that toggles a modal. The
        // overlay paints with a scrim backdrop above the rest
        // of the app; tapping the scrim fires `on_dismiss`.
        view(vec![
            text("Modal").with_style(row_label_sheet()).into(),
            button("Show overlay", {
                let overlay_open = overlay_open;
                move || overlay_open.set(true)
            })
            .into(),
        ])
        .with_style(form_row_sheet())
        .into(),

        // The overlay itself — gated by a `when` so the
        // subtree only mounts while open. On dismiss, flip
        // the signal back to false.
        when(
            move || overlay_open.get(),
            move || {
                overlay(vec![view(vec![
                    text("Hi from an overlay")
                        .with_style(overlay_title_sheet())
                        .into(),
                    text("Tap outside to dismiss.")
                        .with_style(overlay_body_sheet())
                        .into(),
                ])
                .with_style(overlay_card_sheet())
                .into()])
                .placement(ViewportPlacement::default())
                .backdrop(BackdropMode::Dismiss)
                .on_dismiss(move || overlay_open.set(false))
                .into()
            },
            || view(vec![]).into(),
        ),


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
    // Inset the scroll content off the device's status bar and
    // home indicator so the first row + last row aren't hidden
    // under chrome. Background still bleeds under both strips
    // (chrome is translucent).
    .safe_area(SafeAreaSides::VERTICAL)
    .into()
}

/// Second screen of the stack navigator. Mounted lazily by the
/// dispatcher's `Push` path when the home screen's "Push detail
/// screen" button fires. The "Pop back" button fires
/// `handle.pop()` which our `detach_top_navigator_child` helper
/// removes from the Taffy tree + drops the scope.
fn detail_screen(nav: Ref<NavigatorHandle>) -> Primitive {
    view(vec![
        text("Detail screen").with_style(title_sheet()).into(),
        text(
            "Pushed by the dispatcher. Pop returns to the home \
             showcase; the home subtree stays mounted underneath.",
        )
        .with_style(subtitle_sheet())
        .into(),
        button("Pop back", move || {
            if let Some(h) = nav.get() {
                h.pop();
            }
        })
        .into(),
    ])
    .with_style(detail_screen_sheet())
    .into()
}

/// Outer-nav route that hosts a *second*, independent Navigator
/// configured to use `SlideFromBottom` — push/pop animate the
/// top screen up from / down to the bottom edge. Wrapping
/// `Navigator::new(...).into()` in `with_transition(...)`
/// installs the override that the wgpu backend's
/// `create_navigator` consumes when it sees this nested nav.
///
/// `outer_nav` is captured so the inner home can also offer a
/// "Back to home" button that pops the outer stack — handy
/// during the demo to bounce between the three demos without
/// chaining inner-then-outer pops.
fn modal_nav_demo_screen(outer_nav: Ref<NavigatorHandle>) -> Primitive {
    let inner: Ref<NavigatorHandle> = Ref::new();
    render_wgpu::with_transition(Rc::new(render_wgpu::SlideFromBottom::new()), || {
        Navigator::new(&MODAL_INNER_HOME_ROUTE)
            .screen(MODAL_INNER_HOME_ROUTE, move |_| {
                Screen::new(modal_inner_home(inner, outer_nav)).title("Sheet")
            })
            .screen(MODAL_INNER_PUSHED_ROUTE, move |_| {
                Screen::new(modal_inner_pushed(inner)).title("Detail sheet")
            })
            .bind(inner)
            .into()
    })
}

fn modal_inner_home(
    inner: Ref<NavigatorHandle>,
    outer_nav: Ref<NavigatorHandle>,
) -> Primitive {
    view(vec![
        text("Modal nav demo").with_style(title_sheet()).into(),
        text(
            "This inner navigator uses `SlideFromBottom`. Push \
             a screen below to see it slide up from the bottom \
             edge.",
        )
        .with_style(subtitle_sheet())
        .into(),
        button("Push (slide up)", move || {
            if let Some(h) = inner.get() {
                h.push(&MODAL_INNER_PUSHED_ROUTE, ());
            }
        })
        .into(),
        button("Back to outer home", move || {
            if let Some(h) = outer_nav.get() {
                h.pop();
            }
        })
        .into(),
    ])
    .with_style(detail_screen_sheet())
    .into()
}

fn modal_inner_pushed(inner: Ref<NavigatorHandle>) -> Primitive {
    view(vec![
        text("Slid up from below").with_style(title_sheet()).into(),
        text("Pop to slide back down.")
            .with_style(subtitle_sheet())
            .into(),
        button("Pop (slide down)", move || {
            if let Some(h) = inner.get() {
                h.pop();
            }
        })
        .into(),
    ])
    .with_style(detail_screen_sheet())
    .into()
}

/// Outer-nav route hosting a Navigator configured with
/// `InstantTransition` — no animation. The deferred Pop cleanup
/// still runs (so scope drops + Taffy removals stay correct),
/// but the renderer samples progress=1 immediately, so each
/// push/pop snaps in place. Useful for comparing visually
/// against the slide animators above and as a sanity check
/// that the rest of the pipeline works without a slide.
fn instant_nav_demo_screen(outer_nav: Ref<NavigatorHandle>) -> Primitive {
    let inner: Ref<NavigatorHandle> = Ref::new();
    render_wgpu::with_transition(Rc::new(render_wgpu::InstantTransition), || {
        Navigator::new(&INSTANT_INNER_HOME_ROUTE)
            .screen(INSTANT_INNER_HOME_ROUTE, move |_| {
                Screen::new(instant_inner_home(inner, outer_nav)).title("Inner home")
            })
            .screen(INSTANT_INNER_PUSHED_ROUTE, move |_| {
                Screen::new(instant_inner_pushed(inner)).title("Inner pushed")
            })
            .bind(inner)
            .into()
    })
}

fn instant_inner_home(
    inner: Ref<NavigatorHandle>,
    outer_nav: Ref<NavigatorHandle>,
) -> Primitive {
    view(vec![
        text("Instant nav demo").with_style(title_sheet()).into(),
        text(
            "This inner navigator uses `InstantTransition` — \
             pushes and pops snap with no slide. Same dispatcher \
             path as the slide demos; only the sampled frame \
             differs.",
        )
        .with_style(subtitle_sheet())
        .into(),
        button("Push (snap)", move || {
            if let Some(h) = inner.get() {
                h.push(&INSTANT_INNER_PUSHED_ROUTE, ());
            }
        })
        .into(),
        button("Back to outer home", move || {
            if let Some(h) = outer_nav.get() {
                h.pop();
            }
        })
        .into(),
    ])
    .with_style(detail_screen_sheet())
    .into()
}

fn instant_inner_pushed(inner: Ref<NavigatorHandle>) -> Primitive {
    view(vec![
        text("Snapped in").with_style(title_sheet()).into(),
        text("Pop to snap back.")
            .with_style(subtitle_sheet())
            .into(),
        button("Pop (snap)", move || {
            if let Some(h) = inner.get() {
                h.pop();
            }
        })
        .into(),
    ])
    .with_style(detail_screen_sheet())
    .into()
}

/// Outer-nav route hosting a `DrawerNavigator`. Demonstrates
/// the full functional flow:
///   - `handle.open_drawer()` from a hamburger button on the
///     body screen's header
///   - scrim + slide-in animation
///   - tap-outside-to-close (the scrim has a hit region wired
///     to `CloseDrawer`)
///   - manual `Pop back to outer` button so the user can
///     return to the outer navigator
fn drawer_demo_screen(outer_nav: Ref<NavigatorHandle>) -> Primitive {
    let drawer: Ref<DrawerHandle> = Ref::new();
    DrawerNavigator::new(&DRAWER_BODY_ROUTE)
        .screen(DRAWER_BODY_ROUTE, move |_| {
            let drawer = drawer;
            let outer_nav = outer_nav;
            Screen::new(drawer_body_screen(drawer, outer_nav))
                .title("Drawer demo")
                // Hamburger that opens the drawer. The wgpu
                // simulator's chrome_icons maps
                // `line.3.horizontal` to a platform-specific
                // hamburger glyph (3 lines on iOS, condensed
                // hamburger on M3).
                .header_left(HeaderButton::new("line.3.horizontal", move || {
                    if let Some(h) = drawer.get() {
                        h.open();
                    }
                }))
        })
        .content(move |_props| drawer_sidebar(drawer))
        .bind(drawer)
        .into()
}

fn drawer_body_screen(
    drawer: Ref<DrawerHandle>,
    outer_nav: Ref<NavigatorHandle>,
) -> Primitive {
    view(vec![
        text("Drawer demo body")
            .with_style(title_sheet())
            .into(),
        text(
            "Tap the hamburger in the header to open the drawer. \
             Tap the scrim (or the close button in the panel) \
             to dismiss it.",
        )
        .with_style(subtitle_sheet())
        .into(),
        button("Open drawer", move || {
            if let Some(h) = drawer.get() {
                h.open();
            }
        })
        .into(),
        button("Pop back to outer", move || {
            if let Some(h) = outer_nav.get() {
                h.pop();
            }
        })
        .into(),
    ])
    .with_style(detail_screen_sheet())
    .into()
}

fn drawer_sidebar(drawer: Ref<DrawerHandle>) -> Primitive {
    view(vec![
        text("Drawer panel").with_style(title_sheet()).into(),
        text("Tap a row to close the drawer.")
            .with_style(subtitle_sheet())
            .into(),
        button("Close drawer", move || {
            if let Some(h) = drawer.get() {
                h.close();
            }
        })
        .into(),
    ])
    .with_style(drawer_sidebar_sheet())
    .into()
}

// =============================================================================
// Style helpers — every sheet now reads from `AppTheme`.
// =============================================================================

/// Wrap a `StyleRules`-producing closure as a `StyleSheet`.
/// Stylesheets reference theme tokens by *name* via
/// `Tokenized::token(...)` — the framework resolves those names
/// at apply time against the currently-installed token table, so
/// no theme downcast is needed here. Theme swaps push new token
/// values through the framework, which re-fires every styled
/// `Effect`.
fn themed<F>(f: F) -> Rc<StyleSheet>
where
    F: Fn() -> StyleRules + 'static,
{
    Rc::new(StyleSheet::new(move |_vs: &framework_core::VariantSet| {
        f()
    }))
}

/// Build a stylesheet with a `state pressed` overlay. Both
/// closures emit `StyleRules` populated with `Tokenized::token`
/// references; the framework re-resolves them on every theme
/// swap.
fn themed_with_pressed<B, P>(base: B, pressed: P) -> Rc<StyleSheet>
where
    B: Fn() -> StyleRules + 'static,
    P: Fn() -> StyleRules + 'static,
{
    Rc::new(
        StyleSheet::new(move |_vs: &framework_core::VariantSet| base())
            .variant(
                "__state_pressed",
                "on",
                move |_vs: &framework_core::VariantSet| pressed(),
            ),
    )
}

/// Inner content view that lives *inside* the outermost
/// `ScrollView`. The scrollview owns the background + viewport
/// dimensions so the scrollbar can sit flush against the
/// window's right edge; the padding + gap that used to live on
/// a separate root view now live here.
fn inner_content_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
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

/// Drawer sidebar — themed surface, ~75% of viewport width.
/// Same column/padding as the detail-screen sheet; the
/// renderer's drawer-overlay pass paints this on top of the
/// body with the slide transform + scrim behind.
fn drawer_sidebar_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        background: Some(color_token(TOK_SURFACE, default_palette().surface)),
        background_transition: Some(theme_transition()),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Stretch),
        justify_content: Some(JustifyContent::FlexStart),
        padding_top: Some(px(48.0)),
        padding_right: Some(px(24.0)),
        padding_bottom: Some(px(48.0)),
        padding_left: Some(px(24.0)),
        gap: Some(px(16.0)),
        width: Some(pct(75.0)),
        ..Default::default()
    })
}

/// Sheet for the detail screen's root view. Same layout shape as
/// [`inner_content_sheet`] (column, padded), plus the theme
/// background — the home screen gets its background from the
/// outer scroll_view wrapper, but the detail screen is mounted
/// directly into the navigator so it needs to paint its own
/// fill or the home screen behind it would bleed through during
/// the push/pop slide.
fn detail_screen_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        background: Some(color_token(TOK_BACKGROUND, default_palette().background)),
        background_transition: Some(theme_transition()),
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
    themed(|| StyleRules {
        color: Some(color_token(TOK_TEXT, default_palette().text)),
        color_transition: Some(theme_transition()),
        font_size: Some(px(28.0)),
        ..Default::default()
    })
}

fn subtitle_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        color: Some(color_token(TOK_MUTED, default_palette().muted)),
        color_transition: Some(theme_transition()),
        font_size: Some(px(14.0)),
        ..Default::default()
    })
}

fn card_label_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        color: Some(color_token(TOK_TEXT, default_palette().text)),
        color_transition: Some(theme_transition()),
        font_size: Some(px(14.0)),
        ..Default::default()
    })
}

fn count_card_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        background: Some(color_token(TOK_SURFACE, default_palette().surface)),
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
        border_top_color: Some(color_token(TOK_BORDER, default_palette().border)),
        border_right_color: Some(color_token(TOK_BORDER, default_palette().border)),
        border_bottom_color: Some(color_token(TOK_BORDER, default_palette().border)),
        border_left_color: Some(color_token(TOK_BORDER, default_palette().border)),
        border_top_color_transition: Some(theme_transition()),
        gap: Some(px(4.0)),
        ..Default::default()
    })
}

fn count_value_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        color: Some(color_token(TOK_ACCENT, default_palette().accent)),
        color_transition: Some(theme_transition()),
        font_size: Some(px(36.0)),
        ..Default::default()
    })
}

fn pressable_sheet() -> Rc<StyleSheet> {
    themed_with_pressed(
        || StyleRules {
            background: Some(color_token(
                TOK_PRESSABLE_BG,
                default_palette().pressable_bg,
            )),
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
        || StyleRules {
            background: Some(color_token(
                TOK_PRESSABLE_BG_PRESSED,
                default_palette().pressable_bg_pressed,
            )),
            ..Default::default()
        },
    )
}

// ---------- Form input rows ----------

fn form_row_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        background: Some(color_token(TOK_SURFACE, default_palette().surface)),
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
    themed(|| StyleRules {
        background: Some(color_token(TOK_SURFACE, default_palette().surface)),
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
    themed(|| StyleRules {
        color: Some(color_token(TOK_TEXT, default_palette().text)),
        color_transition: Some(theme_transition()),
        font_size: Some(px(15.0)),
        ..Default::default()
    })
}

fn row_value_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        color: Some(color_token(TOK_MUTED, default_palette().muted)),
        color_transition: Some(theme_transition()),
        font_size: Some(px(13.0)),
        ..Default::default()
    })
}

fn spinner_row_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(px(12.0)),
        ..Default::default()
    })
}

fn icon_row_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(px(12.0)),
        ..Default::default()
    })
}

fn icon_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        color: Some(color_token(TOK_TEXT, default_palette().text)),
        width: Some(px(24.0)),
        height: Some(px(24.0)),
        ..Default::default()
    })
}

fn media_thumb_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        width: Some(px(96.0)),
        height: Some(px(64.0)),
        ..Default::default()
    })
}

fn overlay_card_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        background: Some(color_token(TOK_SURFACE, default_palette().surface)),
        padding_top: Some(px(24.0)),
        padding_right: Some(px(28.0)),
        padding_bottom: Some(px(24.0)),
        padding_left: Some(px(28.0)),
        border_top_left_radius: Some(px(16.0)),
        border_top_right_radius: Some(px(16.0)),
        border_bottom_right_radius: Some(px(16.0)),
        border_bottom_left_radius: Some(px(16.0)),
        gap: Some(px(8.0)),
        ..Default::default()
    })
}

fn overlay_title_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        color: Some(color_token(TOK_TEXT, default_palette().text)),
        font_size: Some(px(20.0)),
        ..Default::default()
    })
}

fn overlay_body_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        color: Some(color_token(TOK_MUTED, default_palette().muted)),
        font_size: Some(px(14.0)),
        ..Default::default()
    })
}

fn row_label_pair_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::SpaceBetween),
        ..Default::default()
    })
}

fn slider_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        flex_grow: Some(f32_literal(1.0)),
        ..Default::default()
    })
}

fn text_input_sheet() -> Rc<StyleSheet> {
    themed(|| StyleRules {
        color: Some(color_token(TOK_INPUT_TEXT, default_palette().input_text)),
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
    themed(|| StyleRules {
        background: Some(color_token(TOK_BACKGROUND, default_palette().background)),
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

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

fn pct(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Percent(v))
}

fn f32_literal(v: f32) -> Tokenized<f32> {
    Tokenized::Literal(v)
}
