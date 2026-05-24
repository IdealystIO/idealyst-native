//! Non-Android stub. The actual Android backend lives under
//! [`crate::imp`] behind `#[cfg(target_os = "android")]`. This stub
//! exists so the workspace compiles on host platforms (Linux, macOS)
//! without an NDK toolchain — every `Backend` method panics
//! `unreachable!()` and would only be called if someone routed an
//! `AndroidBackend` to a non-Android render path, which is a
//! configuration bug.

use runtime_core::{Backend, StyleRules};
use std::rc::Rc;

pub struct AndroidBackend;

impl AndroidBackend {
    pub fn new(_context: (), _root: ()) -> Self {
        AndroidBackend
    }
}

impl Backend for AndroidBackend {
    type Node = ();

    fn platform(&self) -> runtime_core::Platform {
        runtime_core::Platform::Android
    }

    fn create_view(&mut self, _a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        unreachable!("backend-android stub: only available on android target")
    }
    fn create_text(&mut self, _content: &str, _a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        unreachable!()
    }
    fn create_button(&mut self, _label: &str, _on_click: &runtime_core::Action, _leading_icon: Option<&runtime_core::IconData>, _trailing_icon: Option<&runtime_core::IconData>, _a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
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
