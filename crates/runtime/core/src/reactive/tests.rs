use super::*;

#[test]
fn signal_is_copy_and_works() {
    let s = Signal::new(7i32);
    let s2 = s; // Copy: no .clone() needed.
    s.set(42);
    assert_eq!(s2.get(), 42);
}

#[test]
fn idle_hook_coalesces_fanout_to_one_window() {
    // Regression: selecting a row in a list where every visible row
    // subscribes to the selection signal must not run one layout pass per
    // row. The reactive-idle hook (a backend's synchronous layout flush)
    // fires at REACTIVE_BUSY→0; a single `Signal::set` that wakes N
    // subscriber effects must be ONE mutation window, not N. On macOS the
    // unguarded path produced N full layout passes per selection — O(N²).
    use std::cell::Cell;
    use std::rc::Rc;

    let fires = Rc::new(Cell::new(0u32));
    let f = fires.clone();
    install_reactive_idle_hook(Rc::new(move || f.set(f.get() + 1)));

    // N effects, all reading the SAME signal (the "every row reads
    // `selected`" shape).
    let sel = Signal::new(0i32);
    const N: usize = 64;
    let mut keep = Vec::with_capacity(N);
    for _ in 0..N {
        keep.push(Effect::new(move || {
            let _ = sel.get();
        }));
    }

    // Ignore the N initial creation runs; measure only the fan-out.
    fires.set(0);
    sel.set(1);

    let fired = fires.get();
    // Value-write window + fan-out window = 2. The old per-effect path
    // fired N+1 (one per subscriber), which is the bug.
    assert!(
        fired <= 2,
        "fan-out of {N} subscribers must coalesce to a single reactive \
         window (≤2 idle fires); got {fired} — run_effects lost its outer \
         busy guard"
    );

    // Don't leak the counting hook onto this worker thread's later tests.
    install_reactive_idle_hook(Rc::new(|| {}));
    drop(keep);
}

#[test]
fn effect_fires_on_change() {
    use std::cell::Cell;
    use std::rc::Rc;
    let count = Signal::new(0i32);
    let observed = Rc::new(Cell::new(0));
    let obs = observed.clone();
    let _e = Effect::new(move || {
        obs.set(count.get());
    });
    assert_eq!(observed.get(), 0);
    count.set(5);
    assert_eq!(observed.get(), 5);
    count.set(11);
    assert_eq!(observed.get(), 11);
}

/// `Effect::persist` outside any reactive scope must keep the effect
/// reacting. A bare handle dropped at end-of-statement (no scope to
/// adopt it) would cancel; `persist` pins it instead. This is the
/// behaviour `doc_controls.rs` relies on when its controls are built
/// ad-hoc / in tests outside a render scope.
#[test]
fn persist_keeps_effect_alive_outside_scope() {
    use std::cell::Cell;
    use std::rc::Rc;
    let src = Signal::new(0i32);
    let runs = Rc::new(Cell::new(0u32));
    let runs_for_effect = runs.clone();
    Effect::new(move || {
        let _ = src.get();
        runs_for_effect.set(runs_for_effect.get() + 1);
    })
    .persist();
    assert_eq!(runs.get(), 1, "effect runs once at creation");
    src.set(1);
    assert_eq!(
        runs.get(),
        2,
        "persisted effect must re-fire on signal change (handle was not held)"
    );
}

/// Contrast for [`persist_keeps_effect_alive_outside_scope`]: WITHOUT
/// `persist`, dropping the handle outside a scope cancels the effect —
/// the exact regression `persist` (and the prior `mem::forget`) guards
/// against.
#[test]
fn dropped_effect_outside_scope_does_not_refire() {
    use std::cell::Cell;
    use std::rc::Rc;
    let src = Signal::new(0i32);
    let runs = Rc::new(Cell::new(0u32));
    let runs_for_effect = runs.clone();
    drop(Effect::new(move || {
        let _ = src.get();
        runs_for_effect.set(runs_for_effect.get() + 1);
    }));
    assert_eq!(runs.get(), 1);
    src.set(1);
    assert_eq!(runs.get(), 1, "dropped effect must not re-fire");
}

/// Regression test for the "self-writing effect breaks after first
/// flip" bug. An effect that bridges two signals — reads from
/// `value`, writes to `shadow` — used to corrupt its own
/// subscription set on the recursive re-fire from `shadow.set`,
/// since `run_effect` calls `clear_effect_dependencies` at the
/// start of every (re-)entry. After fix: re-entrant invocations
/// of the same effect are short-circuited so the outer run's
/// dep recording isn't wiped.
#[test]
fn effect_with_self_write_keeps_firing() {
    use std::cell::Cell;
    use std::rc::Rc;
    let value = Signal::new(0i32);
    let shadow = Signal::new(0i32);
    let mirror_runs = Rc::new(Cell::new(0));
    let r = mirror_runs.clone();
    let _e = Effect::new(move || {
        let v = value.get();
        // Reads `shadow` AND writes it. Pre-fix, the second
        // value.set below leaves the effect dead because the
        // recursive shadow.set wiped its `value` subscription.
        if shadow.get() != v {
            shadow.set(v);
        }
        r.set(r.get() + 1);
    });
    assert_eq!(mirror_runs.get(), 1);
    assert_eq!(shadow.get(), 0);

    value.set(1);
    assert_eq!(shadow.get(), 1);
    let after_first = mirror_runs.get();
    assert!(after_first >= 2, "effect should re-run after first value.set");

    value.set(2);
    assert_eq!(
        shadow.get(),
        2,
        "shadow should track value after the second flip too"
    );
    assert!(
        mirror_runs.get() > after_first,
        "effect must fire again after the second value.set — before \
         the fix this was the broken case"
    );
}

// -----------------------------------------------------------------
// Context (provide / inject)
// -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct Theme(&'static str);

