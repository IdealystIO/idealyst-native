//! Non-Android stub. The actual Android backend lives under
//! [`crate::imp`] behind `#[cfg(target_os = "android")]`. This stub
//! exists so the workspace compiles on host platforms (Linux, macOS)
//! without an NDK toolchain — every `Backend` method panics
//! `unreachable!()` and would only be called if someone routed an
//! `AndroidBackend` to a non-Android render path, which is a
//! configuration bug.

use framework_core::{Backend, StyleRules};
use std::rc::Rc;

pub struct AndroidBackend;

impl AndroidBackend {
    pub fn new(_context: (), _root: ()) -> Self {
        AndroidBackend
    }
}

impl Backend for AndroidBackend {
    type Node = ();

    fn platform(&self) -> framework_core::Platform {
        framework_core::Platform::Android
    }

    fn create_view(&mut self) -> Self::Node {
        unreachable!("backend-android stub: only available on android target")
    }
    fn create_text(&mut self, _content: &str) -> Self::Node {
        unreachable!()
    }
    fn create_button(&mut self, _label: &str, _on_click: &framework_core::Action, _leading_icon: Option<&framework_core::IconData>, _trailing_icon: Option<&framework_core::IconData>) -> Self::Node {
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
