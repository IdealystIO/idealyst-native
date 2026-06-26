# Bug: inventory self-registration is empty at runtime on `dev --web --local` (wasm-split)

**Severity:** high — every app that uses a navigator or an `Element::External`
extension fails to boot on the local web dev path.

**Status:** open. Root-caused; not yet fixed. A per-app workaround exists
(see below).

## Symptom

Running any navigator-using app with:

```
idealyst dev --web --local
```

mounts a blank page. The browser console shows:

```
panicked at crates/backend/web/src/lib.rs:2597:17:
WebBackend::create_navigator: navigator kind 'stack_navigator::StackPresentation'
is not registered. Did the app forget to call
`<navigator-sdk>::register(&mut backend)` during bootstrap?
```

The panic aborts the mount, so `#app` stays empty. (Reproduced with
`examples/conformance`, which mounts a `stack_navigator::Navigator`.)

## Root cause

Navigator and external SDKs self-register with the web backend via `inventory`:

- An SDK's web module `inventory::submit!`s a `WebNavigatorRegistrar`
  (carrying a `fn(&mut WebBackend)`) — e.g.
  `crates/sdk/navigators/stack/src/web.rs:172`.
- `WebBackend::new` → `drain_self_registrars()`
  (`crates/backend/web/src/lib.rs:981`) iterates
  `inventory::iter::<WebNavigatorRegistrar>` and calls each registrar to
  populate `navigator_handlers`.

On the `dev --web --local` build, **`inventory::iter::<WebNavigatorRegistrar>`
yields nothing at runtime**, so `navigator_handlers` is empty and
`create_navigator` panics.

This is *not* the usual inventory-link-gap or dead-data-pruning case:

- **The SDK is linked.** `strings` on the base wasm
  (`target/idealyst/conformance/web/wrapper/pkg/conformance_bg.wasm`) shows 19
  `stack_navigator` / `StackPresentation` symbols; they are in the **base**
  bundle, not a lazy chunk.
- **Dead-data pruning is off in dev.** `prune_dead_data_min: None` on the dev
  path (`crates/dev/reload/src/lib.rs:338`, `crates/tools/cli/src/cmd/dev.rs:1259`);
  only `--release` prunes (`crates/tools/cli/src/cmd/build.rs:308`).

So the registration is lost through the wasm post-processing the dev path runs
on every build — `neutralize_command_export_wrappers` + `run_wasm_split`
(`crates/tools/build/web/src/lib.rs:160-162`). The `inventory` linked-list
node (a `#[used]` static populated by a `__wasm_call_ctors` constructor) is
apparently not surviving / not being run through that transform, even though
the registrar *code* is present.

This contradicts the earlier finding (memory `project_inventory_self_registration`,
2026-06-03) that inventory survives wasm DCE — that was verified on
`idealyst build --web --release` (wasm-opt `-Oz`), which is a **different
pipeline** from the wasm-split dev path. wasm-split is the differentiator.

## Why it matters

It affects **every navigator/external app on `dev --web --local`**, not just
conformance. `--local` is the documented fallback when the runtime-server build
isn't reachable (memory `feedback_use_local_dev_mode`), so this silently breaks
the primary local web workflow for any app with chrome.

## Reproduction

1. `cd examples/conformance`
2. `idealyst dev --web --local --port 8090`
3. Open `http://127.0.0.1:8090/` — blank page; console shows the
   `create_navigator … is not registered` panic.

A native headless repro (full Rust backtrace) is also possible by mounting
`conformance::app()` on `mock_backend::WireHarness` — but note the mock/native
backend path does **not** reproduce the inventory loss (it only happens through
the wasm-split transform), so the native mount succeeds. The bug is specific to
the wasm-split output.

## Workaround (per app)

Register the navigator explicitly in the app's `register_extensions` on wasm,
instead of relying on inventory:

```rust
#[cfg(target_arch = "wasm32")]
pub fn register_extensions(backend: &mut backend_web::WebBackend) {
    stack_navigator::register(backend);
}
#[cfg(not(target_arch = "wasm32"))]
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}
```

Verified to boot the app fully (conformance renders and its e2e suite runs).
This is a workaround, not the fix — it defeats the point of inventory
self-registration and every app needs the same line until the root cause is
fixed.

## Suggested fix direction

Investigate why the `inventory` registrar's `__wasm_call_ctors` constructor (or
its linked-list node static) does not take effect in the
`neutralize_command_export_wrappers` / `run_wasm_split` output on the dev path.
Candidates:

- The neutralize pass strips the ctor call that links the registrar node into
  inventory's global list (it deliberately *spares* `main` / `host_reserve`,
  `crates/tools/wasm-split/wasm-split-cli/src/lib.rs:79-83`, but the SDK
  registrar ctors may be getting gutted along with the command-export wrappers).
- wasm-split relocates the registrar's initializer into a chunk that isn't
  executed at startup.

A fix needs a wasm regression test (mount a navigator app through the
wasm-split output and assert `create_navigator` resolves) so this can't silently
come back — the failure mode is a clean "not registered" panic that looks like
an app bug, which is exactly how this one hid.

## Related

- `docs/accessibility.md` author-surface work was verified on web *after*
  applying the workaround above.
- Memory: `project_inventory_self_registration`,
  `project_wasm_split_release_dupsymbols`, `feedback_use_local_dev_mode`.
