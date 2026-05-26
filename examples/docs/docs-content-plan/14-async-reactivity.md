# Async work as reactive state

The [Reactivity](./03-reactivity.md) page covers signals and effects
— the synchronous mechanism that makes everything else work. This
page is about the async layer on top: how to model "fetch this
data", "submit this form", "save and then navigate" in a way that
plays well with signals and survives across re-renders.

## The model in one paragraph

There are four reactive primitives for async work. Each one wraps a
future, manages its lifecycle (idle / loading / error), and exposes
its state as a signal so the UI can bind to it the same way it
binds to anything else. The four primitives split along two axes:
**when** the work fires (reactively on dep change, or explicitly on
trigger) and **where** the result lives (on the primitive's own
handle, or in a caller-owned state signal).

| | State on the handle | State in a caller signal |
|---|---|---|
| **Reactive on deps** | `resource(deps, fetcher)` | — |
| **Explicit trigger** | `mutation(handler)` | `async_reducer(state, perform, apply)` |

That's the whole surface. Every async pattern most apps need —
load-on-mount, refresh-on-input-change, submit-form, optimistic
update, retry-on-error — composes from one of these three.

> **`mutation` vs `async_reducer`.** They look similar at the call
> site. The difference is where the response goes. `mutation` stores
> the last response on its own handle (`MutationState::data`) — fine
> for fire-and-forget where you mostly care that it happened.
> `async_reducer` folds the response into a `Signal<S>` the caller
> already owns — the right tool when the response is part of your
> app's state (a list grows, a record toggles, a counter increments).
> Most app code wants the second shape.

## `resource(deps, fetcher)` — load on dep change

The dep-driven primitive. Use it for reads.

```rust
use runtime_core::{resource, signal, Resource, ResourceState};

let user_id = signal(1u64);

let user: Resource<User, ServerError> = resource(
    user_id,                                 // dep — tracked
    |id, _cancel| async move {
        get_user(id).await                   // returns Result<User, ServerError>
    },
);
```

Mechanically:

- The fetcher runs **once at construction**, then **again any time
  `deps` change** — tracked via the same signal-subscription
  machinery that powers `Effect`.
- The handle is `Copy`. Pass it freely into child components.
- The handle exposes the lifecycle as a signal (`Resource::state()`
  returns `Signal<ResourceState<T, E>>`), so a UI binding re-renders
  on every transition.

`ResourceState<T, E>` is a struct, not an enum:

```rust
pub struct ResourceState<T, E> {
    pub data:    Option<T>,
    pub error:   Option<E>,
    pub loading: bool,
}
```

All three fields are populated independently. That sounds weird
until you see why: during a re-fetch (`loading: true`, dep changed),
the *prior* `data` is still around. The UI doesn't have to flash
empty while the new fetch is in flight. This is the canonical
"stale-while-revalidate" experience React Query and SWR popularised
— Idealyst's `resource` is the same idea, smaller.

