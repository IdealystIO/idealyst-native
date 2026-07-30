//! `glue` must carry the reactive + identity + event-payload author
//! surface, not just the subset the framework's own code happened to
//! name.
//!
//! Companion to `glue_host_surface.rs`. Same failure mode: the old
//! `runtime_shared` root was `pub use runtime_shared::*;`, so everything
//! public at the shared root reached authors for free. The facade
//! enumerates, so anything nobody listed silently vanished from
//! `runtime_shared::…` — a product regression, not a test gap.
//!
//! Three groups here:
//!
//! 1. **Reactive helpers that had to be REIMPLEMENTED, not re-exported**
//!    (`on`, `memo_with`, `reducer`). The shared originals build
//!    old-arena signals and effects, which nothing on a world mount
//!    subscribes to — re-exporting them would have been worse than the
//!    gap, because the call would compile and then silently do nothing.
//!    These tests assert the mirrors are LIVE on the world kernel.
//! 2. **`identity`** — a straight re-export, plus an explicit pin on the
//!    ambient-source gap (see `use_id_*` below).
//! 3. **Event payloads / recognizer contract / logging** — path pins for
//!    types an SDK must be able to name.

use runtime_vocabulary::glue;
use runtime_world::World;

fn in_world<R>(f: impl FnOnce() -> R) -> (World, R) {
    let world = World::new();
    let out = world.enter(f);
    (world, out)
}

// ---------------------------------------------------------------------------
// 1. Reactive helpers (world-kernel reimplementations)
// ---------------------------------------------------------------------------

/// `on(deps, f)` fires immediately AND on every dependency change, with
/// the previous value threaded through. (The old root's contract; the
/// only difference from `on_defer` is the first run.)
#[test]
fn glue_on_fires_immediately_then_on_every_change() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let seen: Rc<RefCell<Vec<(i32, Option<i32>)>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();

    let (world, s) = in_world(|| {
        let s = glue::signal(1i32);
        let _ = glue::on(s, move |new, prev| sink.borrow_mut().push((*new, prev.copied())));
        s
    });

    assert_eq!(
        seen.borrow().as_slice(),
        [(1, None)],
        "`on` must run its body on the first pass, unlike `on_defer`"
    );

    world.enter(|| s.set(2));
    world.flush();
    assert_eq!(seen.borrow().as_slice(), [(1, None), (2, Some(1))]);

    // Equal set is guarded by the kernel — no extra fire.
    world.enter(|| s.set(2));
    world.flush();
    assert_eq!(seen.borrow().len(), 2, "a guarded no-op set must not re-fire");
}

/// `on_defer` remains the skip-the-first-run sibling — asserted here so
/// the pair's difference is pinned in one place.
#[test]
fn glue_on_defer_skips_the_first_run() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let seen: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();

    let (world, s) = in_world(|| {
        let s = glue::signal(1i32);
        let _ = glue::on_defer(s, move |new, _prev| sink.borrow_mut().push(*new));
        s
    });
    assert!(seen.borrow().is_empty(), "on_defer records only the baseline");

    world.enter(|| s.set(7));
    world.flush();
    assert_eq!(seen.borrow().as_slice(), [7]);
}

/// `memo_with(eq, f)` gates notification on the CALLER's equality, not
/// `PartialEq`. Here the comparison is a tolerance: values within 1.0 of
/// the last emission are "equal enough" and must not notify, even though
/// `f32: PartialEq` says otherwise.
#[test]
fn glue_memo_with_uses_the_caller_supplied_equality() {
    use std::cell::Cell;
    use std::rc::Rc;

    let fires = Rc::new(Cell::new(0usize));
    let counter = fires.clone();

    let (world, src) = in_world(|| {
        let src = glue::signal(0.0f32);
        let m = glue::memo_with(|a: &f32, b: &f32| (a - b).abs() < 1.0, move || src.get());
        let _ = glue::effect(move || {
            let _ = m.get();
            counter.set(counter.get() + 1);
        });
        src
    });

    let baseline = fires.get();

    // Within tolerance ⇒ the memo does not re-emit.
    world.enter(|| src.set(0.5));
    world.flush();
    assert_eq!(
        fires.get(),
        baseline,
        "a change inside the caller's tolerance must not notify"
    );

    // Outside tolerance ⇒ it does.
    world.enter(|| src.set(9.0));
    world.flush();
    assert_eq!(
        fires.get(),
        baseline + 1,
        "a change outside the caller's tolerance must notify exactly once"
    );
}

