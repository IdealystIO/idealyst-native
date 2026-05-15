//! The shared sample tree, used by every backend.
//!
//! Two-screen demo app. A persistent `Header` at the top renders nav
//! buttons (Summary / Performance) and a theme toggle; the body
//! switches between screens based on a `Signal<Screen>` via the
//! framework's reactive `match` lowering (`ui!`'s `match` arm emits a
//! `framework_core::switch(...)` so only the active screen is mounted).
//!
//! - [`summary`] — a quick tour of every primitive category.
//! - [`performance`] — 1000 styled rows, alternating surface /
//!   surfaceAlt backgrounds. Toggling the theme re-runs every row's
//!   style effect; useful for eyeballing how the framework handles
//!   1000-fold style invalidation.

use framework_core::{
    component, install_theme, set_theme, signal, ui, AlignItems, Color, FlexDirection,
    FontWeight, JustifyContent, LayoutProps, Length, Navigator, NavigatorHandle, Overflow,
    Primitive, Ref, Route, RouteParams, Shadow, Signal, TextAlign,
};
use std::collections::HashMap;

// Animated gradient demo. Two flavors share the same WGSL shader
// + pipeline; only the platform glue differs:
//
//   gradient_web.rs       — wasm32: requestAnimationFrame loop,
//                           wasm-bindgen-futures for async init,
//                           js_sys::Date for time.
//   gradient_android.rs   — Android: dedicated render thread,
//                           pollster for async init, std::time
//                           for the clock.
//
// Without the `graphics` feature (or on platforms that don't
// implement Graphics yet), `gradient::gradient_canvas()` returns a
// static placeholder so the app still runs.
#[cfg(all(feature = "graphics", target_arch = "wasm32"))]
#[path = "gradient_web.rs"]
mod gradient;

#[cfg(all(feature = "graphics", target_os = "android"))]
#[path = "gradient_android.rs"]
mod gradient;

#[cfg(all(feature = "graphics", target_os = "ios"))]
#[path = "gradient_ios.rs"]
mod gradient;

#[cfg(not(any(
    all(feature = "graphics", target_arch = "wasm32"),
    all(feature = "graphics", target_os = "android"),
    all(feature = "graphics", target_os = "ios"),
)))]
mod gradient {
    use framework_core::{ui, Primitive};
    /// Stand-in used when no platform-specific gradient module is
    /// active (graphics feature off, or platform without a
    /// Graphics-primitive backend yet — currently iOS).
    pub fn gradient_canvas() -> Primitive {
        ui! {
            View(style = crate::gradient_canvas_style()) {
                Text(style = crate::subtitle_style()) {
                    "GPU canvas — enable the `graphics` feature on a supported platform to render the live gradient."
                }
            }
        }
    }
}

// =============================================================================
// Theme
// =============================================================================

/// App theme. Stylesheets read from this struct; swapping the theme
/// re-fires every styled effect via the framework's reactivity, so
/// dark mode propagates without a re-render.
#[derive(Clone)]
pub struct Theme {
    pub colors: Colors,
    pub spacing: Spacing,
}

#[derive(Clone)]
pub struct Colors {
    pub background: String,
    pub surface: String,
    /// Alternating surface for striped lists (perf screen rows).
    /// Distinct enough from `surface` that the parity is visible
    /// at a glance.
    pub surface_alt: String,
    pub primary: String,
    pub primary_hover: String,
    pub primary_pressed: String,
    pub primary_text: String,
    pub text: String,
    pub muted: String,
    pub border: String,
    pub border_hover: String,
    pub focus_ring: String,
}

#[derive(Clone)]
pub struct Spacing {
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
}

const SPACING: Spacing = Spacing { xs: 4.0, sm: 8.0, md: 16.0, lg: 24.0, xl: 32.0 };

pub fn light_theme() -> Theme {
    Theme {
        colors: Colors {
            background: "#f7f7fb".into(),
            surface: "#ffffff".into(),
            surface_alt: "#eef0f7".into(),
            primary: "#5b6cff".into(),
            primary_hover: "#4a5cf0".into(),
            primary_pressed: "#3947d6".into(),
            primary_text: "#ffffff".into(),
            text: "#1a1a1f".into(),
            muted: "#6b7280".into(),
            border: "#e4e6ef".into(),
            border_hover: "#b9bdcc".into(),
            focus_ring: "#5b6cff".into(),
        },
        spacing: SPACING.clone(),
    }
}

pub fn dark_theme() -> Theme {
    Theme {
        colors: Colors {
            background: "#0f1115".into(),
            surface: "#1a1d24".into(),
            surface_alt: "#262a35".into(),
            primary: "#8b9aff".into(),
            primary_hover: "#9eabff".into(),
            primary_pressed: "#7383f5".into(),
            primary_text: "#0f1115".into(),
            text: "#e8eaf0".into(),
            muted: "#9099a8".into(),
            border: "#2a2e3a".into(),
            border_hover: "#3d4252".into(),
            focus_ring: "#8b9aff".into(),
        },
        spacing: SPACING.clone(),
    }
}

