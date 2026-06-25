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

- `#[offload::job]` — attribute applied to the heavy function. On web it expands
  to `wasmworker`'s `#[webworker_fn]` (registers the fn for the worker); on
  native it's a no-op passthrough (provided by the companion `offload-macro`
  crate).
- `offload::handle!(f)` — builds a typed handle to the job, identical at every
  call site.
- `offload::run(handle, &arg).await` — dispatches the job and resolves with
  `Result<R, OffloadError>`.
- `OffloadError` — single error type; `OffloadError::Canceled` when the
  worker/thread drops the job before returning a result (e.g. it panicked or the
  pool was torn down).

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

- **Web (`wasm32`):** the job runs in a `wasmworker` Web Worker pool, sized to
  `navigator.hardwareConcurrency`. No `SharedArrayBuffer`, so **no COOP/COEP
  cross-origin-isolation headers are required** and embedding keeps working. The
  same `--target web` wasm artifact is reused — there is no second build.
- **Native:** the job runs on a freshly spawned `std::thread`; the result is
  delivered back through a oneshot channel the `.await` polls. (A thread pool is
  a future optimization; one thread per call.)

## Consumer dependency note (web)

A crate that **defines** a job must add `wasmworker` and `wasm-bindgen` as direct
`wasm32` dependencies, because the expansion of `#[offload::job]` on web
references them by name:

```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
wasmworker = { version = "0.4", features = ["macros"] }
wasm-bindgen = "0.2"
```

The call site (`offload::run(offload::handle!(f), &req)`) only names `offload`;
only the `#[job]` expansion needs those two crates in scope.

No permissions required.