/// `memo_with`'s value is live and readable before any flush (seeded
/// untracked at construction, as the old core did).
#[test]
fn glue_memo_with_is_seeded_before_the_first_flush() {
    let (_world, value) = in_world(|| {
        let src = glue::signal(21i32);
        let m = glue::memo_with(|a: &i32, b: &i32| a == b, move || src.get() * 2);
        m.get()
    });
    assert_eq!(value, 42);
}

/// `reducer(initial, fold)` returns `(state, dispatch)`; each dispatch
/// folds the CURRENT state with the action, and two dispatches in one
/// event turn compose (the staged-update contract) instead of both
/// reading the committed value.
#[test]
fn glue_reducer_folds_state_and_composes_within_one_turn() {
    enum Action {
        Inc,
        Add(i32),
        Reset,
    }

    let (world, (state, dispatch)) = in_world(|| {
        glue::reducer(0i32, |n: &i32, a: Action| match a {
            Action::Inc => n + 1,
            Action::Add(k) => n + k,
            Action::Reset => 0,
        })
    });

    dispatch(Action::Inc);
    dispatch(Action::Inc);
    world.flush();
    assert_eq!(
        world.enter(|| state.peek()),
        2,
        "two dispatches in one turn must compose (0→1→2), not both read the committed 0"
    );

    dispatch(Action::Add(10));
    world.flush();
    assert_eq!(world.enter(|| state.peek()), 12);

    dispatch(Action::Reset);
    world.flush();
    assert_eq!(world.enter(|| state.peek()), 0);
}

/// The historical "every dispatch notifies" contract: a fold back to an
/// EQUAL state still wakes subscribers. The kernel's `set`/`update` are
/// equality-guarded, so this only holds because `reducer` follows the
/// fold with a `touch`.
#[test]
fn glue_reducer_notifies_even_when_the_fold_is_a_no_op() {
    use std::cell::Cell;
    use std::rc::Rc;

    let fires = Rc::new(Cell::new(0usize));
    let counter = fires.clone();

    let (world, dispatch) = in_world(|| {
        let (state, dispatch) = glue::reducer(0i32, |n: &i32, _a: ()| *n);
        let _ = glue::effect(move || {
            let _ = state.get();
            counter.set(counter.get() + 1);
        });
        dispatch
    });

    let baseline = fires.get();
    dispatch(());
    world.flush();
    assert_eq!(
        fires.get(),
        baseline + 1,
        "a dispatch must notify even when the folded state is unchanged"
    );
}

// ---------------------------------------------------------------------------
// 2. Identity
// ---------------------------------------------------------------------------

/// `use_id` / `use_id_keyed` / `hash_key` / `Identity` /
/// `with_current_identity` reach authors again. The deletion baseline
/// lists `runtime_shared::identity` as an explicit survivor (§4.2), so
/// their absence from the facade was an oversight.
#[test]
fn glue_reexports_the_identity_surface() {
    let a = glue::Identity::node(glue::Identity::UNIDENTIFIED, 0, None, None);
    let b = glue::Identity::node(glue::Identity::UNIDENTIFIED, 1, None, None);
    assert_ne!(a, b, "distinct slots must produce distinct identities");

    let in_a = glue::with_current_identity(a, glue::use_id);
    let in_b = glue::with_current_identity(b, glue::use_id);
    assert_ne!(in_a, in_b, "use_id must discriminate by ambient identity");
    assert!(in_a.starts_with("ui-"), "documented format, got {in_a}");

    let keyed_name = glue::with_current_identity(a, || glue::use_id_keyed("name"));
    let keyed_age = glue::with_current_identity(a, || glue::use_id_keyed("age"));
    assert_ne!(keyed_name, keyed_age);

    assert_eq!(glue::hash_key("x"), glue::hash_key("x"));
    assert_eq!(glue::current_identity(), glue::Identity::UNIDENTIFIED);
}

/// ⚠️ **Known gap, pinned rather than hidden.**
///
/// `use_id()`'s documented contract is "deterministic per position in the
/// tree": the OLD walker set the ambient identity before every emission
/// (`runtime-core/src/walker.rs::build` → `with_current_identity`), so
/// two components at different positions got different ids. The
/// surviving renderer (`runtime_scene::realize`) does NOT set an ambient
/// identity, so `use_id()` answers from `Identity::UNIDENTIFIED`
/// everywhere and every call site in a tree returns the SAME string.
///
/// It is stable and non-panicking, so nothing crashes — which is exactly
/// why it needs a test: a silently-degraded id generator is the kind of
/// thing that ships. This test documents the current answer. When the
/// renderer starts seeding identity per mount site (the same change that
/// restores the dev-server recorder's identity-keyed `NodeId` reuse —
/// see `dev_server::newcore`'s module docs), this test goes red and
/// should be replaced with a per-position-uniqueness assertion.
#[test]
fn use_id_is_currently_position_independent_because_the_renderer_seeds_no_identity() {
    let outside = glue::use_id();
    let unidentified = glue::with_current_identity(glue::Identity::UNIDENTIFIED, glue::use_id);
    assert_eq!(
        outside, unidentified,
        "with no ambient identity, use_id answers from the UNIDENTIFIED sentinel"
    );
}

