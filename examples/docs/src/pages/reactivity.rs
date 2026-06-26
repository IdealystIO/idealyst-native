//! Reactivity page — built via the `docs!` macro.
//!
//! Long-form coverage of the reactive model: signals, effects,
//! untracked reads, derived values, refs, scopes, cascades, dynamic
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
        Action, Memo, OnCleanup, Reducer, Resource, Context,
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

            let count = signal!(0);
            let name = signal!(String::from("Ada"));
            let items = signal!(Vec::<Item>::new());
        "##),

        p(code("signal!(v)"), " is shorthand for ", code("Signal::new(v)"),
          ". The value is stored in a thread-local arena; the ",
          code("Signal<T>"), " you hold back is a small Copy token (a couple \
           of u32s) that indexes into the arena. This is why you can pass \
           signals into closures and child components without ever calling ",
          code(".clone()"), "."),

        p("Reading and writing:"),

        code(rust, r##"
            let n = count.get();          // tracked read
            count.set(7);                 // replace the value
            count.update(|n| *n += 1);    // mutate in place
        "##),

        p(code(".get()"), " returns a clone of the current value (",
          code("T: Clone"), " is the only bound). ", code(".set(v)"),
          " replaces the value. ", code(".update(|v| ...)"), " is the same as ",
          code("set(f(get()))"), " but skips the clone — useful for collections."),

        p("Both ", code(".set"), " and ", code(".update"), " run synchronously \
           and trigger every dependent Effect to re-run before they return. \
           See Cascades below for what that means in practice."),

        p("Every signal has a stable arena id:"),

        code(rust, r##"
            let id: u64 = count.id();
        "##),

        p("You rarely need this — it's the hook the wire protocol uses to \
           refer to a signal across processes. If you find yourself reaching \
           for ", code(".id()"), " from app code, there's probably a more \
           direct API you want instead."),

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
             ". A closure passed where the framework expected a ",
             code("Derived<T>"), " or reactive source is wrapped in an Effect."],
            ["Reactive ", code("if"), " inside ", code("ui!"), " — an ",
             code("if"), " whose condition contains ", code(".get()"),
             " lowers to a ", code("When"),
             " primitive whose conditional Effect re-fires on change."],
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
                count.update(|n| *n += 1);
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

            let count = signal!(0);
            effect!({
                println!("count is now {}", count.get());
            });
            count.set(1);  // re-runs the effect
        "##),

        p("The macro inserts the ", code("move ||"),
          " for you (always implied — signal handles are ", code("Copy"),
          ") and debug-asserts that a reactive scope is active, because a \
           scope-owned effect only makes sense inside the tree. You rarely \
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
            p("3. Runs synchronously on the change. Idealyst effects fire on \
               the same call stack as the ", code("signal.set()"),
              " that caused them, not after a commit phase. This is faster \
               and more predictable, but means heavy work inside an effect \
               blocks the writer."),
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
          " is owned by the surrounding scope — which is exactly why it \
           debug-asserts one is active. But sometimes you wire reactivity up \
           where there is no scope to own it: at app startup, inside an async \
           callback, in library or platform setup code. That is what ",
          code("watch"), " is for."),

        code(rust, r##"
            use runtime_core::watch;

            // No render scope here — an app-init / async / service context.
            let sub = watch(move || apply_drawer_class(is_open.get()));
            // `sub` is a `Subscription` — YOU own it.
        "##),

        p(code("watch(f)"), " returns a ", code("Subscription"),
          " that you own. The effect runs for as long as you hold the handle; \
           dropping it disposes the effect and runs its cleanups. Store it \
           where its lifetime should match — a struct field, a thread-local, \
           the owning service. For a one-time install that should live for the \
           whole process, call ", code("Subscription::leak()"),
          " — the honest, greppable \"pin forever\"."),

        p("Unlike ", code("effect!"),
          ", a ", code("watch"),
          " is never adopted by an ambient scope, so it behaves identically \
           whether or not one happens to be active. Reach for it the moment \
           you are outside a ", code("#[component]"), " body."),

        p("The raw ", code("Effect::new"),
          " constructor these both build on is sealed — author code uses ",
          code("effect!"), " or ", code("watch"), ", never it."),
    },

    section(heading = "on_cleanup — release on drop") {
        p(code("on_cleanup(callback)"), " registers a teardown that \
           fires when the surrounding Effect or scope drops. Pair it \
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
        p("Cleanup fires before the effect's next re-run AND on final \
           disposal — exactly once per resource lifetime. Multiple ",
          code("on_cleanup"),
          " calls within a single Effect run all fire in LIFO order \
           (last-registered first), the way ", code("defer"),
          " works in other languages."),
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

            let items = signal!(vec![1, 2, 3, 4, 5]);
            let total = memo(move || items.get().iter().sum::<i32>());

            // Many readers — all subscribe to `total`, not to `items`.
            text(move || format!("Sum: {}", total.get()))
        "##),
        p(code("memo_with(eq, || expr)"),
          " takes a custom equality predicate so the cache can skip \
           propagation when the new value is \"equal enough\" — useful \
           for floating-point thresholds, hash comparisons, or any \
           expensive ", code("PartialEq"), " you'd rather not run."),
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
               dependency-tracking model. ", code("memo_with(eq, ...)"),
              " is ", code("createMemo(..., undefined, { equals })"), "."),
        },
    },

    section(heading = "batch() — group writes") {
        p(code("batch(|| { /* multi-set */ })"),
          " defers effect notifications until the closure returns. \
           Multiple ", code(".set(...)"),
          " calls inside the batch produce ONE effect re-run per \
           dependent, not N:"),
        code(rust, r##"
            use runtime_core::{signal, batch};

            let first = signal!("Ada".to_string());
            let last  = signal!("Lovelace".to_string());

            // Without batch: each set fires effects independently.
            // With batch: both writes commit together, downstream
            // effects see the consistent (first, last) pair and run
            // once.
            batch(|| {
                first.set("Alan".into());
                last.set("Turing".into());
            });
        "##),
        p("Use this for any logically-atomic multi-write. The cost is \
           a small allocation per batch; the win is one rebuild \
           instead of N for the worst-case fan-out."),
    },

    section(heading = "Trackable, on(), and on_defer()") {
        p("By default Effects auto-track every signal read on each \
           run. Sometimes you want explicit control — \"re-run only \
           when this specific set of deps changes\":"),
        code(rust, r##"
            use runtime_core::{signal, on, on_defer};

            let count = signal!(0);
            let mood  = signal!("ok");

            // on(deps, run) — fires immediately + on every dep change.
            on(count, move |c| println!("count: {}", c));

            // on((count, mood), ...) — multiple deps, fires on either.
            on((count, mood), move |(c, m)| println!("{} {}", c, m));

            // on_defer — same as on() but skips the initial fire.
            // The closure only runs from the FIRST change onward.
            on_defer(count, move |c| save_to_db(c));
        "##),
        p("Deps go through the ", code("Trackable"),
          " trait — ", code("Signal<T>"),
          " implements it, and so do tuples up to arity 4. Trackable \
           is the \"this set of things is observable as a unit\" \
           abstraction; the Effect uses it to subscribe to all of \
           them at once and read them as a tuple when re-running."),
    },

    section(heading = "reducer() — action-driven state") {
        p(code("reducer(initial, |state, action| next_state)"),
          " gives you Redux-style state: a read-only signal of the \
           current state, and a dispatch function for sending \
           actions. The reducer closure is called inside a tracked \
           context, so reads inside it auto-subscribe:"),
        code(rust, r##"
            use runtime_core::{reducer, Action};

            enum CounterAction { Inc, Dec, Reset }

            let (count, dispatch) = reducer(0i32, |state, action| match action {
                CounterAction::Inc   => state + 1,
                CounterAction::Dec   => state - 1,
                CounterAction::Reset => 0,
            });

            // Read: a regular signal.
            text(move || format!("{}", count.get()));

            // Write: dispatch actions.
            button("+", move || dispatch(CounterAction::Inc));
        "##),
        p("Why reducer when you have signals? Two reasons. The state \
           transitions live in one closure (easier to test, easier to \
           reason about), and the action enum is the natural shape \
           for time-travel / log replay / generator-backend wire \
           formats. Reducer pairs with ", code("Action"),
          " (see Derived values above) for round-tripping through \
           Roku-style generator backends."),
    },

    section(heading = "resource() — async data as a primitive") {
        p(code("resource(deps, async fetch)"),
          " makes async data a reactive value. The fetch runs the \
           first time you read the resource and re-runs whenever the \
           ", code("Trackable"),
          " deps change. The returned ", code("Resource<T, E>"),
          " exposes ", code("data"), " / ", code("error"), " / ",
          code("loading"),
          " as signals, plus a ", code("refetch()"),
          " trigger and a cancellation token:"),
        code(rust, r##"
            use runtime_core::{resource, signal};

            let user_id = signal!(42u32);

            let user = resource(user_id, |id| async move {
                fetch_user(id).await
            });

            // Render against the resource's state.
            text(move || {
                let s = user.state();
                if s.loading {
                    "Loading…".into()
                } else if let Some(err) = s.error {
                    format!("Error: {}", err)
                } else if let Some(u) = s.data {
                    format!("Hi, {}", u.name)
                } else {
                    "".into()
                }
            });

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
            use runtime_core::{provide, inject, inject_or};

            #[derive(Clone)]
            struct Theme { primary: Color }

            // In a parent (or app root):
            provide(Theme { primary: Color::parse("#5b6cff") });

            // In any descendant:
            let theme: Theme = inject().expect("Theme provided upstream");
            // Or with a fallback:
            let theme: Theme = inject_or(Theme::default());
        "##),
        p("Lookup is closest-provider — multiple ", code("provide"),
          " calls in a chain shadow each other the way scoped \
           variables do, not the way a global registry would. \
           ", code("with_inject(|t: &Theme| { ... })"),
          " is the borrowing variant that lets you read without \
           cloning."),
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
        p(code("use_id()"), " returns an opaque, stable string \
           identifier tied to the current call-site identity. Same \
           component instance → same id, every render. Different \
           instances → different ids:"),
        code(rust, r##"
            use runtime_core::use_id;

            let id = use_id();  // e.g. "ui-1a3f9c0d8e4b2671"
            ui! {
                text(style = label_style) { "Label" }
                // Use `id` to wire ARIA / form labels / unique CSS hooks.
            }
        "##),
        p("The format is ", code("ui-<16 hex chars>"),
          " — stable across re-renders, unique within the tree, safe \
           to use as an HTML id, ARIA target, or analytics key. ",
          code("use_id_keyed(key)"),
          " mixes a user-supplied key into the hash for cases where \
           the same component instance needs multiple distinct ids."),
        p("Primarily useful for SSR-related work and accessibility \
           (label/input pairing, ARIA controls/labelledby) — anywhere \
           you'd reach for ", code("React.useId"),
          " or a counter that survives reconciliation."),
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

        p("Most of the time you don't construct a ", code("Derived<T>"),
          " by hand. Inside ", code("ui!"),
          ", the macro recognizes reactive call shapes and emits a ",
          code("Derived<T>"), " for you:"),

        code(rust, r##"
            ui! {
                // Reactive: the macro wraps this in a Derived<bool> Effect.
                if count.get() > 10 {
                    text { "Over ten!" }
                }
            }
        "##),

        p("When you do need an explicit derived value — for example, a \
           computed value used in two places — you can compose the same \
           shape manually with an effect that writes to a derived signal:"),

        code(rust, r##"
            let count = signal!(0);
            let doubled = signal!(0);
            effect!({ doubled.set(count.get() * 2) });
        "##),

        p(code("doubled"), " is now a signal that mirrors ", code("count * 2"),
          ". Anything that reads ", code("doubled.get()"), " re-runs when ",
          code("count"), " changes."),

        p("The first-class ", code("Derived<T>"), " type lives in ",
          code("runtime-core"),
          " and carries both the runtime closure and a structured description \
           (method name + input signal ids). The structured form is what lets \
           generator backends like Roku ship the derived expression to the \
           device without shipping a closure. You only build one explicitly \
           when you're writing a primitive or working at the wire-protocol layer."),

        compare(from = React) {
            p("A pattern like the ", code("doubled"),
              " signal above is the equivalent of ",
              code("useMemo(() => count * 2, [count])"),
              " — but the framework figures out the deps, and the result is \
               a value consumable from anywhere, not bound to a render scope."),
        },
        compare(from = Solid) {
            p(code("Derived<T>"), " is ", code("createMemo"),
              "'s structural cousin. Solid's ", code("createMemo"),
              " lazily caches and recomputes; the ", code("Effect + signal"),
              " pattern shown above is the manual equivalent. An explicit \
               memo primitive is on the roadmap."),
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
          " is a programmatic handle to a primitive. It's allocated in the \
           same arena as signals and effects, but it's not a value type — \
           it's a slot that the framework fills with a handle when the \
           primitive mounts."),

        code(rust, r##"
            use runtime_core::{Ref, ButtonHandle};

            let btn: Ref<ButtonHandle> = Ref::new();

            ui! {
                button(label = "Increment", on_click = on_click).bind(btn)
            }

            // Later, from any signal-write context:
            btn.with(|h| h.trigger());        // call the button's `trigger` method
        "##),

        p(code("Ref::with(|h| ...)"),
          " runs the closure with the handle if the primitive is currently \
           mounted; returns ", code("None"), " if it isn't. ",
          code("Ref::get()"), " and ", code("Ref::is_mounted()"),
          " are convenience variants."),

        p("A Ref isn't reactive in the signal sense — reading ",
          code("is_mounted()"),
          " doesn't subscribe to its mount state. If you need to react to a \
           ref's lifecycle, drive a signal alongside it."),

        p("Refs have their own page — see Refs for the full surface, \
           including handles for built-in primitives and how to declare \
           handles on user components via ", code("methods!"), "."),
    },

    section(heading = "Scopes and cleanup") {
        p("Every Effect and every Signal is owned by a Scope. Scopes form a tree:"),

        list(
            ["The renderer's ", code("Owner"),
             " holds the root scope. When the owner drops, the entire app's \
              reactive state is freed in one shot."],
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
           owns the lifecycle."),

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

    section(heading = "mount() — opening the root scope") {
        p("Scopes are nested, but every tree has a root, and the root has \
           to come from somewhere. The framework's entry point is ",
          code("runtime_core::mount(backend, app)"),
          " — it opens the root reactive scope and runs the user's ",
          code("app"), " constructor inside it:"),

        code(rust, r##"
            use runtime_core::mount;

            // Host glue (web.rs, generated iOS/Android wrappers, etc.):
            let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
            let owner = mount(backend, super::app);
            //                            ^^^^^^^^^^
            //          function pointer (`fn() -> Element`); `mount`
            //          calls it inside the root scope, then walks the
            //          returned tree.
        "##),

        p("The closure form is what makes top-level reactive primitives \
           work as you'd expect. ", code("signal!"), " / ", code("effect!"),
          " / ", code("Ref::new"),
          " declared at the top of ", code("app()"),
          " are adopted by the root scope, so they're freed on ",
          code("Owner"),
          " drop alongside everything inside the tree."),

        p("Concretely — this welcome animation pattern works the way it \
           reads:"),

        code(rust, r##"
            #[component]
            pub fn app() -> Element {
                let phase = signal!(0u8);

                // Schedule a 3-beat timeline. Cleanups fire on
                // page teardown (when Owner drops), not microseconds
                // after `app()` returns.
                effect!({
                    let t1 = after_ms(150, move || phase.set(1));
                    let t2 = after_ms(1050, move || phase.set(2));
                    let t3 = after_ms(2850, move || phase.set(3));
                    on_cleanup(move || { drop(t1); drop(t2); drop(t3); });
                });

                ui! { /* presences keyed off phase.get() */ }
            }
        "##),

        p("Under ", code("mount"), ", the ", code("effect!"),
          " is owned by the root scope; the timer handles stashed via ",
          code("on_cleanup"),
          " stay alive for the page lifetime. Nothing is leaked, nothing \
           is cancelled prematurely."),

        p("Note that ", code("mount"),
          " is the framework entry point — not a per-component lifecycle \
           hook. There's no equivalent of React's ",
          code("useEffect"), " / ", code("componentDidMount"),
          " because the component body itself plays that role: it runs \
           once when the scope opens, again on signal change, and ",
          code("on_cleanup"),
          " teardowns fire when the scope drops."),

        compare(from = React) {
            p(code("mount(backend, app)"), " plays the role of ",
              code("createRoot(container).render(<App />)"),
              " — both are the program → backend attachment point that \
               establishes the reactive root before user code runs."),
        },
        compare(from = Solid) {
            p(code("mount(backend, app)"), " ≈ ",
              code("render(() => <App />, root)"),
              ". Both take a closure so the user's tree-construction code \
               runs inside the framework's reactive root."),
        },
        compare(from = VueThree) {
            p(code("mount(backend, app)"), " ≈ ",
              code("createApp(App).mount('#app')"),
              ". Borrowed the name from Vue 3, in fact — same idea: \
               attach a program to a backend, opening the root scope on \
               the way in."),
        },
    },

    section(heading = "render() — the value-taking variant") {
        p(code("runtime_core::render(backend, primitive_value)"),
          " is the pre-built-tree alternative: it takes a ",
          code("Element"),
          " value that the caller has already constructed, and opens \
           the root scope around the build walk only. It's literally ",
          code("mount(backend, move || tree)"),
          " under the hood:"),

        code(rust, r##"
            pub fn render<B: Backend + 'static>(
                backend: Rc<RefCell<B>>,
                tree: Element,
            ) -> Owner {
                mount(backend, move || tree)
            }
        "##),

        p("Reach for ", code("render"),
          " when there is no user-authored constructor to run inside \
           the scope — e.g. tests that build a fixture ",
          code("Element"),
          " by hand, or wire-protocol replay paths that synthesize a \
           tree from incoming commands. New host glue should prefer ",
          code("mount"),
          " because as soon as user code grows a top-level ",
          code("effect!"),
          ", the value-taking form silently drops its cleanups."),

        p("The CLI scaffold and the generated iOS / Android wrappers \
           already use ", code("mount"),
          " by default — new projects don't have to think about this."),
    },

    section(heading = "Cascades — what happens on a signal change") {
        p("The cascade machinery is documented in detail on the Overview's \
           \"How a render happens\" section. The summary:"),

        list(
            [code("signal.set(v)"), " writes the new value to the arena."],
            ["It snapshots the signal's current subscriber set."],
            ["Each subscriber Effect re-runs in turn: its previous dependency \
              set is cleared, the closure runs with ", code("CURRENT"),
             " set to its id, so any read records as a fresh dependency, and \
              the closure usually makes one backend call."],
            ["If a subscriber's run writes another signal, that signal's \
              subscribers are run before the outer write returns."],
        ),

        p("Cascades are synchronous, depth-first, and bounded by the \
           re-entry guard (an Effect that fires the signal it's currently \
           reading is skipped, matching how Solid, MobX, and Reactively \
           handle the same pattern)."),

        p("There is no scheduler queue, no microtask drain, no batch \
           boundary. By the time ", code("set"),
          " returns, every downstream Effect has either run or been skipped, \
           and every backend call those Effects produced has been made."),

        p("For a single signal write, subscribers run in arena-id order — \
           the order they were created. You shouldn't rely on this for \
           correctness (any Effect should be order-independent given its \
           dependencies), but it's stable and useful for debugging."),

        p("For chained cascades, the order is depth-first: writes from \
           inside an Effect run their consequences before the outer write \
           returns."),
    },

    section(heading = "Dynamic dependencies") {
        p("Each time an Effect runs, its dependency set is rebuilt from \
           scratch. Whatever signals the body reads on this run become the \
           new set; everything from the previous run is dropped."),

        code(rust, r##"
            let mode = signal!("a");
            let a_value = signal!(1);
            let b_value = signal!(2);

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
             code("Signal<T>"), " itself is two ", code("u32"),
             "s. The closure environment doesn't grow with the number of \
              signals captured."],
            ["Arena storage. Signals and effects are slots in a thread-local \
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
            let count = signal!(0);
            ui! {
                text { format!("Count: {}", count.get()) }
                button(label = "++", on_click = move || count.update(|n| *n += 1))
            }
        "##),

        p("Lifting state to a parent. A child component can take a signal \
           as a prop. The signal is Copy, so there's no clone bookkeeping; \
           both parent and child read and write the same arena slot."),

        code(rust, r##"
            #[component]
            fn counter(count: Signal<i32>) -> Element {
                ui! {
                    text { format!("Count: {}", count.get()) }
                    button(label = "++", on_click = move || count.update(|n| *n += 1))
                }
            }

            #[component]
            fn app() -> Element {
                let count = signal!(0);
                ui! {
                    counter(count = count)
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
            let count = signal!(0);
            let doubled = signal!(0);
            effect!({ doubled.set(count.get() * 2) });

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
            fn greeting(name: Signal<String>) -> Element {
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
            let count = signal!(0);
            let initial = count.get();    // 0, frozen
            ui! {
                // Wrong: shows "Initial: 0" forever
                text { format!("Initial: {}", initial) }
            }
        "##),

        p("If you want the current value, read the signal inside the tracked \
           context. If you want a frozen value, that's already what you have \
           — give it a name that says so."),

        p("Writing a signal from inside its own Effect:"),

        code(rust, r##"
            let count = signal!(0);
            effect!({
                let v = count.get();
                count.set(v + 1);    // re-entry: this run is skipped, no loop
            });
        "##),

        p("The re-entry guard skips an Effect that's already running. The \
           write happens, but the Effect doesn't loop. If you needed that \
           write to fire other subscribers, it still does — only the \
           self-fire is suppressed."),

        p("Reading a signal whose scope has dropped:"),

        code(rust, r##"
            let s = signal!(0);          // owned by current scope
            let _ = std::thread::spawn(move || s.get());  // wrong: panic on other thread

            // Inside ui! { for ... { let inner_signal = signal!(0); ... } }
            // If you hold inner_signal past the iteration, .get() panics later.
        "##),

        p("Signals are scope-bound and single-threaded. Reads after the \
           scope's drop panic with a diagnostic. Don't hold signals past \
           their owning scope's lifetime."),
    },

    section(heading = "Where to read more") {
        list(
            ["How a render happens — the mechanism behind cascades, the \
              walker, and reactive subtrees, on the Overview page."],
            ["Refs — the full ref / handle surface and how ", code("methods!"),
             " declares user-component handles."],
            ["Styles — how the styling system uses the reactive substrate \
              internally."],
            ["The wire protocol — how ", code("Derived<T>"), " and ",
             code("Action"), "'s structured form ship to generator backends."],
        ),
    },
}
