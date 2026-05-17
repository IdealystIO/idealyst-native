//! AAS demo with a stack navigator.
//!
//! - `Home` screen: counter + math compute buttons (server-side
//!   compute) + "Open detail →" link to push a second screen.
//! - `Detail` screen: a small page with a back button.
//!
//! The whole app — state, computations, navigator — lives in the
//! dev-server process. Each connected client (web + iOS) is a thin
//! interpreter for the wire.
//!
//! Edit this file and save. The dev-server rebuilds and self-execs.
//! If you had the detail screen open, the server's nav-state
//! snapshot is passed forward in an env var, restored on startup,
//! and replayed via `NavigatorPush` — so the detail screen survives
//! the rebuild.

use std::rc::Rc;

use framework_core::primitives::link::link;
use framework_core::primitives::navigator::{ambient_navigator, Navigator, Route};
use framework_core::{
    AlignItems, Color, FlexDirection, IntoPrimitive, JustifyContent, Length, Primitive, Signal,
    StyleApplication, StyleRules, StyleSheet, TextAlign, TextSource, ThemeTokens, Tokenized,
};

pub mod print_backend;

// ============================================================================
// Theme + style helpers
// ============================================================================

struct DemoTheme;
impl ThemeTokens for DemoTheme {
    fn tokens(&self) -> Vec<framework_core::TokenEntry> {
        Vec::new()
    }
}

fn style(rules: StyleRules) -> framework_core::StyleSource {
    framework_core::StyleSource::Static(style_app(rules))
}

/// Build a `StyleApplication` for the rules. Distinct from
/// [`style`] because the framework's `Bound::with_style` takes
/// `impl IntoStyleSource` — and that trait is implemented for
/// `StyleApplication` but not for the already-wrapped `StyleSource`.
fn style_app(rules: StyleRules) -> StyleApplication {
    let sheet = Rc::new(StyleSheet::new::<DemoTheme, _>(move |_| rules.clone()));
    StyleApplication::new(sheet)
}

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}
fn color(hex: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(hex.to_string()))
}

fn page_style() -> StyleRules {
    StyleRules {
        background: Some(color("#0f172a")),
        color: Some(color("#e2e8f0")),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::FlexStart),
        gap: Some(px(22.0)),
        padding_top: Some(px(40.0)),
        padding_bottom: Some(px(40.0)),
        padding_left: Some(px(48.0)),
        padding_right: Some(px(48.0)),
        min_height: Some(Tokenized::Literal(Length::Percent(100.0))),
        font_family: Some("system-ui, -apple-system, Segoe UI, sans-serif".into()),
        ..StyleRules::default()
    }
}

fn h1_style() -> StyleRules {
    StyleRules {
        color: Some(color("#f8fafc")),
        font_size: Some(px(34.0)),
        font_weight: Some(framework_core::FontWeight::Bold),
        ..StyleRules::default()
    }
}

fn subtitle_style() -> StyleRules {
    StyleRules {
        color: Some(color("#94a3b8")),
        font_size: Some(px(15.0)),
        max_width: Some(px(620.0)),
        ..StyleRules::default()
    }
}

fn primary_btn_style(bg: &'static str) -> StyleRules {
    StyleRules {
        background: Some(color(bg)),
        color: Some(color("#ffffff")),
        font_size: Some(px(14.0)),
        font_weight: Some(framework_core::FontWeight::SemiBold),
        padding_top: Some(px(10.0)),
        padding_bottom: Some(px(10.0)),
        padding_left: Some(px(18.0)),
        padding_right: Some(px(18.0)),
        border_top_left_radius: Some(px(8.0)),
        border_top_right_radius: Some(px(8.0)),
        border_bottom_left_radius: Some(px(8.0)),
        border_bottom_right_radius: Some(px(8.0)),
        text_align: Some(TextAlign::Center),
        ..StyleRules::default()
    }
}

fn row_style() -> StyleRules {
    StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::FlexStart),
        gap: Some(px(10.0)),
        ..StyleRules::default()
    }
}

fn link_pill_style() -> StyleRules {
    StyleRules {
        background: Some(color("#1e293b")),
        color: Some(color("#38bdf8")),
        font_size: Some(px(14.0)),
        font_weight: Some(framework_core::FontWeight::Medium),
        padding_top: Some(px(10.0)),
        padding_bottom: Some(px(10.0)),
        padding_left: Some(px(16.0)),
        padding_right: Some(px(16.0)),
        border_top_left_radius: Some(px(8.0)),
        border_top_right_radius: Some(px(8.0)),
        border_bottom_left_radius: Some(px(8.0)),
        border_bottom_right_radius: Some(px(8.0)),
        ..StyleRules::default()
    }
}