// ---------------------------------------------------------------------------
// 3. Event payloads, recognizer contract, logging, the handler fork
// ---------------------------------------------------------------------------

/// The payload types the exported `*Handler` aliases carry. Without
/// these an SDK cannot spell its own handler's argument type.
#[test]
fn glue_reexports_the_event_payload_types() {
    // Path pins — construction shape is the shared type's business; what
    // matters is that these names resolve through the facade and are the
    // SAME types the handler aliases use.
    fn _drop_handler(f: glue::FileDropHandler) -> glue::FileDropHandler {
        f
    }
    fn _takes_drop(ev: &glue::FileDropEvent) -> bool {
        matches!(ev.phase, glue::FileDropPhase::Dropped(_))
    }
    fn _takes_file(f: &glue::DroppedFile) -> String {
        f.name.clone()
    }
    fn _wheel_handler(f: glue::WheelHandler) -> glue::WheelHandler {
        f
    }
    fn _takes_wheel(ev: &glue::WheelEvent) -> (glue::WheelKind, f32) {
        (ev.kind, ev.delta_y)
    }

    // Same type by identity, not a parallel definition.
    let _: fn(&runtime_shared::WheelEvent) -> (glue::WheelKind, f32) = _takes_wheel;
    let _: fn(&runtime_shared::FileDropEvent) -> bool = _takes_drop;
}

/// The gesture-recognizer contract the pan / zoom / dnd SDKs implement.
#[test]
fn glue_reexports_the_recognizer_contract() {
    fn _ctx() -> &'static glue::RecognizerCtx {
        &glue::RecognizerCtx::UNGATED
    }
    fn _kinds() -> [glue::RecognizerKind; 2] {
        [glue::RecognizerKind::Discrete, glue::RecognizerKind::Continuous]
    }
    fn _states() -> glue::GestureState {
        glue::GestureState::Possible
    }
    fn _update(s: glue::GestureState) -> glue::RecognizerUpdate {
        glue::RecognizerUpdate::new(s, runtime_shared::TouchResponse::default())
    }
    fn _obj(r: &dyn glue::Recognizer) -> glue::RecognizerKind {
        r.kind()
    }
    fn _notifier(n: glue::AsyncNotifier) -> glue::AsyncNotifier {
        n
    }
    let _ = (_ctx as usize, _kinds as usize, _states as usize, _update as usize, _obj as usize, _notifier as usize);
}

/// Touch-claim arbitration + microtask scheduling + the logging module.
#[test]
fn glue_reexports_touch_claim_microtask_and_logging() {
    use std::cell::Cell;
    use std::rc::Rc;

    // Touch claim round-trips through the shared thread-local.
    let claimed = Rc::new(Cell::new(false));
    let flag = claimed.clone();
    glue::set_active_touch_claim(Some(Rc::new(move || flag.set(true))));
    let claim = glue::active_touch_claim().expect("the installed claim must read back");
    claim();
    assert!(claimed.get());
    glue::set_active_touch_claim(None);
    assert!(glue::active_touch_claim().is_none());

    // `schedule_microtask` resolves at the root (no host installed here,
    // so it must not panic — the scheduling registry no-ops).
    glue::schedule_microtask(|| {});

    // The logging module resolves and its level enum is nameable.
    let _: glue::logging::LogLevel = glue::logging::LogLevel::Warn;
}

/// The handler-safe fork. `world_is_entered()` is the public spelling of
/// the probe SDKs branch on to decide "inject the ambient world context"
/// vs "use a build-time-captured handle".
#[test]
fn glue_world_is_entered_reports_the_ambient_world() {
    assert!(
        !glue::world_is_entered(),
        "no world ambient at test entry"
    );
    let world = World::new();
    let inside = world.enter(glue::world_is_entered);
    assert!(inside, "inside enter, the probe must report true");
    assert!(
        !glue::world_is_entered(),
        "the probe must fall back to false after enter returns — this is the \
         event-handler case, and getting it wrong is the theme-swap panic"
    );
}
