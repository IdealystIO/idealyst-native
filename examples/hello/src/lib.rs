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
    component, install_theme, set_theme, signal, ui, AlignItems, AnchorTarget, BackdropMode,
    ButtonHandle, Color, DrawerHandle, DrawerItem, DrawerNavigator, Easing, ElementAlign,
    ElementAnchor, ElementSide, FlexDirection, FontWeight, JustifyContent, LayoutProps, Length,
    Navigator, NavigatorHandle, Overflow, OverlayAnchor, PresenceAnim, PresenceState, Primitive,
    Ref, Route, RouteParams, Shadow, Signal, TabNavigator, TabSpec, TabsHandle, TextAlign,
    ViewportPlacement,
};
use std::collections::HashMap;

// Animated gradient demo. One file, three platforms — the same
// wgpu code runs unmodified because wgpu is already cross-platform.
// What used to require three per-platform files was driver glue
// (frame ticker + async runtime), which now lives behind
// `framework_core::driver::{render_loop, spawn_async}` (gated on
// framework-core's `async-driver` feature, which our `graphics`
// feature forwards to).
//
// The example's `wgpu` dependency is only declared for the three
// targets that actually have a wgpu backend wired up (web / Android
// / iOS — see Cargo.toml). On the host linux/macOS/Windows dev
// builds, or with the feature off entirely, we use a placeholder so
// `cargo check -p hello` still works from a workstation.
#[cfg(all(
    feature = "graphics",
    any(target_arch = "wasm32", target_os = "android", target_os = "ios"),
))]
mod gradient;

