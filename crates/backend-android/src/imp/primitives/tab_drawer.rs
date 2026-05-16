//! TabNavigator + DrawerNavigator on Android.
//!
//! These navigator kinds don't use `RustNavigator` / `FragmentManager`
//! — they're simpler. The native container is a plain
//! `android.widget.FrameLayout` that holds exactly one child View (the
//! currently-active screen). Selection swaps the child:
//! `removeAllViews()` followed by `addView(new_screen)`, with the
//! previous screen's scope released.
//!
//! The author's `.layout(...)` closure on the TabNavigator /
//! DrawerNavigator produces the surrounding chrome (tab bar, drawer
//! sidebar). Tab/drawer-specific visual *widgets* (BottomNavigationView,
//! DrawerLayout) are not used here — the layout slot is the chrome.
//! This keeps the Android implementation small and avoids fighting the
//! root `RustNavigator`'s FragmentManager for back-stack ownership.
//!
//! Both kinds share this impl: the container shape is the same; only
//! the dispatched commands differ. For drawer, the open/close commands
//! flip the framework-side `is_open: Signal<bool>` (the author's layout
//! subscribes to it to drive sidebar visibility) and have no native
//! widget effect at this layer.

use crate::imp::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use framework_core::primitives::navigator::{
    DrawerHandle, DrawerNavigatorCallbacks, NavCommand, NavigatorCallbacks, NavigatorControl,
    NavigatorHandle, NavigatorOps, TabNavigatorCallbacks, TabsHandle,
};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Per-instance state for a tab or drawer navigator. The container
/// is a `FrameLayout` that holds exactly one child (the active
/// screen's native node). `current_scope` is the scope id the
/// framework returned for that child — we release it on swap so the
/// old screen's signals/effects free deterministically.
pub(crate) struct TabDrawerInstance {
    /// FrameLayout. `removeAllViews()` + `addView(new_child)` is how
    /// we swap screens.
    container: GlobalRef,
    /// Currently-mounted screen's view + scope id. `None` only
    /// between creation and the first `attach_initial` call (which
    /// the framework fires synchronously after `create_*`).
    current: Option<(GlobalRef, u64)>,
    /// Used to release the previous scope when we swap screens.
    release_screen: Rc<dyn Fn(u64)>,
    /// Used to mount the next screen on `Select`.
    mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> (GlobalRef, u64)>,
}

pub(crate) type TabDrawerInstances = HashMap<usize, TabDrawerEntry>;

pub(crate) struct TabDrawerEntry {
    pub(crate) instance: Rc<RefCell<TabDrawerInstance>>,
    pub(crate) control: Rc<NavigatorControl>,
}

// ---------------------------------------------------------------------------
// Tab navigator
// ---------------------------------------------------------------------------

/// Create a tab navigator. Returns a `FrameLayout` GlobalRef.
///
/// **Important**: like the stack navigator's `create`, this MUST NOT
/// call `mount_screen` synchronously — the framework holds
/// `backend.borrow_mut()` for the entire `create_*` call and
/// `mount_screen` re-enters the build walker which also borrow_muts.
/// The framework calls `attach_initial` after `create_*` returns with
/// the freshly-built initial screen.
pub(crate) fn create_tab(
    b: &mut AndroidBackend,
    callbacks: TabNavigatorCallbacks<GlobalRef>,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    let TabNavigatorCallbacks { navigator, .. } = callbacks;
    create_inner(b, navigator, control, /* is_drawer */ false, None)
}

/// Create a drawer navigator. Returns a `FrameLayout` GlobalRef.
/// `is_open` is the framework's reactive signal — drawer
/// open/close/toggle commands flip it.
pub(crate) fn create_drawer(
    b: &mut AndroidBackend,
    callbacks: DrawerNavigatorCallbacks<GlobalRef>,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    let DrawerNavigatorCallbacks {
        navigator,
        is_open,
        open_changed,
        ..
    } = callbacks;
    create_inner(b, navigator, control, /* is_drawer */ true, Some((is_open, open_changed)))
}

