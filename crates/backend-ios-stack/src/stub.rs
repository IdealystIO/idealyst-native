use framework_core::{Backend, StyleRules};
use std::rc::Rc;

pub struct IosBackend;

impl Backend for IosBackend {
    type Node = ();

    fn create_view(&mut self) -> Self::Node {
        unreachable!("backend-ios stub: UIKit calls only on iOS target")
    }
    fn create_text(&mut self, _content: &str) -> Self::Node {
        unreachable!()
    }
    fn create_button(&mut self, _label: &str, _on_click: Rc<dyn Fn()>, _leading_icon: Option<&framework_core::IconData>, _trailing_icon: Option<&framework_core::IconData>) -> Self::Node {
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
