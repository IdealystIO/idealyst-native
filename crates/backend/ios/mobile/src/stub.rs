use framework_core::{Backend, StyleRules};
use std::rc::Rc;

pub struct IosBackend;

impl Backend for IosBackend {
    type Node = ();

    fn platform(&self) -> framework_core::Platform {
        framework_core::Platform::Ios
    }

    fn create_view(&mut self, _a11y: &framework_core::accessibility::AccessibilityProps) -> Self::Node {
        unreachable!("backend-ios stub: UIKit calls only on iOS target")
    }
    fn create_text(&mut self, _content: &str, _a11y: &framework_core::accessibility::AccessibilityProps) -> Self::Node {
        unreachable!()
    }
    fn create_button(&mut self, _label: &str, _on_click: &framework_core::Action, _leading_icon: Option<&framework_core::IconData>, _trailing_icon: Option<&framework_core::IconData>, _a11y: &framework_core::accessibility::AccessibilityProps) -> Self::Node {
        unreachable!()
    }
    fn insert(&mut self, _parent: &mut Self::Node, _child: Self::Node) {
        unreachable!()
    }
    fn update_text(&mut self, _node: &Self::Node, _content: &str) {
        unreachable!()
    }
    fn clear_children(&mut self, _node: &Self::Node) {
        unreachable!()
    }
    fn apply_style(&mut self, _node: &Self::Node, _style: &Rc<StyleRules>) {
        unreachable!()
    }
    fn finish(&mut self, _root: Self::Node) {
        unreachable!()
    }
}

impl IosBackend {
    /// Stub for the iOS-only `run_layout` so non-iOS hosts that
    /// reference it (e.g. a shared crate that calls it under cfg)
    /// link cleanly.
    pub fn run_layout(&mut self) {}
}