// ============================================================================
// Server-side math
// ============================================================================

fn square(n: i64) -> i64 {
    n.saturating_mul(n)
}

fn factorial(n: i64) -> String {
    if n < 0 {
        return "(undefined for negatives)".into();
    }
    if n > 20 {
        return "(too large)".into();
    }
    let mut acc: u64 = 1;
    for k in 1..=(n as u64) {
        acc = acc.saturating_mul(k);
    }
    acc.to_string()
}

fn is_prime(n: i64) -> bool {
    if n < 2 {
        return false;
    }
    if n < 4 {
        return true;
    }
    if n % 2 == 0 {
        return false;
    }
    let mut k: i64 = 3;
    while k.saturating_mul(k) <= n {
        if n % k == 0 {
            return false;
        }
        k += 2;
    }
    true
}

// ============================================================================
// Screens
// ============================================================================

fn home_screen(counter: Signal<i64>, last_result: Signal<String>) -> Primitive {
    // Detail route — declared again here because we need it to
    // construct the Link. `Route::new` is a const fn, so this is
    // essentially free.
    let detail_route: Route<()> = Route::new("detail", "/detail");

    let counter_display = StyleRules {
        color: Some(color("#fbbf24")),
        font_size: Some(px(48.0)),
        font_weight: Some(framework_core::FontWeight::Bold),
        font_family: Some("ui-monospace, SFMono-Regular, monospace".into()),
        ..StyleRules::default()
    };
    let result_display = StyleRules {
        color: Some(color("#e2e8f0")),
        background: Some(color("#1e293b")),
        font_size: Some(px(15.0)),
        font_family: Some("ui-monospace, SFMono-Regular, monospace".into()),
        padding_top: Some(px(12.0)),
        padding_bottom: Some(px(12.0)),
        padding_left: Some(px(16.0)),
        padding_right: Some(px(16.0)),
        border_top_left_radius: Some(px(8.0)),
        border_top_right_radius: Some(px(8.0)),
        border_bottom_left_radius: Some(px(8.0)),
        border_bottom_right_radius: Some(px(8.0)),
        min_width: Some(px(360.0)),
        ..StyleRules::default()
    };

    let on_inc = Rc::new(move || counter.set(counter.get() + 1));
    let on_dec = Rc::new(move || counter.set(counter.get() - 1));
    let on_reset = Rc::new(move || counter.set(0));
    let on_square = Rc::new(move || {
        let n = counter.get();
        let r = square(n);
        println!("[server] square({}) = {}", n, r);
        last_result.set(format!("{}² = {}", n, r));
    });
    let on_factorial = Rc::new(move || {
        let n = counter.get();
        let r = factorial(n);
        println!("[server] factorial({}) = {}", n, r);
        last_result.set(format!("{}! = {}", n, r));
    });
    let on_is_prime = Rc::new(move || {
        let n = counter.get();
        let r = is_prime(n);
        println!("[server] is_prime({}) = {}", n, r);
        last_result.set(format!(
            "{} is {}prime",
            n,
            if r { "" } else { "not " }
        ));
    });

    // The Link primitive. The framework's `link()` constructor
    // captures the ambient navigator at call time — which is
    // exactly when this screen is being rendered by the
    // navigator's `mount_screen`. Activating the link dispatches a
    // `Push` against that captured `NavigatorControl`.
    let to_detail = link(
        &detail_route,
        (),
        vec![Primitive::Text {
            source: TextSource::Static("Open detail →".into()),
            style: None,
            ref_fill: None,
            test_id: None,
        }],
    )
    .with_style(style_app(link_pill_style()))
    .into_primitive();

    Primitive::View {
        children: vec![
            Primitive::Text {
                source: TextSource::Static("Server-side state · live".into()),
                style: Some(style(subtitle_style())),
                ref_fill: None,
                test_id: None,
            },
            Primitive::Text {
                source: TextSource::Static("Home".into()),
                style: Some(style(h1_style())),
                ref_fill: None,
                test_id: None,
            },
            Primitive::Text {
                source: TextSource::Reactive(Box::new(move || format!("{}", counter.get()))),
                style: Some(style(counter_display)),
                ref_fill: None,
                test_id: None,
            },
            Primitive::View {
                children: vec![
                    Primitive::Button {
                        label: TextSource::Static("−1".into()),
                        on_click: on_dec,
                        leading_icon: None,
                        trailing_icon: None,
                        style: Some(style(primary_btn_style("#475569"))),
                        ref_fill: None,
                        disabled: None,
                        test_id: None,
                    },
                    Primitive::Button {
                        label: TextSource::Static("+1".into()),
                        on_click: on_inc,
                        leading_icon: None,
                        trailing_icon: None,
                        style: Some(style(primary_btn_style("#2563eb"))),
                        ref_fill: None,
                        disabled: None,
                        test_id: None,
                    },
                    Primitive::Button {
                        label: TextSource::Static("reset".into()),
                        on_click: on_reset,
                        leading_icon: None,
                        trailing_icon: None,
                        style: Some(style(primary_btn_style("#1e293b"))),
                        ref_fill: None,
                        disabled: None,
                        test_id: None,
                    },
                ],
                style: Some(style(row_style())),
                ref_fill: None,
                safe_area_sides: framework_core::SafeAreaSides::NONE,
                test_id: None,
            },
            Primitive::View {
                children: vec![
                    Primitive::Button {
                        label: TextSource::Static("Square it".into()),
                        on_click: on_square,
                        leading_icon: None,
                        trailing_icon: None,
                        style: Some(style(primary_btn_style("#16a34a"))),
                        ref_fill: None,
                        disabled: None,
                        test_id: None,
                    },
                    Primitive::Button {
                        label: TextSource::Static("Factorial".into()),
                        on_click: on_factorial,
                        leading_icon: None,
                        trailing_icon: None,
                        style: Some(style(primary_btn_style("#d97706"))),
                        ref_fill: None,
                        disabled: None,
                        test_id: None,
                    },
                    Primitive::Button {
                        label: TextSource::Static("Prime?".into()),
                        on_click: on_is_prime,
                        leading_icon: None,
                        trailing_icon: None,
                        style: Some(style(primary_btn_style("#7c3aed"))),
                        ref_fill: None,
                        disabled: None,
                        test_id: None,
                    },
                ],
                style: Some(style(row_style())),
                ref_fill: None,
                safe_area_sides: framework_core::SafeAreaSides::NONE,
                test_id: None,
            },
            Primitive::Text {
                source: TextSource::Reactive(Box::new(move || last_result.get())),
                style: Some(style(result_display)),
                ref_fill: None,
                test_id: None,
            },
            // Navigation: push the detail screen.
            to_detail,
        ],
        style: Some(style(page_style())),
        ref_fill: None,
        safe_area_sides: framework_core::SafeAreaSides::NONE,
        test_id: None,
    }
}

