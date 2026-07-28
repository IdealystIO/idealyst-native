//! P2b unit tests: builder coercions, `StyleProp` paths, prop-binding
//! shapes, teardown probes, and the mount-once payload contract —
//! against a minimal recording backend bridged through `LegacyBridge`
//! (the full-op *sequence* parity lives in `scene-parity`'s golden
//! suite; these tests pin the vocabulary-local behaviors in isolation).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{Backend, StateBits, StyleRules, Tokenized};
use runtime_scene::{realize, Registry};
use runtime_vocabulary::builders::{
    button, icon, image, link, pressable, slider, text, toggle, view, TextContent,
};
use runtime_vocabulary::style_attach::IntoStyleProp;
use runtime_vocabulary::{register_builtins, LegacyBridge, StyleProp};
use runtime_world::{signal, Value, World};

// ===========================================================================
// Minimal recording backend
// ===========================================================================

type Log = Rc<RefCell<Vec<String>>>;

struct Mini {
    log: Log,
    next: u32,
    /// State setters handed to `attach_states`, so tests can flip
    /// interaction states like a native event source would.
    state_setters: Rc<RefCell<Vec<Rc<dyn Fn(StateBits, bool)>>>>,
    /// on_change callbacks the backend received (slider snap test).
    slider_changes: Rc<RefCell<Vec<Rc<dyn Fn(f32)>>>>,
    /// on_click callbacks the backend received (pressable block test).
    press_handlers: Rc<RefCell<Vec<Rc<dyn Fn()>>>>,
}

impl Mini {
    fn mint(&mut self, kind: &str) -> u32 {
        let n = self.next;
        self.next += 1;
        self.log.borrow_mut().push(format!("create n{n} {kind}"));
        n
    }
}

fn width_of(rules: &StyleRules) -> String {
    rules
        .width
        .as_ref()
        .map(|w| format!("{w:?}"))
        .unwrap_or_else(|| "none".into())
}

