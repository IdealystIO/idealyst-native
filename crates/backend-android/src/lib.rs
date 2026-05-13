//! Android backend: drives the View system via JNI.
//!
//! Compile-only spike. Real JNI calls under `target_os = "android"`; stub
//! elsewhere so the crate type-checks during cross-compile validation.

use framework_core::{Backend, StyleRules};
use std::rc::Rc;

#[cfg(target_os = "android")]
mod imp {
    use super::*;
    use jni::objects::{GlobalRef, JObject, JValue};
    use jni::JNIEnv;

    pub struct AndroidBackend<'local> {
        env: JNIEnv<'local>,
        context: GlobalRef,
    }

    impl<'local> AndroidBackend<'local> {
        pub fn new(env: JNIEnv<'local>, context: GlobalRef) -> Self {
            Self { env, context }
        }
    }

    impl<'local> Backend for AndroidBackend<'local> {
        type Node = GlobalRef;

        fn create_view(&mut self) -> Self::Node {
            let class = self.env.find_class("android/widget/LinearLayout").unwrap();
            let local = self
                .env
                .new_object(
                    class,
                    "(Landroid/content/Context;)V",
                    &[JValue::Object(self.context.as_obj())],
                )
                .unwrap();
            self.env.new_global_ref(local).unwrap()
        }

        fn create_text(&mut self, content: &str) -> Self::Node {
            let class = self.env.find_class("android/widget/TextView").unwrap();
            let local = self
                .env
                .new_object(
                    class,
                    "(Landroid/content/Context;)V",
                    &[JValue::Object(self.context.as_obj())],
                )
                .unwrap();
            let java_str = self.env.new_string(content).unwrap();
            let java_obj: JObject = java_str.into();
            self.env
                .call_method(
                    &local,
                    "setText",
                    "(Ljava/lang/CharSequence;)V",
                    &[JValue::Object(&java_obj)],
                )
                .unwrap();
            self.env.new_global_ref(local).unwrap()
        }

        fn create_button(&mut self, label: &str, _on_click: Rc<dyn Fn()>) -> Self::Node {
            // OnClickListener bridging not wired in the spike. Render label-only.
            self.create_text(label)
        }

        fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
            self.env
                .call_method(
                    parent.as_obj(),
                    "addView",
                    "(Landroid/view/View;)V",
                    &[JValue::Object(child.as_obj())],
                )
                .unwrap();
        }

        fn update_text(&mut self, node: &Self::Node, content: &str) {
            let java_str = self.env.new_string(content).unwrap();
            let java_obj: JObject = java_str.into();
            self.env
                .call_method(
                    node.as_obj(),
                    "setText",
                    "(Ljava/lang/CharSequence;)V",
                    &[JValue::Object(&java_obj)],
                )
                .unwrap();
        }

        fn clear_children(&mut self, node: &Self::Node) {
            // ViewGroup.removeAllViews()
            self.env
                .call_method(node.as_obj(), "removeAllViews", "()V", &[])
                .unwrap();
        }

        fn apply_style(&mut self, _node: &Self::Node, _style: &Rc<StyleRules>) {
            // Real Android styling would call View setters (setBackgroundColor,
            // setPadding, etc.) or apply theme attributes. Stubbed for the
            // spike; trait shape is what we're validating.
        }

        fn finish(&mut self, _root: Self::Node) {}
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    use super::*;

    pub struct AndroidBackend;

    impl Backend for AndroidBackend {
        type Node = ();

        fn create_view(&mut self) -> Self::Node {
            unreachable!("backend-android stub: JNI calls only on Android target")
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
        fn apply_style(&mut self, _node: &Self::Node, _style: &Rc<StyleRules>) {
            unreachable!()
        }
        fn finish(&mut self, _root: Self::Node) {
            unreachable!()
        }
    }
}

pub use imp::AndroidBackend;
