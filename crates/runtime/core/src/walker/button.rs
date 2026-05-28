//! `Element::Button` build path.
//!
//! Bound labels install an Effect that calls `update_button_label`
//! on every signal change the closure subscribes to. Style + disabled
//! are wired through the standard `attach_style` / `attach_disabled`
//! pair so `state hovered`/`pressed`/`disabled` overlays apply
//! identically to other interactive primitives.

use super::debug::time_backend_create;
use super::style::{attach_disabled, attach_style};
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::derive::Action;
use crate::handles::RefFill;
use crate::primitives;
use crate::reactive::Effect;
use crate::sources::{StyleSource, TextSource};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    label: TextSource,
    on_click: Action,
    leading_icon: Option<primitives::icon::IconData>,
    trailing_icon: Option<primitives::icon::IconData>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    disabled: Option<Box<dyn Fn() -> bool>>,
    a11y: AccessibilityProps,
) -> B::Node {
    // Pull the initial label from the source and create the
    // native widget with it. For reactive labels we install
    // an Effect below that calls `update_button_label` on
    // every signal change the closure subscribes to —
    // mirroring how Image's `src` works.
    let (initial_label, reactive_label) = match label {
        TextSource::Static(s) => (s, None),
        TextSource::Bound(d) => ((d.compute)(), Some(d.compute.clone())),
        // Button labels don't get the JS-binding fast path
        // (Button isn't a hierarchy-scale hot path); fall
        // back to `compute_fallback` as a regular `Bound`-
        // style reactive label. The JS-binding fast path
        // stays exclusive to `Element::Text`.
        TextSource::JsBinding(spec) => {
            let compute = spec.compute_fallback.clone();
            let initial = (compute)();
            (initial, Some(compute))
        }
    };
    // `on_click` is an `Action` carrying both the runtime
    // callable (`fire`) and the structured metadata
    // (`method` + `inputs` + `output`). Backends pick what
    // they need from it — runtime backends call `fire`,
    // generator backends serialize the metadata.
    let n = time_backend_create(pkind!(Button), || {
        backend.borrow_mut().create_button(
            &initial_label,
            &on_click,
            leading_icon.as_ref(),
            trailing_icon.as_ref(),
            &a11y,
        )
    });
    // attach_style returns the state setter so we can drive
    // the DISABLED bit reactively from `disabled` below. If
    // there's no style, we still need to react to disabled to
    // toggle the native widget's inert state, so allocate a
    // no-op-style setter route in that case.
    let state_setter = style.map(|s| attach_style(backend, &n, s));
    if let Some(RefFill::Button(fill)) = ref_fill {
        let handle = backend.borrow().make_button_handle(&n);
        fill(handle);
    }
    if let Some(d) = disabled {
        attach_disabled(backend, &n, d, state_setter);
    }
    // Reactive label effect. The first invocation re-reads
    // the closure (so the initial label and the first
    // effect run produce the same string), but signal reads
    // inside the closure subscribe this effect for future
    // updates.
    if let Some(f) = reactive_label {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let s = f();
            backend.borrow_mut().update_button_label(&node, &s);
        });
    }
    n
}
