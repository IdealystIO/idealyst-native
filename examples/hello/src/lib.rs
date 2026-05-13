//! The shared sample tree, used by every backend.

use framework_core::{
    button, component, install_theme, set_theme, signal, text, ui, view, Border, Color, Primitive,
    Signal, StyleApplication, StyleRules, StyleSheet,
};
use std::rc::Rc;

// =============================================================================
// Theme
// =============================================================================

/// The app's theme type. Stylesheets reference fields of this struct via
/// `Color::from_theme(|t: &Theme| ...)` etc. Authors define their own theme
/// shape; the framework is generic over the concrete type.
#[derive(Clone)]
pub struct Theme {
    pub colors: Colors,
    pub spacing: Spacing,
}

#[derive(Clone)]
pub struct Colors {
    pub surface: String,
    pub foreground: String,
}

#[derive(Clone)]
pub struct Spacing {
    pub medium: f32,
}

pub fn light_theme() -> Theme {
    Theme {
        colors: Colors {
            surface: "#f5f5f5".into(),
            foreground: "#111".into(),
        },
        spacing: Spacing { medium: 16.0 },
    }
}

pub fn dark_theme() -> Theme {
    Theme {
        colors: Colors {
            surface: "#222".into(),
            foreground: "#eee".into(),
        },
        spacing: Spacing { medium: 16.0 },
    }
}

// =============================================================================
// Stylesheets
// =============================================================================

/// Card stylesheet — declares variants. Every (theme × variant)
/// combination is pre-generated on first use; the CSS classes live
/// in the document permanently after that.
///
/// - Base: theme surface/foreground, 8px border radius.
/// - Axis `size`: small / medium / large, default `medium`.
/// - Axis `kind`: elevated / outlined, default `elevated`.
///   * `elevated` — strong surface fill (the default look).
///   * `outlined` — transparent fill with a 2px border in the
///     foreground color.
/// Style for the page banner — a styled `Text` primitive demonstrating
/// that primitives accept `style = ...` orthogonally.
fn banner_style() -> Rc<StyleSheet> {
    thread_local! {
        static SHEET: Rc<StyleSheet> = Rc::new(StyleSheet::new(|t: &Theme| StyleRules {
            color: Some(Color(t.colors.foreground.clone())),
            font_size: Some(24.0),
            padding: Some(8.0),
            ..Default::default()
        }));
    }
    SHEET.with(|s| s.clone())
}

fn card_style() -> Rc<StyleSheet> {
    thread_local! {
        static SHEET: Rc<StyleSheet> = Rc::new(
            StyleSheet::new(|theme: &Theme| StyleRules {
                background: Some(Color(theme.colors.surface.clone())),
                color: Some(Color(theme.colors.foreground.clone())),
                padding: Some(theme.spacing.medium),
                border_radius: Some(8.0),
                ..Default::default()
            })
            .variant("size", "small", |t: &Theme| StyleRules {
                padding: Some(t.spacing.medium * 0.5),
                ..Default::default()
            })
            .variant("size", "medium", |t: &Theme| StyleRules {
                padding: Some(t.spacing.medium),
                ..Default::default()
            })
            .variant("size", "large", |t: &Theme| StyleRules {
                padding: Some(t.spacing.medium * 2.0),
                font_size: Some(18.0),
                ..Default::default()
            })
            .variant_default("size", "medium")
            .variant("kind", "elevated", |t: &Theme| StyleRules {
                // Strong surface; no border. Reads as a filled "raised" card.
                background: Some(Color(t.colors.surface.clone())),
                ..Default::default()
            })
            .variant("kind", "outlined", |t: &Theme| StyleRules {
                // Transparent fill with a 2px ring in the foreground color.
                background: Some(Color("transparent".into())),
                border: Some(Border::new(2, t.colors.foreground.clone())),
                ..Default::default()
            })
            .variant_default("kind", "elevated"),
        );
    }
    SHEET.with(|s| s.clone())
}

