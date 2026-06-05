//! Stack navigator — `io.idealyst.runtime.RustNavigator` plus
//! `RustHostFragment` for per-screen hosting.
//!
//! Each `create_stack` call leaks an `AndroidNavCallbacks` box and
//! hands the pointer to a `RustNavigator` Kotlin instance. The
//! navigator wraps a `FrameLayout` (our visible container) and the
//! Activity's `FragmentManager`; push / pop / replace / reset map
//! directly to fragment transactions, with
//! `RustHostFragment.onDestroyView` trampolining back through JNI to
//! release the matching scope.

use crate::{node_key, AndroidNavCallbacks};
use backend_android::{with_jni_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use runtime_core::primitives::navigator::{NavCommand, NavigatorControl, NavigatorHandle, NavigatorOps};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

// =============================================================================
// Per-navigator state
// =============================================================================

/// Per-navigator state held in the thread-local registry. The
/// `controller` is a GlobalRef to the Kotlin `RustNavigator` instance;
/// the `control` is the framework-side control plane (also referenced
/// by every `NavigatorHandle` clone the user holds). Both halves are
/// dropped together when [`release`] fires.
pub(crate) struct NavigatorEntry {
    pub(crate) controller: GlobalRef,
    pub(crate) control: Rc<NavigatorControl>,
    /// Pointer to the leaked `AndroidNavCallbacks` box. Freed in
    /// [`release`] so late `nativeReleaseScreen` calls don't read freed
    /// memory — see `RustHostFragment.onDestroyView`, which is the only
    /// caller and which always fires *before* the fragment manager
    /// finishes the pop transaction (and thus before [`release`] can
    /// run for the parent navigator).
    pub(crate) callbacks_ptr: jlong,
}

thread_local! {
    /// Per-instance registry keyed by the container's JObject* pointer.
    /// Mirrors what used to live on `AndroidBackend.navigator_instances`;
    /// moved here so the SDK owns it.
    pub(crate) static NAVIGATOR_INSTANCES: RefCell<HashMap<usize, NavigatorEntry>> =
        RefCell::new(HashMap::new());
}

// =============================================================================
// Create / dispatch
// =============================================================================

pub(crate) fn create(
    backend: &mut AndroidBackend,
    callbacks: AndroidNavCallbacks,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    // Leak the callbacks box; the pointer is what Kotlin passes back
    // through `nativeReleaseScreen` on every fragment destruction. Also
    // clone the closures the dispatcher needs.
    let depth_changed = callbacks.depth_changed.clone();
    let mount_screen = callbacks.mount_screen.clone();
    let release_screen = callbacks.release_screen.clone();
    let boxed = Box::new(callbacks);
    let ptr = Box::into_raw(boxed) as jlong;

    let (controller_ref, container_ref) = backend.with_jni(|env, context| {
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
            &[JValue::Object(&context.as_obj()), JValue::Long(ptr)],
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
    // `backend.borrow_mut()` for the entire create call, and
    // `mount_screen` re-enters the build walker which also
    // borrow_muts — double borrow → panic. `attach_initial` is the
    // hook the framework calls *after* create returns, with the
    // already-built initial screen node.

    // Wire the dispatcher onto the control plane. Every command path
    // calls `mount_screen` / the Kotlin controller / `depth_changed`
    // appropriately. Cloning Rcs everywhere is intentional — each
    // call captures only what it needs.
    {
        let controller = controller_ref.clone();
        let mount_for_dispatch = mount_screen.clone();
        let depth_for_dispatch = depth_changed.clone();
        let _release_for_dispatch = release_screen.clone();
        control.install(Box::new(move |cmd| match cmd {
            NavCommand::Push { name, params, url: _, state: _ } => {
                let result = mount_for_dispatch(name, params);
                let view = result.node;
                let scope_id = result.scope_id;
                let new_depth = with_jni_env(|env| {
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
                // The pushed screen's content needs a Taffy layout pass, or its
                // sub-views render at default (0) size until some other trigger
                // forces a relayout — the "Settings screen not laid out" bug.
                // Coalesced (thread-local flag), so this is cheap. Mirrors the
                // drawer's `swap_body` ([[project_swap_body_layout_pass]]).
                backend_android::schedule_layout_pass();
            }
            NavCommand::Pop => {
                let new_depth = with_jni_env(|env| {
                    let _ = env.call_method(controller.as_obj(), "pop", "()V", &[]);
                    env.call_method(controller.as_obj(), "depth", "()I", &[])
                        .and_then(|v| v.i())
                        .unwrap_or(0)
                });
                depth_for_dispatch(new_depth as usize);
                // Relayout the revealed screen (parity with iOS Pop).
                backend_android::schedule_layout_pass();
            }
            NavCommand::Replace { name, params, url: _, state: _ } => {
                let result = mount_for_dispatch(name, params);
                let view = result.node;
                let scope_id = result.scope_id;
                let new_depth = with_jni_env(|env| {
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
                // New content → lay it out (see the Push arm).
                backend_android::schedule_layout_pass();
            }
            NavCommand::Reset { name, params, url: _, state: _ } => {
                let result = mount_for_dispatch(name, params);
                let view = result.node;
                let scope_id = result.scope_id;
                let new_depth = with_jni_env(|env| {
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
                // New content → lay it out (see the Push arm).
                backend_android::schedule_layout_pass();
            }
            // Stack navigator doesn't accept select-shaped or
            // drawer-shaped commands. Panic to surface the mismatch at
            // the call site instead of silently dropping the command.
            NavCommand::Select { .. } | NavCommand::Custom(_) => {
                panic!(
                    "stack Navigator received a non-stack NavCommand — \
                     check that the dispatched command's shape matches \
                     the navigator kind (stack: Push/Pop/Replace/Reset)"
                );
            }
        }));
    }

    // Stash the instance keyed by the container's JObject pointer —
    // that's what we'll get back in [`release`] / [`make_handle`] since
    // the container is what we return as the navigator's node.
    let key = node_key(&container_ref);
    NAVIGATOR_INSTANCES.with(|m| {
        m.borrow_mut().insert(
            key,
            NavigatorEntry {
                controller: controller_ref,
                control,
                callbacks_ptr: ptr,
            },
        );
    });

    container_ref
}

/// Attach the framework-built initial screen to a freshly-created
/// navigator. Called by the framework after `create_stack` returns,
/// outside any active backend borrow — so this is the first point we
/// can safely do the Kotlin-side `mountRoot` call.
///
/// Returns `true` if `navigator` refers to a stack navigator (entry
/// present in the registry), so the unified
/// [`crate::attach_initial`] can short-circuit. Returns `false` if the
/// node belongs to a different kind (tab/drawer) — the unified entry
/// point falls through to the tab_drawer module.
pub(crate) fn attach_initial(navigator: &GlobalRef, screen: &GlobalRef, scope_id: u64) -> bool {
    let key = node_key(navigator);
    let controller = NAVIGATOR_INSTANCES.with(|m| {
        m.borrow().get(&key).map(|e| e.controller.clone())
    });
    let Some(controller) = controller else { return false };
    log::info!("Navigator attach_initial: calling Kotlin mountRoot, scope_id={}", scope_id);
    with_jni_env(|env| {
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
    true
}

/// Returns `true` if `node` was a stack navigator (registry entry
/// removed); `false` otherwise so the unified [`crate::release`] can
/// fall through to tab_drawer.
pub(crate) fn release(node: &GlobalRef) -> bool {
    let key = node_key(node);
    let Some(entry) = NAVIGATOR_INSTANCES.with(|m| m.borrow_mut().remove(&key)) else {
        return false;
    };
    // The Kotlin controller's FragmentManager will tear down active
    // fragments when the Activity destroys; we still want to release
    // any still-mounted scopes proactively in case the navigator
    // outlives the Activity (e.g. a `when` flips past it). The
    // controller exposes no enumeration of scope ids, but every
    // fragment's `onDestroyView` already fires `nativeReleaseScreen` on
    // a normal pop. For an unmount-while-active path, we depend on
    // FragmentManager firing onDestroyView for each mounted fragment
    // as its host activity tears down — Android does this automatically.
    //
    // Free the leaked callbacks box. Late nativeReleaseScreen calls (an
    // in-flight Kotlin handler dispatched before [`release`] ran) check
    // `ptr != 0` and otherwise no-op, so the box being freed here is
    // safe.
    let ptr = entry.callbacks_ptr;
    if ptr != 0 {
        unsafe {
            drop(Box::from_raw(ptr as *mut AndroidNavCallbacks));
        }
    }
    // Drop the controller GlobalRef so the JVM can GC the
    // RustNavigator (along with its FrameLayout, which by this point
    // has been removed from its parent by the framework's
    // `clear_children` upstream).
    drop(entry.controller);
    drop(entry.control);
    true
}

/// Returns `Some(handle)` if `node` is a stack navigator; `None`
/// otherwise so the unified [`crate::make_handle`] can fall through.
pub(crate) fn make_handle(node: &GlobalRef) -> Option<NavigatorHandle> {
    let key = node_key(node);
    NAVIGATOR_INSTANCES.with(|m| {
        m.borrow().get(&key).map(|entry| {
            NavigatorHandle::with_control(Rc::new(()), &ANDROID_NAV_OPS, entry.control.clone())
        })
    })
}

struct AndroidNavigatorOps;
impl NavigatorOps for AndroidNavigatorOps {}
static ANDROID_NAV_OPS: AndroidNavigatorOps = AndroidNavigatorOps;

// =============================================================================
// JNI export — `RustHostFragment.onDestroyView` → release scope
// =============================================================================

/// JNI entry point: `RustHostFragment.onDestroyView` calls
/// `nativeReleaseScreen(nativePtr, scopeId)` to drop the per-screen
/// `Scope`. The pointer is the leaked `AndroidNavCallbacks` box;
/// `scope_id` is what `mount_screen` returned for the screen.
///
/// # Safety
///
/// `ptr` must have been produced by `Box::into_raw` on a
/// `Box<AndroidNavCallbacks>` in [`create`]. If the box has been freed
/// (because [`release`] ran first), `ptr` would dereference invalid
/// memory — so the box is freed *after* the controller drop, and on
/// the controller drop FragmentManager has already fired the
/// onDestroyView calls. Late calls after the box is freed cannot
/// happen in the Android transaction model.
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
