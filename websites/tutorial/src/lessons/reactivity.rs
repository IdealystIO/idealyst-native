//! Track 1 — Reactivity. Signals, the flush boundary, effects, derived
//! state. Everything taught here is the framework's own reactive surface;
//! no component kit is involved in the concepts.
//!
//! Order matters in this track. The flush comes second, immediately after
//! signals, because every later mechanism — when an effect runs, when a
//! memo settles, why there is no `batch` — is a consequence of it.
//!
//! Each Rust snippet is `include_str!`-ed from `crate::samples`, so the
//! compiler checks the teaching material; each lesson also embeds a live
//! panel from `crate::demo` so the reader can watch the mechanism run.

use idea_ui::{typography_kind, Typography};
use runtime_core::{ui, Element};

use crate::common::{Callout, CodePanel, DocsLink, LessonPage};
use crate::demo::{DependencyDemo, DiamondDemo, FlushDemo, StagedWriteDemo};
use crate::routes::{RX_DERIVED_ROUTE, RX_EFFECTS_ROUTE, RX_FLUSH_ROUTE, RX_SIGNALS_ROUTE};
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
                    slot in the arena its world owns. Reads report the committed value and \
                    subscribe whatever effect is running. Writes stage a pending value, and the \
                    world's driver commits every staged write together at the next flush. The \
                    unit of update is the closure that read the signal \u{2014} there is no \
                    virtual DOM and no re-render pass.".to_string()
            )
            CodePanel(src = include_str!("../samples/rx_signals.rs").to_string())

            Typography(content = "A read never sees a staged write".to_string(), kind = typography_kind::H2)
            Typography(
                content = "This is the one rule to internalize, and it holds everywhere: in a \
                    handler, in an effect body, in a component body, in plain code. get() and \
                    peek() report the value that was committed; the value you just set arrives at \
                    the flush. So a handler runs start to finish against one consistent \
                    snapshot.".to_string()
            )
            CodePanel(src = include_str!("../samples/rx_staged.rs").to_string())
            Typography(
                content = "update(|current| new) is the read-modify-write primitive, and it is \
                    the one place a staged value is visible: its closure argument is the pending \
                    value, so two increments in one turn compose to +2. A single set(get() + 1) \
                    per turn is the idiomatic counter and needs nothing else.".to_string()
            )

            Typography(content = "Watch it happen".to_string(), kind = typography_kind::H2)
            Typography(
                content = "The panel below runs the two snippets above. The trace line reports \
                    what the handler saw while it was running; the count line reports what the \
                    flush committed. Press the first button and the count moves by one even \
                    though the handler wrote twice.".to_string()
            )
            StagedWriteDemo()

            Callout(label = "set() is equality-guarded".to_string()) {
                Typography(
                    content = "T: PartialEq, and the comparison happens at commit: if the staged \
                        value equals the committed one \u{2014} including an A\u{2192}B\u{2192}A \
                        round trip within a turn \u{2014} subscribers are left asleep. \
                        set_always(v) stages and forces the notification, touch() notifies with \
                        no value write, and set_untracked(v) writes the committed value directly \
                        and notifies nobody.".to_string(),
                    muted = true,
                )
            }
            CodePanel(src = include_str!("../samples/rx_guarded.rs").to_string())

            Typography(content = "Lifetime is the scope that owns it".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Signals belong to the world their creating scope lives in, and the \
                    scope frees them when it drops. Dropping is the whole teardown story: a \
                    component's signals, effects, and memos are collected as it builds and \
                    released as it unmounts, with no dispose call. A handle that outlives its \
                    world stays safe to write through (the write is a no-op) and panics on read, \
                    which surfaces the leak.".to_string()
            )
            CodePanel(src = include_str!("../samples/rx_teardown.rs").to_string())

            Callout(label = "Why Copy matters".to_string()) {
                Typography(
                    content = "The handle is Copy \u{2014} (world, slot, generation) \u{2014} so \
                        it moves into as many closures as you like with no .clone() ceremony. The \
                        value T needs Clone because get() clones the stored value out; use \
                        with(..) for a borrowed read when cloning is expensive.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "The kernel model \u{2014} per-world arenas, generational handles, \
                    staging, and the flush algorithm.".to_string(),
                link_label = "Reactivity reference".to_string(),
                doc_file = "reactivity.md".to_string(),
            )
        }
    })
}

