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
    component, derived, install_theme, set_theme, signal, ui, AlignItems, Color, FlexDirection,
    FontWeight, JustifyContent, Length, Navigator, NavigatorHandle, Overflow, Primitive, Ref,
    Route, RouteParams, Shadow, Signal, TextAlign,
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

#[cfg(not(any(
    all(feature = "graphics", target_arch = "wasm32"),
    all(feature = "graphics", target_os = "android"),
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

/// The static set of routes the app declares. We use module-level
/// `Route` constants so call sites elsewhere (header buttons, deep
/// links) reference the same path patterns the navigator registers.
pub const HOME_ROUTE: Route<()> = Route::<()>::new("home", "/");
pub const PERF_ROUTE: Route<()> = Route::<()>::new("performance", "/performance");
pub const DETAIL_ROUTE: Route<DetailParams> = Route::<DetailParams>::new("detail", "/detail/:id");

// =============================================================================
// Header
// =============================================================================

pub struct HeaderProps {
    /// The navigator the header drives. The header issues `reset`
    /// for top-level tabs (Summary / Performance) since these are
    /// destinations rather than push-stack steps; deeper screens
    /// pushed elsewhere don't end up below them in the stack.
    pub nav: Ref<NavigatorHandle>,
    /// Active route name, used purely for highlighting the matching
    /// tab. Driven by the buttons themselves — the framework doesn't
    /// expose "what's mounted" from the navigator, so we track it
    /// here. Initialized to the home route at mount time.
    pub active: Signal<&'static str>,
    pub is_dark: Signal<bool>,
}

