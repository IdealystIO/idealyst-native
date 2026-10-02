//! The remote element codec (`src/remote`), both halves in one process.
//!
//! The kernel runs on the loopback engine (`remote-loopback` turns on
//! `runtime-world/loopback-engine`), so a component's `Owned` is a bridge
//! scope that really crosses as an id — the same path a wasm bundle takes,
//! minus the wasm. The wasm transport is covered end to end in
//! crates/streaming/spike/tests.
//!
//! The central check is PARITY: the same tree, mounted natively and mounted
//! after an encode → decode round trip, must drive the backend through the
//! same calls, through the same interactions, and tear down to nothing.
//!
//! ```sh
//! cargo test -p runtime-vocabulary --features remote-loopback --test remote_elements
//! ```

#![cfg(feature = "remote-loopback")]

use std::rc::Rc;

use host_mock::Harness;
use runtime_scene::{component_scope, dyn_keyed, keyed, Element, Registry};
use runtime_shared::{Length, StyleApplication, StyleRules, StyleSheet, Tokenized};
use runtime_vocabulary::builders::{button, pressable, text, view};
use runtime_vocabulary::remote::host::{decode, register_import, DecodeError, Link};
use runtime_vocabulary::remote::{bundle, crossing, to_bytes, Cb, Crossing};
use runtime_world::{signal, Signal};

/// The host's link to "the bundle": its callback table, in-process.
struct InProc;

impl Link for InProc {
    fn call(&self, cb: Cb, args: &[u8]) -> Option<Vec<u8>> {
        Some(bundle::invoke(cb, args))
    }
    fn release(&self, cb: Cb) {
        bundle::release(cb)
    }
}

/// Encode as a bundle would, decode as the host would.
fn cross(element: Element) -> Element {
    decode(Rc::new(InProc), &to_bytes(&bundle::encode(element))).expect("decodes")
}

fn card_sheet() -> Rc<StyleSheet> {
    Rc::new(
        StyleSheet::new(|_| StyleRules {
            background: Some(Tokenized::token("color-surface", runtime_shared::Color("#fff".into()))),
            ..StyleRules::default()
        })
        .variant("tone", "danger", |_| StyleRules {
            width: Some(Tokenized::Literal(Length::Px(320.0))),
            ..StyleRules::default()
        })
        .variant("__state_hovered", "on", |_| StyleRules {
            width: Some(Tokenized::Literal(Length::Px(999.0))),
            ..StyleRules::default()
        }),
    )
}

struct Inputs {
    count: Signal<i32>,
    items: Signal<Vec<u32>>,
    show: Signal<bool>,
}

/// A component exercising every node kind the codec carries: a component
/// boundary (`Owned`) with local state, a sheet with a state axis, dynamic
/// text, a button and a pressable writing state, a guarded hole, a keyed
/// list, and a dynamic style.
fn app(i: &Inputs) -> Element {
    let (count, items, show) = (i.count, i.items, i.show);
    component_scope(move || {
        let clicks = signal(0i32);
        view()
            .style(StyleApplication::new(card_sheet()).with("tone", "danger"))
            .child(text().content(move || format!("count {} clicks {}", count.get(), clicks.get())))
            .child(button().label(move || format!("+{}", clicks.get())).on_press(move || clicks.update(|c| c + 1)))
            .child(pressable(move || count.update(|c| c + 10)).child(text().content("tap")))
            .child(dyn_keyed(
                move || show.get(),
                |&on| if on { text().content("shown").build() } else { view().build() },
            ))
            .child(keyed(move || items.get(), |i| *i as u64, |i| text().content(format!("row {i}")).build()))
            .child(
                text()
                    .style(move || {
                        Rc::new(StyleRules {
                            width: Some(Tokenized::Literal(Length::Px(count.get() as f32))),
                            ..StyleRules::default()
                        })
                    })
                    .content("sized"),
            )
            .build()
    })
}

