//! Non-macOS stub. Lets every consumer crate type-check on any host.
//!
//! The real backend lives under `cfg(target_os = "macos")` in
//! [`crate::imp`]. This stub mirrors the public surface — a unit
//! struct that implements `Backend` by panicking. Reachable only if
//! someone calls the stub at runtime on a non-macOS host, which the
//! cargo target gates already prevent.

use framework_core::{Backend, StyleRules};
use std::rc::Rc;

pub struct MacosBackend;

impl Backend for MacosBackend {
    type Node = ();

    fn platform(&self) -> framework_core::Platform {
        framework_core::Platform::MacOs
    }

    fn create_view(&mut self) -> Self::Node {
        unreachable!("backend-macos stub: AppKit calls only on macOS target")
    }
    fn create_text(&mut self, _content: &str) -> Self::Node {
        unreachable!()
    }
    fn create_button(
        &mut self,
        _label: &str,
        _on_click: &framework_core::Action,
        _leading_icon: Option<&framework_core::IconData>,
        _trailing_icon: Option<&framework_core::IconData>,
    ) -> Self::Node {
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
