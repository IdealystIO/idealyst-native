//! Track 1 — Reactivity. Signals, effects, derived state, batching.
//! Everything taught here is `runtime_core` only; no component kit.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Typography};

use crate::common::{Callout, CodePanel, DocsLink, LessonPage};
use crate::routes::{RX_BATCHING_ROUTE, RX_DERIVED_ROUTE, RX_EFFECTS_ROUTE, RX_SIGNALS_ROUTE};
use crate::shell;

pub fn signals() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = RX_SIGNALS_ROUTE.name(),
            title = "Signals".to_string(),
            lead = "The Copy handle that everything reactive is built on.".to_string(),
        ) {
            Typography(
                content = "A Signal<T> is the framework's reactive primitive: a Copy handle to a \
                    value stored in a thread-local arena. Reads subscribe whatever effect is \
                    running; writes notify subscribers. There is no virtual DOM and no re-render \
                    pass \u{2014} the unit of update is the closure that read the \
                    signal.".to_string()
            )
            CodePanel(src = r##"use runtime_core::{signal, Signal};

let count = signal!(0);        // Signal<i32> — a Copy handle
let n = count.get();           // read (subscribes the running effect)
count.set(5);                  // write — notifies subscribers
count.update(|v| *v += 1);     // in-place mutate, then notify"##.to_string())

            Typography(content = "Why Copy matters".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Because the handle is Copy, you move it into as many closures as you \
                    like with no .clone() ceremony \u{2014} the classic Rust reactive-system \
                    boilerplate disappears. The value T still needs Clone, because get() clones \
                    the stored value out; use with(..) for a borrowed read when cloning is \
                    expensive.".to_string()
            )

            Callout(label = "set() is unconditional".to_string()) {
                Typography(
                    content = "Every set() fires subscribers, even if the new value equals the \
                        old. Equality-gated updates are the job of memo (next-but-one step), not \
                        the signal itself.".to_string(),
                    muted = true,
                )
            }
            Callout(label = "Lifetime is scope-bound".to_string()) {
                Typography(
                    content = "A signal lives as long as the reactive scope that created it. \
                        Reading one after its scope drops is a hard panic, not a silent bug \
                        \u{2014} so signals don't dangle.".to_string(),
                    muted = true,
                )
            }
            DocsLink(
                summary = "The full model \u{2014} the arena, scopes, drop order, and the \
                    notification flow.".to_string(),
                link_label = "Reactivity reference".to_string(),
                doc_file = "reactivity.md".to_string(),
            )
        }
    })
}

pub fn effects() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = RX_EFFECTS_ROUTE.name(),
            title = "Effects".to_string(),
            lead = "Closures that re-run when the signals they read change.".to_string(),
        ) {
            Typography(
                content = "An Effect is a closure that re-runs whenever a signal it read on its \
                    last run changes. It runs once immediately to establish its subscriptions, \
                    then again on every relevant change. Dependencies are tracked by \
                    construction: whatever the closure reads is what it depends on.".to_string()
            )
            CodePanel(src = r##"use runtime_core::{signal, Effect};

let count = signal!(0);

let _e = Effect::new(move || {
    // reads inside the body subscribe automatically:
    log::info!("count is {}", count.get());
});

count.set(1); // re-runs the effect"##.to_string())

            Typography(
                content = "The effect! macro and cleanup".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "effect! { ... } is shorthand that binds the handle to the \
                    surrounding block. Pair it with on_cleanup to release resources \u{2014} the \
                    callback fires before the next re-run and again on disposal.".to_string()
            )
            CodePanel(src = r##"use runtime_core::{effect, after_ms, on_cleanup};

effect!({
    let task = after_ms(500, || tick());
    on_cleanup(move || drop(task)); // cancel before re-run / on teardown
    deps.get();                      // re-run when deps changes
});"##.to_string())

            Callout(label = "Reading without subscribing".to_string()) {
                Typography(
                    content = "Subscriptions are rebuilt on every run, so a branch that stops \
                        reading a signal stops being notified. Wrap a read in untrack(|| \
                        sig.get()) to read without subscribing \u{2014} common when an effect \
                        both reads and writes state.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Notification flow, subscription rebuilding, and deferred teardown.".to_string(),
                link_label = "Reactivity reference".to_string(),
                doc_file = "reactivity.md".to_string(),
            )
        }
    })
}

