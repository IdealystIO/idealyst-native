// `flatten` embeds a token SET; a leaf has nothing to flatten.
use runtime_macros::Tokens;
use runtime_vocabulary::glue::Color;

#[derive(Tokens, Clone)]
pub struct App {
    #[tokens(flatten)]
    pub brand: Color,
}

fn main() {}