#[derive(Clone, Debug, PartialEq)]
struct Locale(&'static str);

#[test]
fn inject_returns_none_without_provider() {
    let mut scope = Scope::new();
    let result: Option<Theme> = with_scope(&mut scope, || inject::<Theme>());
    assert_eq!(result, None);
}

#[test]
fn provide_then_inject_in_same_scope() {
    let mut scope = Scope::new();
    let result = with_scope(&mut scope, || {
        provide(Theme("dark"));
        inject::<Theme>()
    });
    assert_eq!(result, Some(Theme("dark")));
}

#[test]
fn inject_finds_outer_provision_from_inner_scope() {
    let mut outer = Scope::new();
    let result = with_scope(&mut outer, || {
        provide(Theme("dark"));
        let mut inner = Scope::new();
        with_scope(&mut inner, || inject::<Theme>())
    });
    assert_eq!(result, Some(Theme("dark")));
}

#[test]
fn inner_provision_shadows_outer() {
    let mut outer = Scope::new();
    let result = with_scope(&mut outer, || {
        provide(Theme("light"));
        let mut inner = Scope::new();
        let inner_result = with_scope(&mut inner, || {
            provide(Theme("dark"));
            inject::<Theme>()
        });
        // After inner scope drops, the inner provision is gone —
        // outer's "light" is visible again.
        let outer_after = inject::<Theme>();
        (inner_result, outer_after)
    });
    assert_eq!(result, (Some(Theme("dark")), Some(Theme("light"))));
}

#[test]
fn different_types_coexist() {
    let mut scope = Scope::new();
    let (theme, locale) = with_scope(&mut scope, || {
        provide(Theme("dark"));
        provide(Locale("ja-JP"));
        (inject::<Theme>(), inject::<Locale>())
    });
    assert_eq!(theme, Some(Theme("dark")));
    assert_eq!(locale, Some(Locale("ja-JP")));
}

#[test]
fn provision_dies_with_scope() {
    let mut scope = Scope::new();
    with_scope(&mut scope, || provide(Theme("dark")));
    drop(scope);
    // No active scope at all → inject returns None (also exercises
    // the no-active-scope branch in inject).
    assert_eq!(inject::<Theme>(), None);
}

#[test]
fn inject_or_falls_back_to_default() {
    let mut scope = Scope::new();
    let value = with_scope(&mut scope, || inject_or(Theme("default")));
    assert_eq!(value, Theme("default"));
}

#[test]
fn with_inject_reads_by_reference() {
    // Use a non-Clone type to prove `with_inject` doesn't need
    // Clone — only `inject` / `inject_or` do.
    struct NonClone(i32);
    let mut scope = Scope::new();
    let result: Option<i32> = with_scope(&mut scope, || {
        provide(NonClone(42));
        with_inject::<NonClone, _>(|v| v.0)
    });
    assert_eq!(result, Some(42));
}

#[test]
fn provided_signal_is_reactive_for_descendants() {
    use std::cell::Cell;
    use std::rc::Rc;
    // The classic theme-switch pattern: provide a Signal<Theme>;
    // descendant effects subscribe by reading `.get()`.
    let mut scope = Scope::new();
    let observed = Rc::new(Cell::new(""));
    let theme_signal = with_scope(&mut scope, || {
        let theme = Signal::new("light");
        provide(theme);
        let obs = observed.clone();
        let _e = Effect::new(move || {
            let t = inject::<Signal<&'static str>>().expect("provided above");
            obs.set(t.get());
        });
        theme
    });
    assert_eq!(observed.get(), "light");
    theme_signal.set("dark");
    assert_eq!(observed.get(), "dark", "descendant must see signal updates");
}

#[test]
#[should_panic(expected = "outside any active reactive scope")]
fn provide_outside_scope_panics() {
    provide(Theme("nope"));
}

#[test]
#[should_panic(expected = "memo's compute closure")]
fn provide_inside_memo_compute_panics() {
    // `provide` is a side effect that would attach to the
    // memo-creation scope and accumulate duplicates on each
    // recompute. Same guard as `Signal::set`.
    let trigger = Signal::new(0i32);
    let _m = memo(move || {
        let _ = trigger.get();
        provide(Theme("dark")); // ← violation
        7
    });
}

// -----------------------------------------------------------------
// Memo write-during-compute: hard panic
// -----------------------------------------------------------------

#[test]
#[should_panic(expected = "memo's compute closure")]
fn memo_write_during_compute_panics() {
    // A memo whose compute closure writes to a signal — the panic
    // points at the offending write, not the downstream cascade.
    let trigger = Signal::new(0i32);
    let side = Signal::new(0i32);
    let _m = memo(move || {
        let _ = trigger.get();
        side.set(42); // ← violation
        7
    });
}

#[test]
#[should_panic(expected = "memo's compute closure")]
fn memo_update_during_compute_panics() {
    // `update` goes through the same guard as `set`.
    let trigger = Signal::new(0i32);
    let side = Signal::new(0i32);
    let _m = memo(move || {
        let _ = trigger.get();
        side.update(|v| *v += 1);
        7
    });
}

#[test]
fn memo_writing_to_own_output_signal_does_not_panic() {
    // Sanity: the memo's internal `signal.set(new)` (when the
    // computed value differs from `last`) must not be caught by the
    // guard. The guard scope is tight to the user's `f()` only.
    let source = Signal::new(1i32);
    let mut scope = Scope::new();
    let m = with_scope(&mut scope, || memo(move || source.get() * 2));
    assert_eq!(m.get(), 2);
    source.set(5);
    assert_eq!(m.get(), 10, "memo updates its output signal normally");
}

// -----------------------------------------------------------------
// batch()
// -----------------------------------------------------------------

#[test]
fn batch_coalesces_fan_out_to_one_run_per_effect() {
    use std::cell::Cell;
    use std::rc::Rc;
    let a = Signal::new(0i32);
    let b = Signal::new(0i32);
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    let _e = Effect::new(move || {
        let _ = a.get() + b.get();
        r.set(r.get() + 1);
    });
    assert_eq!(runs.get(), 1, "effect runs once on creation");

    batch(|| {
        a.set(5);
        b.set(7);
        a.set(8);
    });
    assert_eq!(
        runs.get(),
        2,
        "three writes inside a batch produce one re-run, not three"
    );
}

#[test]
fn batch_nested_only_flushes_at_outermost() {
    use std::cell::Cell;
    use std::rc::Rc;
    let a = Signal::new(0i32);
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    let _e = Effect::new(move || {
        let _ = a.get();
        r.set(r.get() + 1);
    });
    assert_eq!(runs.get(), 1);

    batch(|| {
        a.set(1);
        // Inner batch must not flush — the outer should keep
        // collecting and fire `_e` exactly once at its own end.
        batch(|| {
            a.set(2);
        });
        assert_eq!(runs.get(), 1, "no flush during inner batch");
        a.set(3);
    });
    assert_eq!(runs.get(), 2, "outermost batch flushes once at exit");
}

#[test]
fn batch_returns_inner_result() {
    let value = batch(|| 42);
    assert_eq!(value, 42);
}

#[test]
#[should_panic(expected = "read re-entrantly while it was mid-mutation")]
fn read_during_own_mutation_reports_reentrancy_not_scope_drop() {
    // A non-batched `update` whose closure writes ANOTHER signal whose
    // synchronous fan-out wakes an effect that reads the target while the
    // target's box is moved out. `with_signal` returns `None` (slot taken,
    // generation still matches), and `get()` must report the re-entrancy —
    // NOT the misleading "signal used after its scope was dropped", which
    // is the genuinely-freed (generation-mismatch) case. This is the same
    // re-entrancy class the `async_reducer` `cycle` wrap defers; here it's
    // reached through a plain unbatched `update` to show the diagnostic
    // fires regardless of the trigger.
    let a = Signal::new(0i32);
    let b = Signal::new(0i32);
    // Effect subscribed to `b` that reads `a`.
    let _e = Effect::new(move || {
        let _ = b.get();
        let _ = a.get();
    });
    // `a`'s box is taken for this closure; `b.set(1)` fans out
    // synchronously (no batch) and wakes the effect, which reads `a`.
    a.update(|x| {
        *x += 1;
        b.set(1);
    });
}

// -----------------------------------------------------------------
// Change-detection (dedup) on set_if_changed / update_if_changed
// -----------------------------------------------------------------

/// Build an effect that reads `sig` and counts its runs.
fn watch<T: Clone + 'static>(sig: Signal<T>) -> (Effect, std::rc::Rc<std::cell::Cell<u32>>) {
    use std::cell::Cell;
    use std::rc::Rc;
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    let e = Effect::new(move || {
        let _ = sig.get();
        r.set(r.get() + 1);
    });
    (e, runs)
}

#[test]
fn set_if_changed_skips_when_value_unchanged() {
    let a = Signal::new(7i32);
    let (_e, runs) = watch(a);
    assert_eq!(runs.get(), 1, "initial effect run");
    a.set_if_changed(7); // same value
    assert_eq!(runs.get(), 1, "no re-run on no-op set");
    a.set_if_changed(8); // real change
    assert_eq!(runs.get(), 2, "re-run on real change");
}

