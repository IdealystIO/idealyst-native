use runtime_core::{stylesheet, ui, Color, Element, FontWeight};

// A stylesheet is declared once, outside your components. `<()>` means it
// reads no theme yet — every value here is a literal.
stylesheet! {
    pub Card<()> {
        base(_t) {
            background: Color("#ffffff".into()),
            padding: 20.0,                 // bare numbers are pixels
            border_radius: 12.0,
            border_width: 1.0,
            border_color: Color("#e2e4ea".into()),
            gap: 8.0,
        }
    }
}

stylesheet! {
    pub Title<()> {
        base(_t) {
            font_size: 20.0,
            font_weight: FontWeight::SemiBold,
            color: Color("#1f2328".into()),
        }
    }
}

stylesheet! {
    pub Body<()> {
        base(_t) {
            font_size: 14.0,
            line_height: 20.0,
            color: Color("#57606a".into()),
        }
    }
}

stylesheet! {
    pub Page<()> {
        base(_t) {
            padding: 24.0,
            background: Color("#f3f4f6".into()),
        }
    }
}

pub fn app() -> Element {
    // `Card()` is the builder the macro generated; hand it to `style`.
    ui! {
        view(style = Page()) {
            view(style = Card()) {
                text(style = Title()) { "A styled card" }
                text(style = Body()) { "Change a color, a radius or the padding, then run it again." }
            }
        }
    }
}
