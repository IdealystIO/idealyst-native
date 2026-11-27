//! Android: androidx `SwipeRefreshLayout` (the `RustSwipeRefresh` subclass
//! in `runtime/kotlin`) around the wrapped child.
//!
//! The layout is registered as a layout-transparent node
//! (`AndroidBackend::register_contents_view`), so the child keeps the
//! Taffy frame it would have unwrapped; `RustSwipeRefresh` places it at
//! that frame (its class doc has the mechanism). A pull reaches Rust
//! through `nativeOnRefresh(handle)`, where `handle` indexes the pull
//! callbacks of the mounted layouts — an id rather than a pointer, so a
//! pull delivered after teardown finds nothing instead of freed memory.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use backend_android::AndroidBackend;
use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::jlong;
use jni::JNIEnv;
use runtime_scene::{Element, MountCx};
use runtime_shared::color::Rgba;
use runtime_shared::logging::{log, LogLevel};
use runtime_vocabulary::style_attach::on_teardown;

use crate::binding::{self, NativeRefresh};
use crate::RefreshablePrim;

const CLASS: &str = "io/idealyst/pullrefresh/RustSwipeRefresh";

runtime_tls::thread_local! {
    static PULLS: RefCell<HashMap<jlong, Rc<dyn Fn()>>> = RefCell::new(HashMap::new());
    static NEXT: Cell<jlong> = const { Cell::new(1) };
}

/// The mounted layout, as the binding drives it.
struct AndroidRefresh {
    view: GlobalRef,
}

impl AndroidRefresh {
    /// Call `name` and read its result with `read`; a JNI failure is
    /// logged, its exception cleared (a pending one would crash the next
    /// JNI call), and reads as `None`.
    fn call<R>(
        &self,
        name: &str,
        sig: &str,
        args: &[JValue],
        read: impl FnOnce(jni::objects::JValueOwned<'_>) -> Option<R>,
    ) -> Option<R> {
        backend_android::with_jni_env(|env| match env.call_method(self.view.as_obj(), name, sig, args) {
            Ok(v) => read(v),
            Err(e) => {
                let _ = env.exception_clear();
                log(LogLevel::Error, &format!("pull-refresh: RustSwipeRefresh.{name}: {e}"));
                None
            }
        })
    }

    fn call_void(&self, name: &str, sig: &str, args: &[JValue]) {
        let _ = self.call(name, sig, args, |_| Some(()));
    }

    fn set_refreshing(&self, on: bool) {
        self.call_void("setRefreshing", "(Z)V", &[JValue::Bool(on.into())]);
    }
}

impl NativeRefresh for AndroidRefresh {
    fn is_refreshing(&self) -> bool {
        self.call("isRefreshing", "()Z", &[], |v| v.z().ok())
            .unwrap_or(false)
    }
    fn begin(&self) {
        self.set_refreshing(true);
    }
    fn end(&self) {
        self.set_refreshing(false);
    }
    fn set_tint(&self, tint: Option<Rgba>) {
        let argb = tint.map(|c| c.to_argb_u32() as i32).unwrap_or(0);
        self.call_void(
            "applyTint",
            "(ZI)V",
            &[JValue::Bool(tint.is_some().into()), JValue::Int(argb)],
        );
    }
}

pub(crate) fn mount(
    cx: &mut MountCx<'_, AndroidBackend>,
    prim: &Rc<RefreshablePrim>,
    children: Vec<Element>,
) -> GlobalRef {
    let backend = cx.backend().clone();
    let handle = NEXT.with(|n| {
        let h = n.get();
        n.set(h + 1);
        h
    });
    let view = backend.borrow().with_jni(|env, context| {
        let built = env.find_class(CLASS).and_then(|class| {
            env.new_object(
                &class,
                "(Landroid/content/Context;J)V",
                &[JValue::Object(&context.as_obj()), JValue::Long(handle)],
            )
        });
        match built {
            Ok(obj) => Some(env.new_global_ref(obj).expect("global ref RustSwipeRefresh")),
            Err(e) => {
                let _ = env.exception_clear();
                log(
                    LogLevel::Error,
                    &format!(
                        "pull-refresh: {CLASS} not found ({e}) — the app was built without \
                         this SDK's Kotlin runtime; rendering the child unwrapped"
                    ),
                );
                None
            }
        }
    });
    let Some(mut view) = view else {
        return crate::mount_passthrough(cx, prim, children);
    };
    backend.borrow_mut().register_contents_view(&view);
    cx.realize_children_into(&mut view, children);

    let native = Rc::new(AndroidRefresh { view: view.clone() });
    let pulled = binding::bind(&prim.props, native.clone(), backend_android::newcore::schedule_flush);
    PULLS.with(|p| p.borrow_mut().insert(handle, pulled));
    on_teardown(move || {
        PULLS.with(|p| p.borrow_mut().remove(&handle));
        native.call_void("detach", "()V", &[]);
    });
    view
}

/// `RustSwipeRefresh.nativeOnRefresh`: the user completed a pull.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_pullrefresh_RustSwipeRefresh_nativeOnRefresh(
    _env: JNIEnv,
    _this: JObject,
    handle: jlong,
) {
    // Cloned out of the map first: the callback runs author code, which
    // may unmount this layout and remove its entry.
    let pulled = PULLS.with(|p| p.borrow().get(&handle).cloned());
    let Some(pulled) = pulled else { return };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pulled())).is_err() {
        // Unwinding into the JVM is undefined behaviour; crash loud.
        log(LogLevel::Error, "pull-refresh: on_refresh panicked");
        std::process::abort();
    }
}
