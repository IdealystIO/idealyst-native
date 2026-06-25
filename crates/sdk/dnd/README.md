# `dnd`

In-app drag and drop — reorderable lists, kanban boards, drag-into-trash,
sortable grids — that behaves identically on every backend with no
per-platform code. The framework converges pointer input below this crate
(web Pointer Events fold mouse + touch + pen into one stream; the native
backends deliver touches through the same `runtime_core::TouchEvent`, each
carrying a `window_position`), so a drag is just a gesture and drop targets
are hit-tested in window space. One pure-Rust crate, no platform crates.

Three small handles compose it. Clone the `DragContext` into every
participant; sources and targets that share a context can interact.

## What you get

- `DragContext<T>` — the shared registry every draggable and droppable in a
  scope reads, holding the live drag session and the set of drop targets.
  `T` is your payload type. Clone is cheap (one `Rc`).
  - `DragContext::new()` / `Default`
  - `dragging() -> Signal<bool>` — reactive "a drag is in flight" flag, for
    dimming non-targets or revealing a trash zone while a drag is happening.
  - `payload() -> Option<T>` — the in-flight payload, cloned out.
- `Draggable<T>` — a drag source carrying a typed payload. Its offset follows
  the finger; on release the payload is delivered to the target under the
  pointer, or the element springs back.
  - `Draggable::new(&ctx, || payload)` — payload closure is sampled fresh at
    each drag start.
  - `.activation(Activation)` — when the drag commits (default
    `Activation::platform_default`).
  - `.snap_back(bool)` — whether a missed drop springs back (default `true`).
  - `.on_start(|| …)` / `.on_release(|DropOutcome| …)` lifecycle hooks.
  - `.offset() -> (AnimatedValue<f32>, AnimatedValue<f32>)` for a custom
    transform, or `.bind(view_ref)` to wire `x → TranslateX` / `y → TranslateY`.
  - `.handler() -> TouchHandler` for the view's `on_touch` slot, or
    `.recognizer() -> DragRecognizer` to compose in a `gesture::GestureGroup`.
- `Droppable<T>` — a drop target with reactive hover state.
  - `Droppable::new(&ctx)` (accepts every payload by default).
  - `.accepts(|&T| bool)` to filter, `.on_enter` / `.on_leave` / `.on_drop`
    callbacks.
  - `.is_over() -> Signal<bool>` — reactive "an accepted payload hovers me".
  - `.bind(view_ref)` registers the view's window-space rect as the drop zone
    and deregisters on scope cleanup.
- `DropOutcome` — `Landed` / `Missed` / `Cancelled`, delivered to
  `Draggable::on_release`.
- `Activation` — `Immediate { slop_px }` or `LongPress { threshold_ms, slop_px }`,
  with `Activation::immediate()`, `Activation::long_press()`, and
  `Activation::platform_default()` (long-press on phones/tablets, immediate
  elsewhere — reads `runtime_core::platform()`).
- `DragRecognizer` — the underlying gesture FSM, exposed for arbitration; plus
  `DragPhase`, `DragSample`, and the `DEFAULT_DRAG_*` / `SNAP_BACK_*` constants.

## Usage

```rust
use dnd::{Activation, DragContext, Draggable, Droppable};
use runtime_core::{signal, text, view, Element, Ref, Signal, ViewHandle};

#[derive(Clone, Copy)]
struct ChipData { label: &'static str }

fn board() -> Element {
    // One context for the whole board; payload is the chip.
    let ctx: DragContext<ChipData> = DragContext::new();
    let bin_slot: Signal<Option<ChipData>> = signal!(None);

    view(vec![
        chip(&ctx, ChipData { label: "Coral" }),
        bin(&ctx, bin_slot),
    ])
    .into()
}

fn chip(ctx: &DragContext<ChipData>, data: ChipData) -> Element {
    let chip_ref: Ref<ViewHandle> = Ref::new();
    let drag = Draggable::new(ctx, move || data)
        .activation(Activation::platform_default())
        .on_release(|outcome| { /* Landed / Missed / Cancelled */ });
    drag.bind(chip_ref);                 // x -> TranslateX, y -> TranslateY
    let handler = drag.handler();

    view(vec![text(data.label).into()])
        .on_touch(move |ev| handler(ev))
        .bind(chip_ref)
        .into()
}

fn bin(ctx: &DragContext<ChipData>, slot: Signal<Option<ChipData>>) -> Element {
    let bin_ref: Ref<ViewHandle> = Ref::new();
    let drop = Droppable::new(ctx).on_drop(move |c| slot.set(Some(c)));
    let over = drop.is_over();           // reactive: highlight the bin while hovered
    drop.bind(bin_ref);

    view(vec![/* styled on `over.get()` */])
        .bind(bin_ref)
        .into()
}
```

Call `drag.bind(...)` / `drop.bind(...)` during render inside the active
reactive scope — the bindings anchor to that scope and clean up when it drops,
exactly like any `AnimatedValue::bind`. On the web backend the app must have
called `backend_web::install_global_self(&backend)` at startup for the animated
ghost offset to take effect (a standard app-bootstrap step, not specific to
this SDK).

See `examples/dnd-demo` for a working screen.

## What it deliberately leaves to you

Auto-scrolling a list while dragging near its edge, reorder animations, and
multi-select drag are **policy** — build them on the lifecycle hooks and the
reactive `DragContext::dragging` / `Droppable::is_over` signals, the same way
`pan` leaves momentum to the caller.

## Native per-platform drag (the seam)

This crate ships the **universal in-app engine**, which is all an in-app drag
needs and works on every backend with no platform code. *Cross-application*
drag (drag a file out to Finder, accept a drop from another app) and the
browser's native HTML5 drag/`DataTransfer` are a **separate, additive
capability that is not implemented here** — see the `native` module docs.

They are a separate phase because they require new `Backend` trait methods on
every backend (begin a native drag session, register a native drop target,
read/write the platform pasteboard) — surface this crate, which depends only
on `runtime-core`, can't reach — and because their output is by design *not*
identical across platforms (each renders through its own OS drag chrome).

| Platform | Native system |
|----------|---------------|
| Web      | HTML5 `dragstart`/`drop` + `DataTransfer` |
| iOS/iPadOS | `UIDragInteraction` / `UIDropInteraction` + `NSItemProvider` |
| Android  | `View.startDragAndDrop` + `ClipData` / `OnDragListener` |
| macOS    | `NSDraggingSource` / `NSDraggingDestination` + `NSPasteboard` |

The author-facing API here is deliberately strategy-agnostic so a native
driver can slot in behind it later (the documented future `DragStrategy` seam)
without changing call sites.

## Permissions

None.
