//! iOS implementation of the Form SDK.
//!
//! There is no UIKit "form" construct, and iOS form affordances
//! (autofill via `textContentType`, return-key submit via
//! `returnKeyType`) live per-field on the inputs, not on a container.
//! So the iOS `Form` is a plain passthrough `UIView`: the framework
//! parents the author's children into it and Taffy lays them out. The
//! `on_submit` closure is NOT auto-triggered here — submission is fired
//! by the author's submit `Button` calling `on_submit` directly.

use crate::{FormOps, FormProps};
use backend_ios::{IosBackend, IosNode};
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2_ui_kit::UIView;

pub(crate) static OPS: &dyn FormOps = &IosFormOps;

/// Register the Form handler against an `IosBackend`. One-line call from
/// app bootstrap.
pub fn register(backend: &mut IosBackend) {
    backend.register_external::<FormProps, _>(|_props, b| build_form(b));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_ios::IosExternalRegistrar(register)
}

fn build_form(b: &mut IosBackend) -> IosNode {
    let mtm = b.mtm();
    // Plain container view. `register_external_view` gives it a Taffy
    // layout node so the flex parent sizes/positions it and the children
    // the framework inserts get laid out beneath it.
    let view: Retained<UIView> = unsafe { msg_send_id![mtm.alloc::<UIView>(), init] };
    b.register_external_view(&view);
    IosNode::View(view)
}

struct IosFormOps;

// `submit` stays the trait default no-op: UIKit has no form-submit
// event to drive. Author code triggers submission by invoking its
// `on_submit` closure from the submit Button's `on_press`.
impl FormOps for IosFormOps {}
