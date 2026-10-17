// The embedding set's own `color_surface` and the flattened base's
// `color.surface` are both `color-surface`: refused where the theme is
// defined, never a silent overwrite.
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
#[tokens(default = app)]
pub struct App {
    pub color_surface: Color,
    #[tokens(flatten)]
    pub base: Base,
}

fn app() -> App {
    App { color_surface: "#fff".into(), base: Base { color: Neutrals { surface: "#000".into() } } }
}

fn main() {}
