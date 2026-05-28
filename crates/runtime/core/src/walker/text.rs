//! `Element::Text` build path.
//!
//! Three sources: static (`create_text` once, no effect), `Bound`
//! (the canonical reactive path — installs an `Effect` that calls
//! `update_text_by_id` or `update_text`), and `JsBinding` (the fast
//! path that hands the structured binding to the backend for JS-side
//! fan-out, falling back to `Bound`-shape behavior when the backend
//! can't host it).

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::reactive::Effect;
use crate::sources::{StyleSource, TextSource};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    source: TextSource,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let n = build_text(backend, source, &a11y);
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    if let Some(RefFill::Text(fill)) = ref_fill {
        let handle = backend.borrow().make_text_handle(&n);
        fill(handle);
    }
    n
}

/// Builds a Text primitive (static or reactive). Style application is
/// handled by the caller via `attach_style` so the content effect and
/// the style effect stay independent.
fn build_text<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    source: TextSource,
    a11y: &AccessibilityProps,
) -> B::Node {
    match source {
        TextSource::Static(content) => {
            time_backend_create(pkind!(Text), || backend.borrow_mut().create_text(&content, a11y))
        }
        TextSource::Bound(d) => {
            // Fast path: backends that return `Some(id)` from
            // `create_text_with_id` get a batched effect closure
            // that updates by id (one FFI per fan-out, regardless
            // of subscriber count). Everything else falls through
            // to the per-fire `update_text(&node, &str)` path.
            //
            // We try the batched path FIRST so backends opt in
            // implicitly — no `if cfg!()` gating, no separate code
            // path in the variant. The default `create_text_with_id`
            // returns `None`, so non-web/non-batching backends
            // see the legacy behavior unchanged.
            let batched = time_backend_create(pkind!(Text), || {
                backend.borrow_mut().create_text_with_id("", a11y)
            });
            let (node, text_id) = match batched {
                Some((n, id)) => (n, Some(id)),
                None => (
                    time_backend_create(pkind!(Text), || backend.borrow_mut().create_text("", a11y)),
                    None,
                ),
            };
            // Only surface structured metadata when the binding
            // actually has any. Opaque Deriveds (closure-only
            // coercions) skip this entirely — generator backends
            // wouldn't be able to do anything useful with an empty
            // method name.
            if !d.is_opaque() {
                let mut b = backend.borrow_mut();
                for (sid, val) in d.inputs.iter().zip(d.initial.iter()) {
                    b.note_signal_initial(*sid, val);
                }
                b.note_text_binding(&node, &d.inputs, d.method);
            }
            // Scope-level cleanup: when the surrounding scope drops
            // (switch-arm flip, component unmount, owner drop), tell
            // the backend to clear the registry slot for this id.
            // Without this, every mount/unmount cycle of a reactive
            // text would leak a JS-side registry entry; over time
            // the backend's `__idealystTextRegistry` would fill with
            // dead-Node holes. `on_cleanup` here registers a
            // scope-level callback (not an effect-level one — we
            // want it firing only on teardown, not on every
            // re-fire of the effect below).
            if let Some(id) = text_id {
                let backend_for_release = backend.clone();
                crate::on_cleanup(move || {
                    backend_for_release.borrow_mut().release_text_id(id);
                });
            }
            // Pre-branch on `text_id` so each fire only runs the
            // chosen body — no per-fire `match` dispatch, no
            // unnecessary capture of `node` in the batched arm.
            // Also lets the batched arm *move* the closure's
            // `String` result straight into the backend's pending
            // buffer (via `update_text_by_id(id, value)`), saving
            // one allocation per fire vs. `&value` + internal
            // `.to_string()`.
            let compute = d.compute.clone();
            let backend_for_effect = backend.clone();
            // `Effect::new_with_stable_deps` is the right fit for
            // reactive text bindings: the closure body is a pure
            // value computation whose dep set is fixed at
            // construction time (the same `signal.get()` calls fire
            // every re-run, in the same order). The fast-path
            // re-run skips `clear_effect_dependencies` and the
            // matching re-track inside `signal.get`, which collapses
            // the per-fire HashSet churn on signals with thousands
            // of subscribers.
            let _e = match text_id {
                Some(id) => Effect::new_with_stable_deps(move || {
                    let value = (compute)();
                    backend_for_effect
                        .borrow_mut()
                        .update_text_by_id(id, value);
                }),
                None => {
                    let node_for_effect = node.clone();
                    Effect::new_with_stable_deps(move || {
                        let value = (compute)();
                        backend_for_effect
                            .borrow_mut()
                            .update_text(&node_for_effect, &value);
                    })
                }
            };
            node
        }
        TextSource::JsBinding(spec) => {
            // Fast path: backend can run the per-fire fan-out on
            // its own side (web → JS). The walker hands over the
            // structured binding and DOESN'T install a Rust Effect
            // — the per-leaf cost at fan-out time drops to whatever
            // the backend can do internally.
            //
            // Fallback: if either (a) the backend doesn't support
            // batched-text ids OR (b) it doesn't support JS-style
            // bindings, lower to the same shape as `Bound` — wrap
            // `compute_fallback` in a `Derived<String>` and re-enter
            // the Bound arm's logic via a tail call below.
            let batched = time_backend_create(pkind!(Text), || {
                backend.borrow_mut().create_text_with_id("", a11y)
            });
            let supports_js = backend.borrow().supports_js_text_bindings();
            match (batched, supports_js) {
                (Some((node, text_id)), true) => {
                    // Hand the binding over. Release on scope drop
                    // so the JS-side registry doesn't accumulate
                    // stale entries on switch-arm flips / unmounts.
                    let parts_refs: Vec<&str> =
                        spec.template_parts.iter().map(|s| s.as_str()).collect();
                    let initials_refs: Vec<&str> =
                        spec.initial_values.iter().map(|s| s.as_str()).collect();
                    backend.borrow_mut().register_reactive_text_binding(
                        text_id,
                        &spec.signal_ids,
                        &parts_refs,
                        &initials_refs,
                        &spec.stringifiers,
                    );
                    let backend_for_release = backend.clone();
                    crate::on_cleanup(move || {
                        let mut b = backend_for_release.borrow_mut();
                        b.release_reactive_text_binding(text_id);
                        b.release_text_id(text_id);
                    });
                    node
                }
                (batched_opt, _) => {
                    // Fallback path: same shape as the legacy
                    // `Bound` arm. We use `compute_fallback` as the
                    // re-fire body. The batched-text-id path still
                    // runs if available (saves per-fire FFI even on
                    // backends that don't support JS bindings);
                    // otherwise plain `update_text`.
                    let (node, text_id) = match batched_opt {
                        Some((n, id)) => (n, Some(id)),
                        None => (
                            time_backend_create(pkind!(Text), || {
                                backend.borrow_mut().create_text("", a11y)
                            }),
                            None,
                        ),
                    };
                    if let Some(id) = text_id {
                        let backend_for_release = backend.clone();
                        crate::on_cleanup(move || {
                            backend_for_release.borrow_mut().release_text_id(id);
                        });
                    }
                    let compute = spec.compute_fallback.clone();
                    let backend_for_effect = backend.clone();
                    let _e = match text_id {
                        Some(id) => Effect::new_with_stable_deps(move || {
                            let value = (compute)();
                            backend_for_effect.borrow_mut().update_text_by_id(id, value);
                        }),
                        None => {
                            let node_for_effect = node.clone();
                            Effect::new_with_stable_deps(move || {
                                let value = (compute)();
                                backend_for_effect
                                    .borrow_mut()
                                    .update_text(&node_for_effect, &value);
                            })
                        }
                    };
                    node
                }
            }
        }
    }
}