pub fn flush() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = RX_FLUSH_ROUTE.name(),
            title = "The flush boundary".to_string(),
            lead = "Where staged writes become one logical update.".to_string(),
        ) {
            Typography(
                content = "A flush is the moment the world commits. It drains every staged write, \
                    settles the derived values that depend on them, and then runs each affected \
                    effect once. One turn of your code therefore produces exactly one logical \
                    update, however many signals it wrote and however many of an effect's \
                    dependencies changed.".to_string()
            )

            Typography(content = "Who calls it".to_string(), kind = typography_kind::H2)
            Typography(
                content = "You don't. Each backend installs a flush driver that commits after \
                    every author-code entry point returns: event handlers, timers, animation \
                    frames, and async-task polls are wrapped at the dispatch site. Your handler \
                    runs to completion against a consistent snapshot, then the flush lands \u{2014} \
                    still in the same tick, before paint. Server rendering is the degenerate \
                    case: one flush per request, after the tree realizes.".to_string()
            )
            CodePanel(src = include_str!("../samples/rx_flush.rs").to_string())

            Typography(content = "Watch it happen".to_string(), kind = typography_kind::H2)
            Typography(
                content = "The reader below is a reactive text node, which is an effect, so its \
                    run count is an effect-run count. \"write both\" writes two signals it depends \
                    on and the count moves by one. The other two buttons write the value the \
                    signal already holds: the guarded set leaves the count alone, and set_always \
                    moves it.".to_string()
            )
            FlushDemo()

            Typography(content = "There is no batch()".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Older versions of the framework had a batch(f) wrapper whose job was \
                    to coalesce the fan-out of several writes into one round of effect runs. \
                    Staging does that for every turn, so the wrapper has no work left and the \
                    function is gone from the surface. Migrating a batch(|| { .. }) call means \
                    deleting the wrapper and keeping the writes.".to_string()
            )
            Callout(label = "What to reach for instead".to_string()) {
                Typography(
                    content = "Coalescing several writes: nothing \u{2014} they already commit \
                        together. Read-modify-write across writes in one turn: update, whose \
                        closure sees the staged value. Forcing a notification the equality guard \
                        would swallow: set_always or touch. Writing without waking anyone: \
                        set_untracked.".to_string(),
                    muted = true,
                )
            }

            Typography(content = "Handlers run outside the world".to_string(), kind = typography_kind::H2)
            Typography(
                content = "The world is entered while the tree builds and while it flushes, which \
                    means a handler executes outside it. Handles are Copy and route to their own \
                    world, so the everyday surface works: get, peek, set, set_always, touch, and \
                    update on anything you captured at build time. What needs the ambient world \
                    does not work \u{2014} creating a signal, effect, or memo inside a handler \
                    panics. Create state at build time and capture the handles.".to_string()
            )
            CodePanel(src = include_str!("../samples/rx_handler.rs").to_string())

            DocsLink(
                summary = "The staged-commit contract, the flush algorithm, and the per-backend \
                    flush drivers.".to_string(),
                link_label = "Runtime v2 migration guide".to_string(),
                doc_file = "migrating-to-runtime-v2.md".to_string(),
            )
        }
    })
}

pub fn effects() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = RX_EFFECTS_ROUTE.name(),
            title = "Effects".to_string(),
            lead = "Closures the flush re-runs when the signals they read change.".to_string(),
        ) {
            Typography(
                content = "An effect is a closure that re-runs when a signal it read on its last \
                    run commits a change. It runs once at creation to establish its \
                    subscriptions, and after that the flush runs it \u{2014} once per flush, no \
                    matter how many of its dependencies changed in that turn. Dependencies come \
                    from what the body actually read: a branch it didn't take this run does not \
                    subscribe it.".to_string()
            )
            CodePanel(src = include_str!("../samples/rx_effects.rs").to_string())

            Typography(content = "Watch it happen".to_string(), kind = typography_kind::H2)
            Typography(
                content = "The reader below takes one of two branches. In the quiet branch it \
                    never reads count, so pressing \"count + 1\" changes nothing it depends on and \
                    its run counter holds. Turn verbose on and the next bump wakes it.".to_string()
            )
            DependencyDemo()

            Typography(
                content = "Cleanup belongs to the effect".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "effect! { .. } is the block form and discards the body's value. When \
                    an effect owns a resource, use the effect(..) function form and return the \
                    cleanup from the body: it runs before each re-run and again when the owning \
                    scope drops. on_cleanup(f) does the same from inside a running effect, and \
                    panics anywhere else \u{2014} the placement is what guarantees a timer can't \
                    outlive the component that started it.".to_string()
            )
            CodePanel(src = include_str!("../samples/rx_effects_cleanup.rs").to_string())

            Callout(label = "Reading without subscribing".to_string()) {
                Typography(
                    content = "Subscriptions are rebuilt from each run, so untrack(|| sig.get()) \
                        reads a value and leaves the dependency set alone \u{2014} the usual need \
                        when an effect both reads and writes state. An effect created inside \
                        untrack still tracks its own reads: the body opens a fresh tracking \
                        window.".to_string(),
                    muted = true,
                )
            }
            CodePanel(src = include_str!("../samples/rx_untrack.rs").to_string())

            DocsLink(
                summary = "Subscription reconciliation, effect classes, and cleanup ordering at \
                    teardown.".to_string(),
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
            lead = "memo \u{2014} a cached, equality-guarded value that stays in sync.".to_string(),
        ) {
            Typography(
                content = "Computed state is a memo: a cached derived value that recomputes when \
                    its dependencies change, compares the result with PartialEq, and wakes its \
                    consumers only when the value actually moved. An equal recompute stops the \
                    cascade there.".to_string()
            )
            CodePanel(src = include_str!("../samples/rx_derived.rs").to_string())

            Typography(content = "Memos settle first".to_string(), kind = typography_kind::H2)
            Typography(
                content = "The flush separates derived values from reactions. It recomputes and \
                    commits every memo reachable from this turn's writes until none is stale, and \
                    only then runs the effects. So the diamond \u{2014} an effect reading both a \
                    signal and a memo over that signal \u{2014} always observes one settled \
                    generation of the graph, and runs once per flush.".to_string()
            )
            DiamondDemo()

            Callout(label = "Memos are read-only and pure".to_string()) {
                Typography(
                    content = "A memo's closure reads signals; it never writes them. That \
                        restriction is what keeps the derived layer acyclic and lets the flush \
                        settle it in one pass. A memo hands out the read half of the surface, so \
                        a component taking a ReadSignal<T> proves in its signature that it only \
                        observes \u{2014} split() and read_only() produce the same narrowing for \
                        a plain signal.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Derivation classes, the equality cut, and how the flush orders memos \
                    against effects.".to_string(),
                link_label = "Reactivity reference".to_string(),
                doc_file = "reactivity.md".to_string(),
            )
        }
    })
}