impl Backend for Mini {
    type Node = u32;

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> u32 {
        self.mint("view")
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> u32 {
        let n = self.next;
        self.next += 1;
        self.log
            .borrow_mut()
            .push(format!("create n{n} text {content:?}"));
        n
    }

    fn create_button(
        &mut self,
        label: &str,
        _on_click: &runtime_core::Action,
        _leading: Option<&runtime_core::IconData>,
        _trailing: Option<&runtime_core::IconData>,
        _a11y: &AccessibilityProps,
    ) -> u32 {
        let n = self.next;
        self.next += 1;
        self.log
            .borrow_mut()
            .push(format!("create n{n} button {label:?}"));
        n
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        _on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> u32 {
        let n = self.next;
        self.next += 1;
        self.log
            .borrow_mut()
            .push(format!("create n{n} toggle {initial_value}"));
        n
    }

    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, _a11y: &AccessibilityProps) -> u32 {
        self.press_handlers.borrow_mut().push(on_click);
        self.mint("pressable")
    }

    fn set_disabled(&mut self, node: &u32, disabled: bool) {
        self.log
            .borrow_mut()
            .push(format!("set_disabled n{node} {disabled}"));
    }

    fn create_slider(
        &mut self,
        _initial: f32,
        _min: f32,
        _max: f32,
        _step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> u32 {
        self.slider_changes.borrow_mut().push(on_change);
        self.mint("slider")
    }

    fn update_text(&mut self, node: &u32, content: &str) {
        self.log
            .borrow_mut()
            .push(format!("update_text n{node} {content:?}"));
    }

    fn update_button_label(&mut self, node: &u32, label: &str) {
        self.log
            .borrow_mut()
            .push(format!("update_button_label n{node} {label:?}"));
    }

    fn update_toggle_value(&mut self, node: &u32, value: bool) {
        self.log
            .borrow_mut()
            .push(format!("update_toggle_value n{node} {value}"));
    }

    fn apply_style(&mut self, node: &u32, style: &Rc<StyleRules>) {
        self.log
            .borrow_mut()
            .push(format!("apply_style n{node} width={}", width_of(style)));
    }

    fn on_node_unstyled(&mut self, node: &u32) {
        self.log.borrow_mut().push(format!("on_node_unstyled n{node}"));
    }

    fn attach_states(&mut self, node: &u32, setter: Rc<dyn Fn(StateBits, bool)>) {
        self.state_setters.borrow_mut().push(setter);
        self.log.borrow_mut().push(format!("attach_states n{node}"));
    }

    fn insert(&mut self, parent: &mut u32, child: u32) {
        self.log
            .borrow_mut()
            .push(format!("insert n{parent} <- n{child}"));
    }

    fn clear_children(&mut self, node: &u32) {
        self.log.borrow_mut().push(format!("clear_children n{node}"));
    }

    fn finish(&mut self, _root: u32) {}
}

struct Harness {
    world: World,
    backend: Rc<RefCell<LegacyBridge<Mini>>>,
    registry: Rc<Registry<LegacyBridge<Mini>>>,
    log: Log,
    state_setters: Rc<RefCell<Vec<Rc<dyn Fn(StateBits, bool)>>>>,
    slider_changes: Rc<RefCell<Vec<Rc<dyn Fn(f32)>>>>,
    press_handlers: Rc<RefCell<Vec<Rc<dyn Fn()>>>>,
}

fn harness() -> Harness {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let state_setters = Rc::new(RefCell::new(Vec::new()));
    let slider_changes = Rc::new(RefCell::new(Vec::new()));
    let press_handlers = Rc::new(RefCell::new(Vec::new()));
    let backend = Rc::new(RefCell::new(LegacyBridge(Mini {
        log: log.clone(),
        next: 0,
        state_setters: state_setters.clone(),
        slider_changes: slider_changes.clone(),
        press_handlers: press_handlers.clone(),
    })));
    let mut registry = Registry::new();
    register_builtins(&mut registry);
    Harness {
        world: World::new(),
        backend,
        registry: Rc::new(registry),
        log,
        state_setters,
        slider_changes,
        press_handlers,
    }
}

impl Harness {
    fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.borrow_mut())
    }
}

fn px(w: f32) -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(runtime_core::Length::Px(w))),
        ..Default::default()
    }
}

// ===========================================================================
// TextContent / IntoValue coercions
// ===========================================================================

#[test]
fn text_content_static_forms_are_const() {
    assert!(matches!("hi".into_content(), Value::Const(s) if s == "hi"));
    assert!(matches!(String::from("hi").into_content(), Value::Const(s) if s == "hi"));
}

#[test]
fn text_content_closure_of_to_string_is_dyn() {
    let v = (|| 42).into_content();
    match v {
        Value::Dyn(f) => assert_eq!(f(), "42"),
        Value::Const(_) => panic!("closure must coerce to Dyn"),
    }
}

#[test]
fn text_content_signal_is_dyn_through_get() {
    let world = World::new();
    world.enter(|| {
        let s = signal(7i32);
        let v = s.into_content();
        match v {
            Value::Dyn(f) => assert_eq!(f(), "7"),
            Value::Const(_) => panic!("signal must coerce to Dyn"),
        }
    });
}

#[test]
fn style_prop_coercions() {
    assert!(matches!(px(1.0).into_style_prop(), StyleProp::Static(_)));
    assert!(matches!(Rc::new(px(1.0)).into_style_prop(), StyleProp::Static(_)));
    assert!(matches!((|| px(1.0)).into_style_prop(), StyleProp::Dynamic(_)));
    assert!(matches!(
        (|| Rc::new(px(1.0))).into_style_prop(),
        StyleProp::Dynamic(_)
    ));
}

// ===========================================================================
// StyleProp paths
// ===========================================================================

