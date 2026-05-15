//! iOS backend: builds UIKit views via objc2.
//!
//! Compile-only spike. Real `objc2-ui-kit` calls under `target_os = "ios"`;
//! a stub on other hosts so the crate type-checks during cross-compile.

use framework_core::{Backend, StyleRules};
use std::rc::Rc;

#[cfg(target_os = "ios")]
mod imp {
    use super::*;
    use framework_core::primitives::navigator::{
        NavCommand, NavigatorCallbacks, NavigatorControl, NavigatorHandle, NavigatorOps,
    };
    use objc2::rc::Retained;
    use objc2_foundation::{MainThreadMarker, NSString};
    use objc2_ui_kit::{UILabel, UINavigationController, UIView, UIViewController};
    use std::cell::RefCell;
    use std::collections::HashMap;

    pub struct IosBackend {
        mtm: MainThreadMarker,
        /// Per-navigator state. Keyed by the raw pointer of the
        /// navigation controller's `.view` (the node we return from
        /// `create_navigator`). The JVM-equivalent on iOS is that
        /// `Retained<UIView>` holds the view alive; we use its raw
        /// pointer as a stable id while we hold the view.
        navigator_instances: HashMap<usize, NavigatorEntry>,
    }

    /// Per-instance navigator state. Holds the navigation controller
    /// itself (so it stays alive past `create_navigator`'s return),
    /// the framework's control plane, and per-screen scope ids so
    /// pops can release them.
    pub(crate) struct NavigatorEntry {
        /// Held to keep the navigation controller alive past
        /// `create_navigator`'s return — UIKit's view-controller
        /// hierarchy relies on the controller's retain count, and
        /// returning only the view would let the controller drop.
        #[allow(dead_code)]
        pub(crate) controller: Retained<UINavigationController>,
        pub(crate) control: Rc<NavigatorControl>,
        /// One entry per pushed screen, top-of-stack last. Each
        /// entry pairs the screen's `UIViewController` (so we can
        /// drop it on pop) with the framework's scope id.
        ///
        /// `Rc<RefCell<...>>` so the dispatcher closure (which lives
        /// on `control`, separate from the navigator_instances map)
        /// can mutate the same stack the backend reads when handing
        /// out handles or tearing down. Currently the backend itself
        /// doesn't read the stack; held for retention symmetry +
        /// future read paths (e.g. exposing depth without going
        /// through the dispatcher).
        #[allow(dead_code)]
        pub(crate) stack: Rc<RefCell<Vec<ScreenEntry>>>,
    }

    pub(crate) struct ScreenEntry {
        pub(crate) vc: Retained<UIViewController>,
        pub(crate) scope_id: u64,
    }

