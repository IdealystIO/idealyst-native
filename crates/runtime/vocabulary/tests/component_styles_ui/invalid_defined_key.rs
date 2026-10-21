// An app-defined axis value becomes part of a class name: a key with
// characters outside `[a-z0-9_]` is rejected at compile time.
use runtime_macros::component_styles;

component_styles! {
    pub Button<()> { slot root { base(_t) {} variant variant { #[default] filled(_t) {} } } }
}

const PILL: ButtonVariant = ButtonVariant::define("Pill Shape");

fn main() {
    let _ = PILL;
}
