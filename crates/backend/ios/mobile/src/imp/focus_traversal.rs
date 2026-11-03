//! Tab / Shift+Tab focus traversal among a window's text inputs.
//!
//! UIKit's public API has no "move to the next key view" on iPhone: the
//! hardware-keyboard focus system (`UIFocusSystem`) runs on iPad and Mac
//! Catalyst, and a `UITextField` on iPhone does nothing with a Tab. So the
//! backend walks the window itself, with the rules UIKit's iPad keyboard
//! navigation applies to Tab — it moves among editable text inputs:
//!
//! - **Stops**: every `UITextField` that is enabled and every editable
//!   `UITextView` that can become first responder — the framework's
//!   `text_input` / `text_area` and a text input a third-party view embeds
//!   alike. Pressables are not Tab stops on iOS (UIKit moves among buttons
//!   with the arrow keys under Full Keyboard Access, not Tab).
//! - **Skipped**: anything inside a hidden, fully transparent or
//!   interaction-disabled subtree — what a user can't see or reach.
//! - **Order**: depth-first subview order, which is the author's tree
//!   order, as on web (DOM order) and GTK (the widget focus chain). Past the
//!   last stop it wraps to the first.
//!
//! Two callers: the robot's key dispatch
//! (`IntrospectionOps::move_focus`) and the hardware keyboard's Tab key
//! commands on a focused field (`keyboard::tab_key_commands`).

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, msg_send_id};
use objc2_ui_kit::UIView;

/// Below this alpha a view is treated as invisible — UIKit's own hit-test
/// cutoff (`alpha < 0.01` views receive no touches).
const INVISIBLE_ALPHA: f64 = 0.01;

/// Move first responder to the next (`forward`) or previous Tab stop in the
/// window holding `within` (or, when `within` is in no window, `fallback`'s).
/// With no stop focused, forward starts at the first and backward at the
/// last. Returns whether a stop took focus.
pub(crate) fn move_focus(within: &UIView, fallback: Option<&UIView>, forward: bool) -> bool {
    let window: Option<Retained<UIView>> = unsafe { msg_send_id![within, window] };
    let window = match window {
        Some(w) => w,
        None => match fallback.and_then(|f| unsafe { msg_send_id![f, window] }) {
            Some(w) => w,
            None => return false,
        },
    };
    let stops = tab_stops(&window);
    if stops.is_empty() {
        return false;
    }
    let at = stops.iter().position(|v| unsafe { msg_send![&**v, isFirstResponder] });
    let next = match (at, forward) {
        (None, true) => 0,
        (None, false) => stops.len() - 1,
        (Some(i), true) => (i + 1) % stops.len(),
        (Some(i), false) => (i + stops.len() - 1) % stops.len(),
    };
    if at == Some(next) {
        return true;
    }
    unsafe { msg_send![&*stops[next], becomeFirstResponder] }
}

/// The window's Tab stops in order (module docs).
pub(crate) fn tab_stops(window: &UIView) -> Vec<Retained<UIView>> {
    let mut out = Vec::new();
    collect(window, &mut out);
    out
}

fn collect(view: &UIView, out: &mut Vec<Retained<UIView>>) {
    let reachable = unsafe {
        let hidden: bool = msg_send![view, isHidden];
        let alpha: f64 = msg_send![view, alpha];
        let interactive: bool = msg_send![view, isUserInteractionEnabled];
        !hidden && alpha >= INVISIBLE_ALPHA && interactive
    };
    if !reachable {
        return;
    }
    if is_tab_stop(view) {
        out.push(unsafe { Retained::retain(view as *const UIView as *mut UIView) }.expect("a live view"));
        // A text input's own subviews (UITextView's text container, a
        // field's clear button) are not stops of their own.
        return;
    }
    let subviews: Retained<AnyObject> = unsafe { msg_send_id![view, subviews] };
    let count: usize = unsafe { msg_send![&*subviews, count] };
    for i in 0..count {
        let child: *mut UIView = unsafe { msg_send![&*subviews, objectAtIndex: i] };
        if let Some(child) = unsafe { child.as_ref() } {
            collect(child, out);
        }
    }
}

fn is_tab_stop(view: &UIView) -> bool {
    unsafe {
        let editable = if msg_send![view, isKindOfClass: objc2::class!(UITextField)] {
            msg_send![view, isEnabled]
        } else if msg_send![view, isKindOfClass: objc2::class!(UITextView)] {
            msg_send![view, isEditable]
        } else {
            false
        };
        editable && msg_send![view, canBecomeFirstResponder]
    }
}

impl super::IosBackend {
    /// `IntrospectionOps::move_focus`: the traversal in `node`'s window —
    /// the host root's when the node is in none — run by the returned
    /// injection, since `becomeFirstResponder` fires the fields' focus
    /// handlers (author code that may borrow the backend).
    pub(crate) fn move_focus_impl(&self, node: &super::IosNode, forward: bool) -> runtime_vocabulary::caps::NativeInjection {
        let within: Retained<UIView> =
            unsafe { Retained::retain(node.as_view() as *const UIView as *mut UIView) }.expect("a live view");
        let host = self.host_root.clone();
        Box::new(move || {
            move_focus(&within, host.as_deref(), forward);
            Ok(())
        })
    }
}
