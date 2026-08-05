use runtime_core::{stylesheet, StyleApplication};

stylesheet! {
    pub Chip<()> {
        base(_t) { padding: 12.0, border_radius: 8.0 }
        variant tone {
            #[default]
            neutral(_t) {}
            primary(_t) { padding: 16.0 }
        }
    }
}

// Overrides merge in last — after base, variants, and compounds — so a
// value that can't be enumerated as a variant still wins.
fn oversized_primary() -> StyleApplication {
    StyleApplication::new(chip_style())
        .with("tone", "primary") // select a variant by (axis, value)
        .override_font_size(18.0) // a one-off value, merged on top
}
