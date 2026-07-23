//! Linux (GTK4) implementation of the Form SDK.
//!
//! GTK has no "form" construct, and desktop form affordances (Enter-to-
//! submit, autofill grouping) that a browser's `<form>` provides have no
//! GTK container equivalent — GTK apps wire the return key and submission
//! per-field / per-button, not on a wrapping widget. So the Linux `Form`
//! is a plain passthrough container: the framework parents the author's
//! children into it and the external layout engine lays them out,
//! exactly like the iOS (`UIView`) and Android (`FrameLayout`) leaves.
//! The `on_submit` closure is NOT auto-triggered here — submission is
//! fired by the author's submit `Button` calling `on_submit` directly.
//!
//! # Why `IdealystView`, not a bare `gtk::Box`
//!
//! The passthrough must be a real container the framework can parent
//! into. `LinuxBackend::insert` only attaches a child *widget* when the
//! parent widget is an `IdealystView` (or a `ScrolledWindow`'s inner
//! `Fixed`); a plain `gtk::Box` would receive the child's Taffy layout
//! node but never the GTK widget, so the form's children would compute a
//! layout yet never appear. `IdealystView` is the same widget
//! `create_view` returns for every framework `view`, so the form node
//! behaves like an ordinary passthrough view — matching iOS's plain
//! `UIView` and Android's `FrameLayout`.

use crate::{FormOps, FormProps};
use backend_linux::{IdealystView, LinuxBackend, LinuxNode};
use gtk4::prelude::*;

pub(crate) static OPS: &dyn FormOps = &LinuxFormOps;

/// Register the Form handler against a `LinuxBackend`. One-line call from
/// app bootstrap so `Form` elements lower to the native passthrough
/// container.
pub fn register(backend: &mut LinuxBackend) {
    backend.register_external::<FormProps, _>(|_props, b| build_form(b));
}

fn build_form(b: &mut LinuxBackend) -> LinuxNode {
    // Plain container view. `IdealystView` is the framework's own
    // container widget, so `register_external_view` gives it a Taffy
    // layout node and `LinuxBackend::insert` will parent the form's
    // children into it just as it would for any `view`.
    let view = IdealystView::new();
    b.register_external_view(view.upcast::<gtk4::Widget>())
}

struct LinuxFormOps;

// `submit` stays the trait default no-op: GTK has no form-submit event
// to drive. Author code triggers submission by invoking its `on_submit`
// closure from the submit Button's `on_press`.
impl FormOps for LinuxFormOps {}