    impl IosBackend {
        pub fn new(mtm: MainThreadMarker) -> Self {
            Self {
                mtm,
                navigator_instances: HashMap::new(),
            }
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

        fn view_key(&self) -> usize {
            self.as_view() as *const UIView as usize
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

        fn apply_style(&mut self, _node: &Self::Node, _style: &Rc<StyleRules>) {
            // Real iOS styling would resolve the StyleApplication to
            // UIColor/CALayer/UIFont property updates here. Stubbed for
            // the spike; the trait shape is what we're validating across
            // platforms.
        }

        /// `Primitive::Navigator` — wrap each screen subtree in a
        /// barebones `UIViewController` and push/pop on a
        /// `UINavigationController`. The framework hands us
        /// `mount_screen` + `release_screen` callbacks; we drive them
        /// from the dispatch closure the user-facing handle sends
        /// commands to.
        ///
        /// Returned node: the navigation controller's `.view`. The
        /// host app is expected to set the nav controller as a
        /// child VC of its root view controller — the spike doesn't
        /// do that automatically (no shared iOS entry point exists),
        /// so without an explicit parent-VC wire-up the nav bar
        /// won't show correctly. Future iOS app entry work will
        /// handle that.
        fn create_navigator(
            &mut self,
            callbacks: NavigatorCallbacks<Self::Node>,
            control: Rc<NavigatorControl>,
        ) -> Self::Node {
            let nav = unsafe { UINavigationController::new(self.mtm) };
            // `view()` is lazily allocated by UIKit on first access;
            // unwrap is safe because the property is non-optional in
            // practice (the runtime materializes it on demand).
            let nav_view = nav.view().expect("UINavigationController.view");

            // Build the initial screen and push it as the root VC.
            let (initial_node, initial_scope_id) =
                (callbacks.mount_screen)(callbacks.initial_route, Box::new(()));
            let root_vc = unsafe { UIViewController::new(self.mtm) };
            root_vc.setView(Some(initial_node.as_view()));
            unsafe {
                nav.setViewControllers_animated(
                    &objc2_foundation::NSArray::from_vec(vec![root_vc.clone()]),
                    false,
                );
            }

            // Build the per-instance state.
            let stack_rc = Rc::new(RefCell::new(vec![ScreenEntry {
                vc: root_vc,
                scope_id: initial_scope_id,
            }]));
            let entry = NavigatorEntry {
                controller: nav.clone(),
                control: control.clone(),
                stack: stack_rc.clone(),
            };
            let key = &*nav_view as *const UIView as usize;
            self.navigator_instances.insert(key, entry);

            // Install the dispatcher. The dispatcher captures the
            // mtm, the controller, the framework callbacks, and a
            // weak-equivalent (a key into navigator_instances would
            // be cleaner but the dispatcher needs to outlive
            // borrow_mut on the backend; we instead clone what's
            // needed and reach back into navigator_instances at
            // dispatch time via a shared map handle — but since
            // backend is the owner, we use Rc<RefCell<...>> on the
            // stack only. Keep it simpler: clone the nav controller
            // Retained reference into the dispatcher.)
            //
            // The Retained ref-counting keeps the nav controller
            // alive for the dispatcher's lifetime regardless of
            // what happens to navigator_instances.
            let mtm = self.mtm;
            let nav_for_dispatch = nav.clone();
            let mount_for_dispatch = callbacks.mount_screen.clone();
            let release_for_dispatch = callbacks.release_screen.clone();
            let depth_for_dispatch = callbacks.depth_changed.clone();
            let stack_ref = stack_rc.clone();

            control.install(Box::new(move |cmd| {
                let mut stack = stack_ref.borrow_mut();
                match cmd {
                    NavCommand::Push { name, params, url: _ } => {
                        let (node, scope_id) = mount_for_dispatch(name, params);
                        let vc = unsafe { UIViewController::new(mtm) };
                        vc.setView(Some(node.as_view()));
                        unsafe { nav_for_dispatch.pushViewController_animated(&vc, true) };
                        stack.push(ScreenEntry { vc, scope_id });
                        depth_for_dispatch(stack.len());
                    }
                    NavCommand::Pop => {
                        if stack.len() <= 1 {
                            return;
                        }
                        let _ = unsafe { nav_for_dispatch.popViewControllerAnimated(true) };
                        if let Some(popped) = stack.pop() {
                            release_for_dispatch(popped.scope_id);
                        }
                        depth_for_dispatch(stack.len());
                    }
                    NavCommand::Replace { name, params, url: _ } => {
                        let (node, scope_id) = mount_for_dispatch(name, params);
                        let vc = unsafe { UIViewController::new(mtm) };
                        vc.setView(Some(node.as_view()));
                        // UINavigationController doesn't have a
                        // first-class "replace top"; rebuild the
                        // VC array with the new top in place of
                        // the old one. Animated=false because the
                        // user-perceived effect of replace is
                        // instant.
                        if let Some(old) = stack.pop() {
                            release_for_dispatch(old.scope_id);
                        }
                        stack.push(ScreenEntry { vc, scope_id });
                        let vcs: Vec<Retained<UIViewController>> =
                            stack.iter().map(|e| e.vc.clone()).collect();
                        unsafe {
                            nav_for_dispatch.setViewControllers_animated(
                                &objc2_foundation::NSArray::from_vec(vcs),
                                false,
                            );
                        }
                        depth_for_dispatch(stack.len());
                    }
                    NavCommand::Reset { name, params, url: _ } => {
                        let (node, scope_id) = mount_for_dispatch(name, params);
                        let vc = unsafe { UIViewController::new(mtm) };
                        vc.setView(Some(node.as_view()));
                        while let Some(prev) = stack.pop() {
                            release_for_dispatch(prev.scope_id);
                        }
                        stack.push(ScreenEntry { vc: vc.clone(), scope_id });
                        unsafe {
                            nav_for_dispatch.setViewControllers_animated(
                                &objc2_foundation::NSArray::from_vec(vec![vc]),
                                false,
                            );
                        }
                        depth_for_dispatch(stack.len());
                    }
                }
            }));

            IosNode::View(nav_view)
        }

        fn release_navigator(&mut self, node: &Self::Node) {
            let key = node.view_key();
            let Some(entry) = self.navigator_instances.remove(&key) else {
                return;
            };
            // Release every still-mounted scope. We can do this
            // without `release_screen` here because the framework's
            // Owner-teardown path drops every signal/effect via
            // the scope itself — but for navigators torn down by a
            // `when()` flip (not Owner teardown), the framework's
            // outer `release_navigator` call is the only signal
            // those scopes have. We don't have direct access to
            // `release_screen` here — the navigator entry holds
            // the controller and stack but not the framework
            // callbacks. The dispatcher closure on `control` is
            // the canonical caller; here we just drop the entry
            // and let the dispatcher's closure (which holds the
            // release_screen Rc) get dropped along with `control`,
            // which transitively drops the still-mounted scopes
            // via the framework's `Scope` registry. The framework
            // keeps the scope registry; this entry's stack only
            // holds VCs, and dropping them drops their views.
            drop(entry);
        }

        fn make_navigator_handle(&self, node: &Self::Node) -> NavigatorHandle {
            let key = node.view_key();
            let Some(entry) = self.navigator_instances.get(&key) else {
                return NavigatorHandle::new(Rc::new(()), &IosNavigatorOps);
            };
            NavigatorHandle::with_control(Rc::new(()), &IosNavigatorOps, entry.control.clone())
        }

        fn finish(&mut self, _root: Self::Node) {}
    }

    struct IosNavigatorOps;
    impl NavigatorOps for IosNavigatorOps {}
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
        fn apply_style(&mut self, _node: &Self::Node, _style: &Rc<StyleRules>) {
            unreachable!()
        }
        fn finish(&mut self, _root: Self::Node) {
            unreachable!()
        }
    }
}

pub use imp::IosBackend;
