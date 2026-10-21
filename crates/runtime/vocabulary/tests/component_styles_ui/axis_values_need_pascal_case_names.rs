// An `axis_values!` value is named in PascalCase; its key is derived from
// the name, so a snake_case name (a key written by hand) is refused.
use runtime_macros::{axis_values, component_styles};
use runtime_vocabulary::glue::Color;

component_styles! {
    pub Chip<()> {
        slot root {
            base(_t) { color: Color("black".into()) }
            variant look { #[default] soft(_t) {} }
        }
    }
}

axis_values! { impl ChipLook { glow } }

fn main() {}
