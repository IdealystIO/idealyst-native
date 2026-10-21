//! Form-control handlers: `toggle`, `slider`, `activity_indicator`,
//! `text_input`, `text_area`.

use std::cell::Cell;
use std::rc::Rc;

use runtime_scene::{Element, MountCx};
use runtime_world::Value;

use crate::caps::{ActivityIndicatorOps, IntrospectionOps, SliderOps, TextInputOps, ToggleOps};
use crate::prims::{ActivityIndicatorPrim, SliderPrim, TextAreaPrim, TextInputPrim, TogglePrim};
use crate::style_attach::{attach_style, StyleServices};

use super::{bind_dyn, bind_value, initial_of};

/// Mount a `toggle`.
///
/// Sequence: `create_toggle(initial, on_change)` → attach_style →
/// controlled-value write-back binding (one `update_toggle_value` at
/// mount, then per change — the widget no-ops on same-value sets, so
/// the round-trip is stable) → disabled binding (gate + `set_disabled`
/// + `DISABLED` bit, same contract as `mount_text_input`) → ref-fill.
pub fn mount_toggle<H>(cx: &mut MountCx<'_, H>, prim: TogglePrim, _children: Vec<Element>) -> H::Node
where
    H: ToggleOps + StyleServices + IntrospectionOps,
{
    let backend = cx.backend().clone();
    let initial = initial_of(&prim.value);
    let (on_change, disabled_gate) = gate_change(&prim.disabled, prim.on_change);
    // What the switch shows, for the robot's `checked`: every change the
    // native widget reports, and every controlled write-back below.
    #[cfg(feature = "robot")]
    let robot_checked = Rc::new(Cell::new(initial));
    #[cfg(feature = "robot")]
    let on_change = observe(&robot_checked, on_change);
    // Scope-guard every author callback before the backend stores it —
    // see `callback_guard` for the abort this prevents.
    let alive = crate::callback_guard::ScopeAlive::current();
    let native_on_change = alive.wrap1(on_change);
    #[cfg(feature = "robot")]
    let robot_native_on_change = native_on_change.clone();
    let node = backend
        .borrow_mut()
        .create_toggle(initial, native_on_change, &prim.a11y);
    // Robot `set_toggle`: a flip as a user makes it — the native switch
    // moves, then its own change callback (disabled gate, scope guard)
    // reports it. The robot refuses a disabled toggle before this runs.
    #[cfg(feature = "robot")]
    let _robot = {
        let set_toggle: Rc<dyn Fn(bool)> = {
            let b = backend.clone();
            let n = node.clone();
            Rc::new(move |v| {
                b.borrow_mut().update_toggle_value(&n, v);
                robot_native_on_change(v);
            })
        };
        let state = {
            let checked = robot_checked.clone();
            let gate = disabled_gate.clone();
            Rc::new(move || crate::robot::ElementState {
                checked: Some(checked.get()),
                disabled: Some(gate_is_up(&gate)),
                ..Default::default()
            }) as Rc<dyn Fn() -> crate::robot::ElementState>
        };
        crate::robot::register_mount(
            &backend,
            &node,
            crate::robot::ElementKind::Toggle,
            prim.test_id,
            None,
            None,
            crate::robot::MountActions {
                set_toggle: Some(set_toggle),
                state: Some(state),
                ..Default::default()
            },
        )
    };
    let state_setter = prim.style.map(|style| {
        let setter = attach_style(&backend, &node, style);
        #[cfg(feature = "robot")]
        crate::robot::link_interaction(&_robot);
        setter
    });
    {
        let b = backend.clone();
        let n = node.clone();
        #[cfg(feature = "robot")]
        let checked = robot_checked.clone();
        bind_value(prim.value, move |&v| {
            #[cfg(feature = "robot")]
            checked.set(v);
            b.borrow_mut().update_toggle_value(&n, v);
        });
    }
    bind_disabled(&backend, &node, prim.disabled, disabled_gate, state_setter);
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_toggle_handle(&node);
        fill(handle);
    }
    node
}

