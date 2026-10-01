# `offload-macro`

The proc-macro that backs [`offload`](../offload)'s `#[offload::job]` attribute.
You almost certainly don't depend on this crate directly — use `offload`, which
re-exports it.

The attribute is a marker that generates nothing, on every target. A job is
dispatched through the function pointer `offload::handle!` captures: natively
it is called on a `std::thread`; on web its function-table index is sent to a
Web Worker running the same module, where it names the same function.

## What you get

- `#[offload_macro::job]` — a no-op passthrough that emits the annotated function
  verbatim.

This crate exists only because attribute macros must live in a `proc-macro`
crate.

See [`offload`](../offload) for the full API and usage.

## Testing checklist

A `proc-macro` crate whose only export is a no-op passthrough — there is no
runtime behavior and no native backend, so verification is purely
compile/expansion, exercised through `offload`.

**Automated**
- [ ] `cargo build -p offload-macro` — the proc-macro crate compiles
- [x] `cargo test -p offload` (native) and `cargo test -p offload --target
  wasm32-unknown-unknown` (browser) — the downstream crate whose tests annotate
  jobs with `#[offload::job]` builds and runs them (this crate has no tests of
  its own; its correctness is that the annotated fn is emitted verbatim)

**Behavior**

Pure compile-time. The only observable property is that `#[offload::job]`
leaves the annotated function unchanged on every target — confirmed by
`offload`'s tests above.
