//! Regression tests for `prim-*` primitive gating (phase 1: virtualizer).
//!
//! The bug class being prevented: with a `prim-*` feature disabled, a
//! primitive that still reaches the walker at runtime (wire-received
//! subtree, hand-built `Element`) must degrade to the backend's standard
//! "unsupported external" placeholder — naming the feature so the remedy
//! is on screen — instead of `unimplemented!`-panicking through the
//! `Backend` default or silently rendering nothing.
//!
//! Compile-time authoring is covered separately by construction: the
//! `flat_list` / `virtualizer` builder fns are `#[cfg(feature)]`-gated, so
//! an app using the tag without the feature fails to compile.
//!
//! Run the gated-off half with:
//!   cargo test -p runtime-core --no-default-features --test prim_gating

#[path = "common/mod.rs"]
mod common;

#[cfg(not(feature = "prim-virtualizer"))]
mod virtualizer_gated_off {
    use super::common::mock_backend::Event;
    use super::common::runtime::TestRuntime;
    use runtime_core::{Element, IntoDerived, ItemKey, ItemSize, VirtualLayout};
    use std::rc::Rc;

    /// Hand-build the `Element::Virtualizer` the gated-out authoring fns
    /// would have produced — this is exactly what a wire client receives
    /// from a runtime-server that was compiled WITH the primitive.
    fn hand_built_virtualizer() -> Element {
        Element::Virtualizer {
            item_count: (|| 3usize).into_derived(),
            item_key: Box::new(|i| i as ItemKey),
            item_size: ItemSize::Known(Rc::new(|_| 20.0)),
            render_item: Rc::new(|_| Element::Fragment { children: vec![] }),
            row_template: None,
            row_index_signal_id: None,
            overscan: 1.0,
            layout: VirtualLayout::default(),
            style: None,
            ref_fill: None,
            accessibility: Default::default(),
        }
    }

    #[test]
    fn regression_gated_virtualizer_renders_placeholder_naming_feature() {
        let rt = TestRuntime::new();
        let _owner = rt.render(hand_built_virtualizer());

        let events = rt.events();
        let placeholder = events.iter().find_map(|e| match e {
            Event::CreateExternal { type_name } => Some(*type_name),
            _ => None,
        });
        let name = placeholder.expect(
            "gated-off virtualizer must fall back to the unsupported-external \
             placeholder, not panic or vanish",
        );
        assert!(
            name.contains("prim-virtualizer"),
            "placeholder label must name the feature to enable, got: {name}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, Event::CreateVirtualizer { .. })),
            "no virtualizer may be created when the feature is off"
        );
    }
}

#[cfg(not(feature = "prim-navigator"))]
mod navigator_gated_off {
    use super::common::mock_backend::Event;
    use super::common::runtime::TestRuntime;
    use runtime_core::primitives::navigator::NavigatorConfig;
    use runtime_core::Element;
    use std::rc::Rc;

    /// Wire-shaped `Element::Navigator` — what a client compiled without
    /// `prim-navigator` receives from a runtime-server that has it.
    fn hand_built_navigator() -> Element {
        struct FakePresentation;
        Element::Navigator {
            type_id: std::any::TypeId::of::<FakePresentation>(),
            type_name: "FakeStack",
            presentation: Rc::new(()),
            config: Box::new(NavigatorConfig::new("home", "/")),
            style: None,
            slot_styles: Vec::new(),
            ref_fill: None,
            accessibility: Default::default(),
        }
    }

    #[test]
    fn regression_gated_navigator_renders_placeholder_naming_feature() {
        let rt = TestRuntime::new();
        let _owner = rt.render(hand_built_navigator());

        let events = rt.events();
        let name = events
            .iter()
            .find_map(|e| match e {
                Event::CreateExternal { type_name } => Some(*type_name),
                _ => None,
            })
            .expect("gated-off navigator must fall back to the placeholder");
        assert!(
            name.contains("prim-navigator"),
            "placeholder label must name the feature to enable, got: {name}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, Event::CreateNavigator { .. })),
            "no navigator may be created when the feature is off"
        );
    }

    #[test]
    fn regression_gated_navigator_outlet_renders_placeholder() {
        let rt = TestRuntime::new();
        let _owner = rt.render(Element::NavigatorOutlet {
            style: None,
            ref_fill: None,
            accessibility: Default::default(),
        });
        let name = rt
            .events()
            .iter()
            .find_map(|e| match e {
                Event::CreateExternal { type_name } => Some(*type_name),
                _ => None,
            })
            .expect("gated-off outlet must fall back to the placeholder");
        assert!(name.contains("prim-navigator"), "got: {name}");
    }
}

#[cfg(not(feature = "prim-lazy"))]
mod lazy_gated_off {
    use super::common::mock_backend::Event;
    use super::common::runtime::TestRuntime;
    use runtime_core::Element;

    #[test]
    fn regression_gated_lazy_renders_placeholder_naming_feature() {
        let rt = TestRuntime::new();
        let _owner = rt.render(Element::Lazy {
            loader: Box::new(|| {
                Box::pin(async { Ok(Element::Fragment { children: vec![] }) })
            }),
            on_state: None,
            placeholder: None,
            error: None,
            style: None,
            ref_fill: None,
            accessibility: Default::default(),
        });
        let name = rt
            .events()
            .iter()
            .find_map(|e| match e {
                Event::CreateExternal { type_name } => Some(*type_name),
                _ => None,
            })
            .expect("gated-off lazy must fall back to the placeholder");
        assert!(
            name.contains("prim-lazy"),
            "placeholder label must name the feature to enable, got: {name}"
        );
    }
}