// =============================================================================
// Stylesheets — page chrome
// =============================================================================

framework_core::stylesheet! {
    pub Page<Theme> {
        base(t) {
            background: Color(t.colors.background.clone()),
            color: Color(t.colors.text.clone()),
            padding: t.spacing.xl,
            gap: Length::Px(t.spacing.lg),
            min_height: Length::pct(100.0),
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

framework_core::stylesheet! {
    pub Row<Theme> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(t.spacing.md),
            align_items: AlignItems::Stretch,
        }
    }
}

framework_core::stylesheet! {
    pub SpacedRow<Theme> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(t.spacing.md),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
        }
    }
}

// =============================================================================
// Stylesheets — typography
// =============================================================================

framework_core::stylesheet! {
    pub Title<Theme> {
        base(t) {
            color: Color(t.colors.text.clone()),
            font_size: 32.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: -0.5,
            line_height: 38.0,
        }
    }
}

framework_core::stylesheet! {
    pub Subtitle<Theme> {
        base(t) {
            color: Color(t.colors.muted.clone()),
            font_size: 16.0,
            line_height: 22.0,
        }
    }
}

framework_core::stylesheet! {
    pub SectionHeading<Theme> {
        base(t) {
            color: Color(t.colors.muted.clone()),
            font_size: 12.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 1.0,
            text_transform: framework_core::TextTransform::Uppercase,
        }
    }
}

framework_core::stylesheet! {
    pub CardTitle<Theme> {
        base(t) {
            color: Color(t.colors.text.clone()),
            font_size: 14.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.5,
            text_transform: framework_core::TextTransform::Uppercase,
        }
    }
}

framework_core::stylesheet! {
    pub LinkText<Theme> {
        base(t) {
            color: Color(t.colors.primary.clone()),
            font_size: 14.0,
            font_weight: FontWeight::SemiBold,
        }
    }
}

framework_core::stylesheet! {
    pub CardValue<Theme> {
        base(t) {
            color: Color(t.colors.text.clone()),
            font_size: 36.0,
            font_weight: FontWeight::Bold,
            letter_spacing: -1.0,
            line_height: 42.0,
        }
    }
}

// =============================================================================
// Stylesheets — components
// =============================================================================

