// A theme assigns a value to EVERY token: leaving one out of a theme
// function is a compile error naming the field.
use runtime_macros::Tokens;
use runtime_vocabulary::glue::Color;

#[derive(Tokens, Clone)]
#[tokens(default = light)]
pub struct AppTheme {
    pub surface: Color,
    pub text: Color,
}

fn light() -> AppTheme {
    AppTheme { surface: "#fff".into() }
}

fn main() {}