#[cfg(not(all(
    feature = "graphics",
    any(target_arch = "wasm32", target_os = "android", target_os = "ios"),
)))]
mod gradient {
    use framework_core::{ui, Primitive};
    /// Stand-in for builds where the live gradient can't render —
    /// the `graphics` feature is off, or this is a host/desktop dev
    /// build with no wgpu backend wired up.
    pub fn gradient_canvas() -> Primitive {
        ui! {
            View(style = crate::gradient_canvas_style()) {
                Text(style = crate::subtitle_style()) {
                    "GPU canvas — enable the `graphics` feature on a supported target (web / Android / iOS) to render the live gradient."
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

/// Each color is a `Tokenized<framework_core::Color>` — a token
/// reference whose fallback is the current theme's literal value.
/// Stylesheets close over these directly; the resulting `StyleRules`
/// carry the token name into the content key, so the same minted CSS
/// class is reused under any theme. Theme swap mutates `:root`
/// variables in place — no `className` mutation on any node.
#[derive(Clone)]
pub struct Colors {
    pub background: framework_core::Tokenized<framework_core::Color>,
    pub surface: framework_core::Tokenized<framework_core::Color>,
    /// Alternating surface for striped lists (perf screen rows).
    /// Distinct enough from `surface` that the parity is visible
    /// at a glance.
    pub surface_alt: framework_core::Tokenized<framework_core::Color>,
    pub primary: framework_core::Tokenized<framework_core::Color>,
    pub primary_hover: framework_core::Tokenized<framework_core::Color>,
    pub primary_pressed: framework_core::Tokenized<framework_core::Color>,
    pub primary_text: framework_core::Tokenized<framework_core::Color>,
    pub text: framework_core::Tokenized<framework_core::Color>,
    pub muted: framework_core::Tokenized<framework_core::Color>,
    pub border: framework_core::Tokenized<framework_core::Color>,
    pub border_hover: framework_core::Tokenized<framework_core::Color>,
    pub focus_ring: framework_core::Tokenized<framework_core::Color>,
    /// Semi-transparent scrim color drawn behind overlays in
    /// dismiss-on-click / opaque-blocking mode. Read by the overlay
    /// screen's `OverlayScrim` stylesheet so the scrim adapts to
    /// theme (darker scrim in dark mode reads better against a
    /// dark surface).
    pub overlay: framework_core::Tokenized<framework_core::Color>,
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

fn tok_color(name: &'static str, fallback: &str) -> framework_core::Tokenized<framework_core::Color> {
    framework_core::Tokenized::token(name, framework_core::Color(fallback.into()))
}

pub fn light_theme() -> Theme {
    Theme {
        colors: Colors {
            background: tok_color("color-background", "#f7f7fb"),
            surface: tok_color("color-surface", "#ffffff"),
            surface_alt: tok_color("color-surface-alt", "#eef0f7"),
            primary: tok_color("color-primary", "#5b6cff"),
            primary_hover: tok_color("color-primary-hover", "#4a5cf0"),
            primary_pressed: tok_color("color-primary-pressed", "#3947d6"),
            primary_text: tok_color("color-primary-text", "#ffffff"),
            text: tok_color("color-text", "#1a1a1f"),
            muted: tok_color("color-muted", "#6b7280"),
            border: tok_color("color-border", "#e4e6ef"),
            border_hover: tok_color("color-border-hover", "#b9bdcc"),
            focus_ring: tok_color("color-focus-ring", "#5b6cff"),
            overlay: tok_color("color-overlay", "rgba(15, 17, 21, 0.45)"),
        },
        spacing: SPACING.clone(),
    }
}

pub fn dark_theme() -> Theme {
    Theme {
        colors: Colors {
            background: tok_color("color-background", "#0f1115"),
            surface: tok_color("color-surface", "#1a1d24"),
            surface_alt: tok_color("color-surface-alt", "#262a35"),
            primary: tok_color("color-primary", "#8b9aff"),
            primary_hover: tok_color("color-primary-hover", "#9eabff"),
            primary_pressed: tok_color("color-primary-pressed", "#7383f5"),
            primary_text: tok_color("color-primary-text", "#0f1115"),
            text: tok_color("color-text", "#e8eaf0"),
            muted: tok_color("color-muted", "#9099a8"),
            border: tok_color("color-border", "#2a2e3a"),
            border_hover: tok_color("color-border-hover", "#3d4252"),
            focus_ring: tok_color("color-focus-ring", "#8b9aff"),
            overlay: tok_color("color-overlay", "rgba(0, 0, 0, 0.55)"),
        },
        spacing: SPACING.clone(),
    }
}

/// Make this theme installable through the framework's tokenized
/// theme installation API. Enumerates every color field as a
/// `TokenEntry` so the web backend can install them as `:root`
/// custom properties — theme swap then becomes one `setProperty`
/// per token, no class regeneration.
impl framework_core::ThemeTokens for Theme {
    fn tokens(&self) -> Vec<framework_core::TokenEntry> {
        fn entry(t: &framework_core::Tokenized<framework_core::Color>) -> framework_core::TokenEntry {
            let name = t.name().expect("hello: Theme color fields must be Tokenized::Token");
            framework_core::TokenEntry {
                name,
                value: framework_core::TokenValue::Color(t.value().clone()),
            }
        }
        let c = &self.colors;
        vec![
            entry(&c.background),
            entry(&c.surface),
            entry(&c.surface_alt),
            entry(&c.primary),
            entry(&c.primary_hover),
            entry(&c.primary_pressed),
            entry(&c.primary_text),
            entry(&c.text),
            entry(&c.muted),
            entry(&c.border),
            entry(&c.border_hover),
            entry(&c.focus_ring),
            entry(&c.overlay),
        ]
    }
}

// =============================================================================
// Stylesheets — page chrome
// =============================================================================

framework_core::stylesheet! {
    pub Page<Theme> {
        base(t) {
            background: t.colors.background.clone(),
            color: t.colors.text.clone(),
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
            color: t.colors.text.clone(),
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
            color: t.colors.muted.clone(),
            font_size: 16.0,
            line_height: 22.0,
        }
    }
}

framework_core::stylesheet! {
    pub SectionHeading<Theme> {
        base(t) {
            color: t.colors.muted.clone(),
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
            color: t.colors.text.clone(),
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
            color: t.colors.primary.clone(),
            font_size: 14.0,
            font_weight: FontWeight::SemiBold,
        }
    }
}

framework_core::stylesheet! {
    pub CardValue<Theme> {
        base(t) {
            color: t.colors.text.clone(),
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
            background: t.colors.surface.clone(),
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
                background: t.colors.primary.clone(),
                color: t.colors.primary_text.clone(),
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
            background: t.colors.primary.clone(),
            color: t.colors.primary_text.clone(),
            padding_vertical: t.spacing.sm,
            padding_horizontal: t.spacing.md,
            border_radius: 8.0,
            font_weight: FontWeight::SemiBold,
            font_size: 14.0,
            letter_spacing: 0.3,
            text_align: TextAlign::Center,
        }
        state hovered(t) {
            background: t.colors.primary_hover.clone(),
        }
        state pressed(t) {
            background: t.colors.primary_pressed.clone(),
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
            color: t.colors.text.clone(),
            padding_vertical: t.spacing.sm,
            padding_horizontal: t.spacing.md,
            border_radius: 8.0,
            border_width: 1.0,
            border_color: t.colors.border.clone(),
            font_weight: FontWeight::Medium,
            font_size: 14.0,
            letter_spacing: 0.3,
            text_align: TextAlign::Center,
        }
        state hovered(t) {
            border_color: t.colors.border_hover.clone(),
        }
        state pressed(t) {
            border_color: t.colors.primary.clone(),
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
            background: t.colors.primary.clone(),
            color: t.colors.primary_text.clone(),
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
            background: t.colors.surface.clone(),
            border_radius: 8.0,
            border_width: 1.0,
            border_color: t.colors.border.clone(),
            height: 200.0,
            overflow: Overflow::Hidden,
        }
    }
}

// Header — full-width nav bar with screen buttons + theme toggle.
framework_core::stylesheet! {
    pub HeaderBar<Theme> {
        base(t) {
            background: t.colors.surface.clone(),
            padding_vertical: t.spacing.sm,
            padding_horizontal: t.spacing.md,
            border_radius: 10.0,
            border_width: 1.0,
            border_color: t.colors.border.clone(),
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
            color: t.colors.text.clone(),
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
            color: t.colors.muted.clone(),
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
                background: t.colors.primary.clone(),
                color: t.colors.primary_text.clone(),
            }
        }
        state hovered(t) {
            color: t.colors.text.clone(),
        }
        transitions {
            background: 200ms EaseOut,
            color: 200ms EaseOut,
        }
    }
}

// =============================================================================
// Stylesheets — drawer chrome
// =============================================================================
//
// The home screen's drawer renders a sidebar on the left containing
// a list of entry buttons. The author's `.layout(...)` closure on
// the DrawerNavigator draws this — the framework only provides the
// `is_open` signal and the outlet where the active body screen
// renders.

// DrawerShell — the outer flex row: sidebar on the left, body on
// the right. On wide screens the sidebar is always pinned; below
// the breakpoint it slides over the body via overlay.
framework_core::stylesheet! {
    pub DrawerShell<Theme> {
        base(t) {
            flex_direction: FlexDirection::Row,
            min_height: Length::pct(100.0),
            background: t.colors.background.clone(),
            color: t.colors.text.clone(),
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

// DrawerSidebar — the side panel itself.
framework_core::stylesheet! {
    pub DrawerSidebar<Theme> {
        base(t) {
            background: t.colors.surface.clone(),
            padding: t.spacing.lg,
            gap: Length::Px(t.spacing.sm),
            border_right_width: 1.0,
            border_right_color: t.colors.border.clone(),
            width: 320.0,
        }
        transitions {
            background: 250ms EaseInOut,
            border_right_color: 250ms EaseInOut,
        }
    }
}

// DrawerBody — outlet container, takes the remaining horizontal
// space. The framework's outlet `View` mounts inside this.
framework_core::stylesheet! {
    pub DrawerBody<Theme> {
        base(t) {
            flex_grow: 1.0,
            padding: t.spacing.xl,
            gap: Length::Px(t.spacing.lg),
            color: t.colors.text.clone(),
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

// DrawerItem — each drawer entry button. `active` variant marks the
// currently-visible body screen.
framework_core::stylesheet! {
    pub DrawerItemButton<Theme> {
        base(t) {
            background: Color("transparent".into()),
            color: t.colors.text.clone(),
            padding_vertical: t.spacing.sm,
            padding_horizontal: t.spacing.md,
            border_radius: 6.0,
            font_size: 14.0,
            font_weight: FontWeight::Medium,
            text_align: TextAlign::Center,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: t.colors.primary.clone(),
                color: t.colors.primary_text.clone(),
            }
        }
        state hovered(t) {
            background: t.colors.surface_alt.clone(),
        }
        transitions {
            background: 200ms EaseOut,
            color: 200ms EaseOut,
        }
    }
}

framework_core::stylesheet! {
    pub DrawerBrand<Theme> {
        base(t) {
            color: t.colors.text.clone(),
            font_size: 18.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: -0.2,
            margin_bottom: t.spacing.md,
        }
    }
}

// =============================================================================
// Stylesheets — tab navigator (sidebar-placement)
// =============================================================================
//
// The drawer's "Tab demo" body renders a TabNavigator with sidebar
// placement: a vertical tab strip on the left of the body, with the
// active tab's screen filling the rest. The TabNavigator's
// `.layout(...)` draws this — same pattern as the drawer.

framework_core::stylesheet! {
    pub TabShell<Theme> {
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(t.spacing.md),
            min_height: 320.0,
            background: t.colors.surface.clone(),
            border_radius: 12.0,
            border_width: 1.0,
            border_color: t.colors.border.clone(),
            overflow: Overflow::Hidden,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

framework_core::stylesheet! {
    pub TabSidebar<Theme> {
        base(t) {
            background: t.colors.surface_alt.clone(),
            padding: t.spacing.md,
            gap: Length::Px(t.spacing.xs),
            border_right_width: 1.0,
            border_right_color: t.colors.border.clone(),
            width: 180.0,
        }
        transitions {
            background: 250ms EaseInOut,
            border_right_color: 250ms EaseInOut,
        }
    }
}

framework_core::stylesheet! {
    pub TabBody<Theme> {
        base(t) {
            flex_grow: 1.0,
            padding: t.spacing.lg,
            gap: Length::Px(t.spacing.md),
            color: t.colors.text.clone(),
        }
        transitions {
            color: 250ms EaseInOut,
        }
    }
}

framework_core::stylesheet! {
    pub TabPillButton<Theme> {
        base(t) {
            background: Color("transparent".into()),
            color: t.colors.muted.clone(),
            padding_vertical: t.spacing.xs,
            padding_horizontal: t.spacing.sm,
            border_radius: 6.0,
            font_size: 13.0,
            font_weight: FontWeight::Medium,
            text_align: TextAlign::Center,
        }
        variant active {
            #[default]
            off(_t) {}
            on(t) {
                background: t.colors.primary.clone(),
                color: t.colors.primary_text.clone(),
            }
        }
        state hovered(t) {
            color: t.colors.text.clone(),
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
            background: t.colors.surface.clone(),
            color: t.colors.text.clone(),
            border_bottom_width: 1.0,
            border_bottom_color: t.colors.border.clone(),
            font_size: 13.0,
            height: 36.0,
            justify_content: JustifyContent::Center,
        }
        variant parity {
            #[default]
            even(_t) {}
            odd(t) {
                background: t.colors.surface_alt.clone(),
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
            background: t.colors.surface.clone(),
            border_radius: 10.0,
            border_width: 1.0,
            border_color: t.colors.border.clone(),
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
            background: t.colors.surface.clone(),
            padding_vertical: t.spacing.sm,
            padding_horizontal: t.spacing.md,
            border_radius: 10.0,
            border_width: 1.0,
            border_color: t.colors.border.clone(),
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
            background: t.colors.background.clone(),
            color: t.colors.text.clone(),
            padding_vertical: t.spacing.xs,
            padding_horizontal: t.spacing.sm,
            border_radius: 6.0,
            border_width: 1.0,
            border_color: t.colors.border.clone(),
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
// Stylesheets — overlay-screen surfaces (modal, popover, drawer, scrim)
// =============================================================================
//
// The overlay primitive itself doesn't draw anything — its job is to
// portal/position the children. These stylesheets define the *content*
// surfaces the children render with, plus an example backdrop that
// reads from the theme so dark mode propagates through.

framework_core::stylesheet! {
    pub ModalSurface<Theme> {
        base(t) {
            background: t.colors.surface.clone(),
            color: t.colors.text.clone(),
            padding: t.spacing.xl,
            border_radius: 14.0,
            border_width: 1.0,
            border_color: t.colors.border.clone(),
            gap: Length::Px(t.spacing.md),
            flex_direction: FlexDirection::Column,
            min_width: 360.0,
            max_width: 520.0,
            shadow: Shadow {
                x: 0.0,
                y: 16.0,
                blur: 40.0,
                color: Color("rgba(15, 17, 21, 0.28)".into()),
            },
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

framework_core::stylesheet! {
    pub PopoverSurface<Theme> {
        base(t) {
            background: t.colors.surface.clone(),
            color: t.colors.text.clone(),
            padding: t.spacing.sm,
            border_radius: 10.0,
            border_width: 1.0,
            border_color: t.colors.border.clone(),
            gap: Length::Px(t.spacing.xs),
            flex_direction: FlexDirection::Column,
            min_width: 200.0,
            shadow: Shadow {
                x: 0.0,
                y: 8.0,
                blur: 24.0,
                color: Color("rgba(15, 17, 21, 0.22)".into()),
            },
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

framework_core::stylesheet! {
    pub DrawerSurface<Theme> {
        base(t) {
            background: t.colors.surface.clone(),
            color: t.colors.text.clone(),
            padding: t.spacing.xl,
            border_left_width: 1.0,
            border_left_color: t.colors.border.clone(),
            gap: Length::Px(t.spacing.md),
            flex_direction: FlexDirection::Column,
            // Drawers run the full height of the viewport — the
            // overlay primitive's `Right` placement already pins
            // top + bottom + right, so we only need to set a fixed
            // width here.
            width: 360.0,
            height: Length::pct(100.0),
            shadow: Shadow {
                x: -8.0,
                y: 0.0,
                blur: 32.0,
                color: Color("rgba(15, 17, 21, 0.28)".into()),
            },
        }
        transitions {
            background: 250ms EaseInOut,
            border_left_color: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

// Theme-aware backdrop. The overlay primitive's default scrim is a
// fixed `rgba(0,0,0,0.45)`; passing `backdrop_style` overrides it
// with whatever you want — useful for tuning opacity per theme or
// adding effects like blur.
framework_core::stylesheet! {
    pub OverlayScrim<Theme> {
        base(t) {
            background: t.colors.overlay.clone(),
        }
        transitions {
            background: 250ms EaseInOut,
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
pub const OVERLAY_ROUTE: Route<()> = Route::<()>::new("overlay", "/overlay");
pub const DETAIL_ROUTE: Route<DetailParams> = Route::<DetailParams>::new("detail", "/detail/:id");

// Routes for the drawer's items. The home screen's body is a
// drawer; each item picks one of these as the visible body.
pub const DRAWER_DASHBOARD_ROUTE: Route<()> =
    Route::<()>::new("drawer/dashboard", "/dashboard");
pub const DRAWER_TABS_ROUTE: Route<()> = Route::<()>::new("drawer/tabs", "/tabs");
pub const DRAWER_SETTINGS_ROUTE: Route<()> =
    Route::<()>::new("drawer/settings", "/settings");

// Routes for the tab navigator that lives inside the drawer's
// "Tab demo" body. Three tabs — illustrating the framework's
// nested-navigator + ambient-Link wiring.
pub const TAB_OVERVIEW_ROUTE: Route<()> =
    Route::<()>::new("tabs/overview", "/tabs/overview");
pub const TAB_ACTIVITY_ROUTE: Route<()> =
    Route::<()>::new("tabs/activity", "/tabs/activity");
pub const TAB_DETAILS_ROUTE: Route<()> =
    Route::<()>::new("tabs/details", "/tabs/details");

// =============================================================================
// Home screen — landing page with cards that push the other screens
// =============================================================================

// =============================================================================
// Drawer item bodies
// =============================================================================

pub struct DashboardProps {
    pub nav: Ref<NavigatorHandle>,
    /// Drawer handle, used to render a hamburger button that opens
    /// the drawer. On Android the drawer is a real native overlay
    /// (DrawerLayout), so this is the only way for the user to get
    /// at the sidebar; on web the layout pins the sidebar beside
    /// the body so the button is still useful but less essential.
    pub drawer: Ref<DrawerHandle>,
}

/// Dashboard body — the landing card grid + links to the secondary
/// screens (Showcase, Performance, Lists, Overlays). These all push
/// onto the *root* stack navigator (the `nav` handle), not the
/// drawer, because the secondary screens are full takeovers.
#[component]
pub fn dashboard(props: &DashboardProps) -> Primitive {
    let nav = props.nav;
    let drawer = props.drawer;
    ui! {
        View {
            // Hamburger row — opens the drawer on tap. On Android
            // this triggers DrawerLayout's slide-in animation; on
            // web the drawer is already visible so this no-ops
            // visually (but still flips the is_open signal).
            View(style = spaced_row_style()) {
                Button(
                    label = "☰ Menu",
                    on_click = move || {
                        if let Some(h) = drawer.get() {
                            h.open();
                        }
                    },
                    style = secondary_button_style()
                )
                Text(style = title_style()) { "Welcome" }
            }
            Text(style = subtitle_style()) {
                "Cross-platform Rust UI framework. Tap ☰ to open the \
                 drawer and pick a destination, or push a deeper demo \
                 onto the root stack from the buttons below. Back \
                 returns here."
            }
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
                Button(
                    label = "Overlays",
                    on_click = move || {
                        if let Some(h) = nav.get() {
                            h.push(&OVERLAY_ROUTE, ());
                        }
                    },
                    style = primary_button_style()
                )
            }
        }
    }
}

#[component]
pub fn taboverview() -> Primitive {
    // Each tab has its own per-screen scope. The counter below
    // is mounted lazily (first visit) and persists across tab
    // switches — try clicking + a few times, switching to Activity,
    // then back: the count is preserved.
    let count = signal!(0_i32);
    ui! {
        View {
            Text(style = section_heading_style()) { "Overview" }
            Text(style = subtitle_style()) {
                "This tab demonstrates LazyPersistent mount policy — \
                 increment the counter, switch tabs, come back: state \
                 is preserved."
            }
            View(style = row_style()) {
                Counter(label = "Visits", value = count)
            }
        }
    }
}

#[component]
pub fn tabactivity() -> Primitive {
    ui! {
        View {
            Text(style = section_heading_style()) { "Activity" }
            Text(style = subtitle_style()) {
                "Each tab's content lives in its own reactive scope. \
                 Switching tabs doesn't rebuild the sidebar, just \
                 swaps the body — exactly like the drawer above."
            }
        }
    }
}

#[component]
pub fn tabdetails() -> Primitive {
    ui! {
        View {
            Text(style = section_heading_style()) { "Details" }
            Text(style = subtitle_style()) {
                "The TabNavigator's default Link kind is Select, so \
                 the sidebar links above don't push onto a stack — \
                 they just swap this body. Same primitive, different \
                 dispatch shape; the framework chooses based on the \
                 ambient navigator."
            }
        }
    }
}

/// Tab chrome — sidebar with three pill buttons plus the outlet.
/// Same pattern as the drawer's layout: `Link`s dispatch `Select`
/// (the default `NavKind` inside a tab navigator).
fn tab_sidebar_layout() -> impl Fn(LayoutProps) -> Primitive + 'static {
    move |props: LayoutProps| {
        let active = props.active_route;
        ui! {
            View(style = tab_shell_style()) {
                View(style = tab_sidebar_style()) {
                    Link(route = &TAB_OVERVIEW_ROUTE, params = ()) {
                        Text(style = TabPillButton().active(if active.get() == TAB_OVERVIEW_ROUTE.name() {
                            TabPillButtonActive::On
                        } else {
                            TabPillButtonActive::Off
                        })) { "Overview" }
                    }
                    Link(route = &TAB_ACTIVITY_ROUTE, params = ()) {
                        Text(style = TabPillButton().active(if active.get() == TAB_ACTIVITY_ROUTE.name() {
                            TabPillButtonActive::On
                        } else {
                            TabPillButtonActive::Off
                        })) { "Activity" }
                    }
                    Link(route = &TAB_DETAILS_ROUTE, params = ()) {
                        Text(style = TabPillButton().active(if active.get() == TAB_DETAILS_ROUTE.name() {
                            TabPillButtonActive::On
                        } else {
                            TabPillButtonActive::Off
                        })) { "Details" }
                    }
                }
                View(style = tab_body_style()) {
                    props.outlet
                }
            }
        }
    }
}

/// Tab demo body — a `TabNavigator` with sidebar-placed tabs. Each
/// tab's screen runs in its own reactive scope; `MountPolicy` is
/// `LazyPersistent` (the default), so first visit mounts the tab's
/// content and subsequent visits show it from cache (state survives
/// switching to another tab and back).
#[component]
pub fn tabdemo() -> Primitive {
    let tabs_ref: Ref<TabsHandle> = Ref::new();
    ui! {
        { TabNavigator::new(&TAB_OVERVIEW_ROUTE)
            .tab(TAB_OVERVIEW_ROUTE, TabSpec::new("Overview"), |_| {
                ui! { TabOverview() }
            })
            .tab(TAB_ACTIVITY_ROUTE, TabSpec::new("Activity"), |_| {
                ui! { TabActivity() }
            })
            .tab(TAB_DETAILS_ROUTE, TabSpec::new("Details"), |_| {
                ui! { TabDetails() }
            })
            .placement(framework_core::TabPlacement::Sidebar)
            .layout(tab_sidebar_layout())
            .bind(tabs_ref) }
    }
}

pub struct SettingsBodyProps {
    pub is_dark: Signal<bool>,
}

/// Settings body — currently just the theme toggle. Toggling theme
/// re-fires every styled effect across the app via the framework's
/// reactivity, so the change propagates to every mounted screen
/// (including the hidden ones in the other drawer items, when their
/// MountPolicy is persistent).
#[component]
pub fn settingsbody(props: &SettingsBodyProps) -> Primitive {
    let is_dark = props.is_dark;
    ui! {
        View {
            Text(style = title_style()) { "Settings" }
            Text(style = section_heading_style()) { "Appearance" }
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

// =============================================================================
// Home screen — the drawer navigator
// =============================================================================

pub struct HomeProps {
    /// Root stack navigator handle. Used by the home screen's body
    /// content (the drawer items) to push secondary screens —
    /// Showcase, Performance, Lists, etc. — onto the root stack.
    pub nav: Ref<NavigatorHandle>,
    /// App-level theme flag, owned at the app level so the theme
    /// outlives any single screen's scope.
    pub is_dark: Signal<bool>,
}

/// Home screen — a `DrawerNavigator`. Three drawer items:
///
/// - **Dashboard**: landing content with cards that push the root
///   stack's secondary screens (Showcase, Performance, Lists, …).
/// - **Tab demo**: hosts a nested `TabNavigator` with sidebar tabs.
/// - **Settings**: app-level controls (theme toggle).
///
/// The drawer's chrome (sidebar + brand + entry buttons) is drawn
/// by the layout closure below. Native backends (Android, iOS) will
/// override this with their own drawer widget (`DrawerLayout`,
/// hand-rolled `UIView` overlay), but on web the layout slot *is*
/// the drawer's visual.
#[component]
pub fn home(props: &HomeProps) -> Primitive {
    let nav = props.nav;
    let is_dark = props.is_dark;
    let drawer_ref: Ref<DrawerHandle> = Ref::new();

    ui! {
        { DrawerNavigator::new(&DRAWER_DASHBOARD_ROUTE)
            .item(DRAWER_DASHBOARD_ROUTE, DrawerItem::new("Dashboard"))
            .item(DRAWER_TABS_ROUTE,      DrawerItem::new("Tab demo"))
            .item(DRAWER_SETTINGS_ROUTE,  DrawerItem::new("Settings"))
            .screen(DRAWER_DASHBOARD_ROUTE, move |_| {
                let nav = nav;
                let drawer = drawer_ref;
                ui! { Dashboard(nav = nav, drawer = drawer) }
            })
            .screen(DRAWER_TABS_ROUTE, move |_| {
                ui! { TabDemo() }
            })
            .screen(DRAWER_SETTINGS_ROUTE, move |_| {
                let is_dark = is_dark;
                ui! { SettingsBody(is_dark = is_dark) }
            })
            // The `.sidebar(...)` slot defines the drawer's side
            // panel. On Android the framework renders it natively
            // beside the body; on web the layout below embeds it
            // via `props.sidebar`. Same closure, both targets.
            .sidebar(home_drawer_sidebar())
            // Web-only chrome. Wraps the drawer in a flex row with
            // the sidebar on the left and the outlet on the right.
            // Native backends ignore this slot — Android renders
            // the sidebar (from above) directly inside its native
            // drawer shell.
            .layout(home_drawer_layout())
            .bind(drawer_ref) }
    }
}

/// Drawer sidebar content. Renders a brand + one entry per
/// registered drawer item, reactively highlighting the active one.
/// `active_route` is read inside each item's style closure so
/// flipping the active route re-styles only the affected
/// buttons (no rebuild of the panel).
///
/// Each entry uses a `Link` primitive — the ambient navigator at
/// build time is the drawer, so the default `NavKind` is `Select`.
/// Clicking dispatches `NavCommand::Select`, which the drawer's
/// dispatcher translates to a body swap.
fn home_drawer_sidebar() -> impl Fn(framework_core::DrawerSidebarProps) -> Primitive + 'static {
    move |props: framework_core::DrawerSidebarProps| {
        let active = props.active_route;
        ui! {
            View(style = drawer_sidebar_style()) {
                Text(style = drawer_brand_style()) { "idealyst" }
                Link(route = &DRAWER_DASHBOARD_ROUTE, params = ()) {
                    Text(style = DrawerItemButton().active(if active.get() == DRAWER_DASHBOARD_ROUTE.name() {
                        DrawerItemButtonActive::On
                    } else {
                        DrawerItemButtonActive::Off
                    })) { "Dashboard" }
                }
                Link(route = &DRAWER_TABS_ROUTE, params = ()) {
                    Text(style = DrawerItemButton().active(if active.get() == DRAWER_TABS_ROUTE.name() {
                        DrawerItemButtonActive::On
                    } else {
                        DrawerItemButtonActive::Off
                    })) { "Tab demo" }
                }
                Link(route = &DRAWER_SETTINGS_ROUTE, params = ()) {
                    Text(style = DrawerItemButton().active(if active.get() == DRAWER_SETTINGS_ROUTE.name() {
                        DrawerItemButtonActive::On
                    } else {
                        DrawerItemButtonActive::Off
                    })) { "Settings" }
                }
            }
        }
    }
}

/// Drawer outer chrome (web-only). The layout closure receives a
/// pre-built sidebar Primitive via `props.sidebar` and embeds it in
/// a flex row alongside the outlet — the framework's hook for
/// "compose the drawer's parts however the web app wants."
///
/// Native backends ignore this slot; Android positions the sidebar
/// directly inside its native drawer shell.
fn home_drawer_layout() -> impl Fn(LayoutProps) -> Primitive + 'static {
    move |props: LayoutProps| {
        ui! {
            View(style = drawer_shell_style()) {
                props.sidebar
                View(style = drawer_body_style()) {
                    props.outlet
                }
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
// Overlay screen — modal, popover, drawer demos.
// =============================================================================
//
// Each overlay is host-controlled: the screen owns a `Signal<bool>`
// per overlay; flipping it mounts/unmounts the `Overlay` primitive
// via `ui!`'s reactive `if`. The framework's walker mounts the
// floating layer on each `if` flip-on, drops the surrounding scope
// (and the overlay's portal) on flip-off.
//
// Three flavors shown, exercising the primitive's anchoring and
// backdrop modes:
//
// 1. **Modal** — `Viewport(Center)` + `BackdropMode::Dismiss` so
//    clicking the scrim or pressing Escape fires `on_dismiss`. The
//    host's `on_dismiss` callback flips the signal to `false` →
//    the surrounding `if` rebuilds the empty branch → the overlay's
//    scope drops.
//
// 2. **Popover** — `Element` anchored to a `Ref<ButtonHandle>` on
//    the trigger button, `BackdropMode::None` so the page behind
//    stays interactive. Escape still dismisses.
//
// 3. **Drawer** — `Viewport(Right)` + `BackdropMode::Dismiss`.
//    Same shape as the modal but pinned to the right edge instead
//    of centered.

pub struct OverlaysProps {
    pub nav: Ref<NavigatorHandle>,
}

#[component]
pub fn overlays(props: &OverlaysProps) -> Primitive {
    let nav = props.nav;

    let modal_open = signal!(false);
    let popover_open = signal!(false);
    let drawer_open = signal!(false);

    // The popover anchors to this ref. Calling `.bind(popover_trigger)`
    // on the trigger button fills it at mount time; the overlay's
    // `Element` anchor measures its rect on each open.
    let popover_trigger: Ref<ButtonHandle> = Ref::new();

    ui! {
        View(style = page_style()) {
            Topbar(title = "Overlays".to_string(), nav = nav)
            Text(style = subtitle_style()) {
                "Overlays render above the layout tree, escaping \
                 parent clipping and stacking contexts. The host \
                 owns each overlay's open/close signal; flipping it \
                 mounts/unmounts the floating subtree."
            }

            // -------- Modal --------
            View {
                Text(style = section_heading_style()) { "Modal" }
                Text(style = subtitle_style()) {
                    "Viewport-centered overlay with a dismiss-on-\
                     click scrim. Press Escape or click outside to \
                     dismiss; both flow through `on_dismiss`."
                }
                Button(
                    label = "Open modal",
                    on_click = move || modal_open.set(true),
                    style = primary_button_style()
                )
                // Presence wraps the conditional mount: the `if`
                // moved INSIDE the Presence's `present` reactive
                // closure. Result: on `modal_open` flipping false,
                // the modal animates out before its scope drops.
                Presence(
                    present = move || modal_open.get(),
                    enter = PresenceAnim::new(
                        PresenceState::default().opacity(0.0).translate_y(8.0),
                        200,
                        Easing::EaseOut,
                    ),
                    exit = PresenceAnim::new(
                        PresenceState::default().opacity(0.0).translate_y(8.0),
                        150,
                        Easing::EaseIn,
                    ),
                ) {
                    Overlay(
                        anchor = OverlayAnchor::Viewport(ViewportPlacement::Center),
                        backdrop = BackdropMode::Dismiss,
                        backdrop_style = overlay_scrim_style(),
                        on_dismiss = move || modal_open.set(false)
                    ) {
                        View(style = modal_surface_style()) {
                            Text(style = title_style()) { "Confirm" }
                            Text(style = subtitle_style()) {
                                "This is a centered modal. Click the \
                                 scrim or press Escape to dismiss."
                            }
                            View(style = row_style()) {
                                Button(
                                    label = "Cancel",
                                    on_click = move || modal_open.set(false),
                                    style = secondary_button_style()
                                )
                                Button(
                                    label = "OK",
                                    on_click = move || modal_open.set(false),
                                    style = primary_button_style()
                                )
                            }
                        }
                    }
                }
            }

            // -------- Popover --------
            View {
                Text(style = section_heading_style()) { "Popover" }
                Text(style = subtitle_style()) {
                    "Element-anchored overlay with no backdrop — \
                     the page behind stays interactive. Anchors to \
                     a `Ref<ButtonHandle>` on the trigger."
                }
                Button(
                    label = "Show options",
                    on_click = move || popover_open.set(true),
                    style = secondary_button_style()
                ).bind(popover_trigger)
                Presence(
                    present = move || popover_open.get(),
                    enter = PresenceAnim::new(
                        PresenceState::default().opacity(0.0).translate_y(-4.0).scale(0.97),
                        140,
                        Easing::EaseOut,
                    ),
                    exit = PresenceAnim::new(
                        PresenceState::default().opacity(0.0).scale(0.97),
                        100,
                        Easing::EaseIn,
                    ),
                ) {
                    Overlay(
                        anchor = OverlayAnchor::Element(ElementAnchor {
                            target: AnchorTarget::from(popover_trigger),
                            side: ElementSide::Below,
                            align: ElementAlign::Start,
                            offset: 6.0,
                        }),
                        backdrop = BackdropMode::None,
                        on_dismiss = move || popover_open.set(false),
                        trap_focus = false
                    ) {
                        View(style = popover_surface_style()) {
                            Button(
                                label = "Edit",
                                on_click = move || popover_open.set(false),
                                style = secondary_button_style()
                            )
                            Button(
                                label = "Duplicate",
                                on_click = move || popover_open.set(false),
                                style = secondary_button_style()
                            )
                            Button(
                                label = "Delete",
                                on_click = move || popover_open.set(false),
                                style = secondary_button_style()
                            )
                        }
                    }
                }
            }

            // -------- Drawer --------
            View {
                Text(style = section_heading_style()) { "Drawer" }
                Text(style = subtitle_style()) {
                    "Right-edge drawer — the same `Overlay` primitive, \
                     placed with `Viewport(Right)` so the backend \
                     pins it to the right edge full-height instead \
                     of centering."
                }
                Button(
                    label = "Open drawer",
                    on_click = move || drawer_open.set(true),
                    style = primary_button_style()
                )
                Presence(
                    present = move || drawer_open.get(),
                    enter = PresenceAnim::new(
                        PresenceState::default().translate_x(360.0),
                        260,
                        Easing::EaseOut,
                    ),
                    exit = PresenceAnim::new(
                        PresenceState::default().translate_x(360.0),
                        220,
                        Easing::EaseIn,
                    ),
                ) {
                    Overlay(
                        anchor = OverlayAnchor::Viewport(ViewportPlacement::Right),
                        backdrop = BackdropMode::Dismiss,
                        backdrop_style = overlay_scrim_style(),
                        on_dismiss = move || drawer_open.set(false)
                    ) {
                        View(style = drawer_surface_style()) {
                            Text(style = title_style()) { "Settings" }
                            Text(style = subtitle_style()) {
                                "Drawers slide in from a viewport edge. \
                                 Real motion needs a `Presence` primitive \
                                 (deferred unmount) — for now the drawer \
                                 mounts/unmounts instantly."
                            }
                            Button(
                                label = "Close",
                                on_click = move || drawer_open.set(false),
                                style = primary_button_style()
                            )
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
            .screen(OVERLAY_ROUTE, move |_| {
                let nav = nav;
                ui! { Overlays(nav = nav) }
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
