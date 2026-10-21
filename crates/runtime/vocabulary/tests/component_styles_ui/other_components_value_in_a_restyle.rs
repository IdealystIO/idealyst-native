// A restyle of Button cannot add an arm keyed by a Card axis value.
use runtime_macros::component_styles;
use runtime_vocabulary::glue::StyleRules;

component_styles! {
    pub Button<()> { slot root { base(_t) {} variant variant { #[default] filled(_t) {} ghost(_t) {} } } }
}
component_styles! {
    pub Card<()> { slot root { base(_t) {} variant variant { #[default] regular(_t) {} flat(_t) {} } } }
}

fn main() {
    ButtonStyles::restyle(|s| {
        s.root().arm(CardVariant::Flat, |_| StyleRules::default());
    });
}
