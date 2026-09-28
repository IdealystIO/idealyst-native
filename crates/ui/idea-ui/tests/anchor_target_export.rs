//! Regression #40: `AnchorTarget` — the type every anchored overlay's
//! `target` prop takes (`Popover`, `Menu`, `Tooltip`) — wasn't exported
//! from idea-ui, so even idea-ui's own recipes reached into
//! `runtime_core::primitives::portal`. It (and the `side` / `align` enums
//! the same props take) must be nameable from the idea-ui root.

use idea_theme::testing::with_test_world;
use idea_ui::{AnchorTarget, ElementAlign, ElementSide, Popover, PopoverProps};
use runtime_core::{PressableHandle, Reactive, Ref};

#[test]
fn regression_anchor_target_is_exported_from_idea_ui() {
    with_test_world(|| {
        let trigger: Ref<PressableHandle> = Ref::new();
        let props = PopoverProps {
            target: Some(AnchorTarget::from(trigger)),
            side: Reactive::Static(ElementSide::Above),
            align: Reactive::Static(ElementAlign::End),
            ..Default::default()
        };
        assert!(props.target.is_some());
        // Builds with the idea-ui-root types — the popover API is usable
        // without a `runtime_core::primitives::portal` import.
        let _ = Popover(props);
        // Same type as the primitive's, not a look-alike.
        let _: Option<runtime_core::primitives::portal::AnchorTarget> =
            Some(AnchorTarget::from(Ref::<PressableHandle>::new()));
    });
}
