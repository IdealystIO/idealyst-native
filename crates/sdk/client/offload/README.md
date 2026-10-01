# `offload`

Run a CPU-heavy function **off the main thread** with one platform-agnostic
call site. You annotate a plain free function once with `#[offload::job]`, then
call it the same way everywhere with `offload::run(offload::handle!(f), &arg)`.
On web the job runs in a real Web Worker; on native it runs on a `std::thread`.
Neither path blocks the UI thread.

The job's argument and return types must be `serde::Serialize + Deserialize` —
the web backend ships them across the worker boundary, and native clones the
argument onto the worker thread.

## What you get

- `#[offload::job]` — marker attribute applied to the heavy function. It
  generates nothing on any target (provided by the companion `offload-macro`
  crate); the handle carries the function pointer.
- `offload::handle!(f)` — builds a typed handle to the job, identical at every
  call site.
- `offload::run(handle, &arg).await` — dispatches the job and resolves with
  `Result<R, OffloadError>`.
- `offload::Handle<T, R>` — the handle type (`Copy`, value-equal).
- `OffloadError` — single error type; `OffloadError::Canceled` when the job did
  not produce a result: it panicked (both platforms), its Web Worker trapped or
  failed to load, or its argument/result failed to (de)serialize. On web the
  cause is logged to the console.

## Usage

```rust
use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize)]
struct Req { width: u32, height: u32 }

#[derive(Serialize, Deserialize)]
struct Out { pixels: Vec<u8> }

// Define the heavy job ONCE — a free fn whose arg + return are serde types.
#[offload::job]
fn rasterize(req: Req) -> Out {
    // ... expensive, pure CPU work ...
    Out { pixels: vec![0; (req.width * req.height) as usize] }
}

// Call it the SAME way on every platform.
async fn go(req: Req) -> Result<Out, offload::OffloadError> {
    offload::run(offload::handle!(rasterize), &req).await
}
```

## Per-platform behavior

- **Web (`wasm32`):** the job runs in a Web Worker that instantiates the **same
  app wasm** (`web_glue::worker` — the framework's own glue, no wasm-bindgen):
  the job crosses as a function-table index (every instance of one module has
  the same table), its argument and result as postcard bytes. Workers start on
  demand, up to `navigator.hardwareConcurrency`, each running one job at a time;
  further jobs queue in submission order. No `SharedArrayBuffer`, so **no
  COOP/COEP cross-origin-isolation headers are required** and embedding keeps
  working. There is no second build artifact: the worker re-imports the page's
  own `pkg/` JS. A worker that panicked or trapped is terminated and replaced.
  Two cases where a job's function has no twin in the worker, both of which
  resolve to `Canceled` rather than misbehave: a job only reachable from a
  lazily-loaded (wasm-split) chunk, and — in a `dev` session — a job a hot patch
  added (an edited body of an existing job keeps running the base build's body
  in workers until a reload). Needs a bundle built by an idealyst CLI with
  worker support (the `pkg/` JS reports its URL); an older bundle logs that and
  cancels the job.
- **Native:** the job runs on a freshly spawned `std::thread`; the result is
  delivered back through a oneshot channel the `.await` polls. (A thread pool is
  a future optimization; one thread per call.)

## Dependencies

None beyond `offload` itself — neither the crate that defines a job nor the
call site needs anything else. (Earlier versions required `wasmworker` and
`wasm-bindgen` as direct `wasm32` dependencies of a job-defining crate; they can
be removed.)

No permissions required.

## Testing checklist

Two divergent implementations behind one call site (web Web Worker / native
`std::thread`), so coverage is split: native logic is unit-tested, web is a
build + runtime check. An unchecked **native** box means the code compiles for
that target but isn't confirmed on real hardware yet.

**Automated**
- [x] `cargo test -p offload` — native path: a job runs off the main thread and
  resolves through the oneshot channel; a panicking job resolves to `Canceled`;
  handle equality (6 unit tests)
- [x] `cargo test -p offload --target wasm32-unknown-unknown` — web path in
  headless Chrome (`tests/web_worker.rs`): a job runs in a Worker and returns a
  serde result; a 4 MB payload round-trips; a panicking job and a trapping job
  each resolve to `Canceled` and the pool keeps serving; concurrent jobs run in
  parallel workers and results reach the right callers

**Behavior**
- [x] **Web** — an `#[offload::job]` dispatched via `offload::run(...)` from an
  app built by `idealyst build --web` executes in a real Web Worker and the
  awaited result updates the UI (`examples/offload-demo`, checked in headless
  Chrome). No COOP/COEP headers required.
- [ ] **iOS** — job runs on a spawned `std::thread`; result resolves the
  `.await` without blocking the run loop. ⚠️ not yet device-confirmed.
- [ ] **Android** — job runs on a `std::thread`; result resolves the `.await`.
  ⚠️ not yet device-confirmed.
- [ ] **macOS** — job runs on a `std::thread`; result resolves the `.await`
  without freezing the UI. ⚠️ not yet device-confirmed.