/// Mount `app`, drive it through a script, and return the backend log at
/// each step.
fn script(remote: bool) -> Vec<Vec<String>> {
    let h = Harness::new();
    let inputs = h.world.enter(|| Inputs { count: signal(1), items: signal(vec![1, 2, 3]), show: signal(false) });
    let tree = h.world.enter(|| app(&inputs));
    let tree = if remote { cross(tree) } else { tree };
    let mut steps = Vec::new();

    let realized = h.mount(tree);
    h.flush();
    steps.push(h.take_log());

    // The button (bundle state) and the pressable (host prop).
    (h.shared.button_presses.borrow()[0].clone())();
    (h.press_handler(0))();
    h.flush();
    steps.push(h.take_log());

    // Hover: the sheet's state axis, resolved by the host's style engine.
    (h.state_setter(0))(runtime_shared::StateBits::HOVERED, true);
    h.flush();
    steps.push(h.take_log());

    // Structure: the guarded hole flips, the keyed list reorders and grows.
    inputs.show.set(true);
    inputs.items.set(vec![3, 1, 4]);
    h.flush();
    steps.push(h.take_log());

    drop(realized);
    h.flush();
    steps.push(h.take_log());
    steps
}

#[test]
fn a_crossed_tree_drives_the_backend_exactly_like_the_native_one() {
    let native = script(false);
    let remote = script(true);
    assert!(native[0].len() > 10, "the script mounts a real tree: {:?}", native[0]);
    for (step, (n, r)) in native.iter().zip(&remote).enumerate() {
        assert_eq!(r, n, "step {step}: remote and native diverge");
    }
}

#[test]
fn every_callback_is_released_when_the_crossed_tree_unmounts() {
    let h = Harness::new();
    let inputs = h.world.enter(|| Inputs { count: signal(1), items: signal(vec![1, 2, 3]), show: signal(true) });
    let tree = h.world.enter(|| cross(app(&inputs)));
    assert!(bundle::live_callbacks() > 0);
    let realized = h.mount(tree);
    h.flush();
    inputs.items.set(vec![2, 5]);
    inputs.show.set(false);
    h.flush();
    drop(realized);
    h.flush();
    // The mock backend keeps a clone of every press handler it was given
    // (so tests can fire them later) — it is a real owner of those two ids
    // until it goes.
    assert_eq!(bundle::live_callback_kinds().iter().map(|e| e.1).collect::<Vec<_>>(), ["fire", "fire"]);
    drop(h);
    assert_eq!(bundle::live_callback_kinds(), vec![], "a callback outlived everything that held it");
}

/// One sheet styling many nodes crosses as ONE bundle entry, and the host
/// builds one proxy for it.
#[test]
fn a_shared_sheet_crosses_once() {
    let h = Harness::new();
    let sheet = card_sheet();
    let tree = h.world.enter(|| {
        view()
            .child(view().style(StyleApplication::new(sheet.clone())))
            .child(view().style(StyleApplication::new(sheet.clone())))
            .child(view().style(StyleApplication::new(sheet.clone()).with("tone", "danger")))
            .build()
    });
    let node = bundle::encode(tree);
    assert_eq!(bundle::live_callbacks(), 1, "three crossings, one entry");
    let realized = h.mount(decode(Rc::new(InProc), &to_bytes(&node)).expect("decodes"));
    h.flush();
    drop(realized);
    assert_eq!(bundle::live_callbacks(), 0, "the refcount balanced");
}