/// Mount a `slider`, including the step-snap wrapper: the user's `on_change` receives values snapped to
/// `step` before dispatch, so every backend produces identical values
/// regardless of native step handling. `disabled` binds after the value
/// write-back, same contract as `mount_toggle`.
pub fn mount_slider<H>(cx: &mut MountCx<'_, H>, prim: SliderPrim, _children: Vec<Element>) -> H::Node
where
    H: SliderOps + StyleServices + IntrospectionOps,
{
    let backend = cx.backend().clone();
    let initial = initial_of(&prim.value);
    let on_change_snapped: Rc<dyn Fn(f32)> = if prim.step.is_some() {
        let user = prim.on_change.clone();
        let (min, step) = (prim.min, prim.step);
        Rc::new(move |v| user(snap(v, min, step)))
    } else {
        prim.on_change.clone()
    };
    let (on_change_snapped, disabled_gate) = gate_change(&prim.disabled, on_change_snapped);
    // The value the slider holds, for the robot's `value`: each change the
    // widget reports, snapped to `step` as the author receives it, and
    // every controlled write-back below.
    #[cfg(feature = "robot")]
    let robot_value = Rc::new(Cell::new(initial));
    #[cfg(feature = "robot")]
    let on_change_snapped = {
        let value = robot_value.clone();
        let (min, step) = (prim.min, prim.step);
        let inner = on_change_snapped;
        Rc::new(move |v: f32| {
            value.set(snap(v, min, step));
            inner(v);
        }) as Rc<dyn Fn(f32)>
    };
    let alive = crate::callback_guard::ScopeAlive::current();
    let native_on_change = alive.wrap1(on_change_snapped);
    #[cfg(feature = "robot")]
    let robot_native_on_change = native_on_change.clone();
    let node = backend.borrow_mut().create_slider(
        initial,
        prim.min,
        prim.max,
        prim.step,
        native_on_change,
        &prim.a11y,
    );
    // Robot `set_slider` and `drag` move the thumb as a finger would: the
    // value is clamped to the track and snapped to `step`, the native
    // thumb moves (`update_slider_value`, even for an uncontrolled
    // slider), then the EXACT callback the native widget was given (step
    // snap, disabled gate, scope guard) reports it — so a robot-set
    // slider produces the values a real drag would. A disabled slider
    // refuses (`false`): the native control is inert, so neither the thumb
    // nor `on_change` moves.
    #[cfg(feature = "robot")]
    let _robot = {
        let apply: Rc<dyn Fn(f32) -> bool> = {
            let b = backend.clone();
            let n = node.clone();
            let gate = disabled_gate.clone();
            let (min, max, step) = (prim.min, prim.max, prim.step);
            Rc::new(move |raw: f32| {
                if gate_is_up(&gate) {
                    return false;
                }
                let value = snap(raw.clamp(min.min(max), max.max(min)), min, step);
                b.borrow_mut().update_slider_value(&n, value);
                robot_native_on_change(value);
                true
            })
        };
        let set_slider: Rc<dyn Fn(f32)> = {
            let apply = apply.clone();
            Rc::new(move |v| {
                apply(v);
            })
        };
        let drag: Rc<dyn Fn(f32) -> bool> = {
            let (min, max) = (prim.min, prim.max);
            Rc::new(move |fraction: f32| apply(min + fraction.clamp(0.0, 1.0) * (max - min)))
        };
        let state = {
            let value = robot_value.clone();
            let gate = disabled_gate.clone();
            Rc::new(move || crate::robot::ElementState {
                value: Some(crate::robot::StateValue::Number(value.get() as f64)),
                disabled: Some(gate_is_up(&gate)),
                ..Default::default()
            }) as Rc<dyn Fn() -> crate::robot::ElementState>
        };
        crate::robot::register_mount(
            &backend,
            &node,
            crate::robot::ElementKind::Slider,
            prim.test_id,
            None,
            None,
            crate::robot::MountActions {
                set_slider: Some(set_slider),
                slider_drag: Some(drag),
                state: Some(state),
                ..Default::default()
            },
        )
    };
    let state_setter = prim.style.map(|style| {
        let setter = attach_style(&backend, &node, style);
        #[cfg(feature = "robot")]
        crate::robot::link_interaction(&_robot);
        setter
    });
    {
        let b = backend.clone();
        let n = node.clone();
        #[cfg(feature = "robot")]
        let value = robot_value.clone();
        bind_value(prim.value, move |&v| {
            #[cfg(feature = "robot")]
            value.set(v);
            b.borrow_mut().update_slider_value(&n, v);
        });
    }
    bind_disabled(&backend, &node, prim.disabled, disabled_gate, state_setter);
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_slider_handle(&node);
        fill(handle);
    }
    node
}

