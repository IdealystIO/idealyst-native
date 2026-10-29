+++
title = "Loading, empty, error and success states"
summary = "The five states of a data screen and the states of an action — what to show for each with idea-ui (Skeleton, Spinner, Alert, EmptyState, Toast) and how to drive them from resource/mutation."
when = "Any screen that loads data or any action that can be slow or fail."
order = 50
tags = ["states", "loading", "empty", "error", "skeleton", "toast", "resource", "feedback"]
+++
# Loading, empty, error and success states

## Data screens: five states

| State | Show | idea-ui |
| --- | --- | --- |
| loading (first load) | skeleton rows/tiles in the data's shape | `Skeleton(width, height)` in the real layout |
| refreshing (data already shown) | keep the data; a small inline indicator | `Spinner(size = Small)` by the title, or `Progress(Indeterminate)` at the top |
| error | what failed + retry, where the data would be | `Alert(tone = Danger, variant = Soft, action = Retry)` |
| empty — first run | what belongs here + the action that adds it | an `EmptyState` (the `collection_states` recipe) |
| empty — no results | which filter/search matched nothing + "Clear" | the same `EmptyState`, different copy |
| data | the content | — |

- **The frame stays put.** Header, actions, search and filters render in
  every state; only the region below them changes, so nothing jumps.
- **Never a full-screen spinner** for content. A spinner says "wait";
  a skeleton says "this is coming, shaped like this".
- **Errors are inline, not toasts.** A toast disappears before it's read;
  a failed load must stay visible with a retry.
- **First-run empty ≠ no results.** Telling a user with 400 customers
  "No customers yet — add one!" because their search missed is a bug.

## Driving it from `resource`

```rust
let customers = resource(query, move |q, _cancel| api::customers(q));
ui! {
    match customers.network_state() {
        NetworkState::Success(rows) => { CustomerResults(rows = rows.clone(), query = query, …) }
        NetworkState::Error(e) => { Alert(title = "Couldn't load customers", body = Some(e.clone()), … ) }
        _ => { ListSkeleton() }
    }
}
```

- `match` inside `ui!` on `network_state()` is reactive; bound values are
  references — `.clone()` what you pass on.
- Give each state its own component when it has branches of its own
  (empty vs no-results): nested reactive branches inside a match arm can't
  move their captures.
- `resource` keeps the last data during a refetch (`data()` stays `Some`
  while `loading()` is true) — show "refreshing", not the skeleton again.
- Retry is `customers.refetch()`.

## Actions: four states

| State | Show |
| --- | --- |
| idle | the button |
| pending | `Button(loading = true)` — same size, spinner inside, no double-submit |
| succeeded | navigate to the result, or a success toast for in-place changes |
| failed | inline error next to what failed; keep the user's input |

- **Toasts** (`push_toast(msg, tone)`) are for confirmations that need no
  action: "Profile saved", "Link copied". One `ToastHost` per app, mounted
  at the root (in the shell). `BottomCenter` on phones is thumb-visible.
- **Undo beats confirm** for reversible actions (archive, remove from a
  list): do it, toast "Archived — Undo". Confirm only what can't be undone
  (read_design_skill("overlays")).
- **Optimistic updates** only when the failure path is designed: roll
  back and say so.

## Long work

- Known progress → `Progress(value = …)` with a label ("3 of 12 uploaded").
- Unknown duration over ~1s → `Progress(mode = Indeterminate)` or a
  spinner with what's happening ("Importing customers…").
- Work that outlives the screen → let the user leave; notify when done.
