//! New-core adoption tests for the Roku backend: old-vs-new
//! COMMAND-STREAM byte parity (the strongest available gate — there is
//! no device/thin-client to render against, so the serialized wire
//! stream IS the observable output), plus op-level coverage of the
//! embedder-driven flush discipline (device event → HandlerTable
//! dispatch → staged writes → `settle()` → emitted commands).
//!
//! Harness notes: the tests install a queue scheduler whose
//! `drain_buffered_microtasks` drains its own queue — modeling an
//! embedder whose scheduler buffers microtasks, which is the
//! configuration where staged-ness is OBSERVABLE (without any
//! scheduler, `schedule_microtask` falls back to synchronous off-web
//! and the deduped flush commits inside the wrapped callback).
//! `install_scheduler` is process-global/first-wins, but the queue
//! state is thread-local, so each test thread drains only its own
//! tasks.

#![cfg(feature = "new-core")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_roku::{HandlerId, RokuBackend, RokuCommand};
use runtime_core::{Color, Length, StyleApplication, StyleRules, StyleSheet, Tokenized};

// ===========================================================================
// Queue scheduler (an embedder that buffers microtasks)
// ===========================================================================

mod test_scheduler {
    use runtime_core::scheduling::{ScheduleHandle, Scheduler};
    use std::cell::RefCell;
    use std::collections::VecDeque;

    thread_local! {
        static QUEUE: RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> =
            RefCell::new(VecDeque::new());
    }

    struct NoopHandle;
    impl ScheduleHandle for NoopHandle {
        fn cancel(&mut self) {}
    }

