//! Reactivity page — built via the `docs!` macro.
//!
//! Long-form coverage of the reactive model: signals, effects,
//! untracked reads, derived values, refs, scopes, cascades, dynamic
//! dependencies, performance properties, patterns, and pitfalls.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{codeblock, pageheader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{body, card, heading, stack};

docs! {
    slug = "reactivity",
    title = "Reactivity",
    category = Foundation,
    description = "The mechanism behind every change in an Idealyst app.",
    related = ["overview", "primitives", "styles", "components", "refs"],
    concepts = [Signal, Effect, Scope, TrackedContext, Derived, Untrack, Action],

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
            use framework_core::signal;

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
            ["Reactive text — ", code("Text { format!(\"count: {}\", count.get()) }"),
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
            ["A manual ", code("Effect"), " — ", code("Effect::new(|| ...)"),
             " is the lowest-level way to make any closure a tracked context."],
        ),

        p("The underlying primitive in every case is ", code("Effect"),
          ". Everything in this list is one or another way of installing one."),

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
        p(code("Effect::new(closure)"), " is the lowest-level reactive \
           primitive. It runs the closure once, recording every signal read, \
           then re-runs the closure whenever any of those signals change."),

        code(rust, r##"
            use framework_core::Effect;

            let _e = Effect::new(move || {
                println!("count is now {}", count.get());
            });
        "##),

        p("You rarely write effects by hand — most of the time the framework \
           installs them for you via ", code("Text"), ", reactive props, ",
          code("when"), ", and so on. The cases where a manual Effect makes \
           sense are:"),

        list(
            ["Debug logging — observe a signal without putting it on screen."],
            ["External side effects — write to a database, fire an analytics \
              event, sync to local storage."],
            ["Imperative work on a Ref handle — read a primitive's frame and \
              do something with it whenever a signal changes."],
        ),

        p(code("Effect::new"), " returns an ", code("Effect"),
          " handle. What happens when the handle drops depends on context:"),

        list(
            ["Inside a component's ", code("app()"), " body, the renderer's \
              surrounding ", code("Owner"), " captures the effect into the \
              current scope. The returned handle's drop is a no-op. The \
              effect lives until the scope drops (when the component's \
              subtree is replaced or torn down)."],
            ["Outside any scope — say, in a top-level helper before the app \
              starts — the handle owns the effect. Drop it to stop the effect \
              from firing."],
        ),

        p("In practice, application code creates effects inside components, \
           so \"hand the returned ", code("Effect"), " to ", code("let _e ="),
          " and forget about it\" is the normal pattern. The scope owns the \
           lifecycle."),

        compare(from = React) {
            p(code("Effect::new"), " is in the family of ", code("useEffect"),
              ", with three concrete differences:"),
            p("1. No deps array. Idealyst tracks dependencies by what the \
               closure actually reads on each run. There is nothing to list \
               out, nothing to forget, no exhaustive-deps lint to fight. \
               Adding a signal read to the closure body subscribes to it; \
               removing the read unsubscribes."),
            p("2. No cleanup function. When the effect's scope drops, its \
               subscriptions are released automatically. If you need to undo \
               a side effect on teardown — close a socket, cancel a timer — \
               you do it by writing a destructor on whatever resource the \
               effect created, not by returning a cleanup function from the \
               effect itself."),
            p("3. Runs synchronously on the change. Idealyst effects fire on \
               the same call stack as the ", code("signal.set()"),
              " that caused them, not after a commit phase. This is faster \
               and more predictable, but means heavy work inside an effect \
               blocks the writer."),
        },
        compare(from = Solid) {
            p(code("Effect::new(...)"), " is ", code("createEffect(...)"),
              ". Same semantics: runs once eagerly, re-runs on dependency \
               change, dependencies recomputed each run."),
        },
        compare(from = VueThree) {
            p(code("Effect::new"), " is ", code("watchEffect"),
              ". Both auto-track reads, both re-run on change, both \
               lifetime-bound to a scope."),
        },
    },

    section(heading = "Untracked reads") {
        p("Sometimes a tracked context needs to read a signal without \
           subscribing — usually because the read is incidental (\"I want \
           the current value, but I don't want to re-run if it changes\")."),

        code(rust, r##"
            use framework_core::untrack;

            Effect::new(move || {
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
                    Text { "Over ten!" }
                }
            }
        "##),

        p("When you do need an explicit derived value — for example, a \
           computed value used in two places — you can compose the same \
           shape manually with an effect that writes to a derived signal:"),

        code(rust, r##"
            let count = signal!(0);
            let doubled = signal!(0);
            let _e = Effect::new(move || doubled.set(count.get() * 2));
        "##),

        p(code("doubled"), " is now a signal that mirrors ", code("count * 2"),
          ". Anything that reads ", code("doubled.get()"), " re-runs when ",
          code("count"), " changes."),

        p("The first-class ", code("Derived<T>"), " type lives in ",
          code("framework-core"),
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
            use framework_core::{Ref, ButtonHandle};

            let btn: Ref<ButtonHandle> = Ref::new();

            ui! {
                Button(label = "Increment", on_click = on_click).bind(btn)
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

            let _e = Effect::new(move || {
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
                Text { format!("Count: {}", count.get()) }
                Button(label = "++", on_click = move || count.update(|n| *n += 1))
            }
        "##),

        p("Lifting state to a parent. A child component can take a signal \
           as a prop. The signal is Copy, so there's no clone bookkeeping; \
           both parent and child read and write the same arena slot."),

        code(rust, r##"
            #[component]
            fn counter(count: Signal<i32>) -> Primitive {
                ui! {
                    Text { format!("Count: {}", count.get()) }
                    Button(label = "++", on_click = move || count.update(|n| *n += 1))
                }
            }

            #[component]
            fn app() -> Primitive {
                let count = signal!(0);
                ui! {
                    counter(count = count)
                    Text { format!("Doubled: {}", count.get() * 2) }
                }
            }
        "##),

        p("Effect for a side effect:"),

        code(rust, r##"
            Effect::new(move || {
                let user = current_user.get();
                save_to_local_storage("user", &user);
            });
        "##),

        p("Reactive condition:"),

        code(rust, r##"
            ui! {
                if logged_in.get() {
                    Text { "Welcome back!" }
                } else {
                    Button(label = "Log in", on_click = move || logged_in.set(true))
                }
            }
        "##),

        p("Computed value used in two places:"),

        code(rust, r##"
            let count = signal!(0);
            let doubled = signal!(0);
            let _e = Effect::new(move || doubled.set(count.get() * 2));

            ui! {
                Text { format!("count={}", count.get()) }
                Text { format!("doubled={}", doubled.get()) }
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
            fn greeting(name: Signal<String>) -> Primitive {
                let greeting_text = format!("Hello, {}", name.get());  // computed ONCE
                ui! {
                    Text { greeting_text.clone() }
                }
            }
        "##),

        p("…the text never updates. ", code("name.get()"),
          " was called outside any tracked context. The fix is to do the \
           read inside the ", code("ui!"), ":"),

        code(rust, r##"
            ui! {
                Text { format!("Hello, {}", name.get()) }
            }
        "##),

        p("Capturing stale values in closures. Same root cause: the closure \
           runs at construction, the read inside is what tracks."),

        code(rust, r##"
            let count = signal!(0);
            let initial = count.get();    // 0, frozen
            ui! {
                // Wrong: shows "Initial: 0" forever
                Text { format!("Initial: {}", initial) }
            }
        "##),

        p("If you want the current value, read the signal inside the tracked \
           context. If you want a frozen value, that's already what you have \
           — give it a name that says so."),

        p("Writing a signal from inside its own Effect:"),

        code(rust, r##"
            let count = signal!(0);
            Effect::new(move || {
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
