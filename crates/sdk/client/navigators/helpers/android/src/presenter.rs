//! Native stack presenter for the outlet-model stack navigator — the
//! Android implementation behind
//! `runtime_vocabulary::handlers::nav_native_push`, and the twin of
//! `ios-navigator-helpers::IosStackPresenter`.
//!
//! # What it adds over the outlet swap
//!
//! Without a presenter every stack reveal is one direction-blind
//! `clear_children` + `insert`: correct, but no transition, and system
//! Back is not routed to the stack at all (it leaves the app from any
//! depth). This seats `io.idealyst.runtime.RustStackPresenter` in the
//! outlet, which plays Material's shared-axis transition for push, pop
//! and replace and takes system Back while more than one screen is
//! stacked.
//!
//! # A view-level container, not FragmentManager
//!
//! Android has no `UINavigationController`-style view that owns a stack.
//! The platform container is FragmentManager, and the legacy engine
//! (`stack.rs`, `RustNavigator.kt`) used it. Its lifecycle fights the
//! handler owning screen lifetimes: fragments outlive or recreate around
//! the Rust content on config change and process death, and commits are
//! asynchronous. The presenter only needs to place views and animate
//! them, so it uses plain views plus the activity's
//! `OnBackPressedDispatcher`.
//!
//! # Layout
//!
//! A screen never enters the outlet's Taffy child list (two screens
//! there would lay out side by side mid-transition). Each is registered
//! as a hosted root (`backend_android::host_screen_root`), computed
//! against the outlet's size and shown in a full-size wrapper.
//!
//! # Chrome stays author layout
//!
//! As on iOS, the author's `StackHeader` is the chrome on every backend
//! (CLAUDE.md §7); only the transition mechanics are platform-idiomatic.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::jlong;
use jni::JNIEnv;
use runtime_vocabulary::handlers::nav_native_push::{NativePushHandle, StackPresenter};

/// State shared by the reveal closures and the `nativeUserPop` export.
struct PresenterState {
    /// The navigator outlet the screens are laid out against.
    outlet: GlobalRef,
    /// The Kotlin `RustStackPresenter`.
    controller: GlobalRef,
    /// Screens the container holds, newest last. Mirrors the Kotlin
    /// wrapper list so a screen leaving the stack is unhosted.
    screens: RefCell<Vec<GlobalRef>>,
    /// The handler's logical-only pop, installed via `set_user_pop`.
    user_pop: RefCell<Option<Rc<dyn Fn()>>>,
}

impl PresenterState {
    fn call(&self, method: &str, screen: Option<&GlobalRef>) {
        backend_android::with_jni_env(|env| {
            let result = match screen {
                Some(s) => env.call_method(
                    self.controller.as_obj(),
                    method,
                    "(Landroid/view/View;)V",
                    &[JValue::Object(&s.as_obj())],
                ),
                None => env.call_method(self.controller.as_obj(), method, "()V", &[]),
            };
            if result.is_err() {
                // A missing method means the embedded Kotlin runtime is
                // older than this native lib. Crash loud: a stack that
                // silently stops navigating is worse.
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                }
                panic!("RustStackPresenter.{method} failed; rebuild the idealyst CLI");
            }
        });
    }

    fn host(&self, screen: &GlobalRef) {
        backend_android::host_screen_root(screen, &self.outlet);
    }

    /// Take the top screen off the mirror and stop laying it out
    /// against the outlet.
    fn drop_top(&self) {
        if let Some(s) = self.screens.borrow_mut().pop() {
            backend_android::unhost_screen_root(&s);
        }
    }

    fn drop_all(&self) {
        for s in self.screens.borrow_mut().drain(..) {
            backend_android::unhost_screen_root(&s);
        }
    }
}

/// Owns the leaked pointer Kotlin holds and the Kotlin side's Back
/// registration. Captured by every reveal closure, so it drops with the
/// `NativePushHandle`, which the handler holds for the navigator's life.
struct Lifetime {
    state: Rc<PresenterState>,
    ptr: jlong,
}

impl Drop for Lifetime {
    fn drop(&mut self) {
        // Kotlin first, so Back can no longer reach the pointer we free.
        self.state.call("dispose", None);
        self.state.drop_all();
        // SAFETY: `ptr` came from `Box::into_raw` in `attach`, and
        // `dispose` removed the only Kotlin path that dereferences it.
        drop(unsafe { Box::from_raw(self.ptr as *mut Rc<PresenterState>) });
    }
}

/// Installed once per thread by `stack-navigator`; consulted by every
/// stack navigator that mounts.
pub struct AndroidStackPresenter;

/// The screen view out of the handler's erased node, or `None` for a
/// foreign node type (a process can host a second backend beside this
/// one, see `StackPresenter::attach`).
fn screen_view(node: &Rc<dyn Any>) -> Option<GlobalRef> {
    node.downcast_ref::<GlobalRef>().cloned()
}

