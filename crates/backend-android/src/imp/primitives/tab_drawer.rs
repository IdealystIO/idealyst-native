//! TabNavigator + DrawerNavigator on Android.
//!
//! These navigator kinds don't use `RustNavigator` / `FragmentManager`
//! — they're simpler. Each one builds a small native subtree
//! described below; the *body* region holds exactly one child View
//! (the currently-active screen) which swaps on Select.
//!
//! # Per-kind shapes
//!
//! - **TabNavigator**: navigator node is just a body `FrameLayout`.
//!   Tab chrome (a tab bar) is the author's responsibility via
//!   `.layout(...)`. Native Android *doesn't* currently render the
//!   author's layout slot for tab navigators — see the TODO at the
//!   end of this file. For now, calling `select(...)` on the handle
//!   works, but there's no native UI to drive it from.
//!
//! - **DrawerNavigator**: navigator node is a horizontal
//!   `LinearLayout` with two children:
//!     1. The author's `.sidebar(...)` subtree (built from
//!        `callbacks.build_sidebar`, or omitted if no sidebar was
//!        registered).
//!     2. A body `FrameLayout` where the active screen mounts.
//!
//!   This is the simplest pinned-sidebar layout — the drawer is
//!   always visible beside the body. Open/close commands flip the
//!   `is_open` signal but don't currently animate the sidebar in
//!   and out; that's a follow-up that needs `DrawerLayout` (or a
//!   hand-rolled translation animation).
//!
//! # Why no FragmentManager
//!
//! Nesting `RustNavigator` would put the drawer's screens on the
//! *same* activity-level back stack as the root stack navigator —
//! tapping a drawer item would push a fragment that the system Back
//! button would later pop, conflicting with stack-navigator
//! semantics. View-swap avoids the conflict entirely: drawer/tab
//! selections never touch the back stack.

use crate::imp::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use framework_core::primitives::navigator::{
    DrawerHandle, DrawerNavigatorCallbacks, NavCommand, NavigatorCallbacks, NavigatorControl,
    NavigatorHandle, NavigatorOps, TabNavigatorCallbacks, TabsHandle,
};
use jni::objects::{GlobalRef, JValue};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Per-instance state for a tab or drawer navigator. The `body` is
/// a `FrameLayout` that holds exactly one child (the active
/// screen's native node). `outer` is the framework-visible
/// navigator node — for tabs this is the same as `body`; for
/// drawer this is a `LinearLayout` containing the sidebar + body.
///
/// `current_scope` is the scope id the framework returned for the
/// active screen — we release it on swap so the old screen's
/// signals/effects free deterministically.
pub(crate) struct TabDrawerInstance {
    /// The framework-visible navigator container — the outer
    /// `LinearLayout` for drawer, or the body `FrameLayout` itself
    /// for tabs.
    #[allow(dead_code)]
    outer: GlobalRef,
    /// The FrameLayout that holds the active screen. Same as
    /// `outer` for tabs; a separate child for drawer.
    body: GlobalRef,
    /// Currently-mounted screen's view + scope id. `None` only
    /// between creation and the first `attach_initial` call.
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

/// Create a tab navigator. Returns a `FrameLayout` GlobalRef (the
/// body, since tabs don't have a sidebar at this layer).
///
/// **Important**: like the stack navigator's `create`, this MUST NOT
/// call `mount_screen` synchronously — the framework holds
/// `backend.borrow_mut()` for the entire `create_*` call and
/// `mount_screen` re-enters the build walker which also borrow_muts.
/// The framework calls `attach_initial` after `create_*` returns
/// with the freshly-built initial screen.
pub(crate) fn create_tab(
    b: &mut AndroidBackend,
    callbacks: TabNavigatorCallbacks<GlobalRef>,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    let TabNavigatorCallbacks { navigator, .. } = callbacks;
    // No sidebar for tabs. The framework-visible outer node and the
    // body are the same FrameLayout.
    let body = make_frame_layout(b);
    install_instance(
        b,
        navigator,
        control,
        body.clone(),
        body,
        /* is_drawer */ false,
        None,
    )
}

/// Create a drawer navigator. The native shape is a horizontal
/// `LinearLayout` containing the sidebar (if one was registered)
/// and a body `FrameLayout`. The outer LinearLayout is what the
/// framework sees as the navigator's node.
pub(crate) fn create_drawer(
    b: &mut AndroidBackend,
    callbacks: DrawerNavigatorCallbacks<GlobalRef>,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    let DrawerNavigatorCallbacks {
        navigator,
        is_open,
        open_changed,
        build_sidebar,
        ..
    } = callbacks;

    // 1. Build the body FrameLayout (where the active screen mounts).
    let body = make_frame_layout(b);

    // 2. Build the outer LinearLayout (horizontal).
    let outer = with_env(|env| {
        let class = env
            .find_class("android/widget/LinearLayout")
            .expect("LinearLayout class not found");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .expect("new LinearLayout failed");
        // 0 = HORIZONTAL.
        let _ = env.call_method(&local, "setOrientation", "(I)V", &[JValue::Int(0)]);
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).expect("global_ref LinearLayout")
    });