/// Mount an `activity_indicator`. A `Dyn` size creates at the
/// closure's initial value and installs the resize binding (first fire
/// at mount).
pub fn mount_activity_indicator<H>(
    cx: &mut MountCx<'_, H>,
    prim: ActivityIndicatorPrim,
    _children: Vec<Element>,
) -> H::Node
where
    H: ActivityIndicatorOps + StyleServices + IntrospectionOps,
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
    // Passive/visual primitive: no actions, but findable by test_id +
    // kind so an E2E suite can assert "this rendered" (the conformance
    // app's `static primitives render` check).
    #[cfg(feature = "robot")]
    let _robot = crate::robot::register_mount(
        &backend,
        &node,
        crate::robot::ElementKind::ActivityIndicator,
        prim.test_id,
        None,
        None,
        crate::robot::MountActions::default(),
    );
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
        #[cfg(feature = "robot")]
        crate::robot::link_interaction(&_robot);
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

/// `v` snapped to the `step` grid anchored at `min` (unchanged without a
/// step) — the one snap the change callback, robot sets and drags share.
fn snap(v: f32, min: f32, step: Option<f32>) -> f32 {
    match step {
        Some(step) if step > 0.0 => min + ((v - min) / step).round() * step,
        _ => v,
    }
}

/// Whether a control's disabled gate is up (`None`: it has no `disabled`).
#[cfg(feature = "robot")]
fn gate_is_up(gate: &Option<Rc<Cell<bool>>>) -> bool {
    gate.as_ref().is_some_and(|g| g.get())
}

/// `on_change`, first recording each reported value in `cell` — the
/// robot's view of what the control shows.
#[cfg(feature = "robot")]
fn observe<T: Copy + 'static>(cell: &Rc<Cell<T>>, on_change: Rc<dyn Fn(T)>) -> Rc<dyn Fn(T)> {
    let cell = cell.clone();
    Rc::new(move |v: T| {
        cell.set(v);
        on_change(v);
    })
}

/// [`observe`] for a text control's `String` value.
#[cfg(feature = "robot")]
fn observe_text(cell: &Rc<std::cell::RefCell<String>>, on_change: Rc<dyn Fn(String)>) -> Rc<dyn Fn(String)> {
    let cell = cell.clone();
    Rc::new(move |v: String| {
        *cell.borrow_mut() = v.clone();
        on_change(v);
    })
}

/// The `disabled` wiring a form control (`text_input`, `text_area`,
/// `toggle`, `slider`) shares with `pressable`'s
/// press-block (see `mount_pressable`): wrap `on_change` so it is dropped
/// while the gate is up. Returns the (possibly wrapped) callback and the
/// gate, `None` when the author attached no `disabled` (zero cost then).
///
/// The gate is what makes "a disabled control never reports a change" hold
/// on EVERY backend: native `set_disabled` already stops typing where the
/// toolkit has an inert state, but a backend with none (or an edit that
/// was in flight when the field went inert) would otherwise still reach
/// the author.
fn gate_change<T: 'static>(
    disabled: &Option<Value<bool>>,
    on_change: Rc<dyn Fn(T)>,
) -> (Rc<dyn Fn(T)>, Option<Rc<Cell<bool>>>) {
    if disabled.is_none() {
        return (on_change, None);
    }
    let gate = Rc::new(Cell::new(false));
    let g = gate.clone();
    let wrapped: Rc<dyn Fn(T)> = Rc::new(move |v: T| {
        if !g.get() {
            on_change(v);
        }
    });
    (wrapped, Some(gate))
}

/// Bind a form control's `disabled` once the node exists — the same
/// order `mount_pressable` uses: the change gate, the backend's native
/// inert state (`set_disabled`: not editable / flippable / draggable AND
/// not keyboard-focusable, which also drops focus if the control held it), then the `DISABLED`
/// state bit so a `state disabled { … }` overlay resolves. Fires once at
/// mount and then per change of a live source.
fn bind_disabled<H: StyleServices + 'static>(
    backend: &Rc<std::cell::RefCell<H>>,
    node: &H::Node,
    disabled: Option<Value<bool>>,
    gate: Option<Rc<Cell<bool>>>,
    state_setter: Option<Rc<dyn Fn(runtime_shared::StateBits, bool)>>,
) where
    H::Node: Clone + 'static,
{
    let (Some(disabled), Some(gate)) = (disabled, gate) else {
        return;
    };
    let b = backend.clone();
    let n = node.clone();
    bind_value(disabled, move |&d| {
        gate.set(d);
        b.borrow_mut().set_disabled(&n, d);
        if let Some(setter) = state_setter.as_ref() {
            setter(runtime_shared::StateBits::DISABLED, d);
        }
    });
}

