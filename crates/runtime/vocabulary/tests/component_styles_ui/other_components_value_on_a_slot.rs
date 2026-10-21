// A Card variant does not fit a Button slot.
use runtime_macros::component_styles;

component_styles! {
    pub Button<()> { slot root { base(_t) {} variant variant { #[default] filled(_t) {} ghost(_t) {} } } }
}
component_styles! {
    pub Card<()> { slot root { base(_t) {} variant variant { #[default] regular(_t) {} flat(_t) {} } } }
}

fn main() {
    let _ = ButtonStyles::root().set(CardVariant::Flat);
}