#[test]
fn set_still_notifies_when_value_unchanged() {
    // The always-notify primitive must keep firing — monotonic
    // counters etc. rely on it.
    let a = Signal::new(7i32);
    let (_e, runs) = watch(a);
    assert_eq!(runs.get(), 1);
    a.set(7); // same value, but `set` always notifies
    assert_eq!(runs.get(), 2);
}

#[test]
fn set_if_changed_net_zero_batch_skips_fanout() {
    // The headline case: A -> B -> A within one batch nets to no
    // change, so the subscriber must NOT wake — even though each
    // individual step was a real change a per-write compare would
    // have notified on.
    let a = Signal::new(1i32);
    let (_e, runs) = watch(a);
    assert_eq!(runs.get(), 1);
    batch(|| {
        a.set_if_changed(2);
        a.set_if_changed(1); // back to the window-initial value
    });
    assert_eq!(runs.get(), 1, "net-zero window must not fan out");
}

#[test]
fn set_if_changed_net_change_batch_fires_once() {
    let a = Signal::new(1i32);
    let (_e, runs) = watch(a);
    assert_eq!(runs.get(), 1);
    batch(|| {
        a.set_if_changed(2);
        a.set_if_changed(3); // net change 1 -> 3
    });
    assert_eq!(runs.get(), 2, "net change fans out exactly once");
}

#[test]
fn plain_set_taints_batch_window_forcing_notify() {
    // A plain `set` anywhere in the window forces notification even
    // if the net value is unchanged and a `set_if_changed` also ran.
    let a = Signal::new(1i32);
    let (_e, runs) = watch(a);
    assert_eq!(runs.get(), 1);
    batch(|| {
        a.set_if_changed(1); // no-op on its own...
        a.set(1); // ...but a force-write taints the window
    });
    assert_eq!(runs.get(), 2, "force-write notifies despite net-zero");
}

#[test]
fn set_if_changed_nan_always_notifies() {
    // NaN != NaN, so a NaN-valued set is never "unchanged".
    let a = Signal::new(0.0f64);
    let (_e, runs) = watch(a);
    assert_eq!(runs.get(), 1);
    a.set_if_changed(f64::NAN);
    assert_eq!(runs.get(), 2, "0.0 -> NaN is a change");
    a.set_if_changed(f64::NAN);
    assert_eq!(runs.get(), 3, "NaN -> NaN still notifies (NaN != NaN)");
}

#[test]
fn update_if_changed_dedups() {
    let a = Signal::new(5i32);
    let (_e, runs) = watch(a);
    assert_eq!(runs.get(), 1);
    a.update_if_changed(|v| *v = 5); // no change
    assert_eq!(runs.get(), 1, "no-op update skips fan-out");
    a.update_if_changed(|v| *v += 1); // real change
    assert_eq!(runs.get(), 2);
}

#[test]
fn set_if_changed_stale_handle_is_noop() {
    // A write through a handle whose slot was freed/recycled must
    // stay a no-op, not touch the new occupant — both inline and
    // when deferred inside a batch.
    let mut scope = Scope::new();
    let stale: Signal<i32> = with_scope(&mut scope, || Signal::new(1));
    drop(scope); // frees the slot, advancing its generation

    let fresh: Signal<u64> = Signal::new(7);
    assert_eq!(fresh.id(), stale.id(), "fresh reuses the freed slot");

    stale.set_if_changed(2); // must not panic / clobber
    batch(|| stale.set_if_changed(3)); // deferred path: also a no-op
    assert_eq!(fresh.get(), 7, "stale write must not touch the recycled signal");
}

// -----------------------------------------------------------------
// Automatic cycle batching (queue effect fan-out to the turn boundary)
// -----------------------------------------------------------------

/// An effect reading all of `a`, `b`, `c`, counting its runs.
fn watch3(
    a: Signal<i32>,
    b: Signal<i32>,
    c: Signal<i32>,
) -> (Effect, std::rc::Rc<std::cell::Cell<u32>>) {
    use std::cell::Cell;
    use std::rc::Rc;
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    let e = Effect::new(move || {
        let _ = (a.get(), b.get(), c.get());
        r.set(r.get() + 1);
    });
    (e, runs)
}

#[test]
fn cycle_coalesces_multiple_writes_to_one_fanout() {
    // The core property the whole auto-batching architecture rests on:
    // N writes inside one cycle wake a shared subscriber exactly once.
    let a = Signal::new(0i32);
    let b = Signal::new(0i32);
    let c = Signal::new(0i32);
    let (_e, runs) = watch3(a, b, c);
    assert_eq!(runs.get(), 1, "initial run");
    cycle(|| {
        a.set(1);
        b.set(1);
        c.set(1);
    });
    assert_eq!(runs.get(), 2, "three writes in one cycle => one re-run");
}

#[test]
fn unbatched_writes_fan_out_per_write() {
    // Contrast: WITHOUT a cycle, the same three writes wake the
    // subscriber three times. This is what the auto-cycle eliminates.
    let a = Signal::new(0i32);
    let b = Signal::new(0i32);
    let c = Signal::new(0i32);
    let (_e, runs) = watch3(a, b, c);
    assert_eq!(runs.get(), 1);
    a.set(1);
    b.set(1);
    c.set(1);
    assert_eq!(runs.get(), 4, "three separate fan-outs (1 initial + 3)");
}

#[test]
fn pressable_handler_is_born_batched() {
    // The architecture's payoff: a handler attached through a core
    // builder (here `pressable`) auto-batches at the point the backend
    // invokes it — no per-backend `batch()` needed. Two writes in the
    // handler wake a shared subscriber once, not twice.
    use std::cell::Cell;
    use std::rc::Rc;
    let a = Signal::new(0i32);
    let b = Signal::new(0i32);
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    let _e = Effect::new(move || {
        let _ = (a.get(), b.get());
        r.set(r.get() + 1);
    });
    assert_eq!(runs.get(), 1);

    let pressable = crate::pressable(Vec::new(), move || {
        a.set(1);
        b.set(2);
    });
    let crate::Element::Pressable { on_click, .. } = pressable.primitive else {
        panic!("pressable did not build a Pressable element");
    };
    // Simulate the backend firing the stored handler on a tap.
    on_click();
    assert_eq!(runs.get(), 2, "born-batched handler => one re-run for two writes");
}

#[test]
fn nested_cycle_joins_outer_window() {
    // A handler that a backend ALSO wraps (nested cycle) must still
    // flush once — the inner cycle joins the outer window.
    let a = Signal::new(0i32);
    let b = Signal::new(0i32);
    let c = Signal::new(0i32);
    let (_e, runs) = watch3(a, b, c);
    assert_eq!(runs.get(), 1);
    cycle(|| {
        a.set(1);
        cycle(|| {
            b.set(1);
            c.set(1);
        });
    });
    assert_eq!(runs.get(), 2, "nested cycles flush once at the outermost");
}

// -----------------------------------------------------------------
// Cycle / depth detection
// -----------------------------------------------------------------