For UIs that don't need that subtlety, project to the simpler
[`NetworkState`](#networkstate--asyncstatus) enum:

```rust
match user.network_state() {
    NetworkState::Loading      => ui!{ Spinner() },
    NetworkState::Success(u)   => ui!{ Text(u.name) },
    NetworkState::Error(e)     => ui!{ Text(format!("{e}")) },
    NetworkState::Idle         => unreachable!(),  // resource always starts Loading
}
```

### Cancellation

`resource`'s fetcher receives a `ResourceCancel` token. The token
fires when:

- The dep changes (the previous fetch is being superseded).
- The owning scope drops (the component unmounted).

Fetchers can poll `cancel.is_cancelled()` between awaits, or
register `cancel.on_cancel(|| ...)` to bridge to whatever
abort-mechanism the underlying IO supports. The framework's stale-
result guard discards any result that arrives after a newer fetch
has started, so cancellation is an *optimisation* (saves wall-clock
+ bytes), not a correctness requirement.

See [Server functions](./16-server-functions.md#cancellation) for
how server-fn calls inside a resource fetcher pick up the cancel
token automatically.

### Refetch on demand

For pull-to-refresh and retry-after-error:

```rust
resource_handle.refetch();
```

Same machinery as a dep change — cancel previous, spawn fresh.

## `mutation(handler)` — explicit trigger, response on handle

The simplest async primitive. Use it for writes whose response is a
*notification* you don't need to keep — analytics pings, save
operations whose success is acknowledged by the UI flipping a
checkmark, anything where the response is its own state.

```rust
use runtime_core::{mutation, Mutation};

let log_event: Mutation<EventName, (), AnalyticsError> =
    mutation(|name| async move {
        analytics::send(name).await
    });

ui! {
    Button {
        on_click: { let log = log_event.clone(); move || log.trigger("clicked_cta") },
    }
}
```

- `.trigger(input)` fires the handler. State transitions to `Loading`,
  then to `Success(T)` or `Error(E)` when the future settles.
- `.run(input).await` does the same but returns the result inline —
  useful in event handlers that want to navigate / commit a
  follow-up only on success.
- The handle's state is a `Signal<MutationState<T, E>>`. Same
  fields as `ResourceState` plus an `Idle` value (no trigger has
  fired yet) — the default value of `loading` is `false`.

### When you should reach for `async_reducer` instead

If you find yourself writing this in `Mutation::trigger`'s
on-success path:

```rust
mutation(|todo| async move {
    save_todo(todo).await
});
// ... then somewhere reading the mutation's data and copying it
// into a Signal<Vec<Todo>>
```

— you want `async_reducer`. `mutation` is the right primitive only
when the response really is the state, not a step toward updating
some larger state.

## `async_reducer(state, perform, apply)` — fold response into state

The reducer shape, async. This is the workhorse for any mutation
whose response is a piece of state your app already manages.

```rust
use runtime_core::{async_reducer, signal, AsyncReducer, AsyncStatus};

let todos: Signal<Vec<Todo>> = signal!(Vec::new());

let create: AsyncReducer<CreateTodo, ServerError> = async_reducer(
    todos,                                                      // state
    |input| async move { create_todo(input).await },            // perform
    |list, new_todo| list.push(new_todo),                       // apply
);

ui! {
    Button {
        on_click: {
            let create = create.clone();
            move || create.trigger(CreateTodo { title: "buy milk".into() })
        },
    }
}
```

The three pieces:

| | Lives where | Job |
|---|---|---|
| `state: Signal<S>` | caller-owned | the data your UI binds to |
| `perform: I → Future<Result<R, E>>` | passed in | the async action |
| `apply: (&mut S, R) → ()` | passed in | how to fold the response into state |

On trigger:
1. The handle's status flips to `Loading`.
2. `perform(input)` is spawned.
3. On `Ok(r)`: `state.update(|s| apply(s, r))`. Status flips to `Idle`.
4. On `Err(e)`: status flips to `Error(e)`. State is **not touched**.

Notably the handle's `AsyncStatus<E>` has no `Success(T)` variant —
successful responses have already gone into your state signal, so
there's nowhere else to keep them.

### The three apply shapes

Most apps need three patterns. They all fit `apply`:

```rust
// PUSH — create returns the new item
async_reducer(todos, create_todo, |list, t| list.push(t));

// REPLACE-BY-KEY — toggle returns the updated item
async_reducer(todos, toggle_todo, |list, t| {
    if let Some(slot) = list.iter_mut().find(|x| x.id == t.id) {
        *slot = t;
    }
});

// REMOVE-BY-KEY — delete returns the id it removed
async_reducer(todos, delete_todo, |list, id| list.retain(|t| t.id != id));
```

The remove case is the interesting one. If your "delete" returns
`()` you'd have to capture the id in the trigger closure to use it
in `apply`. Better: return the id from the server side. Convention:
**mutations that act by identity should echo that identity back on
success.** Keeps `apply` a one-arg fn and reads as a self-describing
reducer step.

### When to reach for it

If you have a list / map / record you mutate via async ops, this
is the right shape. Several actions can target the same `Signal<S>`
— they all fold into one source of truth. Status is per-action;
data is shared.

```rust
let refresh = async_reducer(todos, |_| list_todos(), |s, list| *s = list);
let create  = async_reducer(todos, create_todo, |s, t| s.push(t));
let toggle  = async_reducer(todos, toggle_todo, /* replace-by-id */);
let delete  = async_reducer(todos, delete_todo, /* remove-by-id */);
```

This is exactly the shape `examples/server-fn-demo` uses for a
todo app with create/toggle/delete + a refresh button.

## `NetworkState` / `AsyncStatus`

Two enums for the lifecycle, used in different places.

### `NetworkState<T, E>` — `Resource` and `Mutation` projection

```rust
pub enum NetworkState<T, E> {
    Idle,         // mutations only — resources start Loading
    Loading,
    Success(T),
    Error(E),
}
```

This is the "collapse rich state into one of four cases" projection.
Both `Resource` and `Mutation` expose `.network_state()` returning
it. Useful when your UI doesn't need refetch-while-stale subtlety
and you just want a tidy `match`:

```rust
match user_resource.network_state() {
    NetworkState::Loading      => /* spinner */,
    NetworkState::Success(u)   => /* render */,
    NetworkState::Error(e)     => /* show error */,
    NetworkState::Idle         => unreachable!(),
}
```

The collapse precedence is `Loading > Error > Success > Idle`.
Refetch-while-stale (data present + loading: true) collapses to
`Loading` — if you need the stale data during the new fetch, read
the underlying `ResourceState` instead.

### `AsyncStatus<E>` — `AsyncReducer` status

```rust
pub enum AsyncStatus<E> {
    Idle,
    Loading,
    Error(E),
}
```

Three variants, not four — there's no `Success` because the data
lives in the caller's state signal, not on the handle. The
`AsyncReducer::status()` signal carries one of these:

```rust
match create.status().get() {
    AsyncStatus::Loading => ui!{ Spinner() },
    AsyncStatus::Error(e) => ui!{ Text(format!("Couldn't save: {e}")) },
    _ => ui!{ /* nothing — the list is up-to-date */ },
}
```

## Choosing between the primitives

A flowchart, since the choice usually pivots on two questions:

1. **Is the work driven by deps or by an explicit user action?**
   - Deps → `resource`
   - Explicit → `mutation` or `async_reducer`
2. **For explicit work, does the response feed into existing app
   state, or is the response itself the state?**
   - Feeds existing state → `async_reducer`
   - Response is the state → `mutation`

In practice, most "read" code uses `resource` and most "write" code
uses `async_reducer`. `mutation` is the exception, not the default.

## Cancellation across the family

All three primitives respect cancellation via the same mechanism:
a `ResourceCancel` token (for `resource`) or a `net::CancelToken`
(for the lower-level cases). Cancellation prevents the post-
completion state update from firing; whether it also aborts the
underlying IO depends on the IO layer.

`resource()` always cancels on dep change and on scope drop. The
fetcher decides what to do with the token.

For server-fn calls (the most common async work in real apps), the
`server::with_cancel(resource_cancel, future)` helper bridges
`ResourceCancel` → the HTTP transport's cancel mechanism. See
[Server functions → cancellation](./16-server-functions.md#cancellation)
for the details.

## Performance properties

- Every primitive is built on `Signal` + `Effect` + `spawn_async`.
  No special scheduler, no extra arenas.
- A `Resource`/`Mutation`/`AsyncReducer` handle is `Rc`-cheap to
  clone. State signals are `Copy`.
- The stale-result guard adds one atomic counter increment per
  trigger. Negligible.
- Lifecycle transitions are signal writes — same cost as any other
  signal `set`.

## Where to read more

- [Reactivity](./03-reactivity.md) — the synchronous foundation
  these primitives build on.
- [Net](./15-net.md) — the cross-platform HTTP client used by
  default for server-fn transport. Has its own cancellation
  primitive (`CancelToken`).
- [Server functions](./16-server-functions.md) — the macro and SDK
  that produces typed async fns from a single declaration. Every
  example on this page maps onto a `#[server]` function call.
