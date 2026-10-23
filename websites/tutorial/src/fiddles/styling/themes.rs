use runtime_core::{signal, stylesheet, ui, Color, Element, Length, Tokens};

// A theme is a struct of named values. Each field becomes a style token —
// `surface`, `accent`, `space` — and a value of the struct is one theme.
// The `app` prefix keeps these names clear of idea-ui's (`color-…`, `spacing-…`).
#[derive(Tokens, Clone)]
#[tokens(default = light, prefix = "app")]
pub struct AppTheme {
    pub background: Color,
    pub surface: Color,
    pub text: Color,
    pub accent: Color,
    pub space: Length,
}

fn light() -> AppTheme {
    AppTheme {
        background: "#f3f4f6".into(),
        surface: "#ffffff".into(),
        text: "#111827".into(),
        accent: "#4f46e5".into(),
        space: 16.0.into(),
    }
}

// A second theme is "the first one, except…".
fn dark() -> AppTheme {
    light().with(|t| {
        t.background = "#030712".into();
        t.surface = "#111827".into();
        t.text = "#e5e7eb".into();
        t.accent = "#a5b4fc".into();
    })
}

// `<AppTheme>` names the vocabulary; `t.surface()` refers to a token by name.
stylesheet! {
    pub Screen<AppTheme> {
        base(t) { background: t.background(), padding: t.space(), gap: t.space() }
    }
}

stylesheet! {
    pub Panel<AppTheme> {
        base(t) { background: t.surface(), padding: t.space(), border_radius: 12.0, gap: 8.0 }
    }
}

stylesheet! {
    pub Body<AppTheme> {
        base(t) { color: t.text() }
    }
}

stylesheet! {
    pub Accent<AppTheme> {
        base(t) { color: t.accent(), font_size: 18.0 }
    }
}

pub fn app() -> Element {
    AppTheme::install(light());
    let dark_mode = signal(false);

    ui! {
        view(style = Screen()) {
            view(style = Panel()) {
                text(style = Accent()) { "Themed with tokens" }
                text(style = Body()) { "Flip the switch: every token-styled view repaints." }
            }
            toggle(
                value = dark_mode,
                on_change = move |on| {
                    dark_mode.set(on);
                    AppTheme::set(if on { dark() } else { light() });
                },
            )
        }
    }
}