    struct QueueScheduler;
    impl Scheduler for QueueScheduler {
        fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
            QUEUE.with(|q| q.borrow_mut().push_back(f));
        }
        fn after_animation_frame(&self, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
        fn after_ms(&self, _delay_ms: i32, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
        fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
        // Buffered semantics: `newcore::settle()` reaches these tasks
        // through `drain_buffered_microtasks` — the embedder contract
        // under test.
        fn drain_buffered_microtasks(&self) {
            drain();
        }
    }

    pub fn ensure_installed() {
        if !runtime_core::scheduling::is_scheduler_installed() {
            runtime_core::scheduling::install_scheduler(Box::new(QueueScheduler));
        }
    }

    /// Drain-until-empty.
    pub fn drain() {
        loop {
            let next = QUEUE.with(|q| q.borrow_mut().pop_front());
            match next {
                Some(task) => task(),
                None => break,
            }
        }
    }
}

// ===========================================================================
// Harness
// ===========================================================================

fn fresh_backend() -> Rc<RefCell<RokuBackend>> {
    test_scheduler::ensure_installed();
    Rc::new(RefCell::new(RokuBackend::new()))
}

/// Mount on the OLD core (walker + `runtime_core::mount` — the same
/// path `backend_roku::snapshot` takes, with the Owner kept alive so
/// handler invocations can drive follow-up commands).
fn mount_old(
    tree_fn: impl Fn() -> runtime_core::Element + 'static,
) -> (Rc<RefCell<RokuBackend>>, runtime_core::Owner) {
    let backend = fresh_backend();
    let owner = runtime_core::mount(backend.clone(), tree_fn);
    test_scheduler::drain();
    (backend, owner)
}

/// Mount on the NEW core (`newcore::start`, vocabulary handlers).
fn mount_new(
    build: impl FnOnce() -> runtime_scene::Element,
) -> (Rc<RefCell<RokuBackend>>, backend_roku::newcore::NewCoreApp) {
    let backend = fresh_backend();
    let app = backend_roku::newcore::start(backend.clone(), |_| {}, build);
    backend_roku::newcore::settle();
    (backend, app)
}

/// Drain the backend's command queue and serialize — the exact bytes an
/// embedder would ship to the BrightScript client.
fn drain_json(backend: &Rc<RefCell<RokuBackend>>) -> (Vec<RokuCommand>, String) {
    let cmds = backend.borrow_mut().drain();
    let json = serde_json::to_string(&cmds).expect("commands serialize");
    (cmds, json)
}

/// Apply the ONE sanctioned old→new divergence to an OLD-core stream:
/// the old walker emits a no-op `ClearChildren` on a VIRGIN reactive
/// anchor (an anchor it just created and has never inserted into)
/// before the first population of a `when`/`each` region; the new core
/// deliberately skips it. This is divergence class #2 in
/// docs/migrating-to-runtime-v2.md ("A skipped no-op `clear_children`
/// on a virgin anchor — invisible") / `crates/dev/scene-parity/README.md`
/// — invisible to the device (clearing an empty Group is a no-op), but
/// visible in a command stream. Strip EXACTLY that shape and nothing
/// else: a `ClearChildren` whose parent is a stream-created anchor
/// with no prior `Insert` into it. Every other op is a hard invariant.
fn normalize_sanctioned_old(cmds: &[RokuCommand]) -> String {
    use std::collections::HashSet;
    let mut anchors: HashSet<u64> = HashSet::new();
    let mut populated: HashSet<u64> = HashSet::new();
    let kept: Vec<&RokuCommand> = cmds
        .iter()
        .filter(|c| match c {
            RokuCommand::CreateReactiveAnchor { id } => {
                anchors.insert(id.0);
                true
            }
            RokuCommand::Insert { parent, .. } => {
                populated.insert(parent.0);
                true
            }
            RokuCommand::ClearChildren { parent } => {
                // Virgin-anchor clear: sanctioned skip on the new core.
                !(anchors.contains(&parent.0) && !populated.contains(&parent.0))
            }
            _ => true,
        })
        .collect();
    serde_json::to_string(&kept).expect("commands serialize")
}

/// Assert exact JSON equality with a byte-level first-divergence report
/// (index + surrounding context from both sides) so a parity failure is
/// directly actionable.
fn assert_stream_bytes(name: &str, old: &str, new: &str) {
    if old == new {
        return;
    }
    let at = old
        .bytes()
        .zip(new.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| old.len().min(new.len()));
    let lo = at.saturating_sub(80);
    let old_hi = (at + 80).min(old.len());
    let new_hi = (at + 80).min(new.len());
    panic!(
        "command-stream byte divergence in `{name}` at byte {at}\n\
         old (len {}): …{}…\n\
         new (len {}): …{}…\n\
         full old:\n{}\n\
         full new:\n{}",
        old.len(),
        &old[lo..old_hi],
        new.len(),
        &new[lo..new_hi],
        old,
        new,
    );
}

/// Find a button's wire HandlerId by label in a drained stream.
fn button_handler(cmds: &[RokuCommand], label: &str) -> HandlerId {
    cmds.iter()
        .find_map(|c| match c {
            RokuCommand::CreateButton { label: l, on_click, .. } if l == label => Some(*on_click),
            _ => None,
        })
        .unwrap_or_else(|| panic!("button `{label}` in stream: {cmds:?}"))
}

/// Clone a unit handler's closure out of the HandlerTable so it can be
/// invoked with NO backend borrow held (the old core's synchronous
/// effects re-borrow the backend to emit follow-up commands).
fn unit_handler(backend: &Rc<RefCell<RokuBackend>>, id: HandlerId) -> Rc<dyn Fn()> {
    let b = backend.borrow();
    let table = b.handlers();
    table
        .unit
        .iter()
        .find(|(h, _)| *h == id)
        .map(|(_, f)| f.clone())
        .expect("handler id registered in HandlerTable")
}

fn test_rules(width: f32, background: &str) -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(Length::Px(width))),
        background: Some(Tokenized::Literal(Color(background.to_string()))),
        ..Default::default()
    }
}

fn flex_rules() -> StyleRules {
    StyleRules {
        flex_direction: Some(runtime_core::FlexDirection::Row),
        justify_content: Some(runtime_core::JustifyContent::SpaceBetween),
        align_items: Some(runtime_core::AlignItems::Center),
        gap: Some(Tokenized::Literal(Length::Px(8.0))),
        padding_top: Some(Tokenized::Literal(Length::Px(12.0))),
        padding_left: Some(Tokenized::Literal(Length::Px(16.0))),
        background: Some(Tokenized::Literal(Color("#101020".to_string()))),
        width: Some(Tokenized::Literal(Length::Percent(100.0))),
        ..Default::default()
    }
}

