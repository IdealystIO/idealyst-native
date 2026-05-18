# Lists and virtualization

For lists with more than a few dozen items, you don't want to
build every row up front and hand the backend a huge tree.
Idealyst's `flat_list<T>` is a virtualized list — it only mounts
the rows currently in (or near) the viewport, recycles them as
you scroll, and discards the rest. Memory stays bounded by the
window size; scroll performance stays smooth regardless of how
big the data is.

The wrapper underneath every Idealyst virtualized list is the
`Virtualizer` primitive. `flat_list` is the typed convenience
layer on top — the API you'll actually use from app code.

## The shape

```rust
use framework_core::{flat_list, fixed_size, signal, ui, Signal};

#[derive(Clone)]
pub struct Message {
    pub id: u64,
    pub author: String,
    pub body: String,
}

#[component]
pub fn inbox() -> Primitive {
    let messages: Signal<Vec<Message>> = signal!(load_messages());

    ui! {
        flat_list(
            data       = messages,
            key        = |_idx, msg| msg.id,
            item_size  = fixed_size(72.0),
            render_item = |_idx, msg| message_row(msg),
        )
    }
}

fn message_row(msg: &Message) -> Primitive {
    ui! {
        View {
            Text { msg.author.clone() }
            Text { msg.body.clone() }
        }
    }
}
```

Four required pieces:

- **`data`** — a `Signal<Vec<T>>` holding the source of truth.
- **`key`** — a function that returns a stable `u64` identity per
  item.
- **`item_size`** — either `fixed_size(N)` for uniform-height
  rows, or a per-item closure for variable heights (see below).
- **`render_item`** — builds the subtree for one item. Called
  only when the item enters the mount window.

That's the whole API surface for the common case. The advanced
knobs (`overscan`, `horizontal`) are builder methods on the
returned handle.

## How it stays bounded

The framework never realizes more than the rows the backend
needs. When you scroll, the backend computes which rows are now
inside the viewport (plus an overscan buffer for smooth-feeling
scrolling), tells the framework which to mount and which to
unmount, and the framework runs your `render_item` for the new
ones.

Each rendered item lives in its own **per-item reactive scope**.
When the item scrolls out of the window:

1. The scope drops.
2. Every signal allocated in that item's `render_item` is freed.
3. Every effect, every ref, every backend node tied to that
   item is torn down.

When the item scrolls back in, a fresh scope is built and
`render_item` runs again. State that lived inside the item's
scope doesn't persist across scroll-out events — if you need
state that survives, lift it into the parent's scope or into the
item's `Message` struct.

## Keys — what they're for

The `key` closure returns a stable `u64` identity per item.
Whenever `data` changes (insertions, removals, reorders), the
framework matches old keys against new keys to figure out which
items moved, which stayed, and which need to be torn down or
built up.

Items whose key is still present after a data change **keep their
mounted subtree intact**. They may move in the layout, but their
internal state survives.

```rust
key = |_idx, msg| msg.id           // stable: matches database id
key = |idx, _msg| idx as u64        // unstable: every reorder shuffles state
key = |_idx, msg| hash(&msg.body)   // works, but recomputed every check
```

Two items returning the **same** key is a bug. The framework
treats them as the same logical row and will silently drop one
of them. Use a hashable unique field (a database id, a GUID).

## Sizing strategies

The backend needs to know how big each item is to lay out the
scroll content correctly. Two strategies:

### `Known`

The author tells the framework the exact height. The backend
never measures.

```rust
// Every row is 72px tall.
item_size = fixed_size(72.0)

// Or per-item, from data:
item_size = FlatListItemSize::Known(Rc::new(|_idx, msg: &Message| {
    if msg.has_image { 200.0 } else { 72.0 }
}))
```

Use this whenever the size is a function of data you have. It's
the cheapest path — layout is deterministic, the backend doesn't
install any measurement hooks.

### `Measured`

The author provides an *estimate*; the backend measures the
actual rendered size after mount and stores it. Subsequent
layout uses the measured value.

```rust
item_size = FlatListItemSize::Measured(Rc::new(|_idx, _msg| 100.0))
```

Use this when the rendered size depends on layout/content the
framework can't see ahead of time — wrapped text whose wrap
width depends on the container, items with images of unknown
aspect ratio, anything where "tall enough" needs the backend's
layout pass to determine.

If the item's rendered size *changes* later (the item's content
updates), the backend's native layout observer
(`ResizeObserver` on web, `layoutSubviews` on iOS,
`OnLayoutChangeListener` on Android) re-fires and refreshes
the stored size.

Cost: each measured item carries a layout observer. Use `Known`
when you can.

## Overscan and direction

The two builder methods worth knowing:

- **`.overscan(factor)`** — multiplier on the viewport height
  for the mount window. `1.0` (default) means mount one
  viewport-height of rows above and below the visible area.
  Higher values trade memory for smoother scroll feel on fast
  flicks; lower values save memory.
- **`.horizontal(true)`** — flip the scroll axis. Items are laid
  out left-to-right and the viewport scrolls horizontally.
  Default is vertical.

```rust
ui! {
    flat_list(
        data = items,
        key = |_, item| item.id,
        item_size = fixed_size(120.0),
        render_item = |_, item| card_view(item),
    )
    .overscan(2.0)
    .horizontal(true)
}
```

