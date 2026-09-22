# dev-hot

Thin facade over [`subsecond`](https://docs.rs/subsecond), the hot-reload
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
- **Platform-agnostic.** Built for the runtime-server dylib host today, but the same
  wrappers + jump-table protocol apply when the user's reactive runtime
  lives inside a native dev build (Android, iOS, eventually). The transport
  for delivering patches differs (in-process dlopen vs. WebSocket-shipped
  dylib that the device dlopens locally); `apply_patch` is the same call
  regardless.

## Two modes

- **Off (default)**: `call(f, args)` is `f(args)`. `apply_patch` is a no-op.
  `HotFnPanic` is a unit type that nothing ever constructs.
- **On (`hot` feature)**: `call(f, args)` wraps the inner function in
  `subsecond::HotFn::current(...).call(args)`, going through the global jump
  table. `apply_patch` installs a new jump table. A `HotFnPanic` from a
  stale call site unwinds up to the nearest `catch_unwind` boundary in
  `runtime_core::render`.

## How `#[component]` uses it

Under `runtime-core/hot-reload`, the `#[component]` macro
(`runtime-macros/src/hot_split.rs`) rewrites:

```rust
pub fn Counter(props: &CounterProps) -> Element { /* body */ }
```

into:

```rust
#[doc(hidden)]
#[inline(never)]
fn __Counter_hot_impl(props: &CounterProps) -> Element { /* body */ }

pub fn Counter(props: &CounterProps) -> Element {
    let __idealyst_hot_inner: fn(&CounterProps) -> Element = __Counter_hot_impl;
    ::runtime_core::__hot::call(__idealyst_hot_inner, (props,))
}
```

Three details are load-bearing:

- **The outer keeps name, visibility and signature.** A component's SHAPE —
  props struct, `Tag = TagProps` alias, `BuildElement` impl, every call site
  — is identical with the feature on or off. `runtime-macros`' frozen
  `goldens/component_*.txt` prove the feature-off emission byte for byte.
- **`::runtime_core::__hot`, not `::dev_hot`.** The macro's retarget pass
  rewrites that to `::runtime_vocabulary::glue::__hot`, so a component in any
  crate resolves it without that crate depending on this one.
- **The explicit `fn(..)` local.** A bare fn item is a zero-sized type;
  subsecond would then dispatch through `<F as HotFunction>::call_it` (the
  trait-object path) instead of through the function's own address. The jump
  table is built by pairing `__*_hot_impl` SYMBOLS, so dispatch has to take
  the fn-pointer path. `#[inline(never)]` on the inner half is what keeps
  that symbol in the linked artifact at all (`[profile.dev] opt-level = "z"`
  would otherwise be free to fold it away) —
  `newcore-app`'s `tests/hot_split.rs` reads the binary's symbol table to
  prove it.

A component whose signature cannot be spelled as a plain fn pointer —
generics, arity above 9, `impl Trait`, a destructuring parameter — is emitted
unsplit and falls back to rebuild-and-respawn. See `hot_split::Refusal`.

Without the feature, no wrapper is generated; `Counter` is emitted unchanged.

## Where the patches come from

`idealyst dev` (runtime-server mode) is the live consumer. The host process
watches the user's source tree; on a save whose shape is unchanged it replays
the captured rustc invocation with `--emit=obj`, synthesizes a stub object
that resolves every reference back into the running sidecar's text, links a
patch dylib, and pairs `__*_hot_impl` symbols between the sidecar binary and
the dylib into a `JumpTable`
(`crates/tools/build/runtime-server/src/hotpatch/`). That table crosses the
host↔sidecar pipe as `SidecarIn::ApplyPatch`, and the sidecar calls
[`apply_patch`] and re-runs each mounted session.

Hot-reload is wire-protocol-orthogonal: the `wire::Command` stream and the
hot-patch jump table travel over different layers, and a browser attached to
a runtime-server session sees only ordinary wire commands — no page reload,
no reconnect.