/// Shared implementation. Builds the container, installs the
/// per-instance dispatcher, stashes the instance.
fn create_inner(
    b: &mut AndroidBackend,
    callbacks: NavigatorCallbacks<GlobalRef>,
    control: Rc<NavigatorControl>,
    is_drawer: bool,
    drawer_state: Option<(framework_core::Signal<bool>, Rc<dyn Fn(bool)>)>,
) -> GlobalRef {
    // 1. Build the FrameLayout container.
    let container_ref = with_env(|env| {
        let class = env
            .find_class("android/widget/FrameLayout")
            .expect("FrameLayout class not found");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .expect("new FrameLayout failed");
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).expect("global_ref FrameLayout")
    });

    // 2. Build the per-instance state. Initial `current` is None; the
    //    framework's call to `attach_initial` populates it.
    let instance = Rc::new(RefCell::new(TabDrawerInstance {
        container: container_ref.clone(),
        current: None,
        release_screen: callbacks.release_screen.clone(),
        mount_screen: callbacks.mount_screen.clone(),
    }));

    // 3. Install the dispatcher. Tab + drawer share the Select logic;
    //    drawer also handles Open/Close/Toggle.
    let dispatcher_instance = instance.clone();
    control.install(Box::new(move |cmd| {
        match cmd {
            NavCommand::Select { name, params, url: _ } => {
                let (new_view, new_scope) = {
                    let inst = dispatcher_instance.borrow();
                    // Mount the new screen. mount_screen builds the
                    // subtree in a fresh per-screen Scope and returns
                    // (view, scope_id). It re-enters the build walker
                    // which calls `backend.borrow_mut()`, so we must
                    // not be holding any borrow_mut on the backend
                    // while this runs — and we don't here (we're
                    // running from a Rust callback fired by a Kotlin
                    // tap or by the framework's dispatch path, both
                    // of which are outside any active borrow window).
                    (inst.mount_screen)(name, params)
                };
                // Swap: remove the old child, release its scope, add
                // the new child, remember it.
                let (container, old_scope) = {
                    let mut inst = dispatcher_instance.borrow_mut();
                    let old = inst.current.take().map(|(_, s)| s);
                    (inst.container.clone(), old)
                };
                with_env(|env| {
                    let _ = env.call_method(
                        container.as_obj(),
                        "removeAllViews",
                        "()V",
                        &[],
                    );
                    let _ = env.call_method(
                        container.as_obj(),
                        "addView",
                        "(Landroid/view/View;)V",
                        &[JValue::Object(&new_view.as_obj())],
                    );
                });
                if let Some(scope) = old_scope {
                    // Release the previous scope after the new view
                    // is in place — if we released first, the old
                    // view's reactive effects could fire one more
                    // time during the swap and crash on freed state.
                    let release = dispatcher_instance.borrow().release_screen.clone();
                    release(scope);
                }
                dispatcher_instance.borrow_mut().current = Some((new_view, new_scope));
            }
            NavCommand::Reset { name, params, url: _ } => {
                // For tab/drawer, Reset is the "go home" hatch — same
                // shape as Select.
                let (new_view, new_scope) = {
                    let inst = dispatcher_instance.borrow();
                    (inst.mount_screen)(name, params)
                };
                let (container, old_scope) = {
                    let mut inst = dispatcher_instance.borrow_mut();
                    let old = inst.current.take().map(|(_, s)| s);
                    (inst.container.clone(), old)
                };
                with_env(|env| {
                    let _ = env.call_method(container.as_obj(), "removeAllViews", "()V", &[]);
                    let _ = env.call_method(
                        container.as_obj(),
                        "addView",
                        "(Landroid/view/View;)V",
                        &[JValue::Object(&new_view.as_obj())],
                    );
                });
                if let Some(scope) = old_scope {
                    let release = dispatcher_instance.borrow().release_screen.clone();
                    release(scope);
                }
                dispatcher_instance.borrow_mut().current = Some((new_view, new_scope));
            }
            NavCommand::OpenDrawer => {
                if let Some((sig, cb)) = drawer_state.as_ref() {
                    sig.set(true);
                    cb(true);
                } else {
                    panic!("TabNavigator received OpenDrawer — drawer commands are drawer-only");
                }
            }
            NavCommand::CloseDrawer => {
                if let Some((sig, cb)) = drawer_state.as_ref() {
                    sig.set(false);
                    cb(false);
                } else {
                    panic!("TabNavigator received CloseDrawer — drawer commands are drawer-only");
                }
            }
            NavCommand::ToggleDrawer => {
                if let Some((sig, cb)) = drawer_state.as_ref() {
                    let now = !sig.get();
                    sig.set(now);
                    cb(now);
                } else {
                    panic!("TabNavigator received ToggleDrawer — drawer commands are drawer-only");
                }
            }
            NavCommand::Push { .. } | NavCommand::Pop | NavCommand::Replace { .. } => {
                let kind = if is_drawer { "DrawerNavigator" } else { "TabNavigator" };
                panic!(
                    "{} received an unsupported NavCommand — \
                     tabs/drawer accept Select (+ Reset for go-home, \
                     and drawer accepts Open/Close/ToggleDrawer). \
                     Push/Pop/Replace belong on a stack navigator.",
                    kind
                );
            }
        }
    }));

    // 4. Stash the instance keyed by the container's JObject pointer.
    let key = AndroidBackend::node_key_of(&container_ref);
    b.tab_drawer_instances
        .insert(key, TabDrawerEntry { instance, control });

    container_ref
}

