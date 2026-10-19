// A field that is neither a token value nor a token set is rejected with
// the explanation of what a field may be.
use runtime_macros::Tokens;
use runtime_vocabulary::glue::Color;

#[derive(Clone)]
pub struct Font(pub String);

#[derive(Tokens, Clone)]
pub struct AppTheme {
    pub surface: Color,
    pub font: Font,
}

fn main() {}