// =============================================================================
// Components
// =============================================================================

pub struct CardProps {
    pub title: String,
    pub children: Vec<Primitive>,
    /// Discrete variant for size. Pre-generated.
    pub size: String,
    /// Discrete variant for kind. Pre-generated.
    pub kind: String,
    /// Continuous override for padding. When present, the style
    /// closure reads `.get()` on every re-fire so signal changes
    /// propagate without re-rendering the card.
    pub padding_override: Option<Signal<f32>>,
}

#[component(
    children,
    default(
        size = "medium".to_string(),
        kind = "elevated".to_string(),
        padding_override = None
    )
)]
pub fn card(props: CardProps) -> Primitive {
    let CardProps { title, children, size, kind, padding_override } = props;
    view(vec![text(title), view(children)]).with_style(move || {
        let mut app = StyleApplication::new(card_style())
            .with("size", size.clone())
            .with("kind", kind.clone());
        // `.get()` here subscribes the surrounding effect to `pad`,
        // so signal changes re-fire the effect → re-resolve → re-apply.
        if let Some(sig) = padding_override {
            app = app.override_padding(sig.get());
        }
        app
    })
}

pub struct CounterProps {
    pub label: String,
    pub value: Signal<i32>,
    pub step: i32,
}

#[component(default(step = 1))]
pub fn counter(props: &CounterProps) -> Primitive {
    view(vec![
        text(format!(
            "{} (+{}): {}",
            props.label,
            props.step,
            props.value.get()
        )),
        button("Increment", move || {
            let step = props.step;
            props.value.update(move |n| *n += step)
        }),
    ])
}

// =============================================================================
// App
// =============================================================================

#[component]
pub fn app() -> Primitive {
    // Install the initial theme. Subsequent `set_theme(...)` calls will
    // propagate to every styled component without re-rendering.
    install_theme(light_theme());

    let score = signal!(0);
    let lives = signal!(3);
    let logged_in = signal!(false);
    let is_dark = signal!(false);
    // Continuous-value override: padding driven by buttons. Each unique
    // value mints a fresh CSS class via the framework's cache.
    let pad = signal!(16.0_f32);

    ui! {
        // Styled primitive: a Text node with its own style. The macro
        // strips `style = ...` and emits `.with_style(...)` on the
        // resulting primitive — same `with_style` you can call manually
        // outside `ui!`.
        Text(style = StyleApplication::new(banner_style())) {
            "Hello from idealyst-native"
        }

        Button(
            label = "Toggle theme",
            on_click = move || {
                let now_dark = !is_dark.get();
                is_dark.set(now_dark);
                if now_dark {
                    set_theme(dark_theme());
                } else {
                    set_theme(light_theme());
                }
            }
        )

        Button(label = "Pad +4", on_click = move || pad.update(|p| *p += 4.0))
        Button(label = "Pad -4", on_click = move || pad.update(|p| *p = (*p - 4.0).max(0.0)))

        // Discrete variants — pre-generated; no mint on apply.
        Card(title = "Default card") {
            Counter(label = "Score", value = score)
        }
        Card(title = "Outlined", size = "large".to_string(), kind = "outlined".to_string()) {
            Counter(label = "Lives", value = lives)
        }
        // Variant + continuous override — variant class is a cache hit;
        // override mints a fresh class lazily because padding is dynamic.
        // We pass `pad` (the Signal handle, not a snapshot value) so
        // the style closure can read it reactively.
        Card(
            title = "Override demo",
            size = "small".to_string(),
            padding_override = Some(pad)
        ) {
            Counter(label = "Score (in card)", value = score)
        }

        Counter(label = "Score (view B)", value = score)

        if logged_in.get() {
            Text { "Welcome back!" }
        } else {
            Button(
                label = "Login",
                on_click = move || logged_in.set(true)
            )
        }

        Text { format!("Echo: score={}, lives={}", score.get(), lives.get()) }
    }
}
