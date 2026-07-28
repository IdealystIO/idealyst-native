//! Text-family handlers: `text`, `button`.

use runtime_scene::{Element, MountCx};
use runtime_world::{effect, Value};

use crate::caps::{ButtonOps, IntrospectionOps, TextOps};
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
/// - `JsBinding`: the f-string fast path (walker's `TextSource::JsBinding`
///   arm) — on a `supports_js_text_bindings` backend the structured
///   binding is handed over (`register_reactive_text_binding`) with ONE
///   world-root notifier effect per signal (`notify_signal_text_js`, the
///   signal-class delivery pattern) and NO per-leaf effect; elsewhere it
///   lowers to the `Dyn` shape via `compute_fallback`.
/// - `Runs`: `create_styled_text` once (basic styled runs; theme-cohort
///   re-realization is deferred — crate docs).
///
/// Then attach_style → ref-fill, matching the walker's outer sequence.
pub fn mount_text<H>(cx: &mut MountCx<'_, H>, prim: TextPrim, _children: Vec<Element>) -> H::Node
where
    H: TextOps + StyleServices + IntrospectionOps,
{
    let backend = cx.backend().clone();
    // Robot label capture, per source arm (old `robot_extract_meta`
    // shape): a mount-time snapshot for every source, plus a live
    // recompute for reactive content — the cached string would go
    // stale after the binding updates the backend (the registry entry
    // is never re-registered on signal change).
    #[cfg(feature = "robot")]
    let mut robot_label: Option<String> = None;
    #[cfg(feature = "robot")]
    let mut robot_label_fn: Option<std::rc::Rc<dyn Fn() -> Option<String>>> = None;
    let node = match prim.content {
        TextSourceProp::Value(Value::Const(content)) => {
            #[cfg(feature = "robot")]
            {
                robot_label = Some(content.clone());
            }
            backend.borrow_mut().create_text(&content, &prim.a11y)
        }
        TextSourceProp::Value(Value::Dyn(compute)) => {
            // Box → Rc so the robot's label recompute can share the
            // content closure with the binding effect (calls through
            // the Rc identically; one-time move, no behavior change).
            #[cfg(feature = "robot")]
            let compute: std::rc::Rc<dyn Fn() -> String> = std::rc::Rc::from(compute);
            #[cfg(feature = "robot")]
            {
                robot_label = Some(runtime_world::untrack(|| compute()));
                let c = compute.clone();
                // Untracked: querying the robot must never subscribe
                // the caller's scope to the content's signals.
                robot_label_fn =
                    Some(std::rc::Rc::new(move || Some(runtime_world::untrack(|| c()))));
            }
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
        TextSourceProp::JsBinding(binding) => {
            // The f-string fast path — port of `walker/text.rs`'s
            // `TextSource::JsBinding` arm. Robot label = the whole-text
            // recompute (live, untracked), every path.
            #[cfg(feature = "robot")]
            {
                let compute = binding.compute_fallback.clone();
                robot_label = Some(runtime_world::untrack(|| compute()));
                let c = binding.compute_fallback.clone();
                robot_label_fn =
                    Some(std::rc::Rc::new(move || Some(runtime_world::untrack(|| c()))));
            }
            let batched = backend.borrow_mut().create_text_with_id("", &prim.a11y);
            let supports_js = backend.borrow().supports_js_text_bindings();
            match (batched, supports_js) {
                (Some((node, text_id)), true) => {
                    // Fast path: hand the structured binding to the
                    // backend's own fan-out — NO Rust effect per leaf.
                    // World signals have no `Signal::set` JS write hook
                    // (the old delivery channel), so ensure ONE
                    // world-root notifier effect per signal that ships
                    // commits via `notify_signal_text_js` — the
                    // signal-class notifier pattern, string edition.
                    // First-registrant-wins across class+text bindings
                    // (old single-notifier semantics); installed BEFORE
                    // registration so the first fire seeds the JS-side
                    // value cache.
                    for (sid, read) in binding
                        .signal_ids
                        .iter()
                        .zip(binding.tracked_reads.iter())
                    {
                        let sid = *sid;
                        let read = read.clone();
                        let b = backend.clone();
                        crate::style_attach::ensure_signal_notifier_installed(sid, || {
                            runtime_world::unscoped(|| {
                                let _notifier = effect(move || {
                                    let value = read();
                                    b.borrow_mut().notify_signal_text_js(sid, &value);
                                });
                            });
                        });
                    }
                    {
                        let parts: Vec<&str> =
                            binding.template_parts.iter().map(|s| s.as_str()).collect();
                        let initials: Vec<&str> =
                            binding.initial_values.iter().map(|s| s.as_str()).collect();
                        backend.borrow_mut().register_reactive_text_binding(
                            text_id,
                            &binding.signal_ids,
                            &parts,
                            &initials,
                            &binding.stringifiers,
                        );
                    }
                    // Release the JS-side binding AND the id slot on
                    // teardown (the walker's on_cleanup, same order).
                    let b = backend.clone();
                    on_teardown(move || {
                        let mut bm = b.borrow_mut();
                        bm.release_reactive_text_binding(text_id);
                        bm.release_text_id(text_id);
                    });
                    node
                }
                (batched_opt, _) => {
                    // Fallback: the Bound shape, with `compute_fallback`
                    // as the effect body (batched-id path still used
                    // when available — saves per-fire FFI even without
                    // JS bindings).
                    let compute = binding.compute_fallback.clone();
                    match batched_opt {
                        Some((node, id)) => {
                            let b = backend.clone();
                            on_teardown(move || {
                                b.borrow_mut().release_text_id(id);
                            });
                            let b = backend.clone();
                            let _binding = effect(move || {
                                let value = compute();
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
            }
        }
        TextSourceProp::Runs(runs) => {
            // Styled runs are static content — report the concatenated
            // plain text, the same value the styled node renders (old
            // `label_now`'s Styled arm).
            #[cfg(feature = "robot")]
            {
                robot_label = Some(runtime_core::styled_text::plain_text_of(&runs));
            }
            backend.borrow_mut().create_styled_text(&runs, &prim.a11y)
        }
    };
    #[cfg(feature = "robot")]
    let _robot = crate::robot::register_mount(
        &backend,
        &node,
        crate::robot::ElementKind::Text,
        prim.test_id,
        robot_label,
        robot_label_fn,
        crate::robot::MountActions::default(),
    );
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
    H: ButtonOps + StyleServices + IntrospectionOps,
{
    let backend = cx.backend().clone();
    let (initial_label, dyn_label) = match prim.label {
        Value::Const(s) => (s, None),
        Value::Dyn(f) => (f(), Some(f)),
    };
    // Box → Rc so the robot's live-label recompute can share the label
    // closure with the update binding (see `mount_text`).
    #[cfg(feature = "robot")]
    let dyn_label: Option<std::rc::Rc<dyn Fn() -> String>> = dyn_label.map(std::rc::Rc::from);
    let node = backend.borrow_mut().create_button(
        &initial_label,
        &prim.on_press,
        prim.leading_icon.as_ref(),
        prim.trailing_icon.as_ref(),
        &prim.a11y,
    );
    // Robot: `click` is the action's fire (the same `Rc<dyn Fn()>`
    // runtime backends invoke on tap — old `robot_extract_meta`).
    #[cfg(feature = "robot")]
    let _robot = {
        let robot_label_fn = dyn_label.clone().map(|f| {
            std::rc::Rc::new(move || Some(runtime_world::untrack(|| f())))
                as std::rc::Rc<dyn Fn() -> Option<String>>
        });
        crate::robot::register_mount(
            &backend,
            &node,
            crate::robot::ElementKind::Button,
            prim.test_id,
            Some(initial_label.clone()),
            robot_label_fn,
            crate::robot::MountActions {
                click: Some(prim.on_press.fire.clone()),
                ..Default::default()
            },
        )
    };
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

// ===========================================================================
// Tests — the JS text-binding fast path (fallback + Value paths are
// covered by scene-parity goldens and the vocab integration suite).
// ===========================================================================

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use runtime_core::accessibility::AccessibilityProps;
    use runtime_scene::{realize, Host, Registry};
    use runtime_world::{signal, World};

    use crate::builders::{self, TextContent as _};
    use crate::glue::{__idealyst_text_from_parts, ReactiveTextSlot as _, TextSlotPart};
    use crate::prims::{PrimCell, TextPrim};

    /// Minimal TextOps host recording the JS text-binding surface.
    #[derive(Default)]
    struct JsTextHost {
        supports_js: bool,
        next_text_id: u32,
        registered: Vec<(u32, Vec<u64>, Vec<String>, Vec<String>)>,
        released_bindings: Vec<u32>,
        released_ids: Vec<u32>,
        notified: Rc<RefCell<Vec<(u64, String)>>>,
        updates_by_id: Vec<(u32, String)>,
    }

    impl Host for JsTextHost {
        type Node = u32;
        fn insert(&mut self, _p: &mut u32, _c: u32) {}
        fn insert_at(&mut self, _p: &mut u32, _c: u32, _i: usize) {}
        fn remove_child(&mut self, _p: &u32, _c: &u32) {}
        fn clear_children(&mut self, _n: &u32) {}
        fn create_anchor(&mut self) -> u32 {
            0
        }
        fn supports_splice(&self) -> bool {
            true
        }
    }
    impl crate::caps::ViewOps for JsTextHost {
        fn create_view(&mut self, _a11y: &AccessibilityProps) -> u32 {
            0
        }
    }
    impl crate::caps::DocumentOps for JsTextHost {}
    impl crate::caps::AssetOps for JsTextHost {}
    impl crate::caps::AppEnvOps for JsTextHost {}
    impl crate::caps::IntrospectionOps for JsTextHost {}
    impl crate::caps::StyleOps for JsTextHost {
        fn apply_style(&mut self, _node: &u32, _style: &Rc<runtime_core::StyleRules>) {}
    }
    impl crate::caps::TextOps for JsTextHost {
        fn create_text(&mut self, _content: &str, _a11y: &AccessibilityProps) -> u32 {
            100
        }
        fn update_text(&mut self, _node: &u32, _content: &str) {}
        fn create_text_with_id(
            &mut self,
            _content: &str,
            _a11y: &AccessibilityProps,
        ) -> Option<(u32, u32)> {
            let id = self.next_text_id;
            self.next_text_id += 1;
            Some((200 + id, id))
        }
        fn update_text_by_id(&mut self, id: u32, content: String) {
            self.updates_by_id.push((id, content));
        }
        fn release_text_id(&mut self, id: u32) {
            self.released_ids.push(id);
        }
        fn supports_js_text_bindings(&self) -> bool {
            self.supports_js
        }
        fn register_reactive_text_binding(
            &mut self,
            text_id: u32,
            signal_ids: &[u64],
            template_parts: &[&str],
            initial_values: &[&str],
            _stringifiers: &[Rc<dyn Fn() -> String>],
        ) {
            self.registered.push((
                text_id,
                signal_ids.to_vec(),
                template_parts.iter().map(|s| s.to_string()).collect(),
                initial_values.iter().map(|s| s.to_string()).collect(),
            ));
        }
        fn release_reactive_text_binding(&mut self, text_id: u32) {
            self.released_bindings.push(text_id);
        }
        fn notify_signal_text_js(&mut self, signal_id: u64, value: &str) {
            self.notified.borrow_mut().push((signal_id, value.to_string()));
        }
    }

    fn registry() -> Registry<JsTextHost> {
        let mut registry: Registry<JsTextHost> = Registry::new();
        registry.register::<PrimCell<TextPrim>, _>(|cx, p, children| {
            super::mount_text(cx, p.take(), children)
        });
        registry
    }

    fn fstring_text(sig: runtime_world::Signal<u32>) -> runtime_scene::Element {
        let assembled = __idealyst_text_from_parts(vec![
            TextSlotPart::Lit("g="),
            TextSlotPart::Slot(sig.__idealyst_text_slot(|d| format!("{d}"))),
        ]);
        builders::text().content(assembled).build()
    }

    /// Fast path (`hier_global`'s shape): the structured binding is
    /// registered ONCE, NO per-leaf effect runs on a commit — one
    /// per-signal notifier ships the formatted value; teardown releases
    /// binding + id.
    #[test]
    fn js_binding_fast_path_registers_and_fans_out_via_one_notifier() {
        let world = World::new();
        let backend = Rc::new(RefCell::new(JsTextHost {
            supports_js: true,
            ..Default::default()
        }));
        let registry = Rc::new(registry());
        let (sig, realized) = world.enter(|| {
            let sig = signal(0u32);
            let realized = realize(
                &backend,
                &registry,
                runtime_scene::fragment(vec![fstring_text(sig), fstring_text(sig)]),
            );
            (sig, realized)
        });
        {
            let b = backend.borrow();
            assert_eq!(b.registered.len(), 2, "one registration per leaf");
            let (text_id, ids, parts, initials) = &b.registered[0];
            assert_eq!(*text_id, 0);
            assert_eq!(ids, &vec![sig.raw_id()]);
            assert_eq!(parts, &vec!["g=".to_string(), String::new()]);
            assert_eq!(initials, &vec!["0".to_string()]);
            // Seeding: the (deduped) notifier's creation run shipped the
            // initial value once for BOTH leaves.
            assert_eq!(&*b.notified.borrow(), &vec![(sig.raw_id(), "0".to_string())]);
            assert!(b.updates_by_id.is_empty(), "no per-leaf effect on the fast path");
        }

        // One commit → exactly ONE notify (not per leaf), and still no
        // Rust-side text updates.
        world.enter(|| sig.set(7));
        world.flush();
        {
            let b = backend.borrow();
            assert_eq!(
                &*b.notified.borrow(),
                &vec![
                    (sig.raw_id(), "0".to_string()),
                    (sig.raw_id(), "7".to_string()),
                ],
                "shared-signal fan-out is ONE notify per commit"
            );
            assert!(b.updates_by_id.is_empty());
        }

        // Teardown: release binding then id, per leaf.
        drop(realized);
        let b = backend.borrow();
        assert_eq!(b.released_bindings, vec![0, 1]);
        assert_eq!(b.released_ids, vec![0, 1]);
    }

    /// Without JS-binding support the SAME source lowers to the Bound
    /// shape: batched-id effect via `compute_fallback` (reactivity
    /// identical, delivery via effect).
    #[test]
    fn js_binding_falls_back_to_bound_effect_without_support() {
        let world = World::new();
        let backend = Rc::new(RefCell::new(JsTextHost {
            supports_js: false,
            ..Default::default()
        }));
        let registry = Rc::new(registry());
        let (sig, _realized) = world.enter(|| {
            let sig = signal(0u32);
            let realized = realize(&backend, &registry, fstring_text(sig));
            (sig, realized)
        });
        assert_eq!(
            backend.borrow().updates_by_id,
            vec![(0, "g=0".to_string())],
            "fallback effect's first fire renders the whole text"
        );
        assert!(backend.borrow().registered.is_empty(), "no JS registration");
        world.enter(|| sig.set(3));
        world.flush();
        assert_eq!(
            backend.borrow().updates_by_id.last(),
            Some(&(0, "g=3".to_string()))
        );
    }
}
