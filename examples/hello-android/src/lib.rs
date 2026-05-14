//! Android entry point for the shared `hello` app.
//!
//! The Kotlin Activity calls `NativeBridge.attach(context, rootLayout)`
//! once after `setContentView`. That method is implemented by the
//! `Java_io_idealyst_hello_NativeBridge_attach` native function below,
//! which builds an `AndroidBackend` and calls `framework_core::render`
//! with the shared `hello::app()` tree.
//!
//! The returned `Owner` is stashed in a thread-local so it lives for
//! the duration of the Activity. Dropping the Activity (or calling
//! `detach`) tears the tree down.

#![cfg(target_os = "android")]

use backend_android::AndroidBackend;
use jni::objects::{JClass, JObject};
use jni::JNIEnv;
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    /// Holds the framework's `Owner` for the active mount. Single-Activity
    /// demo, so a thread-local on the UI thread is sufficient.
    static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };
}

/// Attach the framework to an Android `Context` and a parent `ViewGroup`
/// (typically a vertical `LinearLayout` created by the Activity). The
/// shared `hello::app()` tree is built underneath that group.
///
/// Idempotent in the sense that re-calling tears the previous tree down
/// before building a new one. (Useful if you want to hot-reload by
/// calling `detach` then `attach` again.)
#[no_mangle]
pub extern "system" fn Java_io_idealyst_hello_NativeBridge_attach<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    context: JObject<'local>,
    root: JObject<'local>,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Promote both refs to globals so they survive past this JNI call.
        let context_global = env.new_global_ref(&context).expect("new_global_ref context");
        let root_global = env.new_global_ref(&root).expect("new_global_ref root");

        // Tear down any previous mount before building a new one.
        OWNER.with(|slot| slot.borrow_mut().take());

        let backend = Rc::new(RefCell::new(AndroidBackend::new(
            context_global,
            root_global,
        )));
        let owner = framework_core::render(backend, hello::app());

        OWNER.with(|slot| *slot.borrow_mut() = Some(owner));

        log::info!("idealyst: attach complete");
    }));
}

/// Detach the active mount. Drops every signal/effect and (in a future
/// version that wires it) releases the per-button click callbacks.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_hello_NativeBridge_detach<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {
    OWNER.with(|slot| slot.borrow_mut().take());
}