#[test]
#[should_panic(expected = "effect run depth exceeded")]
fn deep_effect_chain_panics_at_depth_threshold() {
    // The same-id reentry guard already prevents an effect from
    // looping on itself, and incidentally catches small mutual
    // cycles (A↔B) because the cycle revisits an effect already on
    // the stack. The depth guard exists for cases reentry doesn't
    // cover: long synchronous *chains* of distinct effects, where
    // no single effect repeats but the cascade depth is unbounded.
    //
    // Construct N forwarding effects (read signals[i], write
    // signals[i+1]). Setting signals[0] cascades the full length;
    // past MAX_EFFECT_DEPTH (256) the depth guard panics with the
    // expected message instead of stack-overflowing.
    const N: usize = 280;
    let signals: Vec<Signal<i32>> = (0..N).map(|_| Signal::new(0i32)).collect();
    let mut effects: Vec<Effect> = Vec::with_capacity(N - 1);
    for i in 0..(N - 1) {
        let read = signals[i];
        let write = signals[i + 1];
        // Wrap each effect's first-run write so the initial fan-out
        // from setup doesn't trigger the cascade prematurely — only
        // the explicit set() below should kick it off.
        let mut first = true;
        let e = Effect::new(move || {
            let v = read.get();
            if first {
                first = false;
                return;
            }
            write.set(v + 1);
        });
        effects.push(e);
    }
    signals[0].set(1);
}

// -----------------------------------------------------------------
// memo()
// -----------------------------------------------------------------

#[test]
fn memo_caches_and_skips_equal_notifications() {
    use std::cell::Cell;
    use std::rc::Rc;
    let source = Signal::new(0i32);

    // Memo: count whether the input is over 10.
    let mut scope = Scope::new();
    let runs = Rc::new(Cell::new(0));
    let m = with_scope(&mut scope, || {
        let m = memo(move || source.get() > 10);
        let r = runs.clone();
        let _e = Effect::new(move || {
            let _ = m.get();
            r.set(r.get() + 1);
        });
        m
    });
    // Initial: subscriber ran once, memo value is `false`.
    assert_eq!(runs.get(), 1);
    assert_eq!(m.get(), false);

    // Bump source within "false" range — memo recomputes but value
    // stays `false`, so subscriber must NOT re-fire.
    source.set(5);
    assert_eq!(m.get(), false);
    assert_eq!(
        runs.get(),
        1,
        "memo gates equal results — subscriber must not re-run"
    );

    source.set(7);
    assert_eq!(runs.get(), 1, "still false → still gated");

    // Cross the threshold: memo flips, subscriber sees the change.
    source.set(11);
    assert_eq!(m.get(), true);
    assert_eq!(runs.get(), 2, "subscriber fires when memo's value actually changes");

    // Back below threshold: flips again, subscriber fires again.
    source.set(3);
    assert_eq!(m.get(), false);
    assert_eq!(runs.get(), 3);
}

#[test]
fn memo_recomputes_once_per_dep_change_regardless_of_subscriber_count() {
    use std::cell::Cell;
    use std::rc::Rc;
    let source = Signal::new(1i32);
    let compute_count = Rc::new(Cell::new(0));
    let c = compute_count.clone();
    let m = memo(move || {
        c.set(c.get() + 1);
        source.get() * 2
    });
    // Three independent readers of the same memo.
    let _e1 = Effect::new(move || {
        let _ = m.get();
    });
    let _e2 = Effect::new(move || {
        let _ = m.get();
    });
    let _e3 = Effect::new(move || {
        let _ = m.get();
    });
    let after_setup = compute_count.get();

    source.set(5);
    assert_eq!(
        compute_count.get(),
        after_setup + 1,
        "memo recomputes once per dep change even when three subscribers exist"
    );
}

// -----------------------------------------------------------------
// on() / on_defer()
// -----------------------------------------------------------------

#[test]
fn on_passes_new_and_previous_values() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let count = Signal::new(0i32);
    let log: Rc<RefCell<Vec<(i32, Option<i32>)>>> = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    let _e = on(count, move |new, prev| {
        l.borrow_mut().push((*new, prev.copied()));
    });
    // Initial fire: prev is None.
    count.set(5);
    count.set(7);
    let recorded = log.borrow().clone();
    assert_eq!(
        recorded,
        vec![(0, None), (5, Some(0)), (7, Some(5))],
        "on() must thread (current, previous) across runs"
    );
}

#[test]
fn on_tuple_subscribes_to_every_member() {
    use std::cell::Cell;
    use std::rc::Rc;
    let first = Signal::new("Jane".to_string());
    let last = Signal::new("Doe".to_string());
    let fires = Rc::new(Cell::new(0));
    let f = fires.clone();
    let _e = on((first, last), move |_new, _prev| {
        f.set(f.get() + 1);
    });
    assert_eq!(fires.get(), 1, "initial fire");
    first.set("Janet".to_string());
    assert_eq!(fires.get(), 2);
    last.set("Smith".to_string());
    assert_eq!(fires.get(), 3);
}

#[test]
fn on_defer_skips_initial_run() {
    use std::cell::Cell;
    use std::rc::Rc;
    let count = Signal::new(0i32);
    let fires = Rc::new(Cell::new(0));
    let f = fires.clone();
    let _e = on_defer(count, move |_new, _prev| {
        f.set(f.get() + 1);
    });
    assert_eq!(fires.get(), 0, "on_defer must not fire on creation");
    count.set(1);
    assert_eq!(fires.get(), 1, "first change after creation fires");
    count.set(2);
    assert_eq!(fires.get(), 2);
}

#[test]
fn on_body_reads_do_not_subscribe() {
    // Body reads `other` but `other` is not in the deps tuple — only
    // `trigger` should re-fire the effect.
    use std::cell::Cell;
    use std::rc::Rc;
    let trigger = Signal::new(0i32);
    let other = Signal::new(0i32);
    let fires = Rc::new(Cell::new(0));
    let f = fires.clone();
    let _e = on(trigger, move |_new, _prev| {
        let _shielded = other.get();
        f.set(f.get() + 1);
    });
    assert_eq!(fires.get(), 1, "initial");
    other.set(99);
    assert_eq!(
        fires.get(),
        1,
        "writes to a signal read inside the body but not in deps must not fire"
    );
    trigger.set(1);
    assert_eq!(fires.get(), 2, "writes to a dep do fire");
}

// -----------------------------------------------------------------
// reducer()
// -----------------------------------------------------------------

#[test]
fn reducer_applies_user_function_to_state() {
    enum Counter {
        Inc,
        Dec,
        Set(i32),
    }
    let (state, dispatch) = reducer(0i32, |&n, action| match action {
        Counter::Inc => n + 1,
        Counter::Dec => n - 1,
        Counter::Set(v) => v,
    });
    assert_eq!(state.get(), 0);
    dispatch(Counter::Inc);
    assert_eq!(state.get(), 1);
    dispatch(Counter::Inc);
    dispatch(Counter::Inc);
    assert_eq!(state.get(), 3);
    dispatch(Counter::Dec);
    assert_eq!(state.get(), 2);
    dispatch(Counter::Set(100));
    assert_eq!(state.get(), 100);
}

#[test]
fn reducer_state_signal_notifies_subscribers() {
    use std::cell::Cell;
    use std::rc::Rc;
    let (state, dispatch) = reducer(0i32, |&n, delta: i32| n + delta);
    let observed = Rc::new(Cell::new(0i32));
    let o = observed.clone();
    let _e = Effect::new(move || {
        o.set(state.get());
    });
    assert_eq!(observed.get(), 0);
    dispatch(5);
    assert_eq!(observed.get(), 5, "subscriber sees the new state after dispatch");
    dispatch(7);
    assert_eq!(observed.get(), 12);
}