    // 3. Build the sidebar (if registered) and addView to outer.
    //    The build_sidebar callback re-enters the build walker, so
    //    it must run outside any active backend borrow. We're
    //    currently inside `create_drawer_navigator` which has
    //    backend.borrow_mut() held by the framework — so this is
    //    NOT safe to call here.
    //
    //    BUT: we're called directly from `create_drawer_navigator`
    //    which itself runs from within the borrow window. The
    //    walker holds borrow_mut() across the whole `create_*`
    //    call. So `build_sidebar()` (which re-enters
    //    backend.borrow_mut()) would panic with a double-borrow.
    //
    //    Workaround: defer the sidebar build + insert to a
    //    microtask, same pattern the web backend uses for layout.
    //    The drawer's outer LinearLayout is returned with just the
    //    body attached; the sidebar gets prepended after we yield.
    if let Some(build_sidebar) = build_sidebar {
        let outer_for_microtask = outer.clone();
        framework_core::schedule_microtask(move || {
            let sidebar_node = build_sidebar();
            with_env(|env| {
                // Add sidebar at index 0 so it's positioned before
                // the body. We can't use plain addView (appends at
                // end) — use addView(View, int) with index=0.
                let _ = env.call_method(
                    outer_for_microtask.as_obj(),
                    "addView",
                    "(Landroid/view/View;I)V",
                    &[
                        JValue::Object(&sidebar_node.as_obj()),
                        JValue::Int(0),
                    ],
                );
            });
        });
    }

    // 4. addView body to outer.
    with_env(|env| {
        let _ = env.call_method(
            outer.as_obj(),
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&body.as_obj())],
        );
        // Give the body weight=1 so it expands to fill the
        // remaining horizontal space (the sidebar gets its own
        // intrinsic size).
        let lp_class = env
            .find_class("android/widget/LinearLayout$LayoutParams")
            .expect("LinearLayout$LayoutParams class");
        // (width=0, height=MATCH_PARENT=-1, weight=1.0)
        let lp = env
            .new_object(
                &lp_class,
                "(IIF)V",
                &[JValue::Int(0), JValue::Int(-1), JValue::Float(1.0)],
            )
            .expect("new LinearLayout.LayoutParams");
        let _ = env.call_method(
            body.as_obj(),
            "setLayoutParams",
            "(Landroid/view/ViewGroup$LayoutParams;)V",
            &[JValue::Object(&lp)],
        );
    });

    install_instance(
        b,
        navigator,
        control,
        outer.clone(),
        body,
        /* is_drawer */ true,
        Some((is_open, open_changed)),
    )
}

fn make_frame_layout(b: &AndroidBackend) -> GlobalRef {
    with_env(|env| {
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
    })
}

/// Install the per-instance state on the backend, wire up the
/// dispatcher, and return the framework-visible outer node.
fn install_instance(
    b: &mut AndroidBackend,
    callbacks: NavigatorCallbacks<GlobalRef>,
    control: Rc<NavigatorControl>,
    outer: GlobalRef,
    body: GlobalRef,
    is_drawer: bool,
    drawer_state: Option<(framework_core::Signal<bool>, Rc<dyn Fn(bool)>)>,
) -> GlobalRef {
    let instance = Rc::new(RefCell::new(TabDrawerInstance {
        outer: outer.clone(),
        body,
        current: None,
        release_screen: callbacks.release_screen.clone(),
        mount_screen: callbacks.mount_screen.clone(),
    }));

    let dispatcher_instance = instance.clone();
    control.install(Box::new(move |cmd| {
        match cmd {
            NavCommand::Select { name, params, url: _ } => {
                swap_body(&dispatcher_instance, name, params);
            }
            NavCommand::Reset { name, params, url: _ } => {
                swap_body(&dispatcher_instance, name, params);
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

    let key = AndroidBackend::node_key_of(&outer);
    b.tab_drawer_instances
        .insert(key, TabDrawerEntry { instance, control });

    outer
}

/// Mount a new screen, addView to the body, release the previous
/// scope. Used by both Select and Reset (same dispatch shape on
/// Android — no back-stack distinction here).
fn swap_body(
    instance: &Rc<RefCell<TabDrawerInstance>>,
    name: &'static str,
    params: Box<dyn Any>,
) {
    let (new_view, new_scope) = {
        let inst = instance.borrow();
        // mount_screen re-enters the build walker which calls
        // backend.borrow_mut(). Safe here because dispatcher
        // callbacks fire outside any active borrow window (Kotlin
        // event handler → JNI → Rust).
        (inst.mount_screen)(name, params)
    };
    let (body, old_scope) = {
        let mut inst = instance.borrow_mut();
        let old = inst.current.take().map(|(_, s)| s);
        (inst.body.clone(), old)
    };
    with_env(|env| {
        let _ = env.call_method(body.as_obj(), "removeAllViews", "()V", &[]);
        let _ = env.call_method(
            body.as_obj(),
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&new_view.as_obj())],
        );
    });
    if let Some(scope) = old_scope {
        // Release the previous scope AFTER the new view is in
        // place. Releasing first would let the old view's
        // reactive effects fire one more time during the swap
        // and crash on freed state.
        let release = instance.borrow().release_screen.clone();
        release(scope);
    }
    instance.borrow_mut().current = Some((new_view, new_scope));
}

/// Attach the framework-built initial screen to the body. Called
/// by the framework after `create_*` returns, outside any active
/// backend borrow.
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
    let body = entry.instance.borrow().body.clone();
    with_env(|env| {
        let _ = env.call_method(
            body.as_obj(),
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
    if let Some((_view, scope)) = entry.instance.borrow_mut().current.take() {
        let release = entry.instance.borrow().release_screen.clone();
        release(scope);
    }
    drop(entry);
}

pub(crate) fn make_tab_handle(b: &AndroidBackend, node: &GlobalRef) -> TabsHandle {
    TabsHandle::from_inner(make_inner_handle(b, node))
}

pub(crate) fn make_drawer_handle(b: &AndroidBackend, node: &GlobalRef) -> DrawerHandle {
    DrawerHandle::from_inner(
        make_inner_handle(b, node),
        Rc::new(std::cell::Cell::new(false)),
    )
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