#[test]
fn an_app_component_is_imported_by_name() {
    #[derive(serde::Serialize, serde::Deserialize)]
    struct BadgeProps {
        label: String,
    }
    register_import("Badge", |p: BadgeProps, children: Vec<Element>| {
        view().child(text().content(format!("badge: {}", p.label))).children(children).build()
    });
    let h = Harness::new();
    let tree = h.world.enter(|| {
        view()
            .child(bundle::import("Badge", &BadgeProps { label: "new".into() }, vec![text().content("inner").build()]))
            .build()
    });
    let realized = h.mount(cross(tree));
    h.flush();
    let root = realized.collect_nodes()[0];
    let tree = h.tree(root);
    assert!(tree.contains(r#"text "badge: new""#) && tree.contains(r#"text "inner""#), "{tree}");
}

#[test]
fn a_missing_app_component_fails_to_decode_by_name() {
    let tree = bundle::import("NotExported", &(), Vec::new());
    let err = decode(Rc::new(InProc), &to_bytes(&bundle::encode(tree))).err();
    assert_eq!(err, Some(DecodeError::MissingImport("NotExported".into())));
}

#[test]
#[should_panic(expected = "primitive `image` can't cross")]
fn an_unsupported_primitive_panics_at_encode_by_name() {
    let tree = runtime_vocabulary::builders::image().src("a.png").build();
    bundle::encode(tree);
}

#[test]
#[should_panic(expected = "`view`'s `on_hover` can't cross")]
fn an_unsupported_field_panics_at_encode_by_name() {
    let tree = view().on_hover(|_| {}).build();
    bundle::encode(tree);
}

/// Every payload `register_builtins` installs has a decision in the codec's
/// `crossing` table. A new builtin fails here until someone decides whether
/// it crosses — the parity rule for remote components.
#[test]
fn every_builtin_primitive_has_a_crossing_decision() {
    let mut registry: Registry<host_mock::HostMock> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    let kinds = registry.kinds();
    let undecided: Vec<_> = kinds.iter().filter(|k| crossing(**k).is_none()).collect();
    assert!(
        undecided.is_empty(),
        "{} builtin payload(s) have no entry in runtime-vocabulary src/remote `crossing`: {:?}",
        undecided.len(),
        undecided.iter().map(|k| runtime_scene::payload_type_name(**k)).collect::<Vec<_>>()
    );
    let supported: Vec<_> =
        kinds.iter().filter_map(|k| match crossing(*k) { Some(Crossing::Supported(n)) => Some(n), _ => None }).collect();
    assert_eq!(supported.len(), 4, "view, pressable, text, button: {supported:?}");
}

/// A link whose bundle can be POISONED mid-life, as a trap poisons a wasm
/// bundle: from then on every call answers `None`.
struct Poisonable(Rc<std::cell::Cell<bool>>);

impl Link for Poisonable {
    fn call(&self, cb: Cb, args: &[u8]) -> Option<Vec<u8>> {
        (!self.0.get()).then(|| bundle::invoke(cb, args))
    }
    fn release(&self, cb: Cb) {
        bundle::release(cb)
    }
}

/// Regression: a reply from a bundle that can no longer be called used to
/// panic the app ("bundle is gone, but a node it built is still live") —
/// so one panic in a bundle's handler took the whole app down with it.
/// Every reply site now falls back: getters keep their last value, holes
/// and keyed rows render nothing, handlers do nothing, style getters
/// resolve to defaults. Driven here through every node kind the codec
/// carries, after the link is cut.
#[test]
fn regression_a_poisoned_bundles_tree_keeps_running_without_it() {
    let h = Harness::new();
    let inputs = h.world.enter(|| Inputs { count: signal(1), items: signal(vec![1, 2, 3]), show: signal(false) });
    let poisoned = Rc::new(std::cell::Cell::new(false));
    let link = Rc::new(Poisonable(poisoned.clone()));
    let tree = h.world.enter(|| decode(link, &to_bytes(&bundle::encode(app(&inputs)))).expect("decodes"));
    let realized = h.mount(tree);
    h.flush();
    let before = h.live_tree(realized.collect_nodes()[0]);
    assert!(before.contains("count 1 clicks 0"), "{before}");

    poisoned.set(true);
    (h.shared.button_presses.borrow()[0].clone())();
    (h.press_handler(0))();
    (h.state_setter(0))(runtime_shared::StateBits::HOVERED, true);
    inputs.count.set(5);
    inputs.show.set(true);
    inputs.items.set(vec![9]);
    h.flush();

    let after = h.live_tree(realized.collect_nodes()[0]);
    assert!(after.contains("count 1 clicks 0"), "a getter keeps its last value: {after}");
    assert!(!after.contains("shown") && !after.contains("row 9"), "new subtrees render nothing: {after}");
    drop(realized);
    h.flush();
}