fn detail_screen() -> Primitive {
    // Grab the ambient `NavigatorControl` so the back button can
    // dispatch `Pop`. The framework sets this thread-local before
    // invoking each screen's render closure.
    let nav = ambient_navigator();
    let on_back: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(n) = &nav {
            n.pop();
        }
    });

    Primitive::View {
        children: vec![
            Primitive::Text {
                source: TextSource::Static("Detail".into()),
                style: Some(style(h1_style())),
                ref_fill: None,
                test_id: None,
            },
            Primitive::Text {
                source: TextSource::Static(
                    "You're one push deep. Reload the page (or restart the iOS app, \
                     or edit hot-reload-demo/src/lib.rs to trigger a server rebuild) \
                     and you should land back here — the dev-server preserves the \
                     navigator stack across both."
                        .into(),
                ),
                style: Some(style(subtitle_style())),
                ref_fill: None,
                test_id: None,
            },
            Primitive::Button {
                label: TextSource::Static("← Back".into()),
                on_click: on_back,
                leading_icon: None,
                trailing_icon: None,
                style: Some(style(primary_btn_style("#475569"))),
                ref_fill: None,
                disabled: None,
                test_id: None,
            },
        ],
        style: Some(style(page_style())),
        ref_fill: None,
        safe_area_sides: framework_core::SafeAreaSides::NONE,
        test_id: None,
    }
}

// ============================================================================
// Root
// ============================================================================

pub fn app_root() -> Primitive {
    framework_core::install_theme(DemoTheme);

    // Signals live in the OUTER (root) scope so they survive
    // across screen mounts. If we created them inside
    // `home_screen()` they'd reset every time we popped back to
    // home.
    let counter = Signal::new(0i64);
    let last_result = Signal::new("(press a compute button)".to_string());

    // `Navigator::new` borrows the initial route; `.screen` takes
    // route by value. `Route::new` is const, so we just construct
    // each one freshly.
    let initial: Route<()> = Route::new("home", "/");
    let home_route: Route<()> = Route::new("home", "/");
    let detail_route: Route<()> = Route::new("detail", "/detail");

    Navigator::new(&initial)
        .screen(home_route, move |_: ()| home_screen(counter, last_result))
        .screen(detail_route, |_: ()| detail_screen())
        .into_primitive()
}