/// The robot reports a text field by its accessible name — the author's
/// a11y label, else the placeholder, the same fallback the browser's
/// accessible-name computation uses. Without it every input came back
/// `label: null`, and a form's fields could only be told apart from a
/// screenshot.
///
/// Mount a `text_input`.
///
/// Sequence: `create_text_input(initial value/placeholder/secure,
/// callbacks)` → keyboard hint (`update_text_input_keyboard`: one call
/// for a static non-`Default` kind, an effect for a live one) →
/// attach_style → focus notifier → controlled-value
/// write-back binding (first fire at mount) → `Dyn`-only secure binding
/// → `Dyn`-only placeholder binding → disabled binding (gate +
/// `set_disabled` + `DISABLED` bit) → `autofocus` (skipped when mounted
/// disabled) → ref-fill.
pub fn mount_text_input<H>(
    cx: &mut MountCx<'_, H>,
    #[cfg_attr(not(feature = "robot"), allow(unused_mut))] mut prim: TextInputPrim,
    _children: Vec<Element>,
) -> H::Node
where
    H: TextInputOps + StyleServices + IntrospectionOps,
{
    let backend = cx.backend().clone();
    let initial_value = initial_of(&prim.value);
    let initial_secure = initial_of(&prim.secure);
    let initial_placeholder = initial_of(&prim.placeholder);
    #[cfg(feature = "robot")]
    let (robot_label, robot_label_fn) = match prim.a11y.label.clone() {
        Some(label) => (Some(label), None),
        // A live placeholder is shared with its binding below so the
        // reported label tracks it.
        None => match std::mem::replace(&mut prim.placeholder, Value::Const(None)) {
            Value::Const(p) => {
                prim.placeholder = Value::Const(p.clone());
                (p, None)
            }
            Value::Dyn(f) => {
                let f: Rc<dyn Fn() -> Option<String>> = Rc::from(f);
                let for_binding = f.clone();
                prim.placeholder = Value::Dyn(Box::new(move || for_binding()));
                let live: Rc<dyn Fn() -> Option<String>> =
                    Rc::new(move || runtime_world::untrack(|| f()));
                (initial_placeholder.clone(), Some(live))
            }
        },
    };
    let (on_change, disabled_gate) = gate_change(&prim.disabled, prim.on_change);
    // The text the field shows, for the robot's `value`: every edit the
    // field reports, and every controlled write-back below.
    #[cfg(feature = "robot")]
    let robot_text = Rc::new(std::cell::RefCell::new(initial_value.clone()));
    #[cfg(feature = "robot")]
    let on_change = observe_text(&robot_text, on_change);
    let alive = crate::callback_guard::ScopeAlive::current();
    let native_on_change = alive.wrap1(on_change);
    #[cfg(feature = "robot")]
    let robot_native_on_change = native_on_change.clone();
    let node = backend.borrow_mut().create_text_input(
        &initial_value,
        initial_placeholder.as_deref(),
        native_on_change,
        prim.on_key_down.map(|f| alive.wrap_key(f)),
        prim.on_blur.map(|f| alive.wrap_blur(f)),
        initial_secure,
        &prim.a11y,
    );
    // Soft-keyboard hint, before anything can focus the field (a keyboard
    // kind set after the keyboard is already up needs a reload on iOS).
    // A static kind is one call, skipped entirely for `Default` (what every
    // backend creates with); a live kind binds an effect that applies the
    // initial value now and re-applies per change.
    match prim.keyboard {
        Value::Const(runtime_shared::primitives::text_input::KeyboardKind::Default) => {}
        Value::Const(kind) => backend.borrow_mut().update_text_input_keyboard(&node, kind),
        live @ Value::Dyn(_) => {
            let b = backend.clone();
            let n = node.clone();
            bind_dyn(live, move |&k| {
                b.borrow_mut().update_text_input_keyboard(&n, k);
            });
        }
    }
    // Editable inputs also get `focus`/`blur` so the robot can drive
    // real keyboard focus. Handles are made ON DEMAND from the backend
    // node at call time.
    // Robot `type_text` replaces the text as a paste would: the field
    // shows it, then its own change callback (disabled gate, scope guard)
    // reports it.
    #[cfg(feature = "robot")]
    let robot_focused = Rc::new(Cell::new(false));
    #[cfg(feature = "robot")]
    let _robot = {
        let focus = {
            let b = backend.clone();
            let n = node.clone();
            Rc::new(move || b.borrow().make_text_input_handle(&n).focus()) as Rc<dyn Fn()>
        };
        let blur = {
            let b = backend.clone();
            let n = node.clone();
            Rc::new(move || b.borrow().make_text_input_handle(&n).blur()) as Rc<dyn Fn()>
        };
        let set_text: Rc<dyn Fn(String)> = {
            let b = backend.clone();
            let n = node.clone();
            Rc::new(move |text: String| {
                b.borrow_mut().update_text_input_value(&n, &text);
                robot_native_on_change(text);
            })
        };
        let state = {
            let text = robot_text.clone();
            let focused = robot_focused.clone();
            let gate = disabled_gate.clone();
            Rc::new(move || crate::robot::ElementState {
                value: Some(crate::robot::StateValue::Text(text.borrow().clone())),
                disabled: Some(gate_is_up(&gate)),
                focused: Some(focused.get()),
                ..Default::default()
            }) as Rc<dyn Fn() -> crate::robot::ElementState>
        };
        crate::robot::register_mount(
            &backend,
            &node,
            crate::robot::ElementKind::TextInput,
            prim.test_id,
            robot_label,
            robot_label_fn,
            crate::robot::MountActions {
                set_text: Some(set_text),
                focus: Some(focus),
                blur: Some(blur),
                state: Some(state),
                ..Default::default()
            },
        )
    };
    let state_setter = prim.style.map(|style| {
        let setter = attach_style(&backend, &node, style);
        #[cfg(feature = "robot")]
        crate::robot::link_interaction(&_robot);
        setter
    });
    // Robot builds always watch focus (for `focused`), passing each change
    // on to the author's `on_focus` when there is one.
    #[cfg(feature = "robot")]
    let on_focus = {
        let focused = robot_focused.clone();
        let author = prim.on_focus;
        Some(Rc::new(move |f: bool| {
            focused.set(f);
            if let Some(author) = author.as_ref() {
                author(f);
            }
        }) as Rc<dyn Fn(bool)>)
    };
    #[cfg(not(feature = "robot"))]
    let on_focus = prim.on_focus;
    // Guarded like the other callbacks: removing a focused input fires its
    // blur DURING the teardown that frees the scope (the web does this
    // synchronously on DOM removal), and the author's handler writes that
    // scope's signals.
    if let Some(on_focus) = on_focus {
        backend
            .borrow_mut()
            .set_text_input_focus_handler(&node, alive.wrap1(on_focus));
    }
    {
        let b = backend.clone();
        let n = node.clone();
        #[cfg(feature = "robot")]
        let text = robot_text.clone();
        bind_value(prim.value, move |v| {
            #[cfg(feature = "robot")]
            {
                *text.borrow_mut() = v.clone();
            }
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
    let mounts_disabled = {
        let gate = disabled_gate.clone();
        bind_disabled(&backend, &node, prim.disabled, disabled_gate, state_setter);
        gate.is_some_and(|g| g.get())
    };
    // `autofocus`: one focus through the handle's own ops, after the
    // value/style/focus-notifier wiring so `on_focus(true)` reaches the
    // author and the field shows its value. No timer — `focus()` is
    // attach-safe by contract (`TextInputOps::focus`), so a node realize
    // hasn't attached yet (a portal, presence) focuses when it lands.
    // Before the ref fill so an author's `on_handle` that blurs wins. The
    // borrow ends before `focus()` runs: a backend that fires its focus
    // notifier synchronously may re-enter the backend. A field that mounts
    // disabled is not focusable, so it does not take the autofocus either
    // (web's `<input disabled autofocus>` doesn't; native toolkits would).
    if prim.autofocus && !mounts_disabled {
        let handle = backend.borrow().make_text_input_handle(&node);
        handle.focus();
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_text_input_handle(&node);
        fill(handle);
    }
    node
}

/// Mount a `text_area`. Placeholder/wrap/rows are
/// create-time config; the controlled `value` write-back binding fires
/// once at mount then per change, then the `disabled` binding (same
/// contract as `mount_text_input`).
pub fn mount_text_area<H>(
    cx: &mut MountCx<'_, H>,
    prim: TextAreaPrim,
    _children: Vec<Element>,
) -> H::Node
where
    H: TextInputOps + StyleServices + IntrospectionOps,
{
    let backend = cx.backend().clone();
    let initial_value = initial_of(&prim.value);
    let (on_change, disabled_gate) = gate_change(&prim.disabled, prim.on_change);
    #[cfg(feature = "robot")]
    let robot_text = Rc::new(std::cell::RefCell::new(initial_value.clone()));
    #[cfg(feature = "robot")]
    let on_change = observe_text(&robot_text, on_change);
    let alive = crate::callback_guard::ScopeAlive::current();
    let native_on_change = alive.wrap1(on_change);
    #[cfg(feature = "robot")]
    let robot_native_on_change = native_on_change.clone();
    let node = backend.borrow_mut().create_text_area(
        &initial_value,
        prim.placeholder.as_deref(),
        prim.wrap,
        prim.min_rows,
        prim.max_rows,
        native_on_change,
        prim.on_key_down.map(|f| alive.wrap_key(f)),
        &prim.a11y,
    );
    // `ElementKind::TextInput`: the robot surface doesn't distinguish
    // single- vs multi-line. `type_text` replaces the text as for
    // `text_input`; `focus` / `blur` go through the area's handle. A text
    // area has no focus
    // notifier, so its state carries no `focused`.
    #[cfg(feature = "robot")]
    let _robot = {
        let focus = {
            let b = backend.clone();
            let n = node.clone();
            Rc::new(move || b.borrow().make_text_area_handle(&n).focus()) as Rc<dyn Fn()>
        };
        let blur = {
            let b = backend.clone();
            let n = node.clone();
            Rc::new(move || b.borrow().make_text_area_handle(&n).blur()) as Rc<dyn Fn()>
        };
        let set_text: Rc<dyn Fn(String)> = {
            let b = backend.clone();
            let n = node.clone();
            Rc::new(move |text: String| {
                b.borrow_mut().update_text_area_value(&n, &text);
                robot_native_on_change(text);
            })
        };
        let state = {
            let text = robot_text.clone();
            let gate = disabled_gate.clone();
            Rc::new(move || crate::robot::ElementState {
                value: Some(crate::robot::StateValue::Text(text.borrow().clone())),
                disabled: Some(gate_is_up(&gate)),
                ..Default::default()
            }) as Rc<dyn Fn() -> crate::robot::ElementState>
        };
        crate::robot::register_mount(
            &backend,
            &node,
            crate::robot::ElementKind::TextInput,
            prim.test_id,
            // Accessible name, as for `text_input`.
            prim.a11y.label.clone().or_else(|| prim.placeholder.clone()),
            None,
            crate::robot::MountActions {
                set_text: Some(set_text),
                focus: Some(focus),
                blur: Some(blur),
                state: Some(state),
                ..Default::default()
            },
        )
    };
    let state_setter = prim.style.map(|style| {
        let setter = attach_style(&backend, &node, style);
        #[cfg(feature = "robot")]
        crate::robot::link_interaction(&_robot);
        setter
    });
    {
        let b = backend.clone();
        let n = node.clone();
        #[cfg(feature = "robot")]
        let text = robot_text.clone();
        bind_value(prim.value, move |v| {
            #[cfg(feature = "robot")]
            {
                *text.borrow_mut() = v.clone();
            }
            b.borrow_mut().update_text_area_value(&n, v);
        });
    }
    let mounts_disabled = {
        let gate = disabled_gate.clone();
        bind_disabled(&backend, &node, prim.disabled, disabled_gate, state_setter);
        gate.is_some_and(|g| g.get())
    };
    // `autofocus`: one focus through the handle's own ops, after the
    // value/style/focus-notifier wiring so `on_focus(true)` reaches the
    // author and the field shows its value. No timer — `focus()` is
    // attach-safe by contract (`TextInputOps::focus`), so a node realize
    // hasn't attached yet (a portal, presence) focuses when it lands.
    // Before the ref fill so an author's `on_handle` that blurs wins. The
    // borrow ends before `focus()` runs: a backend that fires its focus
    // notifier synchronously may re-enter the backend. Skipped when the
    // area mounts disabled — see `mount_text_input`.
    if prim.autofocus && !mounts_disabled {
        let handle = backend.borrow().make_text_area_handle(&node);
        handle.focus();
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_text_area_handle(&node);
        fill(handle);
    }
    node
}