pub fn derived() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = RX_DERIVED_ROUTE.name(),
            title = "Derived state".to_string(),
            lead = "memo, memo_with, and reducer \u{2014} computed values that stay in sync.".to_string(),
        ) {
            Typography(
                content = "Computed state is a memo: a cached, equality-gated derived signal. It \
                    recomputes when its dependencies change, compares the result with PartialEq, \
                    and only notifies subscribers when the value actually changed.".to_string()
            )
            CodePanel(src = r##"use runtime_core::{signal, memo};

let count = signal!(0);

let doubled = memo(move || count.get() * 2);  // Signal<i32>, recomputed on change
let is_big  = memo(move || count.get() > 10); // only fires when the bool flips"##.to_string())

            Typography(content = "memo_with and reducer".to_string(), kind = typography_kind::H2)
            Typography(
                content = "memo_with takes a custom equality function for types that don't \
                    implement PartialEq, or when 'changed' means something domain-specific. \
                    reducer gives you a (Signal<S>, dispatch) pair \u{2014} the typed-action \
                    shape of React's useReducer.".to_string()
            )
            CodePanel(src = r##"use runtime_core::reducer;

let (count, dispatch) = reducer(0_i32, |state, delta: i32| state + delta);
dispatch(1);   // count is now 1
dispatch(-1);  // count is now 0"##.to_string())

            Callout(label = "Memos must be pure".to_string()) {
                Typography(
                    content = "A memo's closure may only read signals, never write them. \
                        Writing inside one panics \u{2014} that restriction is what keeps the \
                        reactive graph acyclic.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "How memo seeds its output and gates notifications.".to_string(),
                link_label = "Reactivity reference".to_string(),
                doc_file = "reactivity.md".to_string(),
            )
        }
    })
}

pub fn batching() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = RX_BATCHING_ROUTE.name(),
            title = "Controlling when effects fire".to_string(),
            lead = "batch coalesces fan-out; untrack and on control what an effect subscribes \
                to.".to_string(),
        ) {
            Typography(
                content = "When several signals change together, batch coalesces the \
                    notifications: each subscriber re-runs once at the end of the batch instead \
                    of once per write. Writes are still immediately visible to reads inside the \
                    batch \u{2014} only the effect fan-out is deferred.".to_string()
            )
            CodePanel(src = r##"use runtime_core::{signal, batch};

let first = signal!(0);
let second = signal!(0);

batch(|| {
    first.set(1);
    second.set(2);
}); // an effect reading both re-runs ONCE, not twice"##.to_string())

            Typography(content = "on \u{2014} explicit dependencies".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Where an Effect subscribes to everything it reads, on(deps, ..) \
                    subscribes only to the listed deps and hands the body the current and \
                    previous values. on_defer behaves the same but skips the first run, so the \
                    body fires only on later changes.".to_string()
            )
            CodePanel(src = r##"use runtime_core::{signal, on};

let query = signal!(String::new());

let _e = on(query, move |current, previous| {
    // body reads are untracked; only `query` retriggers it
    refetch(current);
    let _ = previous;
});"##.to_string())

            Callout(label = "Reach for batch on bulk updates".to_string()) {
                Typography(
                    content = "Theme swaps, list resets, and form hydration all write many \
                        signals at once. Batching turns N\u{00d7}M effect runs into N \u{2014} \
                        the difference between a snappy update and a janky one on a large \
                        tree.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Batching internals and the reactive seams across the framework.".to_string(),
                link_label = "Reactivity reference".to_string(),
                doc_file = "reactivity.md".to_string(),
            )
        }
    })
}
