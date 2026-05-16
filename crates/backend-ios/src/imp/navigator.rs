use framework_core::primitives::navigator::{
    NavCommand, NavigatorCallbacks, NavigatorControl, NavigatorHandle, NavigatorOps,
};
use objc2::rc::Retained;
use objc2_foundation::MainThreadMarker;
use objc2_ui_kit::{UINavigationController, UIView, UIViewController};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::{mount_screen_in_vc, IosNode};

pub(crate) struct NavigatorEntry {
    pub(crate) controller: Retained<UINavigationController>,
    pub(crate) control: Rc<NavigatorControl>,
    pub(crate) stack: Rc<RefCell<Vec<ScreenEntry>>>,
}

pub(crate) struct ScreenEntry {
    pub(crate) vc: Retained<UIViewController>,
    pub(crate) scope_id: u64,
}

pub(crate) struct IosNavigatorOps;
impl NavigatorOps for IosNavigatorOps {}

pub(crate) fn create_navigator(
    mtm: MainThreadMarker,
    navigator_instances: &mut HashMap<usize, NavigatorEntry>,
    callbacks: NavigatorCallbacks<IosNode>,
    control: Rc<NavigatorControl>,
) -> IosNode {
    let nav = unsafe { UINavigationController::new(mtm) };
    let nav_view = nav.view().expect("UINavigationController.view");

    let stack_rc: Rc<RefCell<Vec<ScreenEntry>>> = Rc::new(RefCell::new(Vec::new()));
    let entry = NavigatorEntry {
        controller: nav.clone(),
        control: control.clone(),
        stack: stack_rc.clone(),
    };
    let key = &*nav_view as *const UIView as usize;
    navigator_instances.insert(key, entry);

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
                let vc = mount_screen_in_vc(mtm, node.as_view());
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
                let vc = mount_screen_in_vc(mtm, node.as_view());
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
                let vc = mount_screen_in_vc(mtm, node.as_view());
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
            NavCommand::Select { .. }
            | NavCommand::OpenDrawer
            | NavCommand::CloseDrawer
            | NavCommand::ToggleDrawer => {
                panic!(
                    "stack Navigator received a non-stack NavCommand -- \
                     check that the dispatched command's shape matches \
                     the navigator kind (stack: Push/Pop/Replace/Reset)"
                );
            }
        }
    }));

    IosNode::View(nav_view)
}

pub(crate) fn navigator_attach_initial(
    mtm: MainThreadMarker,
    navigator_instances: &HashMap<usize, NavigatorEntry>,
    navigator: &IosNode,
    screen: IosNode,
    scope_id: u64,
) {
    let key = navigator.view_key();
    let Some(entry) = navigator_instances.get(&key) else {
        return;
    };
    let root_vc = mount_screen_in_vc(mtm, screen.as_view());
    unsafe {
        entry.controller.setViewControllers_animated(
            &objc2_foundation::NSArray::from_vec(vec![root_vc.clone()]),
            false,
        );
    }
    entry
        .stack
        .borrow_mut()
        .push(ScreenEntry { vc: root_vc, scope_id });
}

pub(crate) fn release_navigator(
    navigator_instances: &mut HashMap<usize, NavigatorEntry>,
    node: &IosNode,
) {
    let key = node.view_key();
    if let Some(entry) = navigator_instances.remove(&key) {
        drop(entry);
    }
}

pub(crate) fn make_navigator_handle(
    navigator_instances: &HashMap<usize, NavigatorEntry>,
    node: &IosNode,
) -> NavigatorHandle {
    let key = node.view_key();
    let Some(entry) = navigator_instances.get(&key) else {
        return NavigatorHandle::new(Rc::new(()), &IosNavigatorOps);
    };
    NavigatorHandle::with_control(Rc::new(()), &IosNavigatorOps, entry.control.clone())
}
