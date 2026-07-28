//! Form-control handlers: `toggle`, `slider`, `activity_indicator`,
//! `text_input`, `text_area`.

use std::rc::Rc;

use runtime_scene::{Element, MountCx};
use runtime_world::Value;

use crate::caps::{ActivityIndicatorOps, SliderOps, StyleOps, TextInputOps, ToggleOps};
use crate::prims::{ActivityIndicatorPrim, SliderPrim, TextAreaPrim, TextInputPrim, TogglePrim};
use crate::style_attach::attach_style;

use super::{bind_dyn, bind_value, initial_of};

/// Mount a `toggle` — port of `walker/toggle.rs::build`.
///
/// Sequence: `create_toggle(initial, on_change)` → attach_style →
/// controlled-value write-back binding (one `update_toggle_value` at
/// mount, then per change — the widget no-ops on same-value sets, so
/// the round-trip is stable) → ref-fill.
pub fn mount_toggle<H>(cx: &mut MountCx<'_, H>, prim: TogglePrim, _children: Vec<Element>) -> H::Node
where
    H: ToggleOps + StyleOps,
{
    let backend = cx.backend().clone();
    let initial = initial_of(&prim.value);
    let node = backend
        .borrow_mut()
        .create_toggle(initial, prim.on_change, &prim.a11y);
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }
    {
        let b = backend.clone();
        let n = node.clone();
        bind_value(prim.value, move |&v| {
            b.borrow_mut().update_toggle_value(&n, v);
        });
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_toggle_handle(&node);
        fill(handle);
    }
    node
}

/// Mount a `slider` — port of `walker/slider.rs::build`, including the
/// step-snap wrapper: the user's `on_change` receives values snapped to
/// `step` before dispatch, so every backend produces identical values
/// regardless of native step handling.
pub fn mount_slider<H>(cx: &mut MountCx<'_, H>, prim: SliderPrim, _children: Vec<Element>) -> H::Node
where
    H: SliderOps + StyleOps,
{
    let backend = cx.backend().clone();
    let initial = initial_of(&prim.value);
    let on_change_snapped: Rc<dyn Fn(f32)> = if let Some(step) = prim.step {
        let user = prim.on_change.clone();
        let min = prim.min;
        Rc::new(move |v| {
            let snapped = min + ((v - min) / step).round() * step;
            user(snapped);
        })
    } else {
        prim.on_change.clone()
    };
    let node = backend.borrow_mut().create_slider(
        initial,
        prim.min,
        prim.max,
        prim.step,
        on_change_snapped,
        &prim.a11y,
    );
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }
    {
        let b = backend.clone();
        let n = node.clone();
        bind_value(prim.value, move |&v| {
            b.borrow_mut().update_slider_value(&n, v);
        });
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_slider_handle(&node);
        fill(handle);
    }
    node
}

/// Mount an `activity_indicator` — port of
/// `walker/activity_indicator.rs::build`. A `Dyn` size creates at the
/// closure's initial value and installs the resize binding (first fire
/// at mount).
pub fn mount_activity_indicator<H>(
    cx: &mut MountCx<'_, H>,
    prim: ActivityIndicatorPrim,
    _children: Vec<Element>,
) -> H::Node
where
    H: ActivityIndicatorOps + StyleOps,
{
    let backend = cx.backend().clone();
    let (initial_size, dyn_size) = match prim.size {
        Value::Const(s) => (s, None),
        Value::Dyn(f) => (f(), Some(f)),
    };
    let node =
        backend
            .borrow_mut()
            .create_activity_indicator(initial_size, prim.color.as_ref(), &prim.a11y);
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }
    if let Some(f) = dyn_size {
        let b = backend.clone();
        let n = node.clone();
        let _binding = runtime_world::effect(move || {
            let size = f();
            b.borrow_mut().update_activity_indicator_size(&n, size);
        });
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_activity_indicator_handle(&node);
        fill(handle);
    }
    node
}

/// Mount a `text_input` — port of
/// `walker/text_input.rs::build_text_input`.
///
/// Sequence: `create_text_input(initial value/placeholder/secure,
/// callbacks)` → attach_style → focus notifier → controlled-value
/// write-back binding (first fire at mount) → `Dyn`-only secure binding
/// → `Dyn`-only placeholder binding → ref-fill.
pub fn mount_text_input<H>(
    cx: &mut MountCx<'_, H>,
    prim: TextInputPrim,
    _children: Vec<Element>,
) -> H::Node
where
    H: TextInputOps + StyleOps,
{
    let backend = cx.backend().clone();
    let initial_value = initial_of(&prim.value);
    let initial_secure = initial_of(&prim.secure);
    let initial_placeholder = initial_of(&prim.placeholder);
    let node = backend.borrow_mut().create_text_input(
        &initial_value,
        initial_placeholder.as_deref(),
        prim.on_change,
        prim.on_key_down,
        prim.on_blur,
        initial_secure,
        &prim.a11y,
    );
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }
    if let Some(on_focus) = prim.on_focus {
        backend
            .borrow_mut()
            .set_text_input_focus_handler(&node, on_focus);
    }
    {
        let b = backend.clone();
        let n = node.clone();
        bind_value(prim.value, move |v| {
            b.borrow_mut().update_text_input_value(&n, v);
        });
    }
    {
        let b = backend.clone();
        let n = node.clone();
        bind_dyn(prim.secure, move |&s| {
            b.borrow_mut().update_text_input_secure(&n, s);
        });
    }
    {
        let b = backend.clone();
        let n = node.clone();
        bind_dyn(prim.placeholder, move |p| {
            b.borrow_mut().update_text_input_placeholder(&n, p.as_deref());
        });
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_text_input_handle(&node);
        fill(handle);
    }
    node
}

/// Mount a `text_area` — port of
/// `walker/text_input.rs::build_text_area`. Placeholder/wrap/rows are
/// create-time config; the controlled `value` write-back binding fires
/// once at mount then per change.
pub fn mount_text_area<H>(
    cx: &mut MountCx<'_, H>,
    prim: TextAreaPrim,
    _children: Vec<Element>,
) -> H::Node
where
    H: TextInputOps + StyleOps,
{
    let backend = cx.backend().clone();
    let initial_value = initial_of(&prim.value);
    let node = backend.borrow_mut().create_text_area(
        &initial_value,
        prim.placeholder.as_deref(),
        prim.wrap,
        prim.min_rows,
        prim.max_rows,
        prim.on_change,
        prim.on_key_down,
        &prim.a11y,
    );
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }
    {
        let b = backend.clone();
        let n = node.clone();
        bind_value(prim.value, move |v| {
            b.borrow_mut().update_text_area_value(&n, v);
        });
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_text_area_handle(&node);
        fill(handle);
    }
    node
}
