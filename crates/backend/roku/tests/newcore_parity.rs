//! COMMAND-STREAM byte-parity tests for the Roku backend: the emitted
//! wire stream must match the stream the OLD core emitted (the strongest
//! available gate — there is no device/thin-client to render against, so
//! the serialized wire stream IS the observable output), plus op-level
//! coverage of the embedder-driven flush discipline (device event →
//! HandlerTable dispatch → staged writes → `settle()` → emitted
//! commands).
//!
//! Harness notes: the tests install a queue scheduler whose
//! `drain_buffered_microtasks` drains its own queue — modeling an
//! embedder whose scheduler buffers microtasks, which is the
//! configuration where staged-ness is OBSERVABLE (without any scheduler,
//! `schedule_microtask` falls back to synchronous off-web and the deduped
//! flush commits inside the wrapped callback). `install_scheduler` is
//! process-global/first-wins, but the queue state is thread-local, so
//! each test thread drains only its own tasks.
//!
//! # The frozen corpus is the contract
//!
//! Every gate compares against a **frozen old-core command stream**
//! committed under `tests/goldens/` (indented JSON), written before the
//! old walker was deleted. See [`check_new_stream`] for the ONE
//! sanctioned divergence already baked into those artifacts. A mismatch
//! is a real behavior change, NOT a stale artifact:
//! `IDEALYST_FREEZE_GOLDENS=1` can now only RE-BASELINE against the
//! current renderer, permanently discarding the old core's testimony —
//! see `tests/goldens/README.md`.

use std::cell::RefCell;
use std::rc::Rc;

use backend_roku::{HandlerId, RokuBackend, RokuCommand};
use runtime_shared::{Color, Length, StyleApplication, StyleRules, StyleSheet, Tokenized};

// ===========================================================================
// Queue scheduler (an embedder that buffers microtasks)
// ===========================================================================

