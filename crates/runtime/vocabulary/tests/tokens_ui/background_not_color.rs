// The host surface takes a colour: `app_background` on a length is rejected.
use runtime_macros::Tokens;
use runtime_vocabulary::glue::Length;

#[derive(Tokens, Clone)]
#[tokens(default = light)]
pub struct AppTheme {
    #[tokens(app_background)]
    pub gap: Length,
}

fn light() -> AppTheme {
    AppTheme { gap: 4.0.into() }
}

fn main() {}
