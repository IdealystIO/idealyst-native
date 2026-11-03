//! `view` / `pressable` `on_focus` (`InputOps::install_focus_handler`):
//! a `RustFocusWithinListener` (Kotlin — see its docs for the window-wide
//! focus observer and the touch-mode gate) per node, holding a leaked
//! [`FocusWithinSlot`] it reports into through `nativeFocusWithin`.
//!
//! Leaked like the other per-view JNI callbacks (`callbacks::leak`): the
//! listener can outlive the node's scope (a detach reports after a route
//! change dropped it), and the handler is scope-guarded by the vocabulary,
//! so a late report is inert rather than a use-after-free.
//!
//! A staged Kotlin runtime without the class (CLI not reinstalled) throws
//! `NoClassDefFoundError`; the pending exception is cleared and `on_focus`
//! degrades to never firing.

use jni::objects::{GlobalRef, JValue};
use runtime_shared::FocusWithinHandler;

use crate::imp::callbacks::leak;
use crate::imp::with_env;

const LISTENER_CLASS: &str = "io/idealyst/runtime/RustFocusWithinListener";

/// What `nativeFocusWithin` derefs.
pub(crate) struct FocusWithinSlot {
    pub(crate) handler: FocusWithinHandler,
}

/// Attach a `RustFocusWithinListener` for `handler` to `node`.
pub(crate) fn install(node: &GlobalRef, handler: FocusWithinHandler) {
    let ptr = leak(FocusWithinSlot { handler });
    with_env(|env| {
        let made = env.find_class(LISTENER_CLASS).and_then(|class| {
            env.new_object(
                &class,
                "(JLandroid/view/View;)V",
                &[JValue::Long(ptr), JValue::Object(node.as_obj())],
            )
        });
        // The view keeps the listener alive (its attach-state listeners).
        if made.is_err() {
            let _ = env.exception_clear();
        }
    });
}
