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
#[should_panic(expected = "primitive `graphics` can't cross")]
fn an_unsupported_primitive_panics_at_encode_by_name() {
    let tree = runtime_vocabulary::builders::graphics(|_| {}).build();
    bundle::encode(tree);
}

#[test]
#[should_panic(expected = "`view`'s `ref` can't cross")]
fn an_unsupported_field_panics_at_encode_by_name() {
    let tree = view().on_handle(|_| {}).build();
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
    let mut supported: Vec<_> =
        kinds.iter().filter_map(|k| match crossing(*k) { Some(Crossing::Supported(n)) => Some(n), _ => None }).collect();
    supported.sort();
    assert_eq!(
        supported,
        [
            "activity_indicator", "button", "icon", "image", "link", "portal", "presence", "pressable",
            "repeat (static `for` lowering)", "scroll_view", "slider", "text", "text_area", "text_input", "toggle",
            "view", "virtual_grid", "virtualizer",
        ]
    );
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

// ---- phase A: every leaf control and every event handler ----

mod controls {
    use super::*;
    use runtime_shared::file_drop::{DroppedFile, FileDropEvent, FileDropPhase};
    use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;
    use runtime_shared::primitives::icon::{FillRule, IconData};
    use runtime_shared::primitives::image::ImageLoadEvent;
    use runtime_shared::primitives::key::{KeyEvent, KeyOutcome};
    use runtime_shared::primitives::text_input::BlurOutcome;
    use runtime_shared::styled_text::TextRun;
    use runtime_shared::touch::{TouchEvent, TouchId, TouchPhase, TouchPoint, TouchResponse};
    use runtime_shared::wheel::{WheelEvent, WheelKind};
    use runtime_vocabulary::builders::{
        activity_indicator, icon, image, link, scroll_view, slider, text_area, text_input, toggle,
    };

    const STAR: IconData = IconData { view_box: (24, 24), paths: &["M12 2l3 7h7l-6 4 2 7-6-4-6 4 2-7-6-4h7z"], fill_rule: FillRule::NonZero, filled: true };

    /// Every leaf primitive phase A carries, each handler writing a signal a
    /// text shows — so the backend log records what each event did.
    fn app() -> Element {
        component_scope(|| {
            let seen = signal(String::from("-"));
            let on = signal(false);
            let level = signal(0.25f32);
            let typed = signal(String::from("start"));
            let note = move |s: String| seen.set(s);
            view()
                .on_touch(Rc::new(move |e: &TouchEvent| {
                    note(format!("touch {:?} {}", e.phase, e.position.x));
                    TouchResponse::CONSUMED
                }))
                .on_wheel(Rc::new(move |e: &WheelEvent| {
                    note(format!("wheel {}", e.delta_y));
                    TouchResponse::default()
                }))
                .on_hover(move |h| note(format!("hover {h}")))
                .on_file_drop(Rc::new(move |e: &FileDropEvent| {
                    if let FileDropPhase::Dropped(files) = &e.phase {
                        note(format!("drop {}", files[0].name));
                    }
                    TouchResponse::CONSUMED
                }))
                .child(text().content(move || seen.get()))
                .child(text().runs(vec![TextRun::plain("plain "), TextRun::plain("run")]))
                .child(button().label("star").leading_icon(STAR).on_press(|| {}))
                .child(
                    image()
                        .src(move || if on.get() { "on.png".to_string() } else { "off.png".to_string() })
                        .on_load(Rc::new(move |e: &ImageLoadEvent| note(format!("loaded {}", e.width))))
                        .on_error(Rc::new(move || note("image error".into()))),
                )
                .child(icon().data(STAR).color(runtime_shared::Color("#f00".into())))
                .child(link().url("https://example.com").on_activate(move || note("link".into())).child(text().content("go")))
                .child(toggle().value(move || on.get()).on_change(move |v| on.set(v)))
                .child(slider().value(move || level.get()).range(0.0, 1.0).on_change(move |v| level.set(v)))
                .child(activity_indicator().size(ActivityIndicatorSize::Large))
                .child(
                    text_input()
                        .value(move || typed.get())
                        .on_change(move |v| typed.set(v))
                        .on_key_down(move |e: &KeyEvent| {
                            note(format!("key {}", e.key));
                            if e.key == "Tab" { KeyOutcome::PreventDefault } else { KeyOutcome::Default }
                        })
                        .on_blur(move || BlurOutcome::Keep)
                        .on_focus(move |f| note(format!("focus {f}")))
                        .placeholder("type"),
                )
                .child(text_area().value(move || typed.get()).on_change(move |v| typed.set(v)).placeholder("notes"))
                .child(
                    scroll_view()
                        .on_scroll(move |x, y| note(format!("scroll {x},{y}")))
                        .on_end_reached(move || note("end".into()))
                        .child(text().content(move || format!("on {} level {} typed {}", on.get(), level.get(), typed.get()))),
                )
                .build()
        })
    }

    fn key(k: &str) -> KeyEvent {
        KeyEvent { key: k.into(), shift: false, ctrl: false, alt: false, meta: false, selection_start: 0, selection_end: 0 }
    }

    /// Mount, fire every handler, and return the backend log per step plus
    /// what the handlers that answer the platform replied.
    fn script(remote: bool) -> (Vec<Vec<String>>, Vec<String>) {
        let h = Harness::new();
        let tree = h.world.enter(app);
        let tree = if remote { cross(tree) } else { tree };
        let mut steps = Vec::new();
        let mut replies = Vec::new();
        let realized = h.mount(tree);
        h.flush();
        steps.push(h.take_log());

        let at = TouchPoint { x: 3.0, y: 4.0 };
        let touch = h.shared.touch_handlers.borrow()[0].1.clone();
        let r = touch(&TouchEvent { id: TouchId(1), phase: TouchPhase::Began, position: at, window_position: at, timestamp_ns: 0, force: None });
        replies.push(format!("touch consumed {}", r.consumed));
        h.flush();
        steps.push(h.take_log());

        let wheel = h.shared.wheel_handlers.borrow()[0].1.clone();
        wheel(&WheelEvent { kind: WheelKind::Scroll, delta_x: 0.0, delta_y: 7.0, scale: 1.0, rotation: 0.0, position: at, window_position: at, timestamp_ns: 0 });
        (h.shared.hover_handlers.borrow()[0].1.clone())(true);
        h.flush();
        steps.push(h.take_log());

        let drop_h = h.shared.file_drop_handlers.borrow()[0].1.clone();
        let file = DroppedFile { name: "a.txt".into(), mime: "text/plain".into(), size: Some(3), path: None, source: None };
        let r = drop_h(&FileDropEvent { phase: FileDropPhase::Dropped(vec![file]), position: at });
        replies.push(format!("drop consumed {}", r.consumed));
        h.flush();
        steps.push(h.take_log());

        (h.shared.image_load_handlers.borrow()[0].1.clone())(&ImageLoadEvent { width: 64.0, height: 32.0 });
        h.flush();
        steps.push(h.take_log());
        (h.shared.image_error_handlers.borrow()[0].1.clone())();
        (h.link_activation(0))();
        h.flush();
        steps.push(h.take_log());

        (h.toggle_change(0))(true);
        (h.slider_change(0))(0.75);
        (h.text_input_change(0))("typed".into());
        h.flush();
        steps.push(h.take_log());

        let keys = h.key_down_handler(0).expect("text_input key handler");
        replies.push(format!("tab {:?}", keys(&key("Tab"))));
        replies.push(format!("a {:?}", keys(&key("a"))));
        replies.push(format!("blur {:?}", h.blur_handler(0).expect("blur handler")()));
        (h.shared.focus_handlers.borrow()[0].1.clone())(true);
        h.flush();
        steps.push(h.take_log());

        (h.scroll_handler(0).expect("scroll handler"))(1.0, 2.0);
        let end = h.shared.end_observers.borrow()[0].3.clone();
        end();
        h.flush();
        steps.push(h.take_log());

        drop(realized);
        h.flush();
        steps.push(h.take_log());
        (steps, replies)
    }

    #[test]
    fn every_leaf_control_drives_the_backend_exactly_like_the_native_one() {
        let (native, native_replies) = script(false);
        let (remote, remote_replies) = script(true);
        assert!(native[0].len() > 20, "the script mounts a real tree: {:?}", native[0]);
        for (step, (n, r)) in native.iter().zip(&remote).enumerate() {
            assert_eq!(r, n, "step {step}: remote and native diverge");
        }
        assert_eq!(remote_replies, native_replies);
        assert_eq!(native_replies, ["touch consumed true", "drop consumed true", "tab PreventDefault", "a Default", "blur Keep"]);
    }
}

// ---- phase B: the structural primitives ----

mod structural {
    use super::*;
    use runtime_shared::primitives::portal::{PortalTarget, ViewportPlacement};
    use runtime_shared::primitives::presence::PresenceAnim;
    use runtime_shared::primitives::virtualizer::{ItemDiff, ItemSize};
    use runtime_shared::Easing;
    use runtime_vocabulary::builders::{portal, presence, virtual_grid, virtualizer};

    struct In {
        shown: Signal<bool>,
        /// `(key, label)`: a label can change under a surviving key.
        items: Signal<Vec<(u64, String)>>,
        dismissed: Signal<u32>,
    }

    fn app(i: &In) -> Element {
        let (shown, items, dismissed) = (i.shown, i.items, i.dismissed);
        component_scope(move || {
            view()
                .children(runtime_vocabulary::glue::__static_repeat(3, |i| text().content(format!("rep {i}")).build()))
                .child(
                    presence(|| text().content("present").build())
                        .present(move || shown.get())
                        .enter(PresenceAnim::fade(100, Easing::Linear))
                        .exit(PresenceAnim::fade(50, Easing::Linear)),
                )
                .child(
                    portal(PortalTarget::Viewport(ViewportPlacement::default()))
                        .on_dismiss(move || dismissed.update(|d| d + 1))
                        .child(text().content(move || format!("dismissed {}", dismissed.get()))),
                )
                .child(
                    virtualizer(
                        move || items.get().len(),
                        move |i| items.get()[i].0,
                        ItemSize::Known(Rc::new(|i| 20.0 + i as f32)),
                        move |i| text().content(format!("item {}", items.get()[i].1)).build(),
                    )
                    .item_diff(ItemDiff {
                        capture: Rc::new(move |i| items.get().get(i).map(|v| Box::new(v.1.clone()) as Box<dyn std::any::Any>)),
                        differs: Rc::new(move |snap, i| snap.downcast_ref::<String>() != items.get().get(i).map(|v| &v.1)),
                    }),
                )
                .child(virtual_grid(
                    || 2,
                    || 2,
                    |c| 10.0 + c as f32,
                    |r| 5.0 + r as f32,
                    |r, c| (r * 2 + c) as u64,
                    |r, c| text().content(format!("cell {r},{c}")).build(),
                )
                .build())
                .build()
        })
    }

    fn script(remote: bool) -> (Vec<Vec<String>>, Vec<String>) {
        let h = Harness::new();
        let i = h.world.enter(|| In {
            shown: signal(true),
            items: signal(vec![(1, "a".into()), (2, "b".into()), (3, "c".into())]),
            dismissed: signal(0),
        });
        let tree = h.world.enter(|| app(&i));
        let tree = if remote { cross(tree) } else { tree };
        let mut steps = Vec::new();
        let mut replies = Vec::new();
        let realized = h.mount(tree);
        h.flush();
        steps.push(h.take_log());

        // The backend drives the virtualizer and grid through their callbacks.
        let v = h.virtualizer(0);
        replies.push(format!("count {} key1 {} size2 {}", (v.item_count)(), (v.item_key)(1), (v.item_size)(2)));
        let rows: Vec<(host_mock::Node, u64)> = h.world.enter(|| (0..(v.item_count)()).map(|n| (v.mount_item)(n)).collect());
        let g = h.virtual_grid(0);
        replies.push(format!(
            "grid {}x{} w1 {} h1 {} key {}",
            (g.col_count)(),
            (g.row_count)(),
            (g.col_width)(1),
            (g.row_height)(1),
            (g.cell_key)(1, 1)
        ));
        let _cell = h.world.enter(|| (g.mount_cell)(1, 0));
        h.flush();
        steps.push(h.take_log());

        i.shown.set(false);
        h.flush();
        steps.push(h.take_log());

        let dismiss = h.shared.portal_dismissals.borrow()[0].clone().expect("portal on_dismiss");
        dismiss();
        h.flush();
        steps.push(h.take_log());

        // A data change: key 2 keeps its row but its label changes; the
        // diff (run in the bundle, against a snapshot held there) says so.
        i.items.set(vec![(1, "a".into()), (2, "B".into()), (3, "c".into())]);
        h.flush();
        let changed = v.item_changed.clone().expect("item_diff crosses");
        replies.push(format!("changed {} {} {}", changed(0), changed(1), changed(2)));
        h.world.enter(|| {
            for (_, id) in rows {
                (v.release_item)(id);
            }
        });
        h.flush();
        steps.push(h.take_log());

        drop(realized);
        h.flush();
        steps.push(h.take_log());
        (steps, replies)
    }

    #[test]
    fn every_structural_primitive_drives_the_backend_exactly_like_the_native_one() {
        let (native, native_replies) = script(false);
        let (remote, remote_replies) = script(true);
        assert!(native[0].len() > 10, "{:?}", native[0]);
        for (step, (n, r)) in native.iter().zip(&remote).enumerate() {
            assert_eq!(r, n, "step {step}: remote and native diverge");
        }
        assert_eq!(remote_replies, native_replies);
        assert_eq!(native_replies[2], "changed false true false", "{native_replies:?}");
    }
}
