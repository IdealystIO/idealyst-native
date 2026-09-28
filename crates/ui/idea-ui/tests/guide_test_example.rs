//! The test example from the `idiomatic-components` guide
//! (`crates/mcp/catalog/guides/idiomatic-components.md`, §4 "Tests"),
//! run for real.
//!
//! Regression #34: the guide's example had no world setup and matched
//! `Element::View` / `StyleSource::Static` — shapes that no longer exist
//! (primitives are type-erased `Element::Item`s now) — so a reader who
//! copied it got a test that didn't compile. The guide spells the body
//! as an in-crate unit test (`use super::*;`, `crate::test_support`);
//! this copy differs ONLY in those paths. Change the two together.

use idea_theme::testing::with_test_world;
use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui::test_support::{classify, P};
use idea_ui::{Stack, StackAlign, StackAxis, StackProps};
use runtime_core::{AlignItems, Reactive};

#[test]
fn row_align_center_resolves_to_align_items_center() {
    with_test_world(|| {
        install_idea_theme(light_theme());
        let el = Stack(StackProps {
            axis: Reactive::Static(StackAxis::Row),
            align: Reactive::Static(StackAlign::Center),
            ..Default::default()
        });
        let rules = match classify(el) {
            P::View { style: Some(style), .. } => style.resolve(),
            _ => panic!("Stack renders a styled view"),
        };
        assert_eq!(rules.align_items, Some(AlignItems::Center));
    });
}
