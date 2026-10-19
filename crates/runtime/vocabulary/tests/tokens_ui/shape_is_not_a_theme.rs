// A struct without `#[tokens(default = …)]` is a shape: it cannot be
// installed on its own.
use runtime_macros::Tokens;
use runtime_vocabulary::glue::{Color, Tokens};

#[derive(Tokens, Clone)]
pub struct Swatch {
    pub soft_bg: Color,
}

fn main() {
    Swatch::install(Swatch { soft_bg: "#eef".into() });
}
