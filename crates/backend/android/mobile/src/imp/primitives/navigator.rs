//! `Primitive::Navigator` — `io.idealyst.runtime.RustNavigator` plus
//! `RustHostFragment` for per-screen hosting.
//!
//! Each `create_stack_navigator` call leaks a `NavigatorCallbacks` box and
//! hands the pointer to a `RustNavigator` Kotlin instance. The
//! navigator wraps a `FrameLayout` (our visible container) and the
//! Activity's `FragmentManager`; push / pop / replace / reset map
//! directly to fragment transactions, with `RustHostFragment.onDestroyView`
//! trampolining back through JNI to release the matching scope.

use crate::imp::{with_env, AndroidBackend};
use runtime_core::primitives::navigator::{
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
/// dropped together when `release_stack_navigator` fires.
pub(crate) struct NavigatorEntry {
    pub(crate) controller: GlobalRef,
    pub(crate) control: Rc<NavigatorControl>,
    /// Pointer to the leaked `NavigatorCallbacks<GlobalRef>`. Freed
    /// in `release_stack_navigator` so late `nativeReleaseScreen` calls
    /// don't read freed memory — see `RustHostFragment.onDestroyView`,
    /// which is the only caller and which always fires *before* the
    /// fragment manager finishes the pop transaction (and thus before
    /// `release_stack_navigator` can run for the parent navigator).
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
    // We also clone the closures that the dispatcher needs.
    let depth_changed = callbacks.depth_changed.clone();
    let mount_screen = callbacks.mount_screen.clone();
    let release_screen = callbacks.release_screen.clone();
    let boxed = Box::new(callbacks);
    let ptr = Box::into_raw(boxed) as jlong;

    let (controller_ref, container_ref) = with_env(|env| {
        let nav_class = match env.find_class("io/idealyst/runtime/RustNavigator") {
            Ok(c) => c,
            Err(e) => {
                // Most common cause of a hard failure here: the
                // backend-android Kotlin runtime root was not added to
                // the consuming app's Gradle source sets. Surface this
                // explicitly so the user sees it in logcat.
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                }
                log::error!(
                    "RustNavigator class not found — make sure the consuming app's \
                     build.gradle.kts includes \
                     `crates/backend-android/runtime/kotlin` in its sourceSets, \
                     and that `androidx.fragment:fragment` is on the classpath \
                     (appcompat 1.7 pulls it transitively). Underlying error: {:?}",
                    e
                );
                panic!("RustNavigator class missing from APK");
            }
        };
        let controller = match env.new_object(
            &nav_class,
            "(Landroid/content/Context;J)V",
            &[
                JValue::Object(&b.context.as_obj()),
                JValue::Long(ptr),
            ],
        ) {
            Ok(c) => c,
            Err(e) => {
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                }
                log::error!("RustNavigator construction failed: {:?}", e);
                panic!("RustNavigator construction failed");
            }
        };
        // Retrieve the controller's container FrameLayout — we need
        // to insert it into the parent layout.
        let container = env
            .get_field(&controller, "container", "Landroid/widget/FrameLayout;")
            .and_then(|f| f.l())
            .unwrap_or_else(|e| {
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                }
                log::error!("RustNavigator.container field lookup failed: {:?}", e);
                panic!("RustNavigator.container field");
            });
        log::info!("RustNavigator created, container id resolved");
        (
            env.new_global_ref(&controller).unwrap(),
            env.new_global_ref(&container).unwrap(),
        )
    });

    // NOTE: we DO NOT call `mount_screen` here. The framework holds
    // `backend.borrow_mut()` for the entire create_stack_navigator call,
    // and `mount_screen` re-enters the build walker which also
    // borrow_muts the backend — double borrow → panic.
    // `Backend::stack_navigator_attach_initial` is the hook the framework
    // calls *after* create_stack_navigator returns, with the already-built
    // initial screen node.

    // Wire the dispatcher onto the control plane. Every command path
    // calls `mount_screen` / the Kotlin controller / `depth_changed`
    // appropriately. Cloning Rcs everywhere is intentional — each
    // call captures only what it needs.
    {
        let controller = controller_ref.clone();
        let mount_for_dispatch = mount_screen.clone();
        let depth_for_dispatch = depth_changed.clone();
        // Kept for parity; the Kotlin path handles release via
        // RustHostFragment.onDestroyView → nativeReleaseScreen, but
        // we still want the Rc kept alive on this side.
        let _release_for_dispatch = release_screen.clone();
        control.install(Box::new(move |cmd| match cmd {
            NavCommand::Push { name, params, url: _, state: _ } => {
                let result = mount_for_dispatch(name, params);
                let view = result.node;
                let scope_id = result.scope_id;
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
            NavCommand::Replace { name, params, url: _, state: _ } => {
                let result = mount_for_dispatch(name, params);
                let view = result.node;
                let scope_id = result.scope_id;
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
            NavCommand::Reset { name, params, url: _, state: _ } => {
                let result = mount_for_dispatch(name, params);
                let view = result.node;
                let scope_id = result.scope_id;
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
            // Stack navigator doesn't accept select-shaped or
            // drawer-shaped commands. Panic to surface the mismatch
            // at the call site instead of silently dropping the
            // command.
            NavCommand::Select { .. }
            | NavCommand::OpenDrawer
            | NavCommand::CloseDrawer
            | NavCommand::ToggleDrawer => {
                panic!(
                    "stack Navigator received a non-stack NavCommand — \
                     check that the dispatched command's shape matches \
                     the navigator kind (stack: Push/Pop/Replace/Reset)"
                );
            }
        }));
    }

    // Stash the instance keyed by the *container's* JObject pointer
    // — that's what we'll get back in `release_stack_navigator` /
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

/// Attach the framework-built initial screen to a freshly-created
/// navigator. Called by the framework after `create_stack_navigator`
/// returns, outside any active backend borrow — so this is the
/// first point we can safely do the Kotlin-side `mountRoot` call.
pub(crate) fn attach_initial(
    _b: &mut AndroidBackend,
    navigator: &GlobalRef,
    screen: GlobalRef,
    scope_id: u64,
) {
    let Some(entry) = _b.navigator_instances.get(&AndroidBackend::node_key_of(navigator)) else {
        log::error!("attach_initial: no navigator entry for node");
        return;
    };
    let controller = entry.controller.clone();
    log::info!("Navigator attach_initial: calling Kotlin mountRoot, scope_id={}", scope_id);
    with_env(|env| {
        if let Err(e) = env.call_method(
            controller.as_obj(),
            "mountRoot",
            "(Landroid/view/View;J)V",
            &[
                JValue::Object(&screen.as_obj()),
                JValue::Long(scope_id as jlong),
            ],
        ) {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            log::error!("RustNavigator.mountRoot JNI call failed: {:?}", e);
        }
    });
    log::info!("Navigator attach_initial: mountRoot JNI call returned");
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
    // `release_stack_navigator` ran) check `ptr != 0` and otherwise no-op,
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
/// been freed (because `release_stack_navigator` ran first), `ptr` is
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
