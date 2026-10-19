// A token that does not exist is a compile error, not a string that
// silently resolves to its fallback forever.
use runtime_macros::Tokens;
use runtime_vocabulary::glue::{Color, Tokens};

#[derive(Tokens, Clone)]
pub struct Swatch {
    pub soft_bg: Color,
}

#[derive(Tokens, Clone)]
#[tokens(default = light)]
pub struct AppTheme {
    pub surface: Color,
    pub accent: Swatch,
}

fn light() -> AppTheme {
    AppTheme { surface: "#fff".into(), accent: Swatch { soft_bg: "#eef".into() } }
}

fn main() {
    let t = AppTheme::tokens();
    let _ = t.surfce();
    let _ = t.accent.soft_gb();
}
