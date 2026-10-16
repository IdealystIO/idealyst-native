# Proposal: interaction events on `view`

The framework's interaction surface used to be `button` (a single "tap
activates a verb" callback), the `HOVERED` / `PRESSED` / `FOCUSED` /
`DISABLED` state bits that drive styling, and the controlled inputs
(`text_input`, `toggle`, `slider`). There was no path for interactive content
that isn't one of those — draggable cards, swipeable rows, long-press menus,
hover previews, drawing surfaces — short of platform-specific code in a host
crate.

This proposal opened that gap: **raw interaction events on `view`**, with
named gestures built in Rust on top and no platform gesture recognizers in
the framework.

> **Status: implemented, in the shape [`native-touch-plan.md`](../native-touch-plan.md)
> specified rather than this proposal's.** This proposal sketched an
> `on_pointer(|PointerEvent|)` builder with `Down`/`Move`/`Up`/`Cancel`
> phases, a separate `HoverEvent` stream, a new `Element::View` field per
> listener, and new `Backend` trait methods. What shipped:
>
> - **`view(...).on_touch(handler)`** — the raw touch stream
>   (`runtime_shared::touch::TouchEvent`, stable `TouchId`s,
>   deepest-view-first dispatch that bubbles to ancestors until one
>   consumes, and a `CLAIMED` outcome that preempts ancestor scrollers).
>   Types in `crates/runtime/shared/src/touch/`; the backend capability is
>   `caps::InputOps::install_touch_handler` / `claim_touch`.
> - **`.on_hover(|entered: bool|)`** and **`.on_wheel(handler)`** on the same
>   builder (`crates/runtime/vocabulary/src/builders/view.rs`), backed by
>   `InputOps::install_hover_handler` / `install_wheel_handler`.
> - **Gesture recognizers** (`Tap`, `LongPress`, `Pan`, `Pinch`, `Swipe`,
>   `Rotate`) as Rust state machines over the touch stream
>   (`crates/runtime/shared/src/touch/recognizers.rs`), arbitrated by the
>   `gesture` SDK's `GestureGroup` — see
>   [`gesture-arbiter-plan.md`](../gesture-arbiter-plan.md). The `pan`,
>   `zoom`, and `dnd` SDKs build on the same stream.
> - **App-wide mouse input** (buttons, movement, wheel anywhere, pointer
>   lock) is a separate layer: `on_pointer` / `add_pointer_listener` in the
>   author API, `crates/runtime/shared/src/pointer_input.rs`, documented in
>   [`app-input.md`](../app-input.md).
>
> The rationale below is what carried over.

---

## Why builder-on-`view` rather than a separate primitive

Considered and rejected: a separate `Touchable` primitive that wraps
content.

1. **Every interactive container is structurally a view anyway.** A
   `Touchable` would create exactly the same native container as `view`,
   just with listeners attached. Two primitives for "view with listeners"
   doubles the capability surface for no semantic gain.
2. **No coercion at the boundary.** Tacking `.on_touch(…)` onto a view fits
   naturally; with a separate primitive the author has to remember "wrap in
   `Touchable` to make this interactive."
3. **It matches the state-bit precedent.** Hover / press styling already
   attaches listeners to whichever view the author styled — no wrapper.
4. **`None` is cheap.** A view without listeners pays nothing in the
   backend; the handler skips the install call.

An author who wants a clearly-marked interactive container writes a
one-line `#[component]` that wraps `view(...).on_touch(…)` — it lives in
user code and costs zero framework surface.

## Why gestures live above the framework

The framework delivers the raw stream; gestures are Rust code on top of it.
New gestures land in the recognizer set or an SDK without touching any
backend, and every backend recognizes them identically because the
recognition runs in shared Rust rather than in `UIGestureRecognizer` /
`GestureDetector` / DOM event heuristics.
