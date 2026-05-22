# framework-hot

Thin facade over [`subsecond`](https://docs.rs/subsecond) — the hot-reload
substrate. The framework calls into `hot::call` instead of `subsecond::HotFn`
directly so the rest of the codebase doesn't grow a hard dependency on the
upstream API surface.

## Design goals

- **Zero production cost.** With the `hot` feature off, every public function
  in this crate degrades to a direct call. The `#[component]` macro's
  hot-reload wrapper compiles out entirely. Production binaries pay nothing
  for the dev-only substrate.
- **Easy removal.** Every cross-crate consumer references this crate via the
  `hot-reload` cargo feature and gates integration on
  `#[cfg(feature = "hot-reload")]`. Toggling the feature workspace-wide
  removes every reference; this crate could then be deleted in one PR with
  no other code edits.
- **Platform-agnostic.** Built for the AAS dylib host today, but the same
  wrappers + jump-table protocol apply when the user's reactive runtime lives
  inside a native dev build (Android, iOS, eventually). The transport for
  delivering patches differs (in-process dlopen vs. WebSocket-shipped dylib
  that the device dlopens locally) — `apply_patch` is the same call
  regardless.

## Two modes

- **Off (default)**: `call(f, args)` is `f(args)`. `apply_patch` is a no-op.
  `HotFnPanic` is a unit type that nothing ever constructs.
- **On (`hot` feature)**: `call(f, args)` wraps the inner function in
  `subsecond::HotFn::current(...).call(args)`, going through the global jump
  table. `apply_patch` installs a new jump table. A `HotFnPanic` from a stale
  call site unwinds up to the nearest `catch_unwind` boundary in
  `framework_core::render`.

## How `#[component]` uses it

Under `hot-reload`, the `#[component]` macro rewrites:

```rust
fn Counter(props: &CounterProps) -> Primitive { /* body */ }
```

into:

```rust
fn Counter(props: &CounterProps) -> Primitive {
    ::framework_hot::call(__Counter_hot_impl, (props,))
}
#[doc(hidden)]
fn __Counter_hot_impl(props: &CounterProps) -> Primitive { /* body */ }
```

Without the feature, no wrapper is generated — `Counter` is emitted unchanged.

## Where the patches come from

The dev server bundles a fresh dylib of the user's component crate on each
edit and ships it over the wire. On native targets, [`backend/aas-shell-native`](../../backend/aas-shell-native)
receives the dylib, `dlopen`s it locally, and calls `apply_patch` with the
new jump table. On web, the same flow runs through `backend-web`'s
`dev_transport` module.

Hot-reload is wire-protocol-orthogonal — the `wire::Command` stream and the
hot-patch dylib travel over the same WebSocket but at different layers.