#[test]
fn reducer_dispatch_does_not_subscribe_caller_effect() {
    // The dispatcher reads the current state to compute the next
    // one. That read is `untrack`ed so it doesn't accidentally
    // subscribe the surrounding effect to the reducer's state.
    // (Without that, calling `dispatch` from inside an effect
    // would make the effect re-fire on every state change it
    // caused — easy infinite-loop trap.)
    use std::cell::Cell;
    use std::rc::Rc;
    let trigger = Signal::new(0i32);
    let (state, dispatch) = reducer(0i32, |&n, _: ()| n + 1);
    let fires = Rc::new(Cell::new(0));
    let f = fires.clone();
    let _e = Effect::new(move || {
        // Effect's only declared dep is `trigger`. If `dispatch`
        // ends up subscribing us to `state`, the assertion below
        // catches it.
        let _ = trigger.get();
        f.set(f.get() + 1);
        dispatch(());
    });
    assert_eq!(fires.get(), 1, "initial run");
    let after_initial = state.get();
    assert_eq!(after_initial, 1, "state advanced once on the initial run");
    // External write to a signal we DO depend on triggers a re-run
    // and another dispatch.
    trigger.set(1);
    assert_eq!(fires.get(), 2, "re-fires on trigger");
    assert_eq!(state.get(), 2, "state advanced again");
    // Critically: no additional re-fires beyond the trigger-driven
    // one. If dispatch had subscribed us to `state`, fires would
    // be 3+ here (reentry guard would short-circuit re-entries,
    // but the count would still differ).
    assert_eq!(
        fires.get(),
        2,
        "dispatch must not subscribe caller effect to state"
    );
}

#[test]
fn reducer_state_is_a_plain_signal() {
    // Sanity: the returned `state` is the same `Signal<S>` type
    // every other consumer accepts. This verifies that the
    // pattern composes without inventing a new type.
    let (state, dispatch) = reducer(0i32, |&n, a: i32| n + a);
    // Same Copy semantics as any other Signal.
    let alias: Signal<i32> = state;
    dispatch(10);
    assert_eq!(alias.get(), 10);
    // `.update` works on the same signal directly, bypassing the
    // reducer — useful escape hatch for migrations from
    // signal-based state.
    alias.update(|n| *n = -5);
    assert_eq!(state.get(), -5);
    dispatch(3);
    assert_eq!(state.get(), -2);
}

#[test]
fn effect_macro_runs_and_rebinds_in_scope() {
    use std::cell::Cell;
    use std::rc::Rc;
    let count = Signal::new(0i32);
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    let mut scope = Scope::new();
    with_scope(&mut scope, || {
        crate::effect!({
            let _ = count.get();
            r.set(r.get() + 1);
        });
    });
    assert_eq!(runs.get(), 1);
    count.set(7);
    assert_eq!(runs.get(), 2, "macro-built effect should re-fire on signal change");
    // Scope drop disposes the effect.
    drop(scope);
    count.set(8);
    assert_eq!(runs.get(), 2, "effect should not fire after its scope drops");
}

#[test]
#[should_panic(expected = "no active reactive scope")]
fn scoped_effect_panics_without_active_scope() {
    // `effect!` (→ `Effect::scoped`) is for in-tree reactivity only.
    // Outside a scope it must fail loudly in debug builds rather than
    // silently cancel — out-of-tree code should reach for `watch`.
    Effect::scoped(|| {});
}

#[test]
fn watch_runs_and_refires_until_dropped() {
    use std::cell::Cell;
    use std::rc::Rc;
    let count = Signal::new(0i32);
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    let sub = super::watch(move || {
        let _ = count.get();
        r.set(r.get() + 1);
    });
    assert_eq!(runs.get(), 1, "watch runs once immediately");
    count.set(1);
    assert_eq!(runs.get(), 2, "watch re-fires on dependency change");
    drop(sub);
    count.set(2);
    assert_eq!(runs.get(), 2, "dropping the Subscription disposes the effect");
}

#[test]
fn watch_subscription_drop_runs_cleanup() {
    use std::cell::Cell;
    use std::rc::Rc;
    let dep = Signal::new(0i32);
    let cleaned = Rc::new(Cell::new(0));
    let c = cleaned.clone();
    let sub = super::watch(move || {
        let _ = dep.get();
        let c2 = c.clone();
        on_cleanup(move || c2.set(c2.get() + 1));
    });
    assert_eq!(cleaned.get(), 0);
    dep.set(1);
    assert_eq!(cleaned.get(), 1, "cleanup fires before re-run");
    drop(sub);
    assert_eq!(cleaned.get(), 2, "cleanup fires again on disposal");
}

#[test]
fn watch_leak_survives_handle_drop() {
    use std::cell::Cell;
    use std::rc::Rc;
    let count = Signal::new(0i32);
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    // `.leak()` gives up the handle but pins the effect for the process.
    super::watch(move || {
        let _ = count.get();
        r.set(r.get() + 1);
    })
    .leak();
    assert_eq!(runs.get(), 1);
    count.set(1);
    assert_eq!(runs.get(), 2, "leaked subscription keeps firing past handle drop");
}

#[test]
fn watch_is_caller_owned_not_scope_adopted() {
    use std::cell::Cell;
    use std::rc::Rc;
    let count = Signal::new(0i32);
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    let mut scope = Scope::new();
    // Created inside a scope, but `watch` is never adopted by it.
    let sub = with_scope(&mut scope, || {
        super::watch(move || {
            let _ = count.get();
            r.set(r.get() + 1);
        })
    });
    assert_eq!(runs.get(), 1);
    // Scope teardown must NOT dispose a watch — the caller owns it.
    drop(scope);
    count.set(1);
    assert_eq!(runs.get(), 2, "watch survives its enclosing scope's drop");
    // The caller's handle is what disposes it.
    drop(sub);
    count.set(2);
    assert_eq!(runs.get(), 2, "dropping the Subscription disposes it");
}

#[test]
fn subscription_single_slot_replacement_disposes_prior() {
    use std::cell::Cell;
    use std::rc::Rc;
    let a = Signal::new(0i32);
    let runs = Rc::new(Cell::new(0));
    // Single-slot keepalive: replacing the slot drops the prior
    // Subscription, which must dispose its effect (the theme-keepalive
    // pattern). If it didn't, `a.set` would fire both watches.
    let r1 = runs.clone();
    let mut slot = Some(super::watch(move || {
        let _ = a.get();
        r1.set(r1.get() + 1);
    }));
    assert_eq!(runs.get(), 1);
    let r2 = runs.clone();
    slot = Some(super::watch(move || {
        let _ = a.get();
        r2.set(r2.get() + 1);
    }));
    assert_eq!(runs.get(), 2, "replacement watch runs once at creation");
    a.set(1);
    assert_eq!(
        runs.get(),
        3,
        "only the surviving subscription re-fires; the replaced one was disposed"
    );
    drop(slot);
}

