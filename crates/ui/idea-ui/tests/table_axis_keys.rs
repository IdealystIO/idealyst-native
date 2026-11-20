//! CrewForge: an app restyle of a frozen body cell at rest could only
//! name its state with `raw_compound` strings, because the crate root
//! exported typed keys for `variant` and `head_edge` alone. A renamed axis
//! or value then failed silently. Every axis `TableStyles` declares is now
//! a typed key at the crate root.

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui::{TablePinned, TableRowHovered, TableStyles};
use runtime_core::{resolve_style, Color, StyleRules, Tokenized};

#[test]
fn a_frozen_cell_at_rest_restyles_through_typed_keys() {
    let h = host_mock::Harness::new();
    h.world.enter(|| {
        install_idea_theme(light_theme());
        TableStyles::restyle(|s| {
            s.body_cell().compound((TablePinned::Left, TableRowHovered::Off), |_| StyleRules {
                background: Some(Tokenized::Literal(Color("#ffffff".into()))),
                ..Default::default()
            });
        });
        let app = TableStyles::body_cell().into_style_application().with("pinned", "left").with("row_hovered", "off");
        let rules = resolve_style(&app);
        assert_eq!(rules.background.as_ref().map(|c| c.resolve().0), Some("#ffffff".to_string()));
    });
}
