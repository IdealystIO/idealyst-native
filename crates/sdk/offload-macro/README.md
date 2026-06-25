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