#[test]
fn on_cleanup_fires_before_effect_rerun() {
    use std::cell::Cell;
    use std::rc::Rc;
    let trigger = Signal::new(0i32);
    let cleanup_count = Rc::new(Cell::new(0));
    let run_count = Rc::new(Cell::new(0));
    let c = cleanup_count.clone();
    let r = run_count.clone();
    let _e = Effect::new(move || {
        let _ = trigger.get();
        r.set(r.get() + 1);
        let c2 = c.clone();
        on_cleanup(move || {
            c2.set(c2.get() + 1);
        });
    });
    // First run: 1 run, 0 cleanups so far.
    assert_eq!(run_count.get(), 1);
    assert_eq!(cleanup_count.get(), 0);

    // Re-run drains the previous cleanup and registers a new one.
    trigger.set(1);
    assert_eq!(run_count.get(), 2);
    assert_eq!(cleanup_count.get(), 1);

    trigger.set(2);
    assert_eq!(run_count.get(), 3);
    assert_eq!(cleanup_count.get(), 2);
}

#[test]
fn on_cleanup_fires_on_effect_drop() {
    use std::cell::Cell;
    use std::rc::Rc;
    let cleanup_count = Rc::new(Cell::new(0));
    let c = cleanup_count.clone();
    let e = Effect::new(move || {
        let c2 = c.clone();
        on_cleanup(move || {
            c2.set(c2.get() + 1);
        });
    });
    assert_eq!(cleanup_count.get(), 0);
    drop(e);
    assert_eq!(cleanup_count.get(), 1);
}

#[test]
fn on_cleanup_attaches_to_scope_outside_effect() {
    use std::cell::Cell;
    use std::rc::Rc;
    let cleanup_count = Rc::new(Cell::new(0));
    let c = cleanup_count.clone();
    let mut scope = Scope::new();
    with_scope(&mut scope, || {
        on_cleanup(move || {
            c.set(c.get() + 1);
        });
    });
    assert_eq!(cleanup_count.get(), 0);
    drop(scope);
    assert_eq!(cleanup_count.get(), 1);
}