impl StackPresenter for AndroidStackPresenter {
    fn attach(&self, outlet: Rc<dyn Any>) -> Option<NativePushHandle> {
        let outlet = outlet.downcast_ref::<GlobalRef>()?.clone();

        let controller = backend_android::with_jni_env(|env| -> Option<GlobalRef> {
            let class = env.find_class("io/idealyst/runtime/RustStackPresenter").ok()?;
            // `nativePtr` is set below once the state exists; Kotlin only
            // reads it on Back, which cannot fire before `attach` returns.
            let obj = env
                .new_object(
                    &class,
                    "(Landroid/view/ViewGroup;)V",
                    &[JValue::Object(&outlet.as_obj())],
                )
                .ok()?;
            env.new_global_ref(obj).ok()
        });
        let Some(controller) = controller else {
            // An embedded Kotlin runtime without the class: decline and
            // keep the outlet swap, which still navigates correctly.
            backend_android::with_jni_env(|env| {
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_clear();
                }
            });
            log::warn!(
                "RustStackPresenter unavailable; stack transitions disabled. \
                 Rebuild the idealyst CLI to pick up the runtime change."
            );
            return None;
        };

        let state = Rc::new(PresenterState {
            outlet: outlet.clone(),
            controller: controller.clone(),
            screens: RefCell::new(Vec::new()),
            user_pop: RefCell::new(None),
        });
        let ptr = Box::into_raw(Box::new(state.clone())) as jlong;
        backend_android::with_jni_env(|env| {
            env.set_field(controller.as_obj(), "nativePtr", "J", JValue::Long(ptr))
                .expect("RustStackPresenter.nativePtr");
        });
        let life = Rc::new(Lifetime { state: state.clone(), ptr });

        let container = backend_android::with_jni_env(|env| {
            let c = env
                .get_field(controller.as_obj(), "container", "Landroid/widget/FrameLayout;")
                .and_then(|f| f.l())
                .expect("RustStackPresenter.container");
            env.new_global_ref(c).expect("container global ref")
        });

        // `seat` and `reset` are the same move: collapse to one screen,
        // unanimated.
        let collapse = |life: Rc<Lifetime>| {
            Rc::new(move |node: Rc<dyn Any>| {
                let Some(view) = screen_view(&node) else { return };
                let s = &life.state;
                s.drop_all();
                s.host(&view);
                s.screens.borrow_mut().push(view.clone());
                s.call("reset", Some(&view));
            }) as Rc<dyn Fn(Rc<dyn Any>)>
        };
        let seat = collapse(life.clone());
        let reset = collapse(life.clone());

        let push = {
            let life = life.clone();
            Rc::new(move |node: Rc<dyn Any>| {
                let Some(view) = screen_view(&node) else { return };
                let s = &life.state;
                s.host(&view);
                s.screens.borrow_mut().push(view.clone());
                s.call("push", Some(&view));
            }) as Rc<dyn Fn(Rc<dyn Any>)>
        };

        let pop = {
            let life = life.clone();
            // The revealed screen is already in the container (covered,
            // not removed), so the node is not needed.
            Rc::new(move |_node: Rc<dyn Any>| {
                let s = &life.state;
                if s.screens.borrow().len() <= 1 {
                    return;
                }
                s.drop_top();
                s.call("pop", None);
            }) as Rc<dyn Fn(Rc<dyn Any>)>
        };

        let replace = {
            let life = life.clone();
            Rc::new(move |node: Rc<dyn Any>| {
                let Some(view) = screen_view(&node) else { return };
                let s = &life.state;
                s.drop_top();
                s.host(&view);
                s.screens.borrow_mut().push(view.clone());
                s.call("replace", Some(&view));
            }) as Rc<dyn Fn(Rc<dyn Any>)>
        };

        let set_user_pop = {
            let life = life.clone();
            Rc::new(move |cb: Rc<dyn Fn()>| {
                *life.state.user_pop.borrow_mut() = Some(cb);
            }) as Rc<dyn Fn(Rc<dyn Fn()>)>
        };

        Some(NativePushHandle {
            host: Rc::new(container),
            seat,
            push,
            pop,
            replace,
            reset,
            set_user_pop,
        })
    }
}

/// System Back popped the container (`RustStackPresenter.backCallback`).
/// Reconcile the mirror and tell the handler, which pops its logical
/// stack WITHOUT driving the presenter again.
///
/// # Safety
///
/// `ptr` is the `Box<Rc<PresenterState>>` leaked in `attach`; it stays
/// live until `Lifetime::drop`, which first removes the Back callback
/// (the only caller) on the same thread.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustStackPresenter_nativeUserPop(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    let state = (*(ptr as *const Rc<PresenterState>)).clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.drop_top();
        let cb = state.user_pop.borrow().clone();
        if let Some(cb) = cb {
            cb();
        }
        // This is a raw Activity callback, outside every framework-wrapped
        // handler: nothing else commits the writes the handler just staged
        // (depth, active route, header chrome), so the header would keep
        // showing the popped screen until some unrelated event flushed.
        backend_android::newcore::schedule_flush();
    }));
    if result.is_err() {
        // Crash loud, as the backend's own exports do: unwinding into
        // the JVM is undefined behaviour.
        log::error!("RustStackPresenter.nativeUserPop panicked");
        std::process::abort();
    }
}

/// Install the Android stack presenter. Idempotent (the seam replaces).
pub fn install() {
    runtime_vocabulary::handlers::nav_native_push::install_stack_presenter(Rc::new(
        AndroidStackPresenter,
    ));
}
