//! Text-family handlers: `text`, `button`.

use runtime_scene::{Element, MountCx};
use runtime_world::{effect, Value};

use crate::caps::{ButtonOps, TextOps};
use crate::prims::{ButtonPrim, TextPrim, TextSourceProp};
use crate::style_attach::{attach_style, on_teardown, StyleServices};

use super::bind_value;

/// Mount a `text` — port of `walker/text.rs::build`.
///
/// Sequences by source:
/// - `Const`: `create_text(content)`; no effects.
/// - `Dyn`: `create_text_with_id("")`; on `Some((node, id))` the batched
///   fast path — teardown releases the id (`release_text_id`), and the
///   binding effect moves each computed `String` into
///   `update_text_by_id` (first fire at mount). On `None`, the legacy
///   `create_text("")` + per-fire `update_text` path.
/// - `Runs`: `create_styled_text` once (basic styled runs; theme-cohort
///   re-realization and the `JsBinding` fan-out are deferred — crate
///   docs).
///
/// Then attach_style → ref-fill, matching the walker's outer sequence.
pub fn mount_text<H>(cx: &mut MountCx<'_, H>, prim: TextPrim, _children: Vec<Element>) -> H::Node
where
    H: TextOps + StyleServices,
{
    let backend = cx.backend().clone();
    let node = match prim.content {
        TextSourceProp::Value(Value::Const(content)) => {
            backend.borrow_mut().create_text(&content, &prim.a11y)
        }
        TextSourceProp::Value(Value::Dyn(compute)) => {
            let batched = backend.borrow_mut().create_text_with_id("", &prim.a11y);
            match batched {
                Some((node, id)) => {
                    // Release the backend's id slot when the subtree
                    // tears down (switch-arm flip, unmount) — without
                    // this every mount/unmount cycle leaks a registry
                    // entry (the walker's rationale, ported). Registered
                    // BEFORE the binding effect, like the walker's
                    // on_cleanup-before-Effect ordering.
                    let b = backend.clone();
                    on_teardown(move || {
                        b.borrow_mut().release_text_id(id);
                    });
                    let b = backend.clone();
                    let _binding = effect(move || {
                        let value = compute();
                        // By-value: the computed String moves straight
                        // into the backend's pending buffer.
                        b.borrow_mut().update_text_by_id(id, value);
                    });
                    node
                }
                None => {
                    let node = backend.borrow_mut().create_text("", &prim.a11y);
                    let b = backend.clone();
                    let n = node.clone();
                    let _binding = effect(move || {
                        let value = compute();
                        b.borrow_mut().update_text(&n, &value);
                    });
                    node
                }
            }
        }
        TextSourceProp::Runs(runs) => backend.borrow_mut().create_styled_text(&runs, &prim.a11y),
    };
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_text_handle(&node);
        fill(handle);
    }
    node
}

/// Mount a `button` — port of `walker/button.rs::build`.
///
/// Sequence: `create_button(initial label, action, icons)` →
/// attach_style → ref-fill → disabled binding (`set_disabled`; the
/// state-setter flip rides the deferred state-overlay machinery) →
/// label binding effect for `Dyn` labels (`update_button_label`, first
/// fire at mount — the walker's reactive-label shape).
pub fn mount_button<H>(cx: &mut MountCx<'_, H>, prim: ButtonPrim, _children: Vec<Element>) -> H::Node
where
    H: ButtonOps + StyleServices,
{
    let backend = cx.backend().clone();
    let (initial_label, dyn_label) = match prim.label {
        Value::Const(s) => (s, None),
        Value::Dyn(f) => (f(), Some(f)),
    };
    let node = backend.borrow_mut().create_button(
        &initial_label,
        &prim.on_press,
        prim.leading_icon.as_ref(),
        prim.trailing_icon.as_ref(),
        &prim.a11y,
    );
    let state_setter = prim
        .style
        .map(|style| attach_style(&backend, &node, style));
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_button_handle(&node);
        fill(handle);
    }
    if let Some(disabled) = prim.disabled {
        // A real widget goes inert natively via set_disabled — no
        // press-block flag (that's the bare-pressable path). The
        // DISABLED state-bit flip rides the styled setter so a
        // `state disabled { … }` overlay applies (old `attach_disabled`).
        let b = backend.clone();
        let n = node.clone();
        bind_value(disabled, move |&d| {
            b.borrow_mut().set_disabled(&n, d);
            if let Some(setter) = state_setter.as_ref() {
                setter(runtime_core::StateBits::DISABLED, d);
            }
        });
    }
    if let Some(f) = dyn_label {
        let b = backend.clone();
        let n = node.clone();
        let _binding = effect(move || {
            let label = f();
            b.borrow_mut().update_button_label(&n, &label);
        });
    }
    node
}