#[test]
fn on_cleanup_outside_any_context_is_noop() {
    // Just verify nothing panics. The callback is dropped silently;
    // any side effect from its destructor is the test signal.
    use std::cell::Cell;
    use std::rc::Rc;
    let dropped = Rc::new(Cell::new(false));
    let d = dropped.clone();
    on_cleanup(move || { /* unused */ });
    // The closure captures nothing observable; we just check this
    // didn't panic. For a second pass, register a closure that
    // *does* observe its drop:
    struct Witness(Rc<Cell<bool>>);
    impl Drop for Witness {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    let w = Witness(d);
    on_cleanup(move || {
        let _hold = w;
    });
    // No context → callback dropped synchronously → Witness drops now.
    assert!(dropped.get());
}

#[test]
fn untrack_blocks_subscription() {
    use std::cell::Cell;
    use std::rc::Rc;
    let s = Signal::new(0i32);
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    let _e = Effect::new(move || {
        untrack(|| {
            let _ = s.get();
        });
        r.set(r.get() + 1);
    });
    assert_eq!(runs.get(), 1);
    s.set(99); // should NOT re-fire effect
    assert_eq!(runs.get(), 1);
}

/// Returns (signals_in_use, effects_in_use) — counts of `Some` slots in
/// the arena. Used by leak tests.
fn arena_inuse_counts() -> (usize, usize) {
    ARENA.with(|a| {
        let a = a.borrow();
        (
            a.signals.iter().filter(|s| s.is_some()).count(),
            a.effects.iter().filter(|e| e.is_some()).count(),
        )
    })
}

#[test]
fn scope_frees_signals_and_effects_on_drop() {
    let (s0, e0) = arena_inuse_counts();
    {
        let mut scope = Scope::new();
        with_scope(&mut scope, || {
            let _a = Signal::new(1i32);
            let _b = Signal::new(2i32);
            let _e = Effect::new(|| {});
            let (s1, e1) = arena_inuse_counts();
            assert_eq!(s1, s0 + 2, "two new signal slots in use inside scope");
            assert_eq!(e1, e0 + 1, "one new effect slot in use inside scope");
        });
        // Scope still alive (just not active). Slots still in use.
        let (s_active, e_active) = arena_inuse_counts();
        assert_eq!(s_active, s0 + 2);
        assert_eq!(e_active, e0 + 1);
        // Scope drops here.
    }
    let (s_after, e_after) = arena_inuse_counts();
    assert_eq!(s_after, s0, "all signal slots returned to baseline");
    assert_eq!(e_after, e0, "all effect slots returned to baseline");
}

/// Regression: a write through a STALE signal handle — one whose
/// owning scope unmounted and whose slot was recycled by a
/// different-typed signal — must be a safe no-op, NOT a
/// "signal type mismatch" panic. That panic, fired from a deferred
/// `signal.set` inside a JNI scheduled callback, aborted the whole
/// Android app (SIGABRT, non-unwinding FFI boundary). Generational
/// handles make the stale write detect the bumped generation and do
/// nothing. ARENA is thread-local, so this test thread's arena
/// starts empty and the freed slot is the one `fresh` recycles.
#[test]
fn stale_signal_write_after_scope_drop_is_noop_not_panic() {
    let mut scope = Scope::new();
    let stale: Signal<bool> = with_scope(&mut scope, || Signal::new(false));
    drop(scope); // frees `stale`'s slot and bumps its generation

    // Recycle the just-freed slot with a DIFFERENT-typed signal —
    // the exact aliasing that used to make the stale write panic.
    let fresh: Signal<u64> = Signal::new(7);
    assert_eq!(
        fresh.id(),
        stale.id(),
        "fresh signal should reuse the freed slot (LIFO freelist)"
    );

    // The crash repro: deferred write through the stale handle.
    stale.set(true); // must NOT panic
    assert_eq!(
        fresh.get(),
        7,
        "stale write must not clobber the recycled signal"
    );

    // A stale `update` is likewise a no-op.
    stale.update(|v| *v = true);
    assert_eq!(fresh.get(), 7);

    // The recycled signal still works normally afterward.
    fresh.set(9);
    assert_eq!(fresh.get(), 9);
}

/// A stale write must not fire the recycled occupant's subscribers
/// either — otherwise a disposed signal's deferred `set` could
/// spuriously re-run effects subscribed to whatever took its slot.
#[test]
fn stale_signal_write_does_not_fire_recycled_subscribers() {
    use std::cell::Cell;
    use std::rc::Rc;

    let mut scope = Scope::new();
    let stale: Signal<bool> = with_scope(&mut scope, || Signal::new(false));
    drop(scope);

    let fresh: Signal<u64> = Signal::new(0);
    assert_eq!(fresh.id(), stale.id());

    // Subscribe an effect to the recycled signal.
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    let _e = Effect::new(move || {
        let _ = fresh.get();
        r.set(r.get() + 1);
    });
    assert_eq!(runs.get(), 1, "effect runs once on creation");

    // Stale write to the same slot index must NOT re-run the effect.
    stale.set(true);
    assert_eq!(
        runs.get(),
        1,
        "stale write fired the recycled signal's subscribers"
    );

    // A real write to the recycled signal still re-runs it.
    fresh.set(1);
    assert_eq!(runs.get(), 2);
}

#[test]
fn freelist_recycles_slot_ids_across_scopes() {
    // Repeatedly mount-then-drop a scope holding N signals + N
    // effects. Without the freelist, `arena_stats().effects_total`
    // would grow by N per iteration; with the freelist, it should
    // stay roughly bounded by the largest concurrent scope size.
    const N: usize = 64;
    let stats_before = super::arena_stats();
    for _ in 0..5 {
        let mut scope = Scope::new();
        with_scope(&mut scope, || {
            for _ in 0..N {
                let _ = Signal::new(0_i32);
                let _ = Effect::new(|| {});
            }
        });
        // scope drops, ids recycle to the freelist
    }
    let stats_after = super::arena_stats();
    // Without recycling we'd see signals_total/effects_total grow
    // by ~5N. With recycling, growth is bounded by N (one cohort's
    // worth — the first iteration fills fresh ids, later iterations
    // pop them off the freelist).
    let growth = stats_after.effects_total - stats_before.effects_total;
    assert!(
        growth <= N + 2,
        "effects_total grew by {} (expected ≤ {} with freelist recycling)",
        growth,
        N + 2,
    );
    let sig_growth = stats_after.signals_total - stats_before.signals_total;
    assert!(
        sig_growth <= N + 2,
        "signals_total grew by {} (expected ≤ {} with freelist recycling)",
        sig_growth,
        N + 2,
    );
}

#[test]
fn nested_scopes_drop_independently() {
    let (s0, e0) = arena_inuse_counts();
    let mut outer = Scope::new();
    with_scope(&mut outer, || {
        let _outer_sig = Signal::new("outer".to_string());
        {
            let mut inner = Scope::new();
            with_scope(&mut inner, || {
                let _inner_sig = Signal::new("inner".to_string());
                let _inner_eff = Effect::new(|| {});
                let (s, e) = arena_inuse_counts();
                assert_eq!(s, s0 + 2);
                assert_eq!(e, e0 + 1);
            });
            // inner drops here
        }
        // After inner drops, only outer's signal remains.
        let (s, e) = arena_inuse_counts();
        assert_eq!(s, s0 + 1, "inner scope's signal freed");
        assert_eq!(e, e0, "inner scope's effect freed");
    });
    drop(outer);
    let (s, e) = arena_inuse_counts();
    assert_eq!(s, s0);
    assert_eq!(e, e0);
}

/// Regression test for the framework-purity refactor that moved the
/// wasm-only `PENDING_DROPS` / rAF-sliced drain out of runtime-core
/// and behind `install_drop_deferral`. The seam must:
///
/// 1. Default to synchronous drop when no policy is installed (the
///    native-backend path).
/// 2. Route effect closures + scope guards through an installed
///    policy when one exists (the web backend's rAF drain).
/// 3. Still drop signals/refs synchronously (they don't go through
///    the policy — any deferred drain might need to read them).
#[test]
fn install_drop_deferral_routes_effects_and_guards_not_signals() {
    use std::cell::RefCell;
    use std::rc::Rc;

    // Capture every box the policy receives so we can introspect.
    thread_local! {
        static DEFERRED: RefCell<Vec<Box<dyn std::any::Any>>> =
            RefCell::new(Vec::new());
    }
    fn capturing_policy(mut boxes: Vec<Box<dyn std::any::Any>>) {
        DEFERRED.with(|q| q.borrow_mut().append(&mut boxes));
    }

    // Sentinel guard that marks `dropped` when its Drop fires. Clone
    // is required by `Signal<T>` (T: Clone). The clone target isn't
    // used in the test; we only care about the *last* Drop firing.
    #[derive(Clone)]
    struct Sentinel(Rc<RefCell<bool>>);
    impl Drop for Sentinel {
        fn drop(&mut self) {
            *self.0.borrow_mut() = true;
        }
    }

    // ----- 1) No policy installed (the default): everything drops
    // synchronously, including effects and guards. ------------------
    DROP_DEFERRAL.with(|c| c.set(None));
    let guard_dropped = Rc::new(RefCell::new(false));
    {
        let mut scope = Scope::new();
        with_scope(&mut scope, || {
            let _e = Effect::new(|| {});
            scope_adopt_guard_for_test(Sentinel(guard_dropped.clone()));
        });
        // scope drops here → synchronous drop path
    }
    assert!(
        *guard_dropped.borrow(),
        "without an installed policy, scope guards must drop synchronously"
    );

    // ----- 2) Install a capturing policy: effects + guards go to
    // the policy, signals do NOT. -----------------------------------
    DEFERRED.with(|q| q.borrow_mut().clear());
    install_drop_deferral(capturing_policy);

    let signal_value_drop_observed = Rc::new(RefCell::new(false));
    let guard2_dropped = Rc::new(RefCell::new(false));
    {
        let mut scope = Scope::new();
        with_scope(&mut scope, || {
            // Signal holding a Sentinel — its Drop runs synchronously
            // because signals don't go through the deferral policy.
            let _s: Signal<Sentinel> = Signal::new(Sentinel(signal_value_drop_observed.clone()));
            let _e = Effect::new(|| {});
            scope_adopt_guard_for_test(Sentinel(guard2_dropped.clone()));
        });
    }

    // Effect + guard are in the policy's queue, NOT dropped yet.
    assert!(
        !*guard2_dropped.borrow(),
        "with an installed policy, the scope guard must be parked in the \
         policy queue rather than dropping synchronously",
    );
    let queued = DEFERRED.with(|q| q.borrow().len());
    assert!(
        queued >= 2,
        "policy should have received at least the effect box + the guard box (got {queued})",
    );

    // Signal-held Sentinel dropped synchronously (signals stay
    // outside the deferral path).
    assert!(
        *signal_value_drop_observed.borrow(),
        "signals are not routed through the deferral policy; their \
         contained values must drop synchronously when the scope drops",
    );

    // Now manually drain the policy queue and observe the guard runs.
    DEFERRED.with(|q| q.borrow_mut().clear());
    assert!(
        *guard2_dropped.borrow(),
        "draining the policy queue must finally drop the guard"
    );

    // ----- 3) Reset to no-policy so we don't poison sibling tests. -
    DROP_DEFERRAL.with(|c| c.set(None));
}

/// Regression for the web history-pop abort traced to
/// `idea-ui`'s `Collapsible::measured_body`: it `mem::forget`'d the
/// `LayoutSubscription` (a `ResizeObserver`), so the observer was
/// never disconnected. After the component's scope was disposed (a
/// history-pop detaching the subtree) a late layout callback still
/// fired and read the now-freed `natural_height: Signal<f32>` →
/// "signal used after its scope was dropped" → abort.
///
/// The contract the fix relies on: a `LayoutSubscription` anchored to
/// the scope via [`on_cleanup`] has its drop (= observer disconnect)
/// run when the scope drops. `mem::forget` would skip that drop — this
/// test fails if the anchor regresses back to a leak. A tighter test
/// against `measured_body` itself needs a layout-capable web backend
/// (real `ResizeObserver`), which the headless test env lacks, so we
/// assert the underlying subscription/scope contract instead.
#[test]
fn layout_subscription_via_on_cleanup_unsubscribes_on_scope_drop() {
    use std::cell::Cell;
    use std::rc::Rc;

    let disconnected = Rc::new(Cell::new(false));
    {
        let mut scope = Scope::new();
        let flag = disconnected.clone();
        with_scope(&mut scope, || {
            // Stands in for `ViewHandle::on_layout`'s return — its
            // drop is the observer disconnect.
            let sub = crate::handles::LayoutSubscription::new(move || flag.set(true));
            on_cleanup(move || drop(sub));
        });
        assert!(
            !disconnected.get(),
            "subscription must stay live until the scope drops"
        );
        // scope drops here → on_cleanup fires → sub drops → disconnect.
    }
    assert!(
        disconnected.get(),
        "scope drop must run the LayoutSubscription's drop (observer \
         disconnect); a `mem::forget` anchor would leak it"
    );
}

/// Helper that adopts a guard into the currently-active scope. The
/// production code calls `Scope::adopt_guard` directly through its
/// own crate-internal seams; for the test we just exercise the same
/// path.
fn scope_adopt_guard_for_test<G: 'static>(guard: G) {
    assert!(
        adopt_guard_into_active_scope(guard),
        "test invariant: scope must be active when adopting a guard"
    );
}

