//! iOS backend: builds UIKit views via objc2.
//!
//! Compile-only spike. Real `objc2-ui-kit` calls under `target_os = "ios"`;
//! a stub on other hosts so the crate type-checks during cross-compile.

use framework_core::Backend;
use std::rc::Rc;

#[cfg(target_os = "ios")]
mod imp {
    use super::*;
    use objc2::rc::Retained;
    use objc2_foundation::{MainThreadMarker, NSString};
    use objc2_ui_kit::{UILabel, UIView};

    pub struct IosBackend {
        mtm: MainThreadMarker,
    }

    impl IosBackend {
        pub fn new(mtm: MainThreadMarker) -> Self {
            Self { mtm }
        }
    }

    #[derive(Clone)]
    pub enum IosNode {
        View(Retained<UIView>),
        Label(Retained<UILabel>),
    }

    impl IosNode {
        fn as_view(&self) -> &UIView {
            match self {
                IosNode::View(v) => v,
                IosNode::Label(l) => l,
            }
        }
    }

    impl Backend for IosBackend {
        type Node = IosNode;

        fn create_view(&mut self) -> Self::Node {
            let view = unsafe { UIView::new(self.mtm) };
            IosNode::View(view)
        }

        fn create_text(&mut self, content: &str) -> Self::Node {
            let label = unsafe { UILabel::new(self.mtm) };
            let ns_text = NSString::from_str(content);
            unsafe { label.setText(Some(&ns_text)) };
            IosNode::Label(label)
        }

        fn create_button(&mut self, label: &str, _on_click: Rc<dyn Fn()>) -> Self::Node {
            // Buttons + target/action selectors aren't wired in the spike;
            // render as a label so the trait stays implementable.
            self.create_text(label)
        }

        fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
            let parent_view = parent.as_view();
            let child_view = child.as_view();
            unsafe { parent_view.addSubview(child_view) };
        }

        fn update_text(&mut self, node: &Self::Node, content: &str) {
            if let IosNode::Label(label) = node {
                let ns = NSString::from_str(content);
                unsafe { label.setText(Some(&ns)) };
            }
        }

        fn clear_children(&mut self, node: &Self::Node) {
            // Iterate over the parent's subviews and remove each. UIKit's
            // `subviews` returns a snapshot, so we can iterate without
            // mutation hazards.
            let parent = node.as_view();
            let subviews = parent.subviews();
            for sub in subviews.iter() {
                unsafe { sub.removeFromSuperview() };
            }
        }

        fn finish(&mut self, _root: Self::Node) {}
    }
}

#[cfg(not(target_os = "ios"))]
mod imp {
    use super::*;

    pub struct IosBackend;

    impl Backend for IosBackend {
        type Node = ();

        fn create_view(&mut self) -> Self::Node {
            unreachable!("backend-ios stub: UIKit calls only on iOS target")
        }
        fn create_text(&mut self, _content: &str) -> Self::Node {
            unreachable!()
        }
        fn create_button(&mut self, _label: &str, _on_click: Rc<dyn Fn()>) -> Self::Node {
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
        fn finish(&mut self, _root: Self::Node) {
            unreachable!()
        }
    }
}

pub use imp::IosBackend;
