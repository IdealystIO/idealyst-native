# `offload-macro`

The proc-macro that backs [`offload`](../offload)'s `#[offload::job]` attribute
on **native** targets. You almost certainly don't depend on this crate directly —
use `offload`, which re-exports the right `#[job]` for the current target.

On web, `offload` re-exports `wasmworker`'s real `#[webworker_fn]`, which
generates the worker-side dispatch entry so a job can be invoked by name inside a
Web Worker. On native there is no worker — the job runs on a `std::thread` and is
called through an ordinary function pointer — so the attribute has nothing to
generate.

## What you get

- `#[offload_macro::job]` — a no-op passthrough that emits the annotated function
  verbatim.

This crate exists only because attribute macros must live in a `proc-macro`
crate. Keeping the attribute present (rather than asking callers to `#[cfg]` it
away) means a job is annotated **once** and the call site is identical on every
platform.

See [`offload`](../offload) for the full API and usage.

## Testing checklist

A `proc-macro` crate whose only export is a no-op passthrough — there is no
runtime behavior and no native backend, so verification is purely
compile/expansion, exercised through `offload`.

**Automated**
- [ ] `cargo build -p offload-macro` — the proc-macro crate compiles
- [ ] `cargo test -p offload` — the downstream crate that actually invokes
  `#[offload_macro::job]` builds and its native job runs (this crate has no
  tests of its own; its correctness is that the annotated fn is emitted
  verbatim, observed via the `offload` native path)

**Behavior**

Pure compile-time, no native backend. The only observable property is that
`#[offload::job]` on a native target leaves the annotated function unchanged so
the call site is identical to the web build — confirmed by `offload`'s native
tests above.