## What each backend does

Same Rust contract, different native widgets:

- **Web** — A JS-side scroll handler (in
  `backend-web/runtime/ts/virtualizer.ts`) owns the
  `IntersectionObserver` and the visible-range diff. It calls
  back into Rust only when items enter or leave the window.
  Per-item scopes drop on exit, freeing signals/effects.
- **iOS** — `UICollectionView` with a flow layout that consults
  `item_size`. Real cell recycling: `prepareForReuse` releases
  an item's subtree; `cellForItemAt` builds the next one when
  scrolling brings it back.
- **Android** — `RecyclerView` plus a `ListAdapter` with
  `DiffUtil`. `onBindViewHolder` runs `render_item`;
  `onViewRecycled` releases the scope.
- **Roku** — Different model. As a generator backend, Roku
  can't ship closures, so the row template is built once at
  snapshot time. The device-side BrightScript runtime
  materializes per-row instances and remaps signal references
  per row. Trade-off: more constrained, no measured sizing.

You write one `flat_list(...)` call. Each backend dispatches it
through its native virtualization machinery without you knowing.

## Reactivity

`data` is a signal, so the list is reactive end-to-end:

```rust
let messages = signal!(load_messages());

// Add an item:
messages.update(|v| v.push(new_message));

// Remove by id:
messages.update(|v| v.retain(|m| m.id != target_id));

// Sort:
messages.update(|v| v.sort_by_key(|m| m.timestamp));
```

The framework reads the current snapshot whenever the
virtualizer queries item count, keys, or sizes. Each backend's
diff algorithm figures out the minimal set of mount, unmount,
and reorder operations to apply.

A few specifics:

- **Insertions** mount the new items if they fall inside the
  current window; otherwise they're just bookkeeping.
- **Removals** unmount the items if they were inside the
  window. Their scopes drop.
- **Reorders** preserve mounted subtrees — items whose key
  stayed move to their new position, but their internal state
  survives.
- **Bulk replacements** (assigning a whole new `Vec`) — same
  diff algorithm. Keys that match preserve state; new keys
  build fresh; old keys tear down.

## Common patterns

### Simple list of strings

```rust
let names = signal!(vec!["Ada".to_string(), "Linus".to_string()]);

ui! {
    flat_list(
        data = names,
        key = |idx, s: &String| {
            // Hashing a String works but recomputes each check.
            // For stable data, pass the source's id instead.
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        },
        item_size = fixed_size(44.0),
        render_item = |_, s| ui! { Text { s.clone() } },
    )
}
```

### List with reactive item content

If your items have reactive state, lift it into a struct that
includes the signal, and have `render_item` set up the bindings:

```rust
#[derive(Clone)]
pub struct TodoItem {
    pub id: u64,
    pub title: String,
    pub done: Signal<bool>,    // Signal is Copy — fine to hold in T
}

ui! {
    flat_list(
        data = todos,
        key = |_, t| t.id,
        item_size = fixed_size(56.0),
        render_item = |_, t| {
            let done = t.done;
            ui! {
                Pressable(on_click = move || done.update(|v| *v = !*v)) {
                    Text { t.title.clone() }
                    Text { if done.get() { "✔" } else { "" } }
                }
            }
        },
    )
}
```

The signal lives in the parent's scope (where the `Vec<TodoItem>`
was built), so its lifetime is tied to the parent — not to the
item's mount/unmount cycle. Items can scroll in and out without
losing checked state.

### Horizontal list of cards

```rust
ui! {
    flat_list(
        data = featured,
        key = |_, c| c.id,
        item_size = fixed_size(280.0),    // width here, since horizontal
        render_item = |_, c| card_view(c),
    )
    .horizontal(true)
}
```

## Pitfalls

- **Duplicate keys.** Two items returning the same key get
  conflated. The visible symptom is rows appearing to "lose"
  state on a reorder. Pick a key from a unique field.
- **Index-as-key.** `key = |idx, _| idx as u64` is tempting but
  defeats the point — any insertion or reorder shifts every
  index, so the framework tears down every mounted item and
  rebuilds. Use a stable id from the data instead.
- **Stale closure captures in `render_item`.** `render_item` runs
  per mount — every time an item enters the window. If you
  capture a `Vec` snapshot outside the closure and refer to it
  inside, you'll see the snapshot's value, not the current
  signal value. Read signals inside the closure.
- **Mixing `Known` and `Measured`.** Not directly supported —
  pick one strategy per list. If most items are predictable but
  some need measuring, use `Measured` for the whole list (cost
  is per-item, not per-list-mode).
- **Items with their own scrollable content.** `flat_list`
  expects to control the scroll axis. Putting a `ScrollView`
  inside an item works, but cross-axis scrolling (horizontal
  list of items each with a vertical `ScrollView` inside) is
  the only sane combination — same-axis nesting is a UX
  anti-pattern most platforms will fight you on.

## Where to read more

- [Primitives](#) — the `Virtualizer` primitive entry and the
  rest of the primitive list.
- [Reactivity](#) — per-item scopes and the cleanup model.
- [Backends](#) — what each backend does to implement
  virtualization.
- [Hot reload](#) — what happens to mounted items when the
  source code changes (spoiler: identity-keyed nodes survive).