/// Attach the framework-built initial screen. Called by the
/// framework after `create_tab_navigator` / `create_drawer_navigator`
/// returns, outside any active backend borrow.
pub(crate) fn attach_initial(
    b: &mut AndroidBackend,
    navigator: &GlobalRef,
    screen: GlobalRef,
    scope_id: u64,
) {
    let Some(entry) = b.tab_drawer_instances.get(&AndroidBackend::node_key_of(navigator)) else {
        log::error!("tab_drawer attach_initial: no instance for node");
        return;
    };
    let container = entry.instance.borrow().container.clone();
    with_env(|env| {
        let _ = env.call_method(
            container.as_obj(),
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&screen.as_obj())],
        );
    });
    entry.instance.borrow_mut().current = Some((screen, scope_id));
}

pub(crate) fn release(b: &mut AndroidBackend, node: &GlobalRef) {
    let key = AndroidBackend::node_key_of(node);
    let Some(entry) = b.tab_drawer_instances.remove(&key) else {
        return;
    };
    // Release the active scope. The container itself will be GC'd
    // when its parent removes it (the framework's `clear_children`
    // upstream).
    if let Some((_view, scope)) = entry.instance.borrow_mut().current.take() {
        let release = entry.instance.borrow().release_screen.clone();
        release(scope);
    }
    drop(entry);
}

pub(crate) fn make_tab_handle(b: &AndroidBackend, node: &GlobalRef) -> TabsHandle {
    let inner = make_inner_handle(b, node);
    TabsHandle::from_inner(inner)
}

pub(crate) fn make_drawer_handle(b: &AndroidBackend, node: &GlobalRef) -> DrawerHandle {
    let inner = make_inner_handle(b, node);
    DrawerHandle::from_inner(inner, Rc::new(std::cell::Cell::new(false)))
}

fn make_inner_handle(b: &AndroidBackend, node: &GlobalRef) -> NavigatorHandle {
    let key = AndroidBackend::node_key_of(node);
    let Some(entry) = b.tab_drawer_instances.get(&key) else {
        return NavigatorHandle::new(Rc::new(()), &TabDrawerOps);
    };
    NavigatorHandle::with_control(Rc::new(()), &TabDrawerOps, entry.control.clone())
}

struct TabDrawerOps;
impl NavigatorOps for TabDrawerOps {}