/// The app's persistent header. Three buttons drive the navigator
/// (two tabs + a theme toggle); the navigator owns the actual
/// screen rendering, so the header itself never rebuilds on
/// navigation — just the styles re-resolve via the `active` axis.
#[component]
pub fn header(props: &HeaderProps) -> Primitive {
    let nav = props.nav;
    let active = props.active;
    let is_dark = props.is_dark;

    ui! {
        View(style = header_bar_style()) {
            Text(style = header_title_style()) { "idealyst" }
            View(style = nav_group_style()) {
                Button(
                    label = "Summary",
                    on_click = move || {
                        if let Some(h) = nav.get() {
                            h.reset(&HOME_ROUTE, ());
                        }
                        active.set(HOME_ROUTE.name());
                    },
                    style = NavButton().active(derived(move || if active.get() == HOME_ROUTE.name() {
                        NavButtonActive::On
                    } else {
                        NavButtonActive::Off
                    }))
                )
                Button(
                    label = "Performance",
                    on_click = move || {
                        if let Some(h) = nav.get() {
                            h.reset(&PERF_ROUTE, ());
                        }
                        active.set(PERF_ROUTE.name());
                    },
                    style = NavButton().active(derived(move || if active.get() == PERF_ROUTE.name() {
                        NavButtonActive::On
                    } else {
                        NavButtonActive::Off
                    }))
                )
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

// =============================================================================
// Summary screen — one card per primitive category
// =============================================================================

pub struct SummaryProps {
    /// Handle to the navigator so we can push the detail screen.
    /// `Ref` is `Copy`-ish (numeric id), so passing it costs nothing.
    pub nav: Ref<NavigatorHandle>,
}

#[component]
pub fn summary(props: &SummaryProps) -> Primitive {
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
        View {
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
            View {
                Text(style = section_heading_style()) { "Navigation" }
                Text(style = subtitle_style()) {
                    "Push a detail screen with a typed `:id` param. \
                     On web the URL bar updates and the browser back \
                     button works; on native the platform back \
                     gesture pops the stack."
                }
                View(style = row_style()) {
                    Button(
                        label = "Open detail #42",
                        on_click = move || {
                            if let Some(h) = nav.get() {
                                h.push(&DETAIL_ROUTE, DetailParams { id: 42 });
                            }
                        },
                        style = secondary_button_style()
                    )
                    Button(
                        label = "Open detail #1337",
                        on_click = move || {
                            if let Some(h) = nav.get() {
                                h.push(&DETAIL_ROUTE, DetailParams { id: 1337 });
                            }
                        },
                        style = secondary_button_style()
                    )
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
        View {
            Text(style = title_style()) {
                format!("Detail #{}", id)
            }
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
            View(style = row_style()) {
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
}

// =============================================================================
// Performance screen — 1000 styled rows
// =============================================================================

/// Hard cap so a stray typo can't trigger a 10M-row rebuild.
const PERF_MAX_COUNT: usize = 10_000;

#[component]
pub fn performance() -> Primitive {
    // Three signals drive this screen:
    //
    // - `count_input`: the editing buffer the TextInput is bound to.
    //   Updates per keystroke; nothing else reads it until Apply.
    // - `count`: the *applied* count. Only changes when Apply parses
    //   `count_input` and clamps the result. The list rebuilds when
    //   this changes.
    // - `virtualized`: true means render through `FlatList`
    //   (windowed); false means render every row inline inside a
    //   `ScrollView`. The full point of the comparison.
    //
    // The list itself sits inside a reactive `match` on
    // `(virtualized, count)` so changing either rebuilds the
    // appropriate variant from scratch.
    let count = signal!(1000_usize);
    let count_input = signal!("1000".to_string());
    let virtualized = signal!(true);

    ui! {
        View {
            Text(style = section_heading_style()) { "N styled views" }
            Text(style = subtitle_style()) {
                "Compare windowed vs. all-at-once rendering. Toggle the theme to stress every row's style effect."
            }

            // Controls row.
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
                        // Echo the clamped value back into the input
                        // so the user sees what actually got applied.
                        count_input.set(clamped.to_string());
                    },
                    style = primary_button_style()
                )
                View(style = perf_toggle_group_style()) {
                    // Wrap the if in a single expression so the `ui!`
                    // parser treats this as reactive text content
                    // rather than a `when()` branch that would expect
                    // each arm to produce a `Primitive`.
                    Text(style = subtitle_style()) {
                        (if virtualized.get() { "Virtualized" } else { "All-at-once" }).to_string()
                    }
                    Toggle(
                        value = virtualized,
                        on_change = move |v: bool| virtualized.set(v)
                    )
                }
            }

            // The list. Reactive `match` over (mode, count): both
            // signals contribute to the switch key, so changing
            // either triggers a full rebuild via the framework's
            // `switch` primitive. Captured `n` is `&usize` (match
            // ergonomics over `&(bool, usize)`); we copy it out with
            // `&n` patterns and pass it to either arm's renderer.
            match (virtualized.get(), count.get()) {
                // `__v: &(bool, usize)` inside the switch closure.
                // Match ergonomics: the outer `(_, _)` auto-derefs,
                // and `n` binds as `&usize` under ref mode — we
                // copy it into a `usize` local at the top of the
                // arm body.
                (true, n) => {
                    // Wrap in a Rust block expression so the macro
                    // parser accepts the `let` statements (the
                    // arm body itself only accepts UI nodes; this
                    // inner `{ ... }` goes through the fallback
                    // expression path).
                    //
                    // Virtualized: fresh per-rebuild data signal.
                    // The framework windows what's visible; rows
                    // outside the scroll viewport are not mounted.
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
                    // Same wrapping trick as the virtualized arm:
                    // the inner `{ ... }` is a Rust block expression
                    // that hosts the `let` and ends with a single
                    // `ui!`-built expression.
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
// App
// =============================================================================

#[component]
pub fn app() -> Primitive {
    install_theme(light_theme());

    let is_dark = signal!(false);
    let active = signal!(HOME_ROUTE.name());
    let nav: Ref<NavigatorHandle> = Ref::new();

    ui! {
        View(style = page_style()) {
            // Wrapping View prevents the `ui!` parser from treating
            // the navigator block below as Header's children block.
            View { Header(nav = nav, active = active, is_dark = is_dark) }

            // The navigator is the screen-rendering substrate. It
            // declares every route up-front via `.screen(...)` and
            // exposes an imperative handle through `.bind(nav)`. On
            // web the navigator uses `history.pushState` + popstate
            // so URLs and the back button work; on native it
            // pushes/pops fragments (Android) or view controllers
            // (iOS). Routes are typed: `DETAIL_ROUTE` takes
            // `DetailParams`, so `nav.push(&DETAIL_ROUTE, ...)` is a
            // compile-time check that the call site supplies the
            // right param type.
            //
            // Each `.screen(...)` closure runs every time that
            // screen is mounted — push, replace, deep link, etc.
            // Inside each closure we re-enter `ui!` so the screen
            // builder reads as plain UI; the outer `ui!` doesn't
            // expand the body of closures, so calls like
            // `summary!(...)` would otherwise have to be written
            // raw.
            { Navigator::new(&HOME_ROUTE)
                .screen(HOME_ROUTE, move |_| {
                    let nav = nav;
                    ui! { Summary(nav = nav) }
                })
                .screen(PERF_ROUTE, |_| {
                    ui! { Performance() }
                })
                .screen(DETAIL_ROUTE, move |params: DetailParams| {
                    let nav = nav;
                    let id = params.id;
                    ui! { Detail(id = id, nav = nav) }
                })
                .bind(nav) }
        }
    }
}
