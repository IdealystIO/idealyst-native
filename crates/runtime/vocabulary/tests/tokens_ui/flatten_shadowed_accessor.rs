// The embedding set's `color` leaf would hide the base's `color`
// namespace (`t.color` resolves to the nearer accessor), even though the
// token names differ — refused.
use runtime_macros::Tokens;
use runtime_vocabulary::glue::Color;

#[derive(Tokens, Clone)]
pub struct Neutrals {
    pub surface: Color,
}

#[derive(Tokens, Clone)]
pub struct Base {
    pub color: Neutrals,
}

#[derive(Tokens, Clone)]
pub struct App {
    pub color: Color,
    #[tokens(flatten)]
    pub base: Base,
}

fn main() {}
