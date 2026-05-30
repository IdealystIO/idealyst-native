//! Async reactivity page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "async-reactivity",
    title = "Async reactivity",
    category = Foundation,
    description = "Reactive primitives for async work: mutation, async_reducer, resource, plus the NetworkState / AsyncStatus projections.",
    related = ["reactivity", "lists", "server-functions", "net"],
    concepts = [Mutation, AsyncReducer, NetworkState, AsyncStatus, ResourceCancel],

    section(heading = "Overview") {
        p("The ", link("Reactivity", to = "reactivity"),
          " page covers signals and effects — the synchronous mechanism that makes \
           everything else work. This page is about the async layer on top: how to \
           model \"fetch this data,\" \"submit this form,\" \"save then navigate\" \
           in a way that plays well with signals and survives across re-renders."),
        p("There are three reactive primitives for async work. Each wraps a future, \
           manages its lifecycle (idle / loading / error), and exposes its state as \
           a signal so the UI can bind to it the same way it binds to anything else. \
           The three split along two axes: ", code("when"), " the work fires \
           (reactively on dep change, or explicitly on trigger) and ", code("where"),
          " the result lives (on the primitive's own handle, or in a caller-owned \
           state signal)."),
    },

    section(heading = "The 2×2") {
        p("Three cells filled in, one intentionally empty:"),
        list(
            ["Reactive-on-deps, state-on-handle: ", code("resource(deps, fetcher)")],
            ["Explicit-trigger, state-on-handle: ", code("mutation(handler)")],
            ["Explicit-trigger, state-in-caller-signal: ", code("async_reducer(state, perform, apply)")],
            ["Reactive-on-deps, state-in-caller-signal: ", "(no built-in — write \
             a reactive ", code("Effect"), " that calls an ", code("async_reducer"),
             " on dep change)"],
        ),
        p("Most read code uses ", code("resource"), ". Most write code uses ",
          code("async_reducer"), ". ", code("mutation"),
          " is the exception, not the default — reach for it only when the response \
           is purely a notification you don't store (analytics, telemetry pings)."),
    },

    section(heading = "resource — load on dep change") {
        p("The dep-driven primitive. Use it for reads."),
        code(rust, r##"
            use runtime_core::{resource, signal, Resource};

            let user_id = signal(1u64);

            let user: Resource<User, ServerError> = resource(
                user_id,                                 // dep — tracked
                |id, _cancel| async move {
                    get_user(id).await                   // Result<User, ServerError>
                },
            );
        "##),
        p("Mechanically: the fetcher runs once at construction, then again any time \
           the deps change — tracked via the same signal-subscription machinery \
           that powers ", code("Effect"), ". The handle is ", code("Copy"),
          "; pass it freely into child components."),
        p("State is a ", code("ResourceState<T, E>"), " struct, not an enum:"),
        code(rust, r##"
            pub struct ResourceState<T, E> {
                pub data:    Option<T>,
                pub error:   Option<E>,
                pub loading: bool,
            }
        "##),
        p("All three fields are populated independently. The reason: during a \
           re-fetch (", code("loading: true"),
          ", dep changed), the PRIOR ", code("data"), " is still around. The UI \
           doesn't have to flash empty while the new fetch is in flight. This is \
           the canonical \"stale-while-revalidate\" pattern."),
        p("For UIs that don't need that subtlety, project to the ",
          code("NetworkState"), " enum via ", code(".network_state()"), ":"),
        code(rust, r##"
            match user.network_state() {
                NetworkState::Loading      => ui!{ Spinner() },
                NetworkState::Success(u)   => ui!{ text(u.name) },
                NetworkState::Error(e)     => ui!{ text(format!("{e}")) },
                NetworkState::Idle         => unreachable!(),  // resource always starts Loading
            }
        "##),
    },

    section(heading = "resource — cancellation") {
        p("The fetcher receives a ", code("ResourceCancel"),
          " token. It fires when the dep changes (the previous fetch is being \
           superseded) or when the owning scope drops (the component unmounted)."),
        p("Fetchers can poll ", code("cancel.is_cancelled()"),
          " between awaits, or register ", code("cancel.on_cancel(|| ...)"),
          " to bridge to whatever abort mechanism the underlying IO supports. The \
           framework's stale-result guard discards any result that arrives after a \
           newer fetch has started, so cancellation is an OPTIMISATION (saves \
           wall-clock + bytes), not a correctness requirement."),
        p("For server-fn calls (the most common async work in real apps), use ",
          code("server::with_cancel(resource_cancel, future)"),
          " to bridge automatically — the in-flight HTTP request gets aborted on \
           dep change. See ", link("Server functions", to = "server-functions"), "."),
    },

    section(heading = "resource — refetch on demand") {
        p("For pull-to-refresh and retry-after-error:"),
        code(rust, r##"
            user_resource.refetch();
        "##),
        p("Same machinery as a dep change — cancel previous, spawn fresh."),
    },

    section(heading = "mutation — explicit trigger, response on handle") {
        p("The simplest async primitive. Use it for writes whose response is a \
           NOTIFICATION you don't need to keep — analytics pings, save operations \
           whose success is acknowledged by the UI flipping a checkmark, anything \
           where the response IS its own state."),
        code(rust, r##"
            use runtime_core::{mutation, Mutation};

            let log_event: Mutation<EventName, (), AnalyticsError> =
                mutation(|name| async move {
                    analytics::send(name).await
                });

            ui! {
                button {
                    on_click: { let log = log_event.clone();
                                move || log.trigger("clicked_cta") },
                }
            }
        "##),
        list(
            [code(".trigger(input)"),
             " fires the handler. State transitions to Loading, then to Success(T) \
              or Error(E) when the future settles."],
            [code(".run(input).await"),
             " does the same but returns the result inline — useful in event \
              handlers that want to navigate / commit a follow-up only on success."],
            ["The handle's state is a ", code("Signal<MutationState<T, E>>"),
             ". Same fields as ", code("ResourceState"), " plus an Idle value (no \
              trigger has fired yet) — the default of ", code("loading"),
             " is ", code("false"), "."],
        ),
        p("If you find yourself writing this in a mutation's on-success path:"),
        code(rust, r##"
            mutation(|todo| async move {
                save_todo(todo).await
            });
            // ... then somewhere copying the mutation's data into a Signal<Vec<Todo>>
        "##),
        p("— you want ", code("async_reducer"),
          ". Mutation is the right primitive only when the response really IS the \
           state, not a step toward updating some larger state."),
    },

    section(heading = "async_reducer — fold response into state") {
        p("The reducer shape, async. This is the workhorse for any mutation whose \
           response is a piece of state your app already manages."),
        code(rust, r##"
            use runtime_core::{async_reducer, signal, AsyncReducer, AsyncStatus};

            let todos: Signal<Vec<Todo>> = signal!(Vec::new());

            let create: AsyncReducer<CreateTodo, ServerError> = async_reducer(
                todos,                                                      // state
                |input| async move { create_todo(input).await },            // perform
                |list, new_todo| list.push(new_todo),                       // apply
            );

            ui! {
                button {
                    on_click: {
                        let create = create.clone();
                        move || create.trigger(CreateTodo { title: "buy milk".into() })
                    },
                }
            }
        "##),
        p("The three pieces:"),
        list(
            [code("state: Signal<S>"), " — caller-owned; the data your UI binds to."],
            [code("perform: I → Future<Result<R, E>>"), " — the async action."],
            [code("apply: (&mut S, R) → ()"),
             " — how to fold the response into state."],
        ),
        p("On trigger: status flips to Loading; ", code("perform(input)"),
          " is spawned; on Ok(r), ", code("state.update(|s| apply(s, r))"),
          " runs and status flips back to Idle; on Err(e), status flips to \
           Error(e). Notably the state is NOT touched on error — the apply closure \
           is the only thing that writes to ", code("Signal<S>"), "."),
        p("The handle's ", code("AsyncStatus<E>"),
          " has no Success(T) variant — successful responses have already gone into \
           the state signal, so there's nowhere else to keep them."),
    },

    section(heading = "The three apply shapes") {
        p("Most apps need three patterns. All fit ", code("apply"), ":"),
        code(rust, r##"
            // PUSH — create returns the new item
            async_reducer(todos, create_todo, |list, t: Todo| list.push(t));

            // REPLACE-BY-KEY — toggle returns the updated item
            async_reducer(todos, toggle_todo, |list, t: Todo| {
                if let Some(slot) = list.iter_mut().find(|x| x.id == t.id) {
                    *slot = t;
                }
            });

            // REMOVE-BY-KEY — delete returns the id it removed
            async_reducer(todos, delete_todo, |list, id: u64| {
                list.retain(|t| t.id != id);
            });
        "##),
        p("The remove case is the interesting one. If \"delete\" returns ",
          code("()"),
          " you have to capture the id in the trigger closure to use it in apply. \
           Better: return the id from the server side. Convention: mutations that \
           act by identity should echo that identity back on success. Keeps apply a \
           one-arg fn and reads as a self-describing reducer step."),
    },

    section(heading = "Multiple actions, one state") {
        p("Several actions can target the same ", code("Signal<S>"),
          ". They all fold into one source of truth. Status is per-action; data is \
           shared."),
        code(rust, r##"
            let refresh = async_reducer(todos, |_| list_todos(),  |s, list| *s = list);
            let create  = async_reducer(todos, create_todo,        |s, t|    s.push(t));
            let toggle  = async_reducer(todos, toggle_todo,        |s, t|    /* replace-by-id */);
            let delete  = async_reducer(todos, delete_todo,        |s, id|   /* remove-by-id */);
        "##),
        p("This is exactly the shape ", code("examples/server-fn-demo"),
          " uses for a todo app with create / toggle / delete + a refresh button."),
    },

    section(heading = "NetworkState — projection for Resource and Mutation") {
        p("\"Collapse rich state into one of four cases.\" Both ",
          code("Resource"), " and ", code("Mutation"), " expose ",
          code(".network_state()"), " returning it:"),
        code(rust, r##"
            pub enum NetworkState<T, E> {
                Idle,         // mutations only — resources start Loading
                Loading,
                Success(T),
                Error(E),
            }
        "##),
        p("Useful when your UI doesn't need refetch-while-stale subtlety and you \
           just want a tidy ", code("match"), ". The collapse precedence is ",
          code("Loading > Error > Success > Idle"),
          ". Refetch-while-stale (data present + loading: true) collapses to \
           Loading — if you need the stale data during the new fetch, read the \
           underlying ", code("ResourceState"), " instead."),
    },

    section(heading = "AsyncStatus — async_reducer status") {
        p("Three variants, not four — there's no Success because the data lives in \
           the caller's state signal, not on the handle:"),
        code(rust, r##"
            pub enum AsyncStatus<E> {
                Idle,
                Loading,
                Error(E),
            }
        "##),
        p("The ", code("AsyncReducer::status()"), " signal carries one of these:"),
        code(rust, r##"
            match create.status().get() {
                AsyncStatus::Loading => ui!{ Spinner() },
                AsyncStatus::Error(e) => ui!{ text(format!("Couldn't save: {e}")) },
                _ => ui!{ /* nothing — the list is up-to-date */ },
            }
        "##),
    },

    section(heading = "Choosing between the primitives") {
        p("The choice usually pivots on two questions:"),
        list(
            ["Is the work driven by deps or by an explicit user action?",
             " Deps → ", code("resource"), ". Explicit → ", code("mutation"),
             " or ", code("async_reducer"), "."],
            ["For explicit work, does the response feed into existing app state, \
              or is the response itself the state? Feeds state → ",
             code("async_reducer"), ". Response is state → ", code("mutation"), "."],
        ),
        p("In practice, most read code uses ", code("resource"),
          " and most write code uses ", code("async_reducer"), ". Mutation is the \
           exception."),
    },

    section(heading = "Cancellation across the family") {
        p("All three primitives respect cancellation via the same mechanism: a ",
          code("ResourceCancel"), " token (for ", code("resource"),
          ") or a ", code("net::CancelToken"),
          " (for the lower-level cases). Cancellation prevents the post-completion \
           state update from firing; whether it also aborts the underlying IO \
           depends on the IO layer."),
        p(code("resource()"),
          " always cancels on dep change and on scope drop. The fetcher decides \
           what to do with the token."),
        p("For server-fn calls — the most common async work — the ",
          code("server::with_cancel(resource_cancel, future)"), " helper bridges ",
          code("ResourceCancel"),
          " → the HTTP transport's cancel mechanism. See ",
          link("Server functions", to = "server-functions"), " for the details."),
    },

    section(heading = "Performance properties") {
        list(
            ["Every primitive is built on Signal + Effect + ",
             code("spawn_async"), ". No special scheduler, no extra arenas."],
            ["A ", code("Resource"), " / ", code("Mutation"), " / ",
             code("AsyncReducer"), " handle is ", code("Rc"),
             "-cheap to clone. State signals are ", code("Copy"), "."],
            ["The stale-result guard adds one atomic counter increment per \
              trigger. Negligible."],
            ["Lifecycle transitions are signal writes — same cost as any other \
              signal ", code("set"), "."],
        ),
    },

    section(heading = "Where to read more") {
        list(
            [link("Reactivity", to = "reactivity"),
             " — the synchronous foundation these primitives build on."],
            [link("Net", to = "net"),
             " — the cross-platform HTTP client used by default for server-fn \
              transport. Has its own cancellation primitive (",
             code("CancelToken"), ")."],
            [link("Server functions", to = "server-functions"),
             " — the macro and SDK that produces typed async fns from a single \
              declaration. Every example on this page maps onto a ",
             code("#[server]"), " function call."],
        ),
    },
}
