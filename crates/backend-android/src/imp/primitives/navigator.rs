//! `Primitive::Navigator` — `io.idealyst.runtime.RustNavigator` plus
//! `RustHostFragment` for per-screen hosting.
//!
//! Each `create_navigator` call leaks a `NavigatorCallbacks` box and
//! hands the pointer to a `RustNavigator` Kotlin instance. The
//! navigator wraps a `FrameLayout` (our visible container) and the
//! Activity's `FragmentManager`; push / pop / replace / reset map
//! directly to fragment transactions, with `RustHostFragment.onDestroyView`
//! trampolining back through JNI to release the matching scope.

use crate::imp::{with_env, AndroidBackend};
use framework_core::primitives::navigator::{
    NavCommand, NavigatorCallbacks, NavigatorControl, NavigatorHandle, NavigatorOps,
};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Per-navigator state held on the AndroidBackend. The `controller`
/// is a GlobalRef to the Kotlin `RustNavigator` instance; the
/// `control` is the framework-side control plane (also referenced by
/// every `NavigatorHandle` clone the user holds). Both halves are
/// dropped together when `release_navigator` fires.
pub(crate) struct NavigatorEntry {
    pub(crate) controller: GlobalRef,
    pub(crate) control: Rc<NavigatorControl>,
    /// Pointer to the leaked `NavigatorCallbacks<GlobalRef>`. Freed
    /// in `release_navigator` so late `nativeReleaseScreen` calls
    /// don't read freed memory — see `RustHostFragment.onDestroyView`,
    /// which is the only caller and which always fires *before* the
    /// fragment manager finishes the pop transaction (and thus before
    /// `release_navigator` can run for the parent navigator).
    pub(crate) callbacks_ptr: jlong,
    /// Cached depth probe so we can update the control plane in
    /// `notify_pushed` / `notify_popped` without a JNI round trip.
    #[allow(dead_code)]
    pub(crate) depth: RefCell<usize>,
}

pub(crate) type NavigatorInstances = HashMap<usize, NavigatorEntry>;

pub(crate) fn create(
    b: &mut AndroidBackend,
    callbacks: NavigatorCallbacks<GlobalRef>,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    // Leak the callbacks box; the pointer is what Kotlin passes back
    // through `nativeReleaseScreen` on every fragment destruction.
    let initial_route = callbacks.initial_route;
    let depth_changed = callbacks.depth_changed.clone();
    let mount_screen = callbacks.mount_screen.clone();
    let release_screen = callbacks.release_screen.clone();
    let boxed = Box::new(callbacks);
    let ptr = Box::into_raw(boxed) as jlong;

    let (controller_ref, container_ref) = with_env(|env| {
        let nav_class = env
            .find_class("io/idealyst/runtime/RustNavigator")
            .expect("RustNavigator class — backend-android Kotlin runtime missing from APK");
        let controller = env
            .new_object(
                &nav_class,
                "(Landroid/content/Context;J)V",
                &[
                    JValue::Object(&b.context.as_obj()),
                    JValue::Long(ptr),
                ],
            )
            .expect("RustNavigator construction failed");
        // Retrieve the controller's container FrameLayout — we need
        // to insert it into the parent layout.
        let container = env
            .get_field(&controller, "container", "Landroid/widget/FrameLayout;")
            .and_then(|f| f.l())
            .expect("RustNavigator.container field");
        (
            env.new_global_ref(&controller).unwrap(),
            env.new_global_ref(&container).unwrap(),
        )
    });

    // Mount the initial screen. Param-less from the typed API surface;
    // box `()` to satisfy the type-erased boundary.
    let (initial_view, initial_scope_id) = mount_screen(initial_route, Box::new(()));
    with_env(|env| {
        env.call_method(
            controller_ref.as_obj(),
            "mountRoot",
            "(Landroid/view/View;J)V",
            &[
                JValue::Object(&initial_view.as_obj()),
                JValue::Long(initial_scope_id as jlong),
            ],
        )
        .expect("RustNavigator.mountRoot failed");
    });

    // Wire the dispatcher onto the control plane. Every command path
    // calls `mount_screen` / the Kotlin controller / `depth_changed`
    // appropriately. Cloning Rcs everywhere is intentional — each
    // call captures only what it needs.
    {
        let controller = controller_ref.clone();
        let mount_for_dispatch = mount_screen.clone();
        let depth_for_dispatch = depth_changed.clone();
        let _release_for_dispatch = release_screen.clone(); // kept for parity; Kotlin path handles release via onDestroyView
        control.install(Box::new(move |cmd| match cmd {
            NavCommand::Push { name, params, url: _ } => {
                let (view, scope_id) = mount_for_dispatch(name, params);
                let new_depth = with_env(|env| {
                    let _ = env.call_method(
                        controller.as_obj(),
                        "push",
                        "(Landroid/view/View;J)V",
                        &[
                            JValue::Object(&view.as_obj()),
                            JValue::Long(scope_id as jlong),
                        ],
                    );
                    env.call_method(controller.as_obj(), "depth", "()I", &[])
                        .and_then(|v| v.i())
                        .unwrap_or(0)
                });
                depth_for_dispatch(new_depth as usize);
            }
            NavCommand::Pop => {
                let new_depth = with_env(|env| {
                    let _ = env.call_method(controller.as_obj(), "pop", "()V", &[]);
                    env.call_method(controller.as_obj(), "depth", "()I", &[])
                        .and_then(|v| v.i())
                        .unwrap_or(0)
                });
                depth_for_dispatch(new_depth as usize);
            }
            NavCommand::Replace { name, params, url: _ } => {
                let (view, scope_id) = mount_for_dispatch(name, params);
                let new_depth = with_env(|env| {
                    let _ = env.call_method(
                        controller.as_obj(),
                        "replace",
                        "(Landroid/view/View;J)V",
                        &[
                            JValue::Object(&view.as_obj()),
                            JValue::Long(scope_id as jlong),
                        ],
                    );
                    env.call_method(controller.as_obj(), "depth", "()I", &[])
                        .and_then(|v| v.i())
                        .unwrap_or(0)
                });
                depth_for_dispatch(new_depth as usize);
            }
            NavCommand::Reset { name, params, url: _ } => {
                let (view, scope_id) = mount_for_dispatch(name, params);
                let new_depth = with_env(|env| {
                    let _ = env.call_method(
                        controller.as_obj(),
                        "reset",
                        "(Landroid/view/View;J)V",
                        &[
                            JValue::Object(&view.as_obj()),
                            JValue::Long(scope_id as jlong),
                        ],
                    );
                    env.call_method(controller.as_obj(), "depth", "()I", &[])
                        .and_then(|v| v.i())
                        .unwrap_or(0)
                });
                depth_for_dispatch(new_depth as usize);
            }
        }));
    }

    // Stash the instance keyed by the *container's* JObject pointer
    // — that's what we'll get back in `release_navigator` /
    // `make_handle` since the container is what we return as the
    // navigator's node.
    let key = AndroidBackend::node_key_of(&container_ref);
    b.navigator_instances.insert(
        key,
        NavigatorEntry {
            controller: controller_ref,
            control,
            callbacks_ptr: ptr,
            depth: RefCell::new(1),
        },
    );

    container_ref
}

