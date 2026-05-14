//! The shared sample tree, used by every backend.
//!
//! Reads as a small dashboard app. Two `Counter` cards sit side by
//! side in a sectioned page layout; a footer row exposes "Reset" and
//! "Toggle theme" actions. The reset button drives both counters by
//! holding a `Ref<CounterHandle>` on each and calling `.reset()` —
//! exercises custom-component refs naturally. The login banner uses
//! the reactive `if` form for conditional rendering.

use framework_core::{
    component, install_theme, set_theme, signal, ui, AlignItems, Color, FlexDirection, FontWeight,
    JustifyContent, Length, Primitive, Ref, Shadow, Signal, TextAlign,
};

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

/// Semantic colors. `primary` is the brand accent (buttons, focus
/// states), `background` is the page surface, `surface` is the
/// elevated container surface (cards). `text` and `muted` separate
/// primary body text from secondary annotations.
#[derive(Clone)]
pub struct Colors {
    pub background: String,
    pub surface: String,
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

/// Spacing scale, mobile-friendly. Used as raw px values; the
/// `stylesheet!` macro auto-converts to `Length::Px` via `From<f32>`.
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
// Layout stylesheets
// =============================================================================

// Page — the outermost container. Vertical stack with generous gap,
// centered content with a max width, generous outer padding.
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

// Row — horizontal flex container with configurable gap. Default to
// stretching children to equal height so cards line up cleanly.
framework_core::stylesheet! {
    pub Row<Theme> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(t.spacing.md),
            align_items: AlignItems::Stretch,
        }
    }
}

// SpacedRow — like Row but pushes children to opposite ends. Used by
// the footer action bar.
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
// Typography stylesheets
// =============================================================================

// Title — big page-level title. Tracked letterspacing, semibold weight.
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

// Subtitle — secondary body text under the title.
framework_core::stylesheet! {
    pub Subtitle<Theme> {
        base(t) {
            color: Color(t.colors.muted.clone()),
            font_size: 16.0,
            line_height: 22.0,
        }
    }
}

// SectionHeading — small uppercase label above a content section.
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

// CardTitle — the bold label at the top of a card.
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

// CardValue — the big number inside a stat card.
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
// Component stylesheets
// =============================================================================

// Card — elevated surface, soft shadow, internal vertical layout with
// a small gap between title and content.
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

        // Animate the surface + text color when the theme swaps or
        // the variant flips. The backend handles per-frame
        // interpolation — no Rust-side ticking.
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

// PrimaryButton — branded action button. The label color comes from
// the theme's `primary_text` so it stays legible in both themes.
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

        // Hover (web only — silent no-op on mobile, by design).
        state hovered(t) {
            background: Color(t.colors.primary_hover.clone()),
        }

        // Pressed (web + mobile touch-down).
        state pressed(t) {
            background: Color(t.colors.primary_pressed.clone()),
        }

        // Disabled — driven by `disabled = ...` prop on the Button.
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

// SecondaryButton — outlined alternative for less-prominent actions.
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

// CounterButton — the small "+ 1" button inside a stat card.
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

