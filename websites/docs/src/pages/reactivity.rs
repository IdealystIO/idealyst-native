//! Reactivity page — built via the `docs!` macro.
//!
//! Long-form coverage of the reactive model: signals, effects,
//! untracked reads, derived values, refs, scopes, the flush, dynamic
//! dependencies, performance properties, patterns, and pitfalls.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "reactivity",
    title = "Reactivity",
    category = Foundation,
    description = "The mechanism behind every change in an Idealyst app.",
    related = ["overview", "primitives", "styles", "components", "refs"],
    concepts = [
        Signal, Effect, Scope, Mount, TrackedContext, Derived, Untrack,
        Action, Memo, OnCleanup, OnScopeDrop, Reducer, Resource, Context,
    ],

    section(heading = "Intro") {
        p("This page is the long version of what the Overview introduced. \
           If you only need the gist — \"a signal notifies the small places \
           that read it\" — the Overview already has it. This page is for \
           when you want to know the full surface."),
    },

    section(heading = "The model in one paragraph") {
        p("Idealyst's reactivity is one mechanism applied uniformly. A signal \
           holds a value. When a closure reads the signal inside a tracked \
           context, the framework records the dependency. When the signal \
           changes, the framework re-runs every tracked context that read it, \
           and only those. There is no virtual DOM, no diff, no top-down \
           re-render. State, derived values, styles, themes, conditional \
           rendering, list contents, and navigation all use this same \
           mechanism underneath."),
    },

    section(heading = "Signals") {
        p("Make a signal:"),

        code(rust, r##"
            use runtime_core::signal;

            let count = signal(0);
            let name = signal(String::from("Ada"));
            let items = signal(Vec::<Item>::new());
        "##),

        p(code("signal(v)"),
          " allocates a slot in the enclosing world's arena. The ",
          code("Signal<T>"), " you hold back is a small ", code("Copy"),
          " handle that routes to that slot, which is why you can pass \
           signals into closures and child components without ever calling ",
          code(".clone()"), "."),

        p("Reading and writing:"),

        code(rust, r##"
            let n = count.get();          // tracked read
            count.set(7);                 // stage a replacement value
            count.update(|n| n + 1);      // read-modify-write
        "##),

        p(code(".get()"), " returns a clone of the current value, so it needs ",
          code("T: Clone"), ". Every signal also needs ",
          code("T: PartialEq + 'static"), " (writes are equality-guarded, below). ",
          code(".set(v)"),
          " stages a replacement. ", code(".update(|cur| next)"),
          " takes the current value by reference and returns the next one."),

        p("A write ", code("stages"),
          ": it records the pending value, and nothing observable changes \
           until the world flushes. Reads never see a staged value — a ",
          code(".get()"),
          " on the next line returns the value from before the write. The \
           flush happens automatically when your handler returns; see \
           \"When does the flush happen\" below."),

        p("The one place that difference bites is a read-back increment. ",
          code("set(get() + 1)"),
          " twice in one handler nets +1, because the second ", code("get()"),
          " still sees the committed value. ", code("update"),
          " composes on the STAGED value instead, so increments never get lost:"),

        code(rust, r##"
            // WRONG: both get()s see the committed value -> net +1.
            count.set(count.get() + 1);
            count.set(count.get() + 1);

            // RIGHT: update composes on the staged value -> net +2.
            count.update(|n| n + 1);
            count.update(|n| n + 1);
        "##),

        p(code(".set(v)"), " is equality-guarded (", code("T: PartialEq"),
          "): if the staged value equals the committed one at commit time — \
           including an A→B→A round trip that nets to zero — subscribers are \
           not notified. ", code(".set_always(v)"),
          " forces the notification, ", code(".touch()"),
          " notifies without writing, and ", code(".set_untracked(v)"),
          " writes the committed value directly, bypassing both staging and \
           notification."),

        p("Every signal has a stable identity key:"),

        code(rust, r##"
            let id: u64 = count.raw_id();
        "##),

        p("You rarely need this. It is a key for external registries — the \
           web backend's JS text bindings use it to name a signal. It can't be \
           turned back into a handle. If you find yourself reaching for ",
          code(".raw_id()"), " from app code, there's probably a more direct \
           API you want instead."),

        compare(from = React) {
            p("A ", code("Signal<T>"), " is a small Copy token. You don't \
               wrap it in ", code("useRef"), " to escape the closure rules, \
               you don't pass it through ", code("useCallback"), " to keep \
               referential equality stable — there are no closure rules and \
               no equality dance. The signal is the same value everywhere it \
               appears."),
        },
        compare(from = Solid) {
            p("Signals here behave like ", code("createSignal"), ", but the \
               getter/setter is unified — ", code("count"), " is the signal, ",
              code("count.get()"), " reads, ", code("count.set(...)"),
              " writes. No separate ", code("[count, setCount]"), " tuple."),
        },
        compare(from = SvelteFive) {
            p("A signal here is what ", code("$state"), " produces. The \
               ergonomic difference is that Svelte's runes are \
               compiler-rewritten inside ", code(".svelte"), " files — ",
              code("let count = $state(0); count++"), " reads like a plain \
               variable. Idealyst signals are plain Rust values, so reads \
               and writes are method calls."),
        },
    },

    section(heading = "What gets tracked") {
        p("Anywhere a signal is read inside a tracked context, the framework \
           records the dependency. The tracked contexts in everyday code are:"),

        list(
            ["Reactive text — ", code("text { format!(\"count: {}\", count.get()) }"),
             ". The expression is wrapped in an Effect that re-fires on signal change."],
            ["Closure props — ", code("label = move || format!(\"...\", count.get())"),
             ". A closure passed to a reactive prop is wrapped in an Effect."],
            ["Reactive ", code("if"), " inside ", code("ui!"), " — an ",
             code("if"), " whose condition contains ", code(".get()"),
             " lowers to a ", code("when"),
             " over a condition closure, which re-evaluates on change."],
            ["Reactive ", code("for"), " inside ", code("ui!"), " — a ",
             code("for"), " over a signal-backed source is wrapped so the \
              list rebuilds when the source changes."],
            ["Stylesheets — reading from the active theme is itself a tracked \
              read. Theme tokens propagate to styles automatically."],
            ["A manual effect — ", code("effect!({ ... })"),
             " inside a component, or ", code("watch(|| ...)"),
             " outside the tree, makes any closure a tracked context."],
        ),

        p("The underlying primitive in every case is a reactive effect. \
           Everything in this list is one or another way of installing one."),

        p("A signal read outside any tracked context is just a value read — \
           useful when you want to look at the current value without \
           subscribing to changes. Event handlers are a common example:"),

        code(rust, r##"
            on_click = move || {
                // Not tracked — just a snapshot at click time.
                println!("count is {}", count.get());
                count.update(|n| n + 1);
            }
        "##),

        p("The ", code("count.get()"), " inside the click handler runs at \
           click time, not at render time. It doesn't subscribe the click \
           handler to anything — event handlers are not tracked contexts."),
    },

    section(heading = "Effects") {
        p(code("effect!({ ... })"), " is the way to write a reactive effect \
           inside a component. It runs the body once, recording every signal \
           read, then re-runs the body whenever any of those signals change. \
           There is no handle to manage — the surrounding component scope owns \
           the effect and frees it on teardown."),

        code(rust, r##"
            use runtime_core::{effect, signal};

            let count = signal(0);
            effect!({
                println!("count is now {}", count.get());
            });
            count.set(1);  // stages; the effect re-runs on the flush
        "##),

        p("The macro inserts the ", code("move ||"),
          " for you (always implied — signal handles are ", code("Copy"),
          "). It takes a block: ", code("effect!({ ... })"),
          ". A closure argument (", code("effect!(move || ...)"),
          ") compiles but only builds the closure, so the body never runs. \
           The macro discards the block's value. You rarely \
           write effects by hand anyway — most of the time the framework \
           installs them for you via ", code("Text"), ", reactive props, ",
          code("when"), ", and so on. The cases where a manual effect makes \
           sense are:"),

        list(
            ["Debug logging — observe a signal without putting it on screen."],
            ["External side effects — write to a database, fire an analytics \
              event, sync to local storage."],
            ["Imperative work on a Ref handle — read a primitive's frame and \
              do something with it whenever a signal changes."],
        ),

        compare(from = React) {
            p(code("effect!"), " is in the family of ", code("useEffect"),
              ", with three concrete differences:"),
            p("1. No deps array. Idealyst tracks dependencies by what the \
               closure actually reads on each run. There is nothing to list \
               out, nothing to forget, no exhaustive-deps lint to fight. \
               Adding a signal read to the closure body subscribes to it; \
               removing the read unsubscribes."),
            p("2. Cleanup is separate from the effect. Idealyst gives you ",
              code("on_cleanup(callback)"),
              " — call it from inside an effect to register a teardown \
               that fires before the next re-run and on final disposal. \
               No \"return a function from the body,\" no implicit \
               last-statement-is-cleanup convention. See the ",
              code("on_cleanup"), " section below."),
            p("3. No commit-phase / render-phase split to reason about. An \
               effect runs during the world's flush, once per flush no \
               matter how many of its dependencies changed in that turn, \
               and after every memo it reads has settled — so an effect can \
               never observe a fresh signal next to a stale derivation."),
        },
        compare(from = Solid) {
            p(code("effect!"), " is ", code("createEffect(...)"),
              ". Same semantics: runs once eagerly, re-runs on dependency \
               change, dependencies recomputed each run."),
        },
        compare(from = VueThree) {
            p(code("effect!"), " is ", code("watchEffect"),
              ". Both auto-track reads, both re-run on change, both \
               lifetime-bound to a scope."),
        },
    },

    section(heading = "watch — reactivity outside the tree") {
        p("Reactivity lives in the component tree. ", code("effect!"),
          " is owned by the surrounding scope. But sometimes you want an \
           effect whose lifetime YOU control rather than the component's: a \
           service set up at app start, library or platform setup code. That \
           is what ", code("watch"), " is for."),

        code(rust, r##"
            use runtime_core::watch;

            // Caller-owned: lives as long as `sub`, not as long as a scope.
            let sub = watch(move || apply_drawer_class(is_open.get()));
            // `sub` is a `Subscription` — YOU own it.
        "##),

        p(code("watch(f)"), " returns a ", code("Subscription"),
          " that you own. The effect runs for as long as you hold the handle; \
           dropping it disposes the effect and runs its cleanups. Store it \
           where its lifetime should match — a struct field, a thread-local, \
           the owning service. For a one-time install that should live as long \
           as the world, call ", code("Subscription::leak()"),
          " — the honest, greppable \"pin forever\"."),

        p("Unlike ", code("effect!"),
          ", a ", code("watch"),
          " is never adopted by an ambient scope, so it behaves identically \
           whether or not one happens to be active."),

        p(code("watch"), " still creates a world effect, so it must be \
           called inside the world: a component build, an effect, or a flush. \
           Event handlers and async-task continuations run outside the world, \
           and creating an effect there panics. Set the ", code("watch"),
          " up at build time and capture what the handler needs."),

        p("Both build on the ", code("effect(|| ...)"),
          " function, which is public too. It returns a non-owning ",
          code("Effect"), " handle and, unlike the macro, accepts a cleanup \
           closure returned from the body."),
    },

    section(heading = "on_cleanup — release on drop") {
        p(code("on_cleanup(callback)"), " registers a teardown that \
           fires when the surrounding Effect re-runs or drops. Pair it \
           with any reactive run that allocates an external resource — \
           timers, sockets, native handles, third-party subscriptions:"),
        code(rust, r##"
            use runtime_core::{effect, on_cleanup, after_ms};

            effect!({
                let task = after_ms(500, || tick());
                on_cleanup(move || drop(task));
                deps.get();  // tracked: cleanup fires before each re-run
            });
        "##),
        p(code("on_cleanup"), " requires a RUNNING effect. Calling it from a ",
          code("#[component]"), " body panics with ",
          code("\"on_cleanup called outside an effect\""),
          " — wrap it in an ", code("effect!"),
          " as above, or, when the resource really belongs to \
           the component rather than to a reactive run, reach for ",
          code("on_scope_drop"), " (next section)."),
        p("Don't return a cleanup closure from an ", code("effect!"),
          " block: the macro discards the block's value, so the closure is \
           dropped without running and the resource is never released. Call ",
          code("on_cleanup"), " instead."),
        p("An effect-owned cleanup is also the correct ownership: it cancels \
           its timers when the effect goes away, so a detached callback can \
           never outlive a remounted world."),
        p("Cleanup fires before the effect's next re-run AND on final \
           disposal — exactly once per resource lifetime. Multiple ",
          code("on_cleanup"),
          " calls within a single Effect run fire in registration order."),
    },

    section(heading = "on_scope_drop — release when the component goes away") {
        p(code("on_scope_drop(callback)"),
          " is the same idea anchored one level out: it fires when the \
           surrounding OWNERSHIP SCOPE drops — a ", code("#[component]"),
          " body, a registry mount handler, any ", code("collect_owned"),
          " region. Use it for a resource acquired while BUILDING the tree, \
           where there is no running effect for ", code("on_cleanup"),
          " to attach to:"),
        code(rust, r##"
            use runtime_core::{on_scope_drop, after_animation_frame};

            #[component]
            fn Measured() -> Element {
                // Acquired during the build — no effect is running here.
                let setup = after_animation_frame(move || observe());
                on_scope_drop(move || drop(setup));
                ui! { view() }
            }
        "##),
        p("Inside a running effect it defers to ", code("on_cleanup"),
          ", so a resource acquired during a run still dies on that run's \
           re-run. Outside any world it is inert: nothing owns the scope, so \
           the callback is dropped without firing."),
        p("Pick by ownership, not by convenience. If the resource is \
           re-acquired on every run of an effect, it belongs to the effect (",
          code("on_cleanup"), "). If it is acquired once while the component \
           is built, it belongs to the component (", code("on_scope_drop"),
          ")."),
    },

    section(heading = "memo() — cached derived values") {
        p(code("memo(|| expr)"),
          " caches the result of a reactive computation. Readers \
           subscribe to the cache, not to the underlying signals — so \
           N nodes reading a memoized value pay one computation, not \
           N. The cache invalidates only when one of the memo's \
           tracked dependencies changes:"),
        code(rust, r##"
            use runtime_core::{signal, memo};

            let items = signal(vec![1, 2, 3, 4, 5]);
            let total = memo(move || items.get().iter().sum::<i32>());

            // Many readers — all subscribe to `total`, not to `items`.
            ui! { text { "Sum: {total}" } }
        "##),
        p("Memos are equality-guarded: a recompute that produces a value \
           equal to the cached one does not wake consumers. They also form \
           their own effect class, settling BEFORE user effects run in a \
           flush — so an effect reading both a signal and a memo over that \
           signal can never see the fresh signal with a stale memo."),
        compare(from = React) {
            p(code("memo()"), " is the family of ", code("useMemo"),
              " (cached value) and ", code("useSelector"),
              " (subscribe to a derived slice of state). Same caching \
               semantics; no deps array because tracking is automatic; \
               no shallow-equality skip because re-runs are already \
               only \"when dependencies actually change.\""),
        },
        compare(from = Solid) {
            p(code("memo()"), " is ", code("createMemo()"),
              " — identical caching semantics, identical \
               dependency-tracking model, and the same default \
               equality cut on the memo's output."),
        },
    },

    section(heading = "When does the flush happen") {
        p("You never call it. Each backend installs a flush driver that \
           commits after every author-code entry point returns: event \
           handlers, timers, animation frames, and async-task polls are all \
           wrapped at the dispatch site. Your handler runs to completion \
           against a consistent pre-write snapshot, then the flush commits \
           every staged write as ONE logical update — still in the same tick, \
           before paint."),
        p("So every handler is implicitly a batch. There is no ",
          code("batch(|| ...)"),
          " wrapper to write: two writes in one turn already coalesce into a \
           single fan-out, and a downstream effect sees the consistent pair."),
        code(rust, r##"
            let first = signal("Ada".to_string());
            let last  = signal("Lovelace".to_string());

            on_click = move || {
                first.set("Alan".into());
                last.set("Turing".into());
                // Both commit together when the handler returns; an effect
                // reading both re-runs ONCE, with the consistent pair.
            }
        "##),
        p("SSR is the degenerate case: one flush per request, after the tree \
           realizes. Web additionally exposes ",
          code("backend_web::newcore::flush_sync()"),
          " for foreign entry points (bench drivers, robot verbs) that need \
           the commit to have happened before they return."),
    },

    section(heading = "Trackable and on_defer()") {
        p("By default Effects auto-track every signal read on each \
           run. Sometimes you want explicit control — \"re-run only \
           when this specific set of deps changes, and not on the first \
           pass\":"),
        code(rust, r##"
            use runtime_core::{signal, on_defer};

            let count = signal(0);
            let mood  = signal("ok");

            // on_defer(deps, run) — skips the initial fire; the closure
            // only runs from the FIRST change onward, and receives the
            // new value plus the previous one.
            on_defer(count, move |c, _prev| save_to_db(*c));

            // Several deps: combine them in a memo and pass that.
            let both = memo(move || (count.get(), mood.get()));
            on_defer(both, move |(c, m), _prev| log(*c, m));
        "##),
        p("Deps go through the ", code("Trackable"),
          " trait — ", code("Signal<T>"), ", ", code("ReadSignal<T>"), ", ",
          code("Memo<T>"), ", and ", code("Reactive<T>"),
          " implement it. Tuples don't, so combine several deps in a ",
          code("memo"), " as above. The callback receives ",
          code("&T"), " and ", code("Option<&T>"),
          " and runs untracked, so reads inside it never widen the \
           subscription. ", code("on(deps, run)"),
          " is the same thing without the skipped first run."),
    },

    section(heading = "Action-driven state") {
        p("Redux-style state — one closure holding every transition, an \
           action enum as the write vocabulary — is what ",
          code("reducer(initial, fold)"),
          " gives you. It returns the state signal and a dispatch function:"),
        code(rust, r##"
            use runtime_core::reducer;

            enum CounterAction { Inc, Dec, Reset }

            let (count, dispatch) = reducer(0i32, |state: &i32, action: CounterAction| match action {
                CounterAction::Inc   => state + 1,
                CounterAction::Dec   => state - 1,
                CounterAction::Reset => 0,
            });

            ui! {
                // Read: a regular signal.
                text { "{count}" }
                // Write: dispatch actions.
                button(label = "+", on_click = move || dispatch(CounterAction::Inc))
            }
        "##),
        p("Each dispatch folds against the staged value, so two dispatches \
           in one turn apply in order and commit as one logical update. A \
           dispatch always notifies subscribers, even when the fold returns \
           an equal state. The same shape works by hand with a signal and ",
          code("update"), " if you'd rather not use the helper."),
        p("The payoff: transitions live in one closure (easier to \
           test, easier to reason about), and the action enum is the natural \
           form for time-travel and log replay."),
    },

    section(heading = "resource() — async data as a primitive") {
        p(code("resource(deps, async fetch)"),
          " makes async data a reactive value. The fetch runs once \
           when the resource is created and again whenever the ",
          code("Trackable"),
          " deps change. The returned ", code("Resource<T, E>"),
          " exposes ", code("data"), " / ", code("error"), " / ",
          code("loading"),
          " as signals, plus a ", code("refetch()"),
          " trigger and a cancellation token:"),
        code(rust, r##"
            use runtime_core::{resource, signal};

            let user_id = signal(42u32);

            let user = resource(user_id, |id, _cancel| async move {
                fetch_user(id).await
            });

            // Render against the resource's state.
            ui! {
                text {
                    move || {
                        let s = user.state();
                        if s.loading {
                            "Loading…".to_string()
                        } else if let Some(err) = s.error {
                            format!("Error: {}", err)
                        } else if let Some(u) = s.data {
                            format!("Hi, {}", u.name)
                        } else {
                            String::new()
                        }
                    }
                }
            }

            // Changing `user_id` cancels the in-flight fetch and
            // starts a new one against the new id.
            user_id.set(43);
        "##),
        p("Cancellation is real — when a new fetch starts, the \
           previous future's ", code("on_cancel"),
          " hook fires so it can release its resources. Stale \
           responses can't race against fresher ones because each \
           run carries a sequence number and the response is \
           discarded if a newer fetch already started."),
        p("Feature-gated behind the framework's ",
          code("async-driver"),
          " feature (Resource depends on ",
          code("spawn_async"), " for the per-fetch task)."),
    },

    section(heading = "Context — provide and inject") {
        p("Context propagates a value down the render tree without \
           threading it through every component's props. ",
          code("provide(value)"), " inside a parent component makes \
           the value visible to ", code("inject::<T>()"),
          " in any descendant:"),
        code(rust, r##"
            use runtime_core::{provide, inject};

            #[derive(Clone, Default)]
            struct Theme { primary: Color }

            // In a parent (or app root):
            provide(Theme { primary: Color::parse("#5b6cff") });

            // In any descendant:
            let theme: Theme = inject().expect("Theme provided upstream");
            // Or with a fallback:
            let theme: Theme = inject().unwrap_or_default();
        "##),
        p(code("inject::<T>()"), " returns ", code("Option<T>"),
          " — a clone of the most recent live ", code("provide"),
          " of that type, so ", code("T: Clone"),
          ". Multiple ", code("provide"),
          " calls shadow each other the way scoped variables do. Like ",
          code("signal"), ", both must be called inside the world (at \
           build time), not from an event handler."),
        p("Reads through ", code("inject"), " are NOT reactive on \
           their own — context is a structural concept, not a \
           reactive one. If you want the provided value itself to be \
           reactive, provide a ", code("Signal<T>"),
          " and read its ", code(".get()"),
          " inside whatever effect/memo needs reactivity."),
        compare(from = React) {
            p(code("provide"), " / ", code("inject"),
              " is ", code("React.createContext"),
              " + ", code("Provider"), " / ", code("useContext"),
              ". Closest-provider semantics match. The main \
               difference: React forces a re-render of the whole \
               consumer subtree on context value change; Idealyst \
               doesn't, because consumers only re-run if they read \
               something reactive INSIDE the injected value."),
        },
    },

    section(heading = "use_id() — stable identifiers") {
        p(code("use_id()"), " returns a string id of the form ",
          code("ui-<16 hex chars>"),
          ", derived from the current component identity. It is stable \
           and safe to use as an HTML id or ARIA target."),
        p("It is NOT unique per component today. The realizer doesn't set \
           a per-position identity, so every ", code("use_id()"),
          " call in an app returns the same string. When you need distinct \
           ids — label/input pairing, ", code("aria-controls"),
          " — use ", code("use_id_keyed(&key)"),
          " with a key that is unique within the app:"),
        code(rust, r##"
            use runtime_core::use_id_keyed;

            // The key must be unique app-wide: use_id_keyed only mixes it
            // into the shared base id.
            let name_id = use_id_keyed(&("signup-form", "name"));
            let email_id = use_id_keyed(&("signup-form", "email"));
        "##),
    },

    section(heading = "Untracked reads") {
        p("Sometimes a tracked context needs to read a signal without \
           subscribing — usually because the read is incidental (\"I want \
           the current value, but I don't want to re-run if it changes\")."),

        code(rust, r##"
            use runtime_core::untrack;

            effect!({
                let user = current_user.get();              // tracked: re-fire if user changes
                let pref = untrack(|| theme_pref.get());    // untracked: just a snapshot
                log_visit(&user, &pref);
            });
        "##),

        p("Anything inside ", code("untrack(|| ...)"),
          " runs without recording its reads. Subscriptions added before the ",
          code("untrack"), " call are preserved."),

        p("You'll know you need ", code("untrack"),
          " when you find an effect re-firing more often than makes sense."),

        compare(from = React) {
            p("Closest analog: reading ", code("ref.current"),
              " instead of a state value. The intent is the same — get the \
               current value without participating in the dependency graph."),
        },
    },

    section(heading = "Derived values") {
        p("A derived value is a function of one or more signals. Reading the \
           derived value reads its inputs reactively, so any tracked context \
           that depends on the derived value re-runs when the inputs change."),

        p("Most of the time you don't build one by hand. Inside ", code("ui!"),
          ", a reactive ", code("if"), " lowers its condition to a closure \
           that the conditional re-evaluates when the signals it reads change:"),

        code(rust, r##"
            ui! {
                // Reactive: the condition becomes a closure the branch re-runs.
                if count.get() > 10 {
                    text { "Over ten!" }
                }
            }
        "##),

        p("Any prop that accepts a value also accepts a closure, so a one-off \
           derived value is just ", code("move || count.get() * 2"),
          ". When the same derived value is used in several places, or is \
           expensive, use ", code("memo"), ":"),

        code(rust, r##"
            let count = signal(0);
            let doubled = memo(move || count.get() * 2);
        "##),

        p(code("doubled"), " is a ", code("Memo<i32>"),
          ". Anything that reads ", code("doubled.get()"), " re-runs when ",
          code("count"), " changes — and only when the doubled value \
           actually differs. Don't write a derived value as an effect that \
           sets another signal: ", code("memo"),
          " settles before effects run, while a mirror signal updates one \
           round later and can be observed stale."),

        compare(from = React) {
            p(code("memo"), " is the equivalent of ",
              code("useMemo(() => count * 2, [count])"),
              " — but the framework figures out the deps, and the result is \
               a value consumable from anywhere, not bound to a render scope."),
        },
        compare(from = Solid) {
            p(code("memo"), " is ", code("createMemo"),
              ", with the same caching and equality cut. See the ",
              code("memo()"), " section above."),
        },
        compare(from = SvelteFive) {
            p("A derived signal is what ", code("$derived(...)"), " is in a ",
              code(".svelte"), " file."),
        },
        compare(from = VueThree) {
            p("A derived signal corresponds to ", code("computed(...)"), "."),
        },
    },

    section(heading = "Refs") {
        p("A ", code("Ref<H>"),
          " is a programmatic handle to a primitive: a slot the framework \
           fills with a handle when the primitive mounts."),

        code(rust, r##"
            use runtime_core::{Ref, TextInputHandle};

            let draft = signal(String::new());
            let input: Ref<TextInputHandle> = Ref::new();

            ui! {
                text_input(value = draft, on_change = move |s: String| draft.set(s), bind = input)
                button(label = "Focus", on_click = move || {
                    if let Some(h) = input.get() {
                        h.focus();
                    }
                })
            }
        "##),

        p(code("Ref::get()"),
          " returns a clone of the handle once the primitive has mounted, or ",
          code("None"), " before that. ", code("Ref::with(|h| ...)"),
          " borrows instead of cloning, and ", code("Ref::is_mounted()"),
          " reports whether the slot is filled."),

        p("A Ref isn't reactive in the signal sense — reading ",
          code("is_mounted()"),
          " doesn't subscribe to its mount state. If you need to react to a \
           ref's lifecycle, drive a signal alongside it."),

        p("Refs have their own page — see ", link("Refs", to = "refs"),
          " for the full surface, including handles for built-in primitives \
           and how to declare handles on user components with ",
          code("#[method]"), "."),
    },

    section(heading = "Scopes and cleanup") {
        p("Every Effect and every Signal is owned by a Scope. Scopes form a tree:"),

        list(
            ["The world holds the root scope. When the world drops, the \
              entire app's reactive state is freed in one shot."],
            ["Reactive subtrees create nested scopes: the active branch of a ",
             code("When"),
             " lives in its own scope; flipping the condition drops the old \
              scope and builds the new branch in a fresh one. The same \
              applies to a ", code("Switch"),
             " (multi-way conditional) and to each iteration of a reactive ",
             code("for"), "."],
        ),

        p("When a scope drops:"),

        list(
            ["Every signal allocated inside it is freed. Reads against a \
              freed signal panic with a diagnostic message (no silent corruption)."],
            ["Every Effect allocated inside it is freed. Its subscriptions \
              are removed from every signal it was reading."],
            ["Every primitive built inside it is torn down — the backend \
              gets ", code("clear_children"),
             " (or the equivalent) on the relevant parent nodes."],
        ),

        p("This is why you don't write component teardown code. The scope \
           owns the lifecycle — there is no separate dispose call."),

        p("One ordering note: within the release window of an unmounting \
           subtree, each effect's own cleanups run in the order they were \
           registered. Sibling cleanups should not depend on each other's \
           relative order."),

        compare(from = React) {
            p("Closest analog: a component unmounting causes its hooks to \
               clean up. The difference: in React, you write the cleanup \
               function (", code("return () => clearInterval(id)"),
              "); in Idealyst, the scope drop is implicit — every signal, \
               effect, and node inside is freed together. If you have a \
               resource (a socket, a subscription) that needs an explicit ",
              code("Drop"), ", wrap it in a Rust type with a ", code("Drop"),
              " impl and let the type system handle teardown."),
        },
    },

    section(heading = "Booting — where the world comes from") {
        p("Scopes are nested, but every tree has a root, and the root has \
           to come from somewhere. Each backend exposes a boot entry that \
           creates the ", code("World"),
          ", registers the primitive handlers, runs your root component \
           inside ", code("World::enter"),
          ", realizes the returned tree, and installs the flush driver:"),

        code(rust, r##"
            // Web (client render):
            backend_web::newcore::start(|| app());
            // …with your own primitive handlers registered first:
            backend_web::newcore::start_in("#app", my_app::register_scene_extensions, app);
        "##),

        p("The native hosts mirror the shape — ",
          code("host_appkit::newcore::run"), " on macOS, ",
          code("backend_ios::newcore::run_in_view"), " on iOS, ",
          code("host_winit::newcore::run"),
          " for the GPU desktop renderer, ",
          code("backend_ssr::newcore::render_to_string"),
          " for SSR. The CLI-generated wrappers call the right one for you; \
           see the boot table in the migration guide for the full list."),

        p("Because the build runs inside ", code("World::enter"),
          ", top-level ", code("signal(…)"), " / ", code("effect!"),
          " / ", code("Ref::new"),
          " declared at the top of ", code("app()"),
          " are owned by the world root — they live for the app and are \
           freed when the world drops."),

        p("Concretely — this welcome animation pattern works the way it \
           reads:"),

        code(rust, r##"
            #[component]
            pub fn app() -> Element {
                let phase = signal(0u8);

                // Schedule a 3-beat timeline. The cleanup is owned by the
                // effect, so the timers are cancelled when the world tears
                // down — not microseconds after `app()` returns.
                effect!({
                    let t1 = after_ms(150, move || phase.set(1));
                    let t2 = after_ms(1050, move || phase.set(2));
                    let t3 = after_ms(2850, move || phase.set(3));
                    on_cleanup(move || { drop(t1); drop(t2); drop(t3); });
                });

                ui! { /* presences keyed off phase.get() */ }
            }
        "##),

        p("There is no per-component lifecycle hook to pair with this — no \
           equivalent of React's ", code("useEffect"), " / ",
          code("componentDidMount"),
          " — because the component body itself plays that role: it runs \
           once when the component mounts, and its effects' cleanups fire \
           when the owning scope drops."),

        p("Handles are strict about lifetime. A read or write through a \
           signal whose scope has dropped — while its world is still alive — \
           panics with a stale-handle diagnostic. Only a write after the \
           whole WORLD has dropped is a silent no-op. So an async task that \
           can finish after its component unmounts must check that it is \
           still live before it writes; ", code("resource"), ", ",
          code("mutation"), ", and ", code("async_reducer"),
          " do this for you (see ",
          link("Async reactivity", to = "async-reactivity"), ")."),

        compare(from = React) {
            p("The boot entry plays the role of ",
              code("createRoot(container).render(<App />)"),
              " — both are the program → backend attachment point that \
               establishes the reactive root before user code runs."),
        },
        compare(from = Solid) {
            p("≈ ", code("render(() => <App />, root)"),
              ". Both take a closure so the user's tree-construction code \
               runs inside the framework's reactive root."),
        },
        compare(from = VueThree) {
            p("≈ ", code("createApp(App).mount('#app')"),
              " — same idea: attach a program to a backend, opening the \
               root scope on the way in."),
        },
    },

    section(heading = "The flush — what happens on a signal change") {
        p("A write does not run anything. It stages, and the driver flushes:"),

        list(
            [code("signal.set(v)"),
             " records the pending value in the signal's slot. Nothing \
              observable changes; reads still return the committed value."],
            ["When the author-code entry point returns (handler, timer, frame, \
              async poll), the backend's flush driver calls ",
             code("World::flush"), "."],
            ["The flush settles DERIVATIONS first — memos recompute, in \
              dependency order, each one equality-guarded."],
            ["Then user effects run, once each no matter how many of their \
              dependencies changed in the turn. Each effect's previous \
              dependency set is cleared and rebuilt from what the body \
              actually reads on this run; the body usually makes one backend \
              call."],
            ["Writes made from inside an effect are staged too and settle \
              within the same flush, so a memo chain converges in one flush \
              rather than cascading out."],
        ),

        p("Two consequences worth internalizing. First, an effect can never \
           observe a fresh signal beside a stale memo over it — the \
           derivation class settles before user effects run, which is the \
           classic \"diamond glitch\" closed structurally. Second, the whole \
           turn is ONE logical update: N writes to N signals feeding one \
           effect produce one effect run, not N."),

        p("For a single flush, effects run in slot order — the order they \
           were created. You shouldn't rely on this for correctness (any \
           effect should be order-independent given its dependencies), but \
           it's stable and useful for debugging."),
    },

    section(heading = "Dynamic dependencies") {
        p("Each time an Effect runs, its dependency set is rebuilt from \
           scratch. Whatever signals the body reads on this run become the \
           new set; everything from the previous run is dropped."),

        code(rust, r##"
            let mode = signal("a");
            let a_value = signal(1);
            let b_value = signal(2);

            effect!({
                if mode.get() == "a" {
                    println!("a = {}", a_value.get());
                } else {
                    println!("b = {}", b_value.get());
                }
            });

            // Initial run: reads `mode` and `a_value`. Subscribed to both.
            // Now flip the branch:
            mode.set("b");
            // Re-run: reads `mode` and `b_value`. Subscriptions: `mode` + `b_value`.
            // `a_value` no longer notifies this effect — it dropped on the re-run.
        "##),

        p("You don't maintain a dependency array. You don't lint for missing \
           deps. You change what the closure reads and the framework adjusts."),

        compare(from = React) {
            p("This is the biggest practical difference from ",
              code("useEffect"),
              ". The deps array is the framework's only way to know what to \
               track; getting it wrong is one of React's classic bug classes \
               (stale closures, missed updates, infinite loops). Here the \
               framework reads what your closure reads, every run."),
        },
    },

    section(heading = "Performance properties") {
        p("A few characteristics that influence how to think about reactive code:"),

        list(
            ["Signals are Copy. Passing a signal into a closure, a child \
              component, or a slot doesn't clone anything heap-allocated; the ",
             code("Signal<T>"), " itself is three ", code("u32"),
             "s (world, slot, generation). The closure environment doesn't grow with the number of \
              signals captured."],
            ["Arena storage. Signals and effects are slots in the world's \
              arena, not individual heap allocations. The cost of making a \
              signal is bumping an index."],
            ["Per-update cost is proportional to changed nodes. A signal \
              change visits only the Effects subscribed to it; each Effect \
              usually makes one backend call. The framework does less work \
              as your app grows, not more — a 1000-component app and a \
              10-component app pay the same cost to update one node."],
            ["Cleanup is bidirectional. Subscriber sets and dependency sets \
              are kept consistent on both ends — there are no stale entries \
              to sweep. Dropping an Effect immediately removes its id from \
              every signal it was subscribed to."],
        ),
    },

    section(heading = "Patterns") {
        p("The smallest reactive thing — a counter:"),

        code(rust, r##"
            let count = signal(0);
            ui! {
                text { format!("Count: {}", count.get()) }
                button(label = "++", on_click = move || count.update(|n| n + 1))
            }
        "##),

        p("Lifting state to a parent. A child component can take a signal \
           as a prop. The signal is Copy, so there's no clone bookkeeping; \
           both parent and child read and write the same arena slot."),

        code(rust, r##"
            #[component]
            fn Counter(count: Signal<i32>) -> Element {
                ui! {
                    text { format!("Count: {}", count.get()) }
                    button(label = "++", on_click = move || count.update(|n| n + 1))
                }
            }

            #[component]
            fn App() -> Element {
                let count = signal(0);
                ui! {
                    Counter(count = count)
                    text { format!("Doubled: {}", count.get() * 2) }
                }
            }
        "##),

        p("Effect for a side effect:"),

        code(rust, r##"
            effect!({
                let user = current_user.get();
                save_to_local_storage("user", &user);
            });
        "##),

        p("Reactive condition:"),

        code(rust, r##"
            ui! {
                if logged_in.get() {
                    text { "Welcome back!" }
                } else {
                    button(label = "Log in", on_click = move || logged_in.set(true))
                }
            }
        "##),

        p("Computed value used in two places:"),

        code(rust, r##"
            let count = signal(0);
            let doubled = memo(move || count.get() * 2);

            ui! {
                text { format!("count={}", count.get()) }
                text { format!("doubled={}", doubled.get()) }
            }
        "##),
    },

    section(heading = "Pitfalls") {
        p("Components run once. The component function — the body of a ",
          code("#[component] fn"), ", including everything outside the ",
          code("ui!"), " block — runs once when the component mounts. \
           Variables assigned there are computed at that point and never \
           recomputed."),

        p("If you write:"),

        code(rust, r##"
            #[component]
            fn Greeting(name: Signal<String>) -> Element {
                let greeting_text = format!("Hello, {}", name.get());  // computed ONCE
                ui! {
                    text { greeting_text.clone() }
                }
            }
        "##),

        p("…the text never updates. ", code("name.get()"),
          " was called outside any tracked context. The fix is to do the \
           read inside the ", code("ui!"), ":"),

        code(rust, r##"
            ui! {
                text { format!("Hello, {}", name.get()) }
            }
        "##),

        p("Capturing stale values in closures. Same root cause: the closure \
           runs at construction, the read inside is what tracks."),

        code(rust, r##"
            let count = signal(0);
            let initial = count.get();    // 0, frozen
            ui! {
                // Wrong: shows "Initial: 0" forever
                text { format!("Initial: {}", initial) }
            }
        "##),

        p("If you want the current value, read the signal inside the tracked \
           context. If you want a frozen value, that's already what you have \
           — give it a name that says so."),

        p("Creating reactive state inside an event handler. Handlers run \
           OUTSIDE the world — signal handles are ", code("Copy"),
          " and route to their own world, so ", code("get"), " / ",
          code("set"), " / ", code("update"),
          " all work in a handler, but ", code("signal(…)"), ", ",
          code("effect(…)"), ", ", code("memo(…)"), ", ",
          code("provide"), " / ", code("inject"),
          " panic there. Create state at build time and capture the handles."),

        p("The free theme functions have the same shape: ",
          code("install_tokens(…)"),
          " & co. resolve the ambient world's theme context and panic in a \
           handler. Capture the context at build time and call its methods, \
           which are handler-safe:"),

        code(rust, r##"
            let theme = theme_ctx();                 // build time (world entered)
            ui! {
                button(label = "Dark", on_click = move || {
                    theme.install_tokens(&dark_tokens());   // handler-safe method
                })
            }
        "##),

        p("Reading a signal whose scope has dropped:"),

        code(rust, r##"
            // Inside ui! { for ... { let inner_signal = signal(0); ... } }
            // If you hold inner_signal past the iteration, .get() panics later.
        "##),

        p("Signals are scope-bound and single-threaded. ", code("Signal<T>"),
          " is not ", code("Send"), ", so moving one into ",
          code("std::thread::spawn"),
          " is a compile error. A read OR write through a handle whose scope \
           has dropped panics with a diagnostic; only a write after the whole \
           world has dropped is a silent no-op. Don't hold signals past their \
           owning scope's lifetime, and guard async continuations that might \
           outlive it."),

        p("Forcing a rebuild with ", code("touch()"),
          " on a reactive ", code("match"), " scrutinee. A reactive ",
          code("match"), " / ", code("switch"),
          " is keyed on the scrutinee VALUE: a dependency re-fire that \
           produces an equal value keeps the mounted arm instead of \
           rebuilding it. If you need a forced rebuild, change the value — \
           e.g. fold a generation counter into the scrutinee tuple."),
    },

    section(heading = "Where to read more") {
        list(
            ["How a render happens — the mechanism behind cascades, the \
              walker, and reactive subtrees, on the Overview page."],
            [link("Refs", to = "refs"), " — the full ref / handle surface and how ",
             code("#[method]"), " declares user-component handles."],
            ["Styles — how the styling system uses the reactive substrate \
              internally."],
            ["The wire protocol — how derived values and structured actions \
              ship to generator backends."],
        ),
    },
}