/// Static-sheet application used on BOTH sides. Roku reports
/// `handles_states_natively() == true`, so the framework routes every
/// sheet application — overlays or not — through `apply_styled_states`
/// (one `ApplyStyleStates` wire op). The new side must ride the same
/// route: a raw-rules `.style(rules)` would take the plain
/// `apply_style` path and emit a different (equally correct, but
/// different) op, which is a scene-equivalence mismatch, not a core
/// divergence.
fn static_style(rules: StyleRules) -> StyleApplication {
    StyleApplication::new(Rc::new(StyleSheet::r#static(rules)))
}

/// A sheet with a token-referencing base and a `state hovered` overlay
/// — exercises `apply_styled_states` (Roku's `ApplyStyleStates` op with
/// a `WireColor::Token` payload).
fn hover_sheet() -> Rc<StyleSheet> {
    Rc::new(
        StyleSheet::new(|_vs| StyleRules {
            width: Some(Tokenized::Literal(Length::Px(100.0))),
            background: Some(Tokenized::token("color-surface", Color("#000".into()))),
            ..Default::default()
        })
        .variant("__state_hovered", "on", |_vs| StyleRules {
            background: Some(Tokenized::Literal(Color("#222".into()))),
            ..Default::default()
        }),
    )
}

/// `static`, NOT `const`: Roku's `WireIconData.cache_key` is derived
/// from the paths slice ADDRESS (stable-identity contract — real icon
/// sets are `static`s). A `const` re-materializes its value at every
/// use site, promoting a fresh anonymous array per mention and forking
/// the cache_key between the two mounts; a `static` has one fixed
/// initializer, so both cores read the same pointer.
static TEST_ICON: runtime_core::primitives::icon::IconData =
    runtime_core::primitives::icon::IconData {
        view_box: (24, 24),
        paths: &["M0 0h24v24H0z"],
        fill_rule: runtime_core::primitives::icon::FillRule::NonZero,
        filled: false,
    };

// ===========================================================================
// 1. Full-scene stream snapshot: same tree, both cores, identical bytes
// ===========================================================================

/// The rule-7 gate for this port: a torture scene (nested flex-styled
/// views, text, button, toggle, slider, text input, pressable, image,
/// icon, activity indicator, scroll view, state-overlay styling) built
/// on the OLD core (walker + `runtime_core::mount`) and the NEW core
/// (`newcore::start`, vocabulary handlers) must serialize to
/// byte-identical command streams — NodeIds, HandlerIds, style
/// payloads, ordering, everything.
#[test]
fn newcore_full_scene_command_stream_parity() {
    let (old_backend, owner) = mount_old(|| {
        use runtime_core::primitives::activity_indicator::activity_indicator;
        use runtime_core::primitives::slider::slider;
        use runtime_core::{
            button, icon, image, pressable, scroll_view, signal, text, text_input, toggle, view,
            IntoElement,
        };
        let on = signal(true);
        let val = signal(2.5f32);
        let input = signal(String::from("hi"));
        view(vec![
            view(vec![
                text("hello roku").into_element(),
                text("styled")
                    .with_style(static_style(test_rules(20.0, "#334455")))
                    .into_element(),
            ])
            .with_style(static_style(flex_rules()))
            .into_element(),
            view(vec![])
                .with_style(StyleApplication::new(hover_sheet()))
                .into_element(),
            button("Go", || {}).into_element(),
            toggle(on, |_| {}).into_element(),
            slider(val, |_| {}).range(0.0, 10.0).step(0.5).into_element(),
            text_input(input, |_| {})
                .placeholder("Type here".to_string())
                .into_element(),
            pressable(vec![text("press me").into_element()], || {}).into_element(),
            image("logo.png").alt("Logo".to_string()).into_element(),
            icon(TEST_ICON).into_element(),
            activity_indicator().into_element(),
            scroll_view(vec![text("scrollable").into_element()]).into_element(),
        ])
        .into_element()
    });
    let (new_backend, app) = mount_new(|| {
        use runtime_vocabulary::builders::{
            activity_indicator, button, icon, image, pressable, scroll_view, slider, text,
            text_input, toggle, view,
        };
        use runtime_world::signal;
        let on = signal(true);
        let val = signal(2.5f32);
        let input = signal(String::from("hi"));
        view()
            .child(
                view()
                    .style(static_style(flex_rules()))
                    .child(text().content("hello roku"))
                    .child(text().content("styled").style(static_style(test_rules(20.0, "#334455")))),
            )
            .child(view().style(StyleApplication::new(hover_sheet())))
            .child(button().label("Go").on_press(|| {}))
            .child(toggle().value(on).on_change(|_| {}))
            .child(slider().value(val).on_change(|_| {}).range(0.0, 10.0).step(0.5))
            .child(text_input().value(input).on_change(|_| {}).placeholder("Type here"))
            .child(pressable(|| {}).child(text().content("press me")))
            .child(image().src("logo.png").alt("Logo"))
            .child(icon().data(TEST_ICON))
            .child(activity_indicator())
            .child(scroll_view().child(text().content("scrollable")))
            .build()
    });
    let (old_cmds, old_json) = drain_json(&old_backend);
    let (_, new_json) = drain_json(&new_backend);
    assert_stream_bytes("full_scene", &normalize_sanctioned_old(&old_cmds), &new_json);
    // Sanity: the stream is live (every primitive family present).
    for needle in [
        "CreateView",
        "CreateText",
        "CreateButton",
        "CreateToggle",
        "CreateSlider",
        "CreateTextInput",
        "CreatePressable",
        "CreateImage",
        "CreateIcon",
        "CreateActivityIndicator",
        "CreateScrollView",
        "ApplyStyleStates",
        "Finish",
    ] {
        assert!(old_json.contains(needle), "{needle} emitted:\n{old_cmds:?}");
    }
    drop(owner);
    app.stop();
}

// ===========================================================================
// 2. Portal — Roku wires CreatePortal (viewport + dismiss handler)
// ===========================================================================

/// Portals emit a `CreatePortal` wire op with the placement intent and
/// an on_dismiss HandlerId; both cores must mint the same ids and
/// payload bytes.
#[test]
fn newcore_portal_command_stream_parity() {
    use runtime_core::primitives::portal::{PortalTarget, ViewportPlacement};
    let (old_backend, owner) = mount_old(|| {
        use runtime_core::{portal, text, view, IntoElement};
        view(vec![
            text("under").into_element(),
            portal(
                PortalTarget::Viewport(ViewportPlacement::Center),
                vec![text("modal body").into_element()],
            )
            .on_dismiss(|| {})
            .trap_focus(true)
            .into_element(),
        ])
        .into_element()
    });
    let (new_backend, app) = mount_new(|| {
        use runtime_vocabulary::builders::{portal, text, view};
        view()
            .child(text().content("under"))
            .child(
                portal(PortalTarget::Viewport(ViewportPlacement::Center))
                    .on_dismiss(|| {})
                    .trap_focus(true)
                    .child(text().content("modal body")),
            )
            .build()
    });
    let (old_cmds, old_json) = drain_json(&old_backend);
    let (_, new_json) = drain_json(&new_backend);
    assert_stream_bytes("portal", &normalize_sanctioned_old(&old_cmds), &new_json);
    assert!(old_json.contains("CreatePortal"), "portal emitted:\n{old_cmds:?}");
    assert!(old_json.contains("trap_focus\":true"), "trap_focus shipped:\n{old_json}");
    drop(owner);
    app.stop();
}

// ===========================================================================
// 3. Dyn branch — committed initial state, both branches
// ===========================================================================

#[test]
fn newcore_dyn_branch_parity_both_initial_states() {
    for initial in [true, false] {
        let (old_backend, owner) = mount_old(move || {
            use runtime_core::{signal, text, view, when, IntoElement};
            let show = signal(initial);
            view(vec![
                text("before").into_element(),
                when(
                    move || show.get(),
                    || {
                        view(vec![text("shown").into_element()])
                            .with_style(static_style(test_rules(60.0, "#606060")))
                            .into_element()
                    },
                    || text("hidden").into_element(),
                ),
                text("after").into_element(),
            ])
            .into_element()
        });
        let (new_backend, app) = mount_new(move || {
            use runtime_scene::dyn_keyed;
            use runtime_vocabulary::builders::{text, view};
            use runtime_world::signal;
            let show = signal(initial);
            view()
                .child(text().content("before"))
                .child(dyn_keyed(
                    move || show.get(),
                    move |&on| {
                        if on {
                            view()
                                .style(static_style(test_rules(60.0, "#606060")))
                                .child(text().content("shown"))
                                .build()
                        } else {
                            text().content("hidden").build()
                        }
                    },
                ))
                .child(text().content("after"))
                .build()
        });
        let (old_cmds, old_json) = drain_json(&old_backend);
        let (_, new_json) = drain_json(&new_backend);
        assert_stream_bytes(
            if initial { "dyn_branch_then" } else { "dyn_branch_else" },
            &normalize_sanctioned_old(&old_cmds),
            &new_json,
        );
        let want = if initial { "shown" } else { "hidden" };
        assert!(old_json.contains(want), "committed initial branch emitted:\n{old_cmds:?}");
        drop(owner);
        app.stop();
    }
}

// ===========================================================================
// 4. Keyed list + reorder — mount stream AND the post-event follow-up
// ===========================================================================

/// Keyed rows on both cores, then a device-event-driven reorder (a
/// button handler sets the items signal): the mount streams must match
/// byte-for-byte AND the follow-up streams after the SAME handler
/// invocation must match byte-for-byte (old = synchronous walker
/// reconcile, new = staged writes committed by `settle()`).
#[test]
fn newcore_keyed_list_reorder_follow_up_stream_parity() {
    let (old_backend, owner) = mount_old(|| {
        use runtime_core::{button, each_keyed, signal, text, view, EachKey, EachRowBuild, IntoElement};
        let items = signal(vec![1u32, 2, 3]);
        view(vec![
            text("header").into_element(),
            each_keyed(move || {
                items
                    .get()
                    .into_iter()
                    .map(|n| {
                        let build: EachRowBuild =
                            Box::new(move || vec![text(format!("row-{n}")).into_element()]);
                        (EachKey::new(n), build)
                    })
                    .collect()
            }),
            button("shuffle", move || items.set(vec![3, 1, 2])).into_element(),
        ])
        .into_element()
    });
    let (new_backend, app) = mount_new(|| {
        use runtime_scene::keyed;
        use runtime_vocabulary::builders::{button, text, view};
        use runtime_world::signal;
        let items = signal(vec![1u32, 2, 3]);
        view()
            .child(text().content("header"))
            .child(keyed(
                move || items.get(),
                |n| *n,
                |n| text().content(format!("row-{n}")).build(),
            ))
            .child(button().label("shuffle").on_press(move || items.set(vec![3, 1, 2])))
            .build()
    });
    let (old_cmds, old_json) = drain_json(&old_backend);
    let (new_cmds, new_json) = drain_json(&new_backend);
    assert_stream_bytes("keyed_list_mount", &normalize_sanctioned_old(&old_cmds), &new_json);
    assert!(old_json.contains("row-1") && old_json.contains("row-3"), "rows:\n{old_cmds:?}");

    // Fire the reorder through the HandlerTable — the embedder model —
    // on BOTH cores, then compare the follow-up streams.
    let old_h = unit_handler(&old_backend, button_handler(&old_cmds, "shuffle"));
    old_h();
    test_scheduler::drain(); // old core: run any deferred reconcile work
    let (old_up_cmds, _) = drain_json(&old_backend);

    let new_h = unit_handler(&new_backend, button_handler(&new_cmds, "shuffle"));
    new_h();
    backend_roku::newcore::settle();
    let (_, new_up_json) = drain_json(&new_backend);

    assert_stream_bytes(
        "keyed_list_reorder_follow_up",
        &normalize_sanctioned_old(&old_up_cmds),
        &new_up_json,
    );
    assert!(
        !old_up_cmds.is_empty(),
        "reorder produced wire traffic (the parity compare must not be vacuous)"
    );
    drop(owner);
    app.stop();
}

// ===========================================================================
// 5. Interaction — device event → staged writes → settle() → commands
// ===========================================================================

/// REGRESSION (embedder flush discipline): on the new core a handler's
/// `Signal::set` is STAGED — the command queue stays empty until
/// `settle()` commits. The caps layer wraps every author callback
/// before registration, so the closure the embedder pulls from the
/// HandlerTable already carries the flush scheduling; the follow-up
/// stream after `settle()` must be byte-identical to the old core's
/// after the same handler invocation.
#[test]
fn newcore_button_press_stays_staged_until_settle_then_matches_old() {
    let (old_backend, owner) = mount_old(|| {
        use runtime_core::{button, signal, text, view, IntoElement};
        let count = signal(0i32);
        view(vec![
            text(move || format!("count: {}", count.get())).into_element(),
            button("inc", move || count.set(count.get() + 1)).into_element(),
        ])
        .into_element()
    });
    let (new_backend, app) = mount_new(|| {
        use runtime_vocabulary::builders::{button, text, view};
        use runtime_world::signal;
        let count = signal(0i32);
        view()
            .child(text().content(move || format!("count: {}", count.get())))
            .child(button().label("inc").on_press(move || count.update(|c| c + 1)))
            .build()
    });
    let (old_cmds, old_json) = drain_json(&old_backend);
    let (new_cmds, new_json) = drain_json(&new_backend);
    assert_stream_bytes("counter_mount", &normalize_sanctioned_old(&old_cmds), &new_json);
    assert!(old_json.contains("count: 0"), "initial text emitted:\n{old_cmds:?}");

    // Old core: synchronous — the follow-up UpdateText is in the queue
    // as soon as the handler (plus any deferred effects) ran.
    let old_h = unit_handler(&old_backend, button_handler(&old_cmds, "inc"));
    old_h();
    test_scheduler::drain();
    let (_, old_up_json) = drain_json(&old_backend);
    assert!(old_up_json.contains("count: 1"), "old core follow-up:\n{old_up_json}");

    // New core: the write is STAGED — nothing on the wire until the
    // embedder's settle() fence. This is the staged-write model's whole
    // point; without the assert a synchronous-flush regression would
    // pass silently.
    let new_h = unit_handler(&new_backend, button_handler(&new_cmds, "inc"));
    new_h();
    let staged = new_backend.borrow_mut().drain();
    assert!(
        staged.is_empty(),
        "write must stay staged until settle() commits, got: {staged:?}"
    );
    backend_roku::newcore::settle();
    let (_, new_up_json) = drain_json(&new_backend);
    assert_stream_bytes("counter_follow_up", &old_up_json, &new_up_json);

    drop(owner);
    app.stop();
}

// ===========================================================================
// 6. Teardown — late handler fire + settle after stop() are safe no-ops
// ===========================================================================

/// Boot bookkeeping: `is_booted` tracks start/stop; after `stop()` a
/// late device event (the transport races teardown) dispatched through
/// a retained HandlerTable closure plus a `settle()` must not panic and
/// must emit nothing — the handler's write is a dead-world no-op and
/// the flush driver is uninstalled.
#[test]
fn newcore_stop_makes_late_handler_fire_and_settle_no_ops() {
    let (backend, app) = mount_new(|| {
        use runtime_vocabulary::builders::{button, text, view};
        use runtime_world::signal;
        let count = signal(0i32);
        view()
            .child(text().content(move || format!("n: {}", count.get())))
            .child(button().label("late").on_press(move || count.update(|c| c + 1)))
            .build()
    });
    assert!(backend_roku::newcore::is_booted());
    let (cmds, _) = drain_json(&backend);
    let late = unit_handler(&backend, button_handler(&cmds, "late"));

    app.stop();
    assert!(!backend_roku::newcore::is_booted());

    // The transport delivers a stale event after shutdown.
    late();
    backend_roku::newcore::settle();
    test_scheduler::drain();
    let leftovers = backend.borrow_mut().drain();
    assert!(
        leftovers.is_empty(),
        "post-stop dispatch must emit nothing, got: {leftovers:?}"
    );
    // And a bare schedule_flush is equally inert.
    backend_roku::newcore::schedule_flush();
    test_scheduler::drain();
}

// ===========================================================================
// 7. Splice contract pin
// ===========================================================================

/// Host's structural seam must track the Backend splice contract via
/// delegation, and on Roku that contract is ANCHORED (`false`): the
/// wire's `Slot`/anchor replay model on the BrightScript side assumes
/// anchor nodes exist (`CreateReactiveAnchor` + `ClearChildren`
/// rebuilds); `remove_child`/`insert_at` are walker-side no-op
/// defaults. If either half flips independently, anchor placement
/// diverges between cores and the device runtime's teardown walk
/// breaks.
#[test]
fn newcore_host_splice_delegates_and_is_anchored() {
    use runtime_core::Backend;
    use runtime_scene::Host;
    let b = RokuBackend::new();
    assert_eq!(
        Host::supports_splice(&b),
        Backend::supports_child_splice(&b),
        "Host::supports_splice must delegate to the Backend contract"
    );
    assert!(
        !Host::supports_splice(&b),
        "Roku is an anchored backend — flipping this requires a real \
         remove_child/insert_at wire story AND a device-side runtime to honor it"
    );
}