#[test]
fn static_style_applies_once_and_releases_on_teardown() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view().style(px(120.0)).build(),
        )
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 view".to_string(),
            "apply_style n0 width=Literal(Px(120.0))".to_string(),
        ],
        "static path: exactly one apply, no attach_states"
    );
    drop(realized);
    assert_eq!(
        h.take_log(),
        vec!["on_node_unstyled n0".to_string()],
        "teardown fires on_node_unstyled exactly once"
    );
}

#[test]
fn dynamic_style_reapplies_on_signal_change_and_state_flip() {
    let h = harness();
    let world = h.world.clone();
    let (realized, wide) = world.enter(|| {
        let wide = signal(false);
        let realized = realize(
            &h.backend,
            &h.registry,
            view()
                .style(move || if wide.get() { px(300.0) } else { px(100.0) })
                .build(),
        );
        (realized, wide)
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 view".to_string(),
            "apply_style n0 width=Literal(Px(100.0))".to_string(),
            "attach_states n0".to_string(),
        ],
        "dynamic path: initial apply + state hookup"
    );

    // Signal change re-applies with the new rules.
    wide.set(true);
    world.flush();
    assert_eq!(
        h.take_log(),
        vec!["apply_style n0 width=Literal(Px(300.0))".to_string()]
    );

    // A native state flip (through the attach_states setter) re-fires
    // the binding — the re-apply contract the old event-driven path
    // has. (Overlay resolution is deferred; the rules are unchanged.)
    let setter = h.state_setters.borrow()[0].clone();
    setter(StateBits::HOVERED, true);
    world.flush();
    assert_eq!(
        h.take_log(),
        vec!["apply_style n0 width=Literal(Px(300.0))".to_string()],
        "state flip re-applies via the binding effect"
    );

    drop(realized);
    assert_eq!(h.take_log(), vec!["on_node_unstyled n0".to_string()]);
}

// ===========================================================================
// Prop bindings
// ===========================================================================

#[test]
fn const_button_label_installs_no_update() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            button().label("Go").on_press(|| {}).build(),
        )
    });
    assert_eq!(
        h.take_log(),
        vec!["create n0 button \"Go\"".to_string()],
        "Const label: create only, no mount-time update_button_label"
    );
    drop(realized);
}

#[test]
fn dyn_button_label_updates_at_mount_and_per_change() {
    let h = harness();
    let world = h.world.clone();
    let (realized, label) = world.enter(|| {
        let label = signal(String::from("Start"));
        let realized = realize(
            &h.backend,
            &h.registry,
            button().label(move || label.get()).on_press(|| {}).build(),
        );
        (realized, label)
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 button \"Start\"".to_string(),
            "update_button_label n0 \"Start\"".to_string(),
        ],
        "Dyn label: created at initial value, binding re-applies at mount (walker shape)"
    );
    label.set("Stop".to_string());
    world.flush();
    assert_eq!(
        h.take_log(),
        vec!["update_button_label n0 \"Stop\"".to_string()]
    );
    drop(realized);
}

#[test]
fn controlled_toggle_writes_back_on_change() {
    let h = harness();
    let world = h.world.clone();
    let (realized, on) = world.enter(|| {
        let on = signal(true);
        let realized = realize(
            &h.backend,
            &h.registry,
            toggle().value(on).on_change(|_| {}).build(),
        );
        (realized, on)
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 toggle true".to_string(),
            "update_toggle_value n0 true".to_string(),
        ]
    );
    on.set(false);
    world.flush();
    assert_eq!(h.take_log(), vec!["update_toggle_value n0 false".to_string()]);
    drop(realized);
}

