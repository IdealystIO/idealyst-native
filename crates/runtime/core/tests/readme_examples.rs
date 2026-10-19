//! The "Core concepts" snippets in the repository README, compiled.
//!
//! The README is the first code a new author copies, so its examples must
//! track the real author surface. Each item below mirrors one README block
//! with imports from `runtime_core` alone; when a signature or macro grammar
//! changes, this file fails until the README is updated with it.
//!
//! Compile-only: nothing here mounts a tree, so the functions are never run.

#![allow(dead_code)]

use runtime_core::{
    component, memo, signal, stylesheet, ui, Color, Element, FlexDirection, Length, Tokens,
};

// README: Signals
fn signals() {
    let count = signal(0);

    count.set(5);
    count.update(|n| n + 1);
    let _now = count.get();
}

// README: Memos
fn memos() {
    let items = signal(vec![3, 8, 12]);
    let total = memo(move || items.get().iter().sum::<i32>());
    let _over_budget = memo(move || total.get() > 20);
}

// README: Style tokens
#[derive(Tokens, Clone)]
pub struct Swatch {
    pub bg: Color,
    pub text: Color,
}

#[derive(Tokens, Clone)]
#[tokens(default = light)]
pub struct AppTheme {
    pub surface: Color,
    pub text: Color,
    pub accent: Swatch,
    pub space: Length,
    pub radius: Length,
}

fn light() -> AppTheme {
    AppTheme {
        surface: "#ffffff".into(),
        text: "#111827".into(),
        accent: Swatch { bg: "#eef2ff".into(), text: "#4338ca".into() },
        space: 12.0.into(),
        radius: 8.0.into(),
    }
}

fn dark() -> AppTheme {
    light().with(|t| {
        t.surface = "#111827".into();
        t.text = "#e5e7eb".into();
        t.accent.bg = "#1e1b4b".into();
    })
}

fn install_theme() {
    AppTheme::follow_system(light(), dark());
    AppTheme::set(dark());
}

// README: Stylesheets
stylesheet! {
    pub Card<AppTheme> {
        base(t) {
            flex_direction: FlexDirection::Column,
            padding: t.space(),
            border_radius: t.radius(),
            background: t.surface(),
            color: t.text(),
        }
        variant tone {
            #[default]
            plain(_t) {}
            accent(t) {
                background: t.accent.bg(),
                color: t.accent.text(),
            }
        }
        state hovered(t) {
            border_color: t.accent.text(),
        }
        breakpoint md(_t) {
            flex_direction: FlexDirection::Row,
        }
    }
}

// README: Components
#[component]
pub fn Counter(label: String, #[prop(default = 1)] step: i32) -> Element {
    let count = signal(0);
    let doubled = memo(move || count.get() * 2);

    ui! {
        view(style = Card().tone(CardTone::Accent)) {
            text { "{label}: {count} (doubled: {doubled})" }
            button(
                label = "Increment",
                on_click = move || count.update(|n| n + step.get()),
            )
            if count.get() >= 10 {
                text { "That's a lot." }
            }
        }
    }
}

fn counter_call_site() -> Element {
    ui! { Counter(label = "Clicks".to_string(), step = 2) }
}