/// The OTHER half of feature-mismatch safety: the walker HAS a family
/// (`prim-icon` on runtime-core — e.g. forwarded by an SDK) but the
/// BACKEND was compiled without it, so the trait's default `create_icon`
/// runs. That default used to `unimplemented!` — a runtime abort for a
/// pure packaging mistake. It must degrade to the same labeled
/// "unsupported" placeholder as the walker's own gated-off dispatch.
#[cfg(feature = "prim-icon")]
mod backend_missing_family {
    use runtime_core::accessibility::AccessibilityProps;
    use runtime_core::{Backend, IntoElement};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Minimal backend: implements the always-on core surface only —
    /// notably NOT `create_icon` — plus `create_external` so the
    /// placeholder path has somewhere to land (every real backend has it).
    struct NoIconBackend {
        next: RefCell<u32>,
        externals: Rc<RefCell<Vec<&'static str>>>,
    }
    impl NoIconBackend {
        fn mint(&self) -> u32 {
            let id = *self.next.borrow();
            *self.next.borrow_mut() = id + 1;
            id
        }
    }
    impl Backend for NoIconBackend {
        type Node = u32;
        fn handles_states_natively(&self) -> bool {
            true
        }
        fn create_view(&mut self, _a11y: &AccessibilityProps) -> u32 {
            self.mint()
        }
        fn create_text(&mut self, _content: &str, _a11y: &AccessibilityProps) -> u32 {
            self.mint()
        }
        fn create_button(
            &mut self,
            _label: &str,
            _on_click: &runtime_core::Action,
            _leading: Option<&runtime_core::primitives::icon::IconData>,
            _trailing: Option<&runtime_core::primitives::icon::IconData>,
            _a11y: &AccessibilityProps,
        ) -> u32 {
            self.mint()
        }
        fn insert(&mut self, _parent: &mut u32, _child: u32) {}
        fn update_text(&mut self, _node: &u32, _content: &str) {}
        fn clear_children(&mut self, _node: &u32) {}
        fn apply_style(&mut self, _node: &u32, _style: &Rc<runtime_core::StyleRules>) {}
        fn execute_batch(&mut self, batch: runtime_core::BackendBatch) -> Vec<u32> {
            (0..batch.node_count).map(|_| self.mint()).collect()
        }
        fn insert_many(&mut self, _parent: &mut u32, _children: Vec<u32>) {}
        fn finish(&mut self, _root: u32) {}
        fn create_external(
            &mut self,
            _type_id: std::any::TypeId,
            type_name: &'static str,
            _payload: &Rc<dyn std::any::Any>,
            _a11y: &AccessibilityProps,
        ) -> u32 {
            self.externals.borrow_mut().push(type_name);
            self.mint()
        }
    }

    #[test]
    fn regression_backend_without_family_renders_placeholder_not_panic() {
        let externals = Rc::new(RefCell::new(Vec::new()));
        let backend = Rc::new(RefCell::new(NoIconBackend {
            next: RefCell::new(0),
            externals: externals.clone(),
        }));
        let icon = runtime_core::icon(runtime_core::primitives::icon::IconData {
            view_box: (4, 4),
            paths: &["M0 0h4v4z"],
            fill_rule: runtime_core::FillRule::NonZero,
            filled: false,
        });
        let (_root, _scope) =
            runtime_core::build_detached(&backend, icon.into_element(), None);
        let seen = externals.borrow();
        assert_eq!(
            seen.len(),
            1,
            "the missing-family default must land on create_external"
        );
        assert!(
            seen[0].contains("prim-icon"),
            "placeholder label must name the missing backend feature, got: {}",
            seen[0]
        );
    }
}

#[cfg(feature = "prim-virtualizer")]
mod virtualizer_gated_on {
    /// Anchor: under default features the authoring surface exists and the
    /// full behavior is covered by the walker suite
    /// (`ui_iteration_and_branches` flat_list tests). This test only pins
    /// that the default feature set actually includes the primitive.
    #[test]
    fn default_features_include_virtualizer_authoring() {
        // Reference the gated fn; fails to compile if the default set drops it.
        let _ = runtime_core::flat_list::<
            i32,
            fn(usize, &i32) -> runtime_core::ItemKey,
            (),
            fn(usize, &i32) -> runtime_core::Element,
        >;
    }

    /// Same pin for every other gated family: the authoring fns must exist
    /// under the default feature set. (The gated-off placeholder path is one
    /// shared code path, regression-tested via the virtualizer test above.)
    #[test]
    fn default_features_include_all_prim_families() {
        let _ = runtime_core::icon;
        let _ = runtime_core::image::<&'static str>;
        let _ = runtime_core::text_input::<fn(String)>;
        let _ = runtime_core::text_area::<fn(String)>;
        let _ = runtime_core::toggle::<fn(bool)>;
        let _ = runtime_core::primitives::slider::slider::<fn(f32)>;
        let _ = runtime_core::primitives::activity_indicator::activity_indicator;
        let _ = runtime_core::overlay;
        let _ = runtime_core::presence::<fn() -> runtime_core::Element>;
        let _ = runtime_core::primitives::graphics::graphics::<
            fn(runtime_core::primitives::graphics::OnReadyEvent),
        >;
        // prim-lazy: the `lazy!` / `#[lazy_component]` constructor.
        let _ = runtime_core::primitives::lazy::lazy_split::<
            fn() -> runtime_core::primitives::lazy::LazyFuture,
        >;
        // prim-navigator gates no runtime-core authoring fn (SDK crates own
        // the authoring surface and forward the feature); pin the default
        // set directly.
        assert!(cfg!(feature = "prim-navigator"));
    }
}