framework_core::stylesheet! {
    pub Card<Theme> {
        base(t) {
            background: Color(t.colors.surface.clone()),
            padding: t.spacing.lg,
            border_radius: 12.0,
            gap: Length::Px(t.spacing.sm),
            flex_grow: 1.0,
            shadow: Shadow {
                x: 0.0,
                y: 4.0,
                blur: 16.0,
                color: Color("rgba(15, 17, 21, 0.08)".into()),
            },
        }

        variant tone {
            #[default]
            neutral(_t) {}
            primary(t) {
                background: Color(t.colors.primary.clone()),
                color: Color(t.colors.primary_text.clone()),
            }
        }

        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

framework_core::stylesheet! {
    pub PrimaryButton<Theme> {
        base(t) {
            background: Color(t.colors.primary.clone()),
            color: Color(t.colors.primary_text.clone()),
            padding_vertical: t.spacing.sm,
            padding_horizontal: t.spacing.md,
            border_radius: 8.0,
            font_weight: FontWeight::SemiBold,
            font_size: 14.0,
            letter_spacing: 0.3,
            text_align: TextAlign::Center,
        }
        state hovered(t) {
            background: Color(t.colors.primary_hover.clone()),
        }
        state pressed(t) {
            background: Color(t.colors.primary_pressed.clone()),
        }
        state disabled(_t) {
            opacity: 0.4,
        }
        transitions {
            background: 150ms EaseOut,
            color: 200ms EaseOut,
            opacity: 200ms EaseOut,
        }
    }
}

framework_core::stylesheet! {
    pub SecondaryButton<Theme> {
        base(t) {
            background: Color("transparent".into()),
            color: Color(t.colors.text.clone()),
            padding_vertical: t.spacing.sm,
            padding_horizontal: t.spacing.md,
            border_radius: 8.0,
            border_width: 1.0,
            border_color: Color(t.colors.border.clone()),
            font_weight: FontWeight::Medium,
            font_size: 14.0,
            letter_spacing: 0.3,
            text_align: TextAlign::Center,
        }
        state hovered(t) {
            border_color: Color(t.colors.border_hover.clone()),
        }
        state pressed(t) {
            border_color: Color(t.colors.primary.clone()),
        }
        transitions {
            color: 200ms EaseOut,
            border_color: 150ms EaseOut,
        }
    }
}

// CounterButton — the small "+ N" button inside a stat card.
framework_core::stylesheet! {
    pub CounterButton<Theme> {
        base(t) {
            background: Color(t.colors.primary.clone()),
            color: Color(t.colors.primary_text.clone()),
            padding_vertical: t.spacing.xs,
            padding_horizontal: t.spacing.md,
            border_radius: 6.0,
            font_weight: FontWeight::SemiBold,
            font_size: 12.0,
            letter_spacing: 0.5,
            text_align: TextAlign::Center,
            margin_top: t.spacing.sm,
        }
        transitions {
            background: 200ms EaseOut,
            color: 200ms EaseOut,
        }
    }
}

framework_core::stylesheet! {
    pub GradientCanvas<Theme> {
        base(t) {
            background: Color(t.colors.surface.clone()),
            border_radius: 8.0,
            border_width: 1.0,
            border_color: Color(t.colors.border.clone()),
            height: 200.0,
            overflow: Overflow::Hidden,
        }
    }
}

// Header — full-width nav bar with screen buttons + theme toggle.
framework_core::stylesheet! {
    pub HeaderBar<Theme> {
        base(t) {
            background: Color(t.colors.surface.clone()),
            padding_vertical: t.spacing.sm,
            padding_horizontal: t.spacing.md,
            border_radius: 10.0,
            border_width: 1.0,
            border_color: Color(t.colors.border.clone()),
            flex_direction: FlexDirection::Row,
            gap: Length::Px(t.spacing.sm),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

// HeaderTitle — brand label on the left side of the header.
framework_core::stylesheet! {
    pub HeaderTitle<Theme> {
        base(t) {
            color: Color(t.colors.text.clone()),
            font_size: 18.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: -0.2,
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

// NavGroup — horizontal grouping of nav buttons + theme toggle.
framework_core::stylesheet! {
    pub NavGroup<Theme> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(t.spacing.xs),
            align_items: AlignItems::Center,
        }
    }
}

// NavButton — header nav button. The `active` variant marks the
// current screen so the highlighted button stands out without us
// rebuilding the header on every navigation.
framework_core::stylesheet! {
    pub NavButton<Theme> {
        base(t) {
            background: Color("transparent".into()),
            color: Color(t.colors.muted.clone()),
            padding_vertical: t.spacing.xs,
            padding_horizontal: t.spacing.md,
            border_radius: 6.0,
            font_weight: FontWeight::Medium,
            font_size: 14.0,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: Color(t.colors.primary.clone()),
                color: Color(t.colors.primary_text.clone()),
            }
        }
        state hovered(t) {
            color: Color(t.colors.text.clone()),
        }
        transitions {
            background: 200ms EaseOut,
            color: 200ms EaseOut,
        }
    }
}

// PerfRow — the row stylesheet used 1000× by the performance screen.
// `parity` variant flips between surface and surface_alt so adjacent
// rows alternate. Both base and variant read from the theme, so a
// theme toggle re-fires every row's apply-style effect.
framework_core::stylesheet! {
    pub PerfRow<Theme> {
        base(t) {
            padding_horizontal: t.spacing.md,
            padding_vertical: t.spacing.sm,
            background: Color(t.colors.surface.clone()),
            color: Color(t.colors.text.clone()),
            border_bottom_width: 1.0,
            border_bottom_color: Color(t.colors.border.clone()),
            font_size: 13.0,
            height: 36.0,
            justify_content: JustifyContent::Center,
        }
        variant parity {
            #[default]
            even(_t) {}
            odd(t) {
                background: Color(t.colors.surface_alt.clone()),
            }
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

// PerfList — outer container for the perf-screen scroller.
framework_core::stylesheet! {
    pub PerfList<Theme> {
        base(t) {
            background: Color(t.colors.surface.clone()),
            border_radius: 10.0,
            border_width: 1.0,
            border_color: Color(t.colors.border.clone()),
            height: 500.0,
            overflow: Overflow::Hidden,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

// PerfControls — horizontal row hosting the count input, Apply button,
// and the virtualized toggle. Tight padding, soft background, sits
// directly above the list.
framework_core::stylesheet! {
    pub PerfControls<Theme> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(t.spacing.md),
            align_items: AlignItems::Center,
            background: Color(t.colors.surface.clone()),
            padding_vertical: t.spacing.sm,
            padding_horizontal: t.spacing.md,
            border_radius: 10.0,
            border_width: 1.0,
            border_color: Color(t.colors.border.clone()),
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

// PerfCountInput — the numeric TextInput. Fixed width so it doesn't
// stretch the rest of the controls row.
framework_core::stylesheet! {
    pub PerfCountInput<Theme> {
        base(t) {
            background: Color(t.colors.background.clone()),
            color: Color(t.colors.text.clone()),
            padding_vertical: t.spacing.xs,
            padding_horizontal: t.spacing.sm,
            border_radius: 6.0,
            border_width: 1.0,
            border_color: Color(t.colors.border.clone()),
            font_size: 14.0,
            width: 120.0,
        }
    }
}

// PerfToggleGroup — Toggle + label, packed side-by-side on the right
// edge of the controls row.
framework_core::stylesheet! {
    pub PerfToggleGroup<Theme> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(t.spacing.sm),
            align_items: AlignItems::Center,
            // Push to the end of the row; the input + button occupy
            // the start.
            margin_left: Length::Auto,
        }
    }
}

// =============================================================================
// Counter (carried over — used by the Summary screen)
// =============================================================================

pub struct CounterProps {
    pub label: String,
    pub value: Signal<i32>,
    pub step: i32,
    pub tone: CardTone,
}

#[component(default(step = 1, tone = CardTone::Neutral))]
pub fn counter(props: &CounterProps) -> framework_core::Bindable<CounterHandle> {
    let value = props.value;
    let step = props.step;
    let tone = props.tone;
    let label = props.label.clone();
    let label_for_button = format!("+{}", step);

    methods! {
        fn reset(&self) {
            value.set(0);
        }
    }

    ui! {
        View(style = Card().tone(tone)) {
            Text(style = card_title_style()) { label }
            Text(style = card_value_style()) { format!("{}", value.get()) }
            Button(
                label = label_for_button,
                on_click = move || value.update(move |n| *n += step),
                style = counter_button_style()
            )
        }
    }
}

// =============================================================================
// Routing — see Navigator usage at the bottom of this file.
// =============================================================================

/// Typed params for the "detail" screen. The web backend serializes
/// these to `:id` in the URL and parses them back on deep links.
/// Native backends pass the struct unchanged across the framework's
/// type-erased command channel.
#[derive(Clone, Debug)]
pub struct DetailParams {
    pub id: u32,
}

impl RouteParams for DetailParams {
    fn to_path(&self, pattern: &str) -> String {
        // Substitute `:id` with our `id` field. The framework only
        // supports literal substitution; anything more complex
        // (encoding, multi-segment slugs) is the impl's problem.
        pattern.replace(":id", &self.id.to_string())
    }

    fn from_segments(segments: &HashMap<String, String>) -> Option<Self> {
        let raw = segments.get("id")?;
        let id = raw.parse().ok()?;
        Some(Self { id })
    }
}

/// The static set of routes the app declares. Module-level `Route`
/// constants so call sites everywhere (home cards, deep links) point
/// at the same path patterns the navigator registers.
///
/// Routes are organized as a flat list — the navigator is a single
/// stack, all screens push onto it from `Home`. Back goes home.
pub const HOME_ROUTE: Route<()> = Route::<()>::new("home", "/");
pub const SHOWCASE_ROUTE: Route<()> = Route::<()>::new("showcase", "/showcase");
pub const PERF_ROUTE: Route<()> = Route::<()>::new("performance", "/performance");
pub const LISTS_ROUTE: Route<()> = Route::<()>::new("lists", "/lists");
pub const DETAIL_ROUTE: Route<DetailParams> = Route::<DetailParams>::new("detail", "/detail/:id");

// =============================================================================
// Home screen — landing page with cards that push the other screens
// =============================================================================

pub struct HomeProps {
    /// Navigator handle. Every card click pushes a screen onto the
    /// stack; back from the pushed screen returns here.
    pub nav: Ref<NavigatorHandle>,
    /// App-level theme flag, owned at the app level so the theme
    /// outlives any single screen's scope. Flipped from this
    /// screen's "Toggle theme" button.
    pub is_dark: Signal<bool>,
}

/// Home / landing screen. Three nav cards push the matching screen;
/// the bottom card houses the theme toggle. No persistent header —
/// each pushed screen owns its own back button (and the platform
/// back gesture works automatically).
#[component]
pub fn home(props: &HomeProps) -> Primitive {
    let nav = props.nav;
    let is_dark = props.is_dark;

    // Every pushed screen owns its own background via `page_style`.
    // Without this, the screen's root would be transparent and a
    // hidden underlying fragment could bleed through (notably on
    // Android, where `FragmentTransaction.hide()` interacts oddly
    // with `setTransition(TRANSIT_FRAGMENT_OPEN)` and may not
    // actually remove the underlying fragment from drawing).
    ui! {
        View(style = page_style()) {
            Text(style = title_style()) { "idealyst" }
            Text(style = subtitle_style()) {
                "Cross-platform Rust UI framework. Pick a demo below; \
                 each screen pushes a new view-controller / fragment / \
                 path. Use the platform back gesture or the in-screen \
                 Back button to return."
            }

            // Demo cards. Each one pushes its target route. The
            // outer View uses `row_style` so cards sit side-by-side
            // when the screen is wide; on phones they wrap.
            View(style = row_style()) {
                Button(
                    label = "Primitives showcase",
                    on_click = move || {
                        if let Some(h) = nav.get() {
                            h.push(&SHOWCASE_ROUTE, ());
                        }
                    },
                    style = primary_button_style()
                )
                Button(
                    label = "Performance stress",
                    on_click = move || {
                        if let Some(h) = nav.get() {
                            h.push(&PERF_ROUTE, ());
                        }
                    },
                    style = primary_button_style()
                )
                Button(
                    label = "Lists / virtualizer",
                    on_click = move || {
                        if let Some(h) = nav.get() {
                            h.push(&LISTS_ROUTE, ());
                        }
                    },
                    style = primary_button_style()
                )
            }

            // Theme toggle. Pushing a screen and changing theme
            // are orthogonal — theme lives at the app level so it
            // persists across navigation.
            View {
                Text(style = section_heading_style()) { "Settings" }
                Button(
                    label = if is_dark.get() { "Light mode".to_string() } else { "Dark mode".to_string() },
                    on_click = move || {
                        let now_dark = !is_dark.get();
                        is_dark.set(now_dark);
                        if now_dark {
                            set_theme(dark_theme());
                        } else {
                            set_theme(light_theme());
                        }
                    },
                    style = secondary_button_style()
                )
            }
        }
    }
}

/// Small reusable top bar for pushed screens. Shows the screen
/// title and a Back button that pops the navigator. The platform
/// back gesture (Android back / iOS swipe / browser back) also
/// works; this is just the in-screen affordance.
///
/// Named `topbar` rather than the more natural `screen_header`
/// because the `ui!` macro lower-cases CamelCase identifiers without
/// snake-casing (`ScreenHeader` -> `screenheader`, not
/// `screen_header`). A single-token name avoids the mismatch.
#[component]
pub fn topbar(props: &TopbarProps) -> Primitive {
    let title = props.title.clone();
    let nav = props.nav;
    ui! {
        View(style = spaced_row_style()) {
            Text(style = title_style()) { title }
            Button(
                label = "Back",
                on_click = move || {
                    if let Some(h) = nav.get() {
                        h.pop();
                    }
                },
                style = secondary_button_style()
            )
        }
    }
}

pub struct TopbarProps {
    pub title: String,
    pub nav: Ref<NavigatorHandle>,
}

// =============================================================================
// Showcase screen — one card per primitive category
// =============================================================================

pub struct ShowcaseProps {
    /// Handle to the navigator so we can push the detail screen
    /// (and back-pop to home). `Ref` is `Copy`-ish (numeric id), so
    /// passing it costs nothing.
    pub nav: Ref<NavigatorHandle>,
}

#[component]
pub fn showcase(props: &ShowcaseProps) -> Primitive {
    let nav = props.nav;
    let score = signal!(0);
    let lives = signal!(3);

    // Form-control demos.
    let name = signal!("".to_string());
    let notifications_on = signal!(true);
    let volume = signal!(0.5_f32);
    let loading = signal!(false);

    let score_ref: Ref<CounterHandle> = Ref::new();
    let lives_ref: Ref<CounterHandle> = Ref::new();

    ui! {
        View(style = page_style()) {
            Topbar(title = "Primitives showcase".to_string(), nav = nav)

            // Interactive: counters drive a Ref-based "reset all".
            View {
                Text(style = section_heading_style()) { "Interactive" }
                View(style = row_style()) {
                    Counter(
                        label = "Score",
                        value = score,
                        step = 5,
                        tone = CardTone::Primary
                    ).bind(score_ref)
                    Counter(
                        label = "Lives",
                        value = lives
                    ).bind(lives_ref)
                }
                Button(
                    label = "Reset all counters",
                    on_click = move || {
                        if let Some(h) = score_ref.get() { h.reset(); }
                        if let Some(h) = lives_ref.get() { h.reset(); }
                    },
                    style = primary_button_style(),
                    disabled = move || score.get() == 0 && lives.get() == 0
                )
            }

            // Form controls: text input + toggle + slider.
            View {
                Text(style = section_heading_style()) { "Form controls" }
                Text(style = subtitle_style()) {
                    format!(
                        "Hello{}",
                        if name.get().is_empty() {
                            String::new()
                        } else {
                            format!(", {}", name.get())
                        }
                    )
                }
                TextInput(
                    value = name,
                    on_change = move |v: String| name.set(v),
                    placeholder = "Your name"
                )
                View(style = spaced_row_style()) {
                    Text(style = subtitle_style()) {
                        format!(
                            "Notifications: {}",
                            if notifications_on.get() { "on" } else { "off" }
                        )
                    }
                    Toggle(
                        value = notifications_on,
                        on_change = move |v: bool| notifications_on.set(v)
                    )
                }
                Text(style = subtitle_style()) {
                    format!("Volume: {}%", (volume.get() * 100.0).round() as i32)
                }
                Slider(
                    value = volume,
                    on_change = move |v: f32| volume.set(v),
                    min = 0.0_f32,
                    max = 1.0_f32,
                    step = 0.05_f32
                )
            }

            // Feedback: activity indicator with a manual toggle.
            View {
                Text(style = section_heading_style()) { "Feedback" }
                View(style = spaced_row_style()) {
                    Button(
                        label = "Toggle spinner",
                        on_click = move || loading.update(|b| *b = !*b),
                        style = secondary_button_style()
                    )
                    if loading.get() {
                        ActivityIndicator()
                    } else {
                        View {}
                    }
                }
            }

            // Navigation: push a typed-param detail screen onto the
            // navigator stack. Web: URL becomes /detail/42; native:
            // pushes a child VC / fragment. Either way the back
            // affordance returns you here.
            //
            // Three shapes for the same navigation, side by side:
            // - Imperative `nav.push` from a Button on_click.
            // - Declarative `Link` primitive with no plumbing — it
            //   finds the ambient navigator automatically. On web
            //   this renders a real `<a href>` so middle-click /
            //   cmd-click "open in new tab" works.
            View {
                Text(style = section_heading_style()) { "Navigation" }
                Text(style = subtitle_style()) {
                    "Three shapes for the same hop: imperative \
                     `nav.push` from a Button, and the `Link` \
                     primitive (declarative, finds the ambient \
                     navigator on its own — on web it's a real \
                     `<a href>` so middle-click opens a new tab)."
                }
                View(style = row_style()) {
                    Button(
                        label = "Open detail #42 (imperative)",
                        on_click = move || {
                            if let Some(h) = nav.get() {
                                h.push(&DETAIL_ROUTE, DetailParams { id: 42 });
                            }
                        },
                        style = secondary_button_style()
                    )
                    Link(route = &DETAIL_ROUTE, params = DetailParams { id: 1337 }) {
                        Text(style = link_text_style()) { "Open detail #1337 (link)" }
                    }
                }
            }

            // Graphics: GPU-rendered animated gradient. Feature-gated
            // so non-graphics builds get a flat tile.
            View {
                Text(style = section_heading_style()) { "GPU canvas" }
                gradient::gradient_canvas()
            }
        }
    }
}

// =============================================================================
// Detail screen — typed `:id` param demo
// =============================================================================

pub struct DetailProps {
    /// The id from the URL / push call. Lives only as long as this
    /// screen's per-screen scope — popping the screen drops every
    /// signal/effect that captured it.
    pub id: u32,
    /// Handle to the navigator so the back button works on web (the
    /// browser back button works automatically; this is for the
    /// in-page button).
    pub nav: Ref<NavigatorHandle>,
}

#[component]
pub fn detail(props: &DetailProps) -> Primitive {
    let id = props.id;
    let nav = props.nav;

    // Per-screen state: a click counter. Lives in this screen's
    // scope; popping the screen drops it. The point of this signal
    // is to show that per-screen state really is isolated — push
    // detail #42, click 3 times, pop, push detail #42 again: the
    // counter resets to 0 because the previous scope was dropped.
    // i32 to match Counter's expected `Signal<i32>`.
    let clicks = signal!(0_i32);

    ui! {
        View(style = page_style()) {
            Topbar(title = format!("Detail #{}", id), nav = nav)
            Text(style = subtitle_style()) {
                "This screen owns its own reactive scope. Anything \
                 the screen body captured (the counter below) goes \
                 away when you pop back, even if you push the same \
                 route again."
            }
            View(style = row_style()) {
                Counter(
                    label = "Clicks on this screen",
                    value = clicks
                )
            }
        }
    }
}

// =============================================================================
// Performance screen — pure 1000-row styled-views stress
// =============================================================================
//
// All-at-once rendering: every row is mounted inline inside a
// `ScrollView`. The point is to stress per-styled-node effects —
// toggling the theme re-fires every row's apply-style effect. No
// virtualization here on purpose; for the windowed comparison see
// the Lists screen.

/// Hard cap so a stray typo can't trigger a 10M-row rebuild. Same
/// limit on both Performance and Lists for consistency.
const PERF_MAX_COUNT: usize = 100_000;

pub struct PerformanceProps {
    pub nav: Ref<NavigatorHandle>,
}

#[component]
pub fn performance(props: &PerformanceProps) -> Primitive {
    let nav = props.nav;
    // Two signals: the editing-buffer for the TextInput, and the
    // applied count that drives the rebuild. `count_input` updates
    // per keystroke; `count` only changes when Apply parses + clamps.
    let count = signal!(1000_usize);
    let count_input = signal!("1000".to_string());

    ui! {
        View(style = page_style()) {
            Topbar(title = "Performance".to_string(), nav = nav)
            Text(style = subtitle_style()) {
                "Inline rendering of N styled rows. Toggling the theme \
                 re-fires every row's apply-style effect; this is the \
                 worst case for the style pipeline."
            }

            // Controls row: count input + Apply button.
            View(style = perf_controls_style()) {
                TextInput(
                    value = count_input,
                    on_change = move |v: String| count_input.set(v),
                    placeholder = "Item count",
                    style = perf_count_input_style()
                )
                Button(
                    label = "Apply",
                    on_click = move || {
                        let parsed = count_input.get().trim().parse::<usize>().unwrap_or(0);
                        let clamped = parsed.min(PERF_MAX_COUNT);
                        count.set(clamped);
                        // Echo the clamped value back so the user sees
                        // what actually got applied.
                        count_input.set(clamped.to_string());
                    },
                    style = primary_button_style()
                )
            }

            // Reactive `match` on `count` — the list rebuilds from
            // scratch when Apply commits a new count. Wrapping in a
            // single-arm match (instead of a `for`-loop sitting
            // directly in the View) lets us scope the count read.
            match count.get() {
                n => {
                    {
                        let n: usize = *n;
                        ui! {
                            ScrollView(style = perf_list_style()) {
                                for i in 0..n {
                                    View(style = PerfRow().parity(if i % 2 == 0 {
                                        PerfRowParity::Even
                                    } else {
                                        PerfRowParity::Odd
                                    })) {
                                        Text { format!("Row #{}", i) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// =============================================================================
// Lists screen — virtualizer / FlatList demo
// =============================================================================
//
// Same row stylesheet as Performance, but with a virtualized-vs-
// all-at-once toggle. The point is to compare the windowing behavior
// of `FlatList` against the inline `for` loop: scrolling, theme
// invalidation cost, and the per-item scope teardown that
// virtualization gives you for free.

pub struct ListsProps {
    pub nav: Ref<NavigatorHandle>,
}

#[component]
pub fn lists(props: &ListsProps) -> Primitive {
    let nav = props.nav;
    let count = signal!(1000_usize);
    let count_input = signal!("1000".to_string());
    let virtualized = signal!(true);

    ui! {
        View(style = page_style()) {
            Topbar(title = "Lists".to_string(), nav = nav)
            Text(style = subtitle_style()) {
                "Windowed (FlatList) vs. all-at-once rendering. With \
                 virtualization on, rows outside the viewport are not \
                 mounted — scroll a large list to see the per-item \
                 mount/release lifecycle in action."
            }

            // Controls row: count + Apply + virtualization toggle.
            View(style = perf_controls_style()) {
                TextInput(
                    value = count_input,
                    on_change = move |v: String| count_input.set(v),
                    placeholder = "Item count",
                    style = perf_count_input_style()
                )
                Button(
                    label = "Apply",
                    on_click = move || {
                        let parsed = count_input.get().trim().parse::<usize>().unwrap_or(0);
                        let clamped = parsed.min(PERF_MAX_COUNT);
                        count.set(clamped);
                        count_input.set(clamped.to_string());
                    },
                    style = primary_button_style()
                )
                View(style = perf_toggle_group_style()) {
                    Text(style = subtitle_style()) {
                        (if virtualized.get() { "Virtualized" } else { "All-at-once" }).to_string()
                    }
                    Toggle(
                        value = virtualized,
                        on_change = move |v: bool| virtualized.set(v)
                    )
                }
            }

            // Reactive `match` over (mode, count): both signals
            // contribute to the switch key, so changing either
            // triggers a full rebuild via the framework's `switch`
            // primitive.
            match (virtualized.get(), count.get()) {
                (true, n) => {
                    {
                        let n: usize = *n;
                        let data = signal!((0..n as u64).collect::<Vec<u64>>());
                        framework_core::IntoPrimitive::into_primitive(
                            framework_core::primitives::flat_list::flat_list::<
                                u64, _, (), _,
                            >(
                                data,
                                |_idx, item: &u64| *item,
                                framework_core::primitives::flat_list::fixed_size::<u64>(36.0),
                                move |idx, _item: &u64| {
                                    ui! {
                                        View(style = PerfRow().parity(if idx % 2 == 0 {
                                            PerfRowParity::Even
                                        } else {
                                            PerfRowParity::Odd
                                        })) {
                                            Text { format!("Row #{}", idx) }
                                        }
                                    }
                                },
                            )
                            .with_style(perf_list_style()),
                        )
                    }
                }
                (false, n) => {
                    {
                        let n: usize = *n;
                        ui! {
                            ScrollView(style = perf_list_style()) {
                                for i in 0..n {
                                    View(style = PerfRow().parity(if i % 2 == 0 {
                                        PerfRowParity::Even
                                    } else {
                                        PerfRowParity::Odd
                                    })) {
                                        Text { format!("Row #{}", i) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// =============================================================================
// Web layout — chrome wrapper invoked only on backends that opt in
// (web today; future SSR). Native backends draw their own nav chrome
// and ignore `.layout()`.
// =============================================================================

/// Build the web layout subtree. Receives reactive nav-state
/// signals + the outlet primitive; returns the chrome subtree with
/// the outlet embedded.
///
/// The signals are framework-owned and re-fire dependent effects on
/// each push/pop — so the route-name `Text` re-renders whenever
/// `active_route` changes, but the surrounding chrome (sidebar
/// background, layout boxes) stays mounted.
/// Build the web layout for the app. Takes the app-level
/// `is_dark` signal so the persistent header can host a theme
/// toggle that survives navigation — the layout chrome stays
/// mounted across push/pop, so the toggle is exactly one click
/// away regardless of which screen the user is on.
///
/// Returns a closure suitable for `Navigator::new(...).layout(...)`.
/// Each per-screen mount re-evaluates the layout's reactive bits
/// (route name, back-button visibility, dark-mode label) but the
/// surrounding chrome stays mounted.
pub fn web_layout_with_theme(is_dark: Signal<bool>) -> impl Fn(LayoutProps) -> Primitive + 'static {
    move |props: LayoutProps| {
        let active_route = props.active_route;
        let can_go_back = props.can_go_back;
        let on_back = props.on_back;

        ui! {
            View(style = page_style()) {
                // Persistent web chrome. Lives across navigation; only
                // its reactive bits (active route name, back-button
                // visibility, dark-mode label) re-fire on push/pop.
                // The outlet below hosts the active screen — the
                // framework physically swaps the outlet's child
                // without rebuilding this surrounding tree.
                View(style = header_bar_style()) {
                    Text(style = header_title_style()) {
                        format!("idealyst — {}", active_route.get())
                    }
                    View(style = nav_group_style()) {
                        Button(
                            label = if is_dark.get() {
                                "Light mode".to_string()
                            } else {
                                "Dark mode".to_string()
                            },
                            on_click = move || {
                                let now_dark = !is_dark.get();
                                is_dark.set(now_dark);
                                if now_dark {
                                    set_theme(dark_theme());
                                } else {
                                    set_theme(light_theme());
                                }
                            },
                            style = secondary_button_style()
                        )
                        if can_go_back.get() {
                            Button(
                                label = "Back",
                                on_click = {
                                    let on_back = on_back.clone();
                                    move || on_back()
                                },
                                style = secondary_button_style()
                            )
                        } else {
                            View {}
                        }
                    }
                }

                // The outlet: where the active screen renders. The
                // framework swaps the child of this View on push/pop.
                props.outlet
            }
        }
    }
}

// =============================================================================
// App
// =============================================================================

#[component]
pub fn app() -> Primitive {
    install_theme(light_theme());

    // App-level state lives here so it survives navigation — every
    // pushed screen drops its own per-screen scope on pop, but the
    // theme flag is owned by `app` whose scope is the framework's
    // root scope.
    let is_dark = signal!(false);
    let nav: Ref<NavigatorHandle> = Ref::new();

    // The navigator is the root primitive. Each screen owns its own
    // `page_style()` wrapper with padding/background, so no outer
    // View is needed. On native (iOS, Android) the navigator maps
    // to the platform nav container (UINavigationController /
    // FragmentManager) and should be full-bleed; on web the
    // `.layout()` chrome provides the surrounding page structure.
    ui! {
        { Navigator::new(&HOME_ROUTE)
            .screen(HOME_ROUTE, move |_| {
                let nav = nav;
                let is_dark = is_dark;
                ui! { Home(nav = nav, is_dark = is_dark) }
            })
            .screen(SHOWCASE_ROUTE, move |_| {
                let nav = nav;
                ui! { Showcase(nav = nav) }
            })
            .screen(PERF_ROUTE, move |_| {
                let nav = nav;
                ui! { Performance(nav = nav) }
            })
            .screen(LISTS_ROUTE, move |_| {
                let nav = nav;
                ui! { Lists(nav = nav) }
            })
            .screen(DETAIL_ROUTE, move |params: DetailParams| {
                let nav = nav;
                let id = params.id;
                ui! { Detail(id = id, nav = nav) }
            })
            .layout(web_layout_with_theme(is_dark))
            .bind(nav) }
    }
}