/// Regression test for the "memo / resource leak inside scope" audit
/// finding. `memo_with` and `resource` both end with `mem::forget(e)`
/// on their internal Effect. The audit claimed this caused arena
/// growth even inside an active render scope.
///
/// Verify that when a memo is created INSIDE a `with_scope`, the
/// scope's drop frees both the memo's output Signal and its driving
/// Effect — the `forget` is harmless in that path because the local
/// handle's `owns` flag is already false (scope adopted the slot).
#[test]
fn memo_in_scope_releases_signal_and_effect_on_scope_drop() {
    let source = Signal::new(0i32);
    let (s0, e0) = arena_inuse_counts();

    {
        let mut scope = Scope::new();
        with_scope(&mut scope, || {
            for _ in 0..16 {
                let _m = memo(move || source.get() * 2);
            }
        });
        let (s_active, e_active) = arena_inuse_counts();
        // 16 memos × (1 output Signal + 1 driving Effect) inside the scope.
        // The internal Signal `last` rc-cell isn't an arena allocation,
        // so we only count one signal + one effect per memo.
        assert_eq!(
            s_active - s0,
            16,
            "expected 16 memo output signals in arena (was +{})",
            s_active - s0
        );
        assert_eq!(
            e_active - e0,
            16,
            "expected 16 memo driver effects in arena (was +{})",
            e_active - e0
        );
        // scope drops here.
    }

    let (s_after, e_after) = arena_inuse_counts();
    assert_eq!(
        s_after, s0,
        "memo output signals must be freed on scope drop \
         (the mem::forget on the Effect must not pin the Signal)"
    );
    assert_eq!(
        e_after, e0,
        "memo driver effects must be freed on scope drop \
         (mem::forget is harmless when scope owns the slot)"
    );
}

/// Regression test for the ACTIVE_THEME-style accumulating-subscriber concern.
///
/// A hot, thread-lifetime signal that many short-lived scopes read inside
/// effects must not accumulate dead `EffectId`s in its subscriber set across
/// mount/unmount cycles. The fix path (`take_effects_batched` → `retain`)
/// runs at every `Scope::drop`; this test asserts that property end-to-end.
#[test]
fn hot_signal_subscribers_pruned_on_scope_drop() {
    // Thread-lifetime "active theme" analogue: a signal that outlives every
    // render scope and that every component subscribes to.
    let hot = Signal::new(0i32);

    let base_subs = ARENA.with(|a| {
        a.borrow()
            .signal_subscribers
            .get(hot.id.0 as usize)
            .map(|s| s.len())
            .unwrap_or(0)
    });
    assert_eq!(base_subs, 0, "fresh signal has no subscribers");

    // Mount-and-drop many scopes, each running an effect that reads `hot`.
    // Without subscriber pruning on Scope::drop, `hot`'s subscriber set
    // would grow to ~ROUNDS * EFFECTS_PER_SCOPE.
    const ROUNDS: usize = 32;
    const EFFECTS_PER_SCOPE: usize = 16;
    for _ in 0..ROUNDS {
        let mut scope = Scope::new();
        with_scope(&mut scope, || {
            for _ in 0..EFFECTS_PER_SCOPE {
                let _e = Effect::new(move || {
                    let _ = hot.get();
                });
            }
        });
        // scope drops here → take_effects_batched must remove every
        // effect's subscription from `hot`.
    }

    let subs_after = ARENA.with(|a| {
        a.borrow()
            .signal_subscribers
            .get(hot.id.0 as usize)
            .map(|s| s.len())
            .unwrap_or(0)
    });
    assert_eq!(
        subs_after, 0,
        "hot signal must have zero subscribers after all reading scopes drop; \
         accumulating dead EffectIds here is the LEAK_REPORT bug",
    );

    // And the framework must still deliver writes to a freshly-subscribed
    // effect after all that churn — the prune must not have damaged the
    // signal's internal state.
    use std::cell::Cell;
    use std::rc::Rc;
    let observed = Rc::new(Cell::new(-1));
    let o = observed.clone();
    let _e = Effect::new(move || o.set(hot.get()));
    hot.set(42);
    assert_eq!(observed.get(), 42);
}

fn arena_refs_inuse() -> usize {
    ARENA.with(|a| a.borrow().refs.iter().filter(|r| r.is_some()).count())
}

/// Stand-in for a component-defined handle. Closes over a Cell so we
/// can assert that `with(|h| h.method())` reaches the body. Clone
/// is required so `Ref::get()` can hand back an owned copy.
#[derive(Clone)]
struct DummyHandle {
    counter: std::rc::Rc<std::cell::Cell<u32>>,
}
impl DummyHandle {
    fn bump(&self) { self.counter.set(self.counter.get() + 1); }
}

#[test]
fn ref_fills_and_clears() {
    use std::cell::Cell;
    use std::rc::Rc;
    let mut scope = Scope::new();
    let r: Ref<DummyHandle> = with_scope(&mut scope, Ref::new);
    let counter = Rc::new(Cell::new(0));

    // Pre-mount: with() is None, bump never reaches handle.
    assert!(!r.is_mounted());
    assert!(r.with(|h| h.bump()).is_none());
    assert_eq!(counter.get(), 0);

    r.fill(DummyHandle { counter: counter.clone() });
    assert!(r.is_mounted());
    r.with(|h| h.bump());
    assert_eq!(counter.get(), 1);

    r.clear();
    assert!(!r.is_mounted());
    assert!(r.with(|h| h.bump()).is_none());
    assert_eq!(counter.get(), 1);
}

#[test]
fn scope_drop_frees_ref_slot() {
    let baseline = arena_refs_inuse();
    {
        let mut scope = Scope::new();
        let r: Ref<DummyHandle> = with_scope(&mut scope, Ref::new);
        r.fill(DummyHandle { counter: std::rc::Rc::new(std::cell::Cell::new(0)) });
        assert_eq!(arena_refs_inuse(), baseline + 1, "ref slot in use inside scope");
        // scope drops here
    }
    assert_eq!(arena_refs_inuse(), baseline, "ref slot freed at scope drop");
}

#[test]
fn ref_get_returns_owned_clone() {
    use std::cell::Cell;
    use std::rc::Rc;
    let mut scope = Scope::new();
    let r: Ref<DummyHandle> = with_scope(&mut scope, Ref::new);
    let counter = Rc::new(Cell::new(0));

    // Pre-mount: get() returns None.
    assert!(r.get().is_none());

    r.fill(DummyHandle { counter: counter.clone() });

    // The ergonomic call site: get a handle, call a method on it,
    // no closure needed.
    r.get().map(|h| h.bump());
    assert_eq!(counter.get(), 1);

    // Cloned handle outlives the temporary inside get(): the Rc
    // bump means the underlying counter is still reachable.
    let owned = r.get().unwrap();
    owned.bump();
    owned.bump();
    assert_eq!(counter.get(), 3);

    r.clear();
    assert!(r.get().is_none(), "post-unmount get() returns None");
}