// LoginBanner — the conditional welcome strip shown until the user
// "logs in". Subtle, takes the primary tone.
framework_core::stylesheet! {
    pub LoginBanner<Theme> {
        base(t) {
            background: Color(t.colors.primary.clone()),
            color: Color(t.colors.primary_text.clone()),
            padding_vertical: t.spacing.sm,
            padding_horizontal: t.spacing.md,
            border_radius: 8.0,
            font_size: 14.0,
            text_align: TextAlign::Center,
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

// FlatList container — fixed height so the inner scroller actually
// scrolls. Without a bounded height, flex-column ancestors would let
// the list grow unbounded.
framework_core::stylesheet! {
    pub ListContainer<Theme> {
        base(t) {
            background: Color(t.colors.surface.clone()),
            border_radius: 8.0,
            border_width: 1.0,
            border_color: Color(t.colors.border.clone()),
            height: 300.0,
        }
    }
}

// FlatList row — each item is a fixed-height row with subtle
// separator (via a 1px bottom border).
framework_core::stylesheet! {
    pub ListRow<Theme> {
        base(t) {
            padding_horizontal: t.spacing.md,
            padding_vertical: t.spacing.sm,
            border_bottom_width: 1.0,
            border_bottom_color: Color(t.colors.border.clone()),
            font_size: 14.0,
            color: Color(t.colors.text.clone()),
            // Match the fixed_size(48.0) we use in the FlatList call.
            height: 48.0,
            justify_content: JustifyContent::Center,
        }
    }
}

// =============================================================================
// Components
// =============================================================================

pub struct CounterProps {
    pub label: String,
    pub value: Signal<i32>,
    pub step: i32,
    pub tone: CardTone,
}

/// `Counter` is a stat card: it renders a labeled value and exposes an
/// increment button. The component declares a `reset()` method via
/// `methods!`, which the macro turns into a `CounterHandle` the parent
/// can bind to via `Ref<CounterHandle>`. The reset button in the
/// footer uses this to zero all counters at once.
#[component(default(step = 1, tone = CardTone::Neutral))]
pub fn counter(props: &CounterProps) -> framework_core::Bindable<CounterHandle> {
    // Pull the relevant fields out of `props` into owned/Copy locals
    // so the resulting `Bindable` doesn't borrow the props ref. The
    // component receives `&CounterProps` for ergonomics at the call
    // site; the body is responsible for projecting what it needs.
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
            // Reactive text: `.get()` inside `text(...)` (which the
            // macro emits from this position) triggers the reactivity
            // rewriter to wrap the argument in a closure automatically.
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
// App
// =============================================================================

#[component]
pub fn app() -> Primitive {
    install_theme(light_theme());

    let score = signal!(0);
    let lives = signal!(3);
    let is_dark = signal!(false);
    let logged_in = signal!(false);

    // Refs for resetting both counters in one parent action.
    let score_ref: Ref<CounterHandle> = Ref::new();
    let lives_ref: Ref<CounterHandle> = Ref::new();

    // Signals driving the primitives showcase.
    let name = signal!("".to_string());
    let notifications_on = signal!(true);
    let volume = signal!(0.5_f32);
    let loading = signal!(false);

    // Demo data for the FlatList. Each item has a stable u64 id so
    // the virtualizer can preserve mounted subtrees across data
    // changes. 1,000 rows — enough that mounting all would be silly,
    // demonstrating the windowing.
    let list_items = signal!(
        (0..1000u64)
            .map(|i| (i, format!("Item #{}", i)))
            .collect::<Vec<(u64, String)>>()
    );

    ui! {
        View(style = page_style()) {
            // Header section: title + subtitle.
            View {
                Text(style = title_style()) { "idealyst dashboard" }
                Text(style = subtitle_style()) {
                    "A tiny demo of the framework's signals, refs, and styles."
                }
            }

            // Welcome banner — visible until the user dismisses it via
            // "Reset all". The reactive `if` form rebuilds the subtree
            // when `logged_in` flips.
            if !logged_in.get() {
                View(style = login_banner_style()) {
                    Text { "Welcome — try incrementing a counter or toggling the theme." }
                }
            } else {
                View {}
            }

            // Stats section: section heading + two counter cards laid
            // horizontally with equal width (each Card has flex_grow: 1).
            View {
                Text(style = section_heading_style()) { "Stats" }
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
            }

            // Primitives showcase: text input + toggle + image. Each
            // is controlled — the parent owns the source-of-truth
            // signal, the primitive's `on_change` updates it, and the
            // framework writes the new value back to the native widget.
            View {
                Text(style = section_heading_style()) { "Primitives" }
                View {
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
                    // Slider — controlled f32 in [0.0, 1.0], step 0.05
                    // (so values snap to 0, 5%, 10%, ..., 100%).
                    View {
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
                    // ActivityIndicator + toggle to show/hide it. The
                    // `if` form rebuilds the subtree when `loading`
                    // flips; mounting an indicator is what triggers
                    // the keyframes injection on first use.
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
            }

            // FlatList showcase: a virtualized 1,000-row list. The
            // FlatList primitive owns a scrolling container; only
            // items near the viewport are mounted (windowing). On
            // web the JS shim handles the scroll math + ResizeObserver
            // for measured sizes; on native (Android v1) all items
            // are mounted up-front pending a RecyclerView follow-up.
            View {
                Text(style = section_heading_style()) { "FlatList — 1,000 rows" }
                FlatList(
                    data = list_items,
                    key = |_idx, item: &(u64, String)| item.0,
                    size = framework_core::primitives::flat_list::fixed_size::<(u64, String)>(48.0),
                    render = move |_idx, item: &(u64, String)| {
                        let label = item.1.clone();
                        ui! {
                            View(style = list_row_style()) {
                                Text { label }
                            }
                        }
                    },
                    style = list_container_style()
                )
            }

            // Action bar: primary "Reset all" pushes left, secondary
            // theme toggle pushes right. Demonstrates ref-driven
            // multi-component action: one click resets two independent
            // components by calling their handles.
            View(style = spaced_row_style()) {
                // Primary action — disabled when both counters are
                // already at zero. The `disabled = ...` closure reads
                // both signals; the framework wraps it in an Effect
                // so changes propagate automatically. While disabled,
                // PrimaryButton's `state disabled { opacity: 0.4 }`
                // overlay applies via the framework's state
                // machinery, and the native widget is marked inert
                // (web: `disabled` attr; Android: `setEnabled(false)`).
                Button(
                    label = "Reset all",
                    on_click = move || {
                        if let Some(h) = score_ref.get() { h.reset(); }
                        if let Some(h) = lives_ref.get() { h.reset(); }
                        logged_in.set(true);
                    },
                    style = primary_button_style(),
                    disabled = move || score.get() == 0 && lives.get() == 0
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
