// `accent_bg` and `accent.bg` both kebab to `accent-bg`: they would be one
// token, so the theme is rejected where it is defined.
use runtime_macros::Tokens;
use runtime_vocabulary::glue::Color;

#[derive(Tokens, Clone)]
pub struct Accent {
    pub bg: Color,
}

#[derive(Tokens, Clone)]
#[tokens(default = light)]
pub struct AppTheme {
    pub accent_bg: Color,
    pub accent: Accent,
}

fn light() -> AppTheme {
    AppTheme { accent_bg: "#fff".into(), accent: Accent { bg: "#000".into() } }
}

fn main() {}