pub(crate) fn release(b: &mut AndroidBackend, node: &GlobalRef) {
    let key = AndroidBackend::node_key_of(node);
    let Some(entry) = b.navigator_instances.remove(&key) else {
        return;
    };
    // The Kotlin controller's FragmentManager will tear down active
    // fragments when the Activity destroys; we still want to release
    // any still-mounted scopes proactively in case the navigator
    // outlives the Activity (e.g. a `when` flips past it). The
    // controller exposes no enumeration of scope ids, but every
    // fragment's `onDestroyView` already fires `nativeReleaseScreen`
    // on a normal pop. For an unmount-while-active path, we depend
    // on FragmentManager firing onDestroyView for each mounted
    // fragment as its host activity tears down — Android does this
    // automatically.
    //
    // Free the leaked callbacks box. Late nativeReleaseScreen calls
    // (an in-flight Kotlin handler dispatched before
    // `release_navigator` ran) check `ptr != 0` and otherwise no-op,
    // so the box being freed here is safe.
    let ptr = entry.callbacks_ptr;
    if ptr != 0 {
        unsafe {
            drop(Box::from_raw(ptr as *mut NavigatorCallbacks<GlobalRef>));
        }
    }
    // Drop the controller GlobalRef so the JVM can GC the
    // RustNavigator (along with its FrameLayout, which by this point
    // has been removed from its parent by the framework's
    // `clear_children` upstream).
    drop(entry.controller);
    drop(entry.control);
}

pub(crate) fn make_handle(b: &AndroidBackend, node: &GlobalRef) -> NavigatorHandle {
    let key = AndroidBackend::node_key_of(node);
    let Some(entry) = b.navigator_instances.get(&key) else {
        return NavigatorHandle::new(Rc::new(()), &AndroidNavigatorOps);
    };
    NavigatorHandle::with_control(Rc::new(()), &AndroidNavigatorOps, entry.control.clone())
}

struct AndroidNavigatorOps;
impl NavigatorOps for AndroidNavigatorOps {}

// Re-export the type alias the JNI export below needs.
pub(crate) type AndroidNavCallbacks = NavigatorCallbacks<GlobalRef>;

/// JNI entry point: `RustHostFragment.onDestroyView` calls
/// `nativeReleaseScreen(nativePtr, scopeId)` to drop the per-screen
/// `Scope`. The pointer is the leaked `NavigatorCallbacks` box;
/// `scope_id` is what `mount_screen` returned for the screen.
///
/// # Safety
///
/// `ptr` must have been produced by `Box::into_raw` on a
/// `Box<NavigatorCallbacks<GlobalRef>>` in `create`. If the box has
/// been freed (because `release_navigator` ran first), `ptr` is
/// still passed but we'd dereference invalid memory — so the box is
/// freed *after* the controller drop, and on the controller drop
/// FragmentManager has already fired the onDestroyView calls. Late
/// calls after the box is freed cannot happen in the Android
/// transaction model.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustHostFragment_nativeReleaseScreen(
    _env: jni::JNIEnv,
    _this: jni::objects::JObject,
    ptr: jlong,
    scope_id: jlong,
) {
    if ptr == 0 {
        return;
    }
    let cbs = &*(ptr as *const AndroidNavCallbacks);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (cbs.release_screen)(scope_id as u64);
    }));
}