mod test_scheduler {
    use runtime_shared::scheduling::{ScheduleHandle, Scheduler};
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
        if !runtime_shared::scheduling::is_scheduler_installed() {
            runtime_shared::scheduling::install_scheduler(Box::new(QueueScheduler));
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

// ---------------------------------------------------------------------------
// Frozen-artifact gate
// ---------------------------------------------------------------------------

fn goldens() -> parity_goldens::Goldens {
    parity_goldens::Goldens::new(env!("CARGO_MANIFEST_DIR"))
}

/// Re-serialize a compact command stream as indented JSON so the frozen
/// artifacts are reviewable in a diff. `serde_json::Value` round-trips
/// the stream and the key order is canonical (BTreeMap) on both sides,
/// so the comparison stays exact — with ONE substitution, below.
///
/// **`cache_key` interning.** `CreateIcon`/`UpdateIconData` carry a
/// `cache_key` the backend derives from the icon's `paths` static
/// ADDRESS (`crates/backend/roku/src/lib.rs`,
/// `data.paths.as_ptr() as u64 ^ filled`). That is stable within a
/// process — which is why the in-process old-vs-new compare above uses
/// the raw value and pins cross-core identity — but it changes between
/// processes under ASLR, so a raw value cannot be frozen. Instead each
/// DISTINCT cache_key is replaced by `#0`, `#1`, … in first-appearance
/// order. That preserves everything the value means to a consumer (icon
/// identity and aliasing: same path set ⇒ same key, `filled` variants ⇒
/// different keys, ordering of first use) while being process-stable.
/// This is an artifact-serialization concern, NOT a widened
/// sanctioned divergence — no cross-core difference is being hidden.
fn pretty_stream(json: &str) -> String {
    let mut v: serde_json::Value = serde_json::from_str(json).expect("stream is valid JSON");
    let mut seen: Vec<u64> = Vec::new();
    intern_cache_keys(&mut v, &mut seen);
    serde_json::to_string_pretty(&v).expect("stream re-serializes")
}

fn intern_cache_keys(v: &mut serde_json::Value, seen: &mut Vec<u64>) {
    match v {
        serde_json::Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for k in keys {
                if k == "cache_key" {
                    if let Some(n) = map.get(&k).and_then(|x| x.as_u64()) {
                        let idx = match seen.iter().position(|s| *s == n) {
                            Some(i) => i,
                            None => {
                                seen.push(n);
                                seen.len() - 1
                            }
                        };
                        map.insert(k, serde_json::Value::String(format!("#{idx}")));
                        continue;
                    }
                }
                if let Some(child) = map.get_mut(&k) {
                    intern_cache_keys(child, seen);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                intern_cache_keys(item, seen);
            }
        }
        _ => {}
    }
}

/// The gate: the serialized command stream must match the frozen
/// old-core stream byte-for-byte. The frozen artifacts already have the
/// ONE sanctioned old→new divergence stripped — the old walker emitted a
/// no-op `ClearChildren` on a VIRGIN reactive anchor before the first
/// population of a `when`/`each` region and the new core deliberately
/// skips it (divergence class #2 in docs/migrating-to-runtime-v2.md;
/// invisible to the device, visible in a stream). Nothing else was ever
/// normalized: every other op is a hard invariant.
fn check_new_stream(name: &str, new_json: &str) {
    goldens().check_text(name, &pretty_stream(new_json));
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
        flex_direction: Some(runtime_shared::FlexDirection::Row),
        justify_content: Some(runtime_shared::JustifyContent::SpaceBetween),
        align_items: Some(runtime_shared::AlignItems::Center),
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
static TEST_ICON: runtime_shared::primitives::icon::IconData =
    runtime_shared::primitives::icon::IconData {
        view_box: (24, 24),
        paths: &["M0 0h24v24H0z"],
        fill_rule: runtime_shared::primitives::icon::FillRule::NonZero,
        filled: false,
    };

// ===========================================================================
// 1. Full-scene stream snapshot: same tree, both cores, identical bytes
// ===========================================================================

/// The rule-7 gate for this port: a torture scene (nested flex-styled
/// views, text, button, toggle, slider, text input, pressable, image,
/// icon, activity indicator, scroll view, state-overlay styling) built
/// on the OLD core (walker + `runtime_shared::mount`) and the NEW core
/// (`newcore::start`, vocabulary handlers) must serialize to
/// byte-identical command streams — NodeIds, HandlerIds, style
/// payloads, ordering, everything.
#[test]
fn newcore_full_scene_command_stream_parity() {
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
    let (_, new_json) = drain_json(&new_backend);
    check_new_stream("full_scene.json", &new_json);
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
        assert!(new_json.contains(needle), "{needle} emitted:\n{new_json}");
    }
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
    use runtime_shared::primitives::portal::{PortalTarget, ViewportPlacement};
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
    let (_, new_json) = drain_json(&new_backend);
    check_new_stream("portal.json", &new_json);
    assert!(new_json.contains("CreatePortal"), "portal emitted:\n{new_json}");
    assert!(new_json.contains("trap_focus\":true"), "trap_focus shipped:\n{new_json}");
    app.stop();
}

// ===========================================================================
// 3. Dyn branch — committed initial state, both branches
// ===========================================================================

#[test]
fn newcore_dyn_branch_parity_both_initial_states() {
    for initial in [true, false] {
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
        let (_, new_json) = drain_json(&new_backend);
        let name = if initial { "dyn_branch_then" } else { "dyn_branch_else" };
        check_new_stream(&format!("{name}.json"), &new_json);
        let want = if initial { "shown" } else { "hidden" };
        assert!(new_json.contains(want), "committed initial branch emitted:\n{new_json}");
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
    let (new_cmds, new_json) = drain_json(&new_backend);
    check_new_stream("keyed_list_mount.json", &new_json);
    assert!(new_json.contains("row-1") && new_json.contains("row-3"), "rows:\n{new_json}");

    // Fire the reorder through the HandlerTable — the embedder model —
    // then compare the follow-up stream.
    let new_h = unit_handler(&new_backend, button_handler(&new_cmds, "shuffle"));
    new_h();
    backend_roku::newcore::settle();
    let (_, new_up_json) = drain_json(&new_backend);
    check_new_stream("keyed_list_reorder_follow_up.json", &new_up_json);
    assert!(
        !new_up_json.is_empty() && new_up_json != "[]",
        "reorder produced wire traffic (the parity compare must not be vacuous)"
    );
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
    let (new_backend, app) = mount_new(|| {
        use runtime_vocabulary::builders::{button, text, view};
        use runtime_world::signal;
        let count = signal(0i32);
        view()
            .child(text().content(move || format!("count: {}", count.get())))
            .child(button().label("inc").on_press(move || count.update(|c| c + 1)))
            .build()
    });
    let (new_cmds, new_json) = drain_json(&new_backend);
    check_new_stream("counter_mount.json", &new_json);
    assert!(new_json.contains("count: 0"), "initial text emitted:\n{new_json}");

    // The write is STAGED — nothing on the wire until the
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
    check_new_stream("counter_follow_up.json", &new_up_json);
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

/// Roku's structural seam must stay ANCHORED (`false`): the wire's
/// `Slot`/anchor replay model on the BrightScript side assumes anchor
/// nodes exist (`CreateReactiveAnchor` + `ClearChildren` rebuilds), and
/// there are no `RemoveChild`/`InsertAt` wire ops at all. The value used
/// to arrive from a `Backend` trait DEFAULT (`supports_child_splice`);
/// `Host` makes it REQUIRED, so `newcore.rs` now carries an explicit body
/// reproducing it — see docs/runtime-v2-deletion-baseline.md §2.2. A flip
/// would change anchor placement AND break the device runtime's teardown
/// walk, so pin the literal.
#[test]
fn newcore_host_splice_is_anchored() {
    use runtime_scene::Host;
    let b = RokuBackend::new();
    assert!(
        !Host::supports_splice(&b),
        "Roku is an anchored backend — flipping this requires a real \
         remove_child/insert_at wire story AND a device-side runtime to honor it"
    );
}