#[test]
fn slider_on_change_snaps_to_step() {
    let h = harness();
    let world = h.world.clone();
    let received: Rc<RefCell<Vec<f32>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = received.clone();
    let realized = world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            slider()
                .value(0.0f32)
                .on_change(move |v| sink.borrow_mut().push(v))
                .range(0.0, 10.0)
                .step(0.5)
                .build(),
        )
    });
    // Simulate a native drag reporting a raw, unsnapped value.
    let native_on_change = h.slider_changes.borrow()[0].clone();
    native_on_change(3.34);
    assert_eq!(
        *received.borrow(),
        vec![3.5],
        "the wrapper snaps to step before dispatch — uniform across backends"
    );
    drop(realized);
}

// ===========================================================================
// Teardown probe + payload contract
// ===========================================================================

#[test]
fn on_teardown_fires_once_at_drop_not_per_flush() {
    let world = World::new();
    let fired = Rc::new(Cell::new(0u32));
    let owned = world.enter(|| {
        let fired = fired.clone();
        let ((), owned) = runtime_world::collect_owned(|| {
            runtime_vocabulary::on_teardown(move || fired.set(fired.get() + 1));
        });
        owned
    });
    world.flush();
    world.flush();
    assert_eq!(fired.get(), 0, "no teardown before the scope drops");
    drop(owned);
    assert_eq!(fired.get(), 1, "exactly once at scope drop");
}

#[test]
#[should_panic(expected = "mounted twice")]
fn prim_cell_take_twice_panics() {
    let cell = runtime_vocabulary::prims::PrimCell::new(1u32);
    let _ = cell.take();
    let _ = cell.take();
}

#[test]
#[should_panic(expected = ".src(...)")]
fn image_without_src_panics_at_build() {
    let _ = image().build();
}

#[test]
#[should_panic(expected = ".data(...)")]
fn icon_without_data_panics_at_build() {
    let _ = icon().build();
}

#[test]
#[should_panic(expected = "on_activate")]
fn non_external_link_without_on_activate_panics_at_mount() {
    let h = harness();
    let _ = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            link().url("/somewhere").build(),
        )
    });
}

/// Regression shape for the walker's uniform-disable contract: a bare
/// pressable is not a native form control, so the press must be blocked
/// at the callback (the shared flag), not just via `set_disabled`.
#[test]
fn pressable_disabled_blocks_press_uniformly() {
    let h = harness();
    let world = h.world.clone();
    let pressed = Rc::new(Cell::new(0u32));
    let sink = pressed.clone();
    let (realized, disabled) = world.enter(|| {
        let disabled = signal(false);
        let realized = realize(
            &h.backend,
            &h.registry,
            pressable(move || sink.set(sink.get() + 1))
                .disabled(disabled)
                .build(),
        );
        (realized, disabled)
    });
    // Mount: create + set_disabled(false) from the binding's first fire.
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 pressable".to_string(),
            "set_disabled n0 false".to_string(),
        ]
    );
    // The wrapped callback the backend received fires while enabled…
    let native_press = h.press_handlers.borrow()[0].clone();
    native_press();
    assert_eq!(pressed.get(), 1);
    // …and is blocked once the disabled binding flips the shared flag.
    disabled.set(true);
    world.flush();
    assert_eq!(h.take_log(), vec!["set_disabled n0 true".to_string()]);
    native_press();
    assert_eq!(pressed.get(), 1, "press blocked while disabled");
    drop(realized);
}

// ===========================================================================
// Reactive text through the builder (fallback update_text path — Mini
// has no batched-id support)
// ===========================================================================

#[test]
fn dyn_text_uses_update_text_fallback_without_id_support() {
    let h = harness();
    let world = h.world.clone();
    let (realized, count) = world.enter(|| {
        let count = signal(0i32);
        let realized = realize(
            &h.backend,
            &h.registry,
            text().content(move || format!("c={}", count.get())).build(),
        );
        (realized, count)
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 text \"\"".to_string(),
            "update_text n0 \"c=0\"".to_string(),
        ],
        "no create_text_with_id support: legacy create+update path"
    );
    count.set(5);
    world.flush();
    assert_eq!(h.take_log(), vec!["update_text n0 \"c=5\"".to_string()]);
    drop(realized);
}
