# backend-web

Web backend: drives DOM nodes via `web-sys` / `wasm-bindgen`. The reference
backend, with the most complete primitive coverage. Every framework test and
example targets it first.

## Bootstrap: every web host must do this

Before constructing a `WebBackend` or calling `runtime_core::mount(...)`,
the host **must** call:

```rust
backend_web::install_scheduler();
backend_web::install_time_source();
```

- **`install_scheduler`** wires `runtime_core::scheduling::after_ms` /
  `schedule_microtask` / `request_animation_frame` to `setTimeout` /
  `queueMicrotask` / `requestAnimationFrame`. Without it, timer-driven
  features (presence animations, anything that calls `after_ms`) fire
  synchronously or never fire at all.
- **`install_time_source`** wires `runtime_core::time::now_micros` to
  `performance.now()`. Without it, `now_micros()` returns 0 on wasm32, which
  means every `PhaseTimer` records duration 0; counts are real but timings
  are useless.

Both are documented as project-memory entries (`project_web_bootstrap_scheduler`)
because the failure mode (animations and `debug-stats` silently no-oping)
is non-obvious.

## File layout

- **`style.rs`**: CSS converters (`rules_to_css` + per-enum helpers),
  stylesheet rule-index bookkeeping (`insert_rule` / `delete_rule` on
  `WebBackend`), and the register/apply `Backend` methods that live next to
  the data they mutate.
- **`defaults.rs`**: global baselines, including the `.ui-default` class,
  spinner keyframes, virtualizer JS shim, and dynamic-slot teardown.
- **`primitives/`**: one module per `Element` kind. Each owns its
  create/update functions, any `Ops` impl, and the `make_*_handle` builder.
  The `impl Backend for WebBackend` block at the bottom of `lib.rs` is a
  thin delegation layer.
- **`batch_queue.rs`** + **`animated.rs`**: the JS-side dispatcher pattern
  for reactive bindings. A single FFI call ships a batch of property writes
  rather than one call per property; the JS dispatcher reads capability
  flags to choose the fastest available update path.
- **`dev_transport.rs`** (feature `aas-shell`): `web_sys::WebSocket` + rAF
  outbound pump for hot-reload / runtime-server over the wire protocol on web.

## Style architecture

Two distinct caches:

1. **Pre-generated cache.** Holds classes minted via `register_stylesheet`,
   keyed by variant combinations × theme. Content-keyed and shared across
   nodes. Lifecycle is anchored by the framework's `register_stylesheet` /
   `unregister_stylesheet` calls.
2. **Dynamic slots, one per styled node.** When a node's resolved style
   doesn't match any pre-generated class, the backend mints a per-node class
   for it. Each styled node owns at most one dynamic class. When the
   resolved style changes:
   1. Mint the new class (insert a CSS rule).
   2. Swap the node's `className`.
   3. Remove the old class's CSS rule.

Dynamic classes are not shared across nodes; two nodes with the same
dynamic style get separate classes. The cost (slight CSS duplication) is
intentional: it eliminates content-keyed cache contention for per-instance
values and keeps dynamic-class lifecycle simple (one class per node,
replaced atomically).

## Animation

`AnimatedValue::bind` silently no-ops on the web backend unless the host
also calls:

```rust
backend_web::install_global_self(&backend);
```

after `WebBackend::new`. See `project_web_install_global_self_for_animation`
in memory.

The animated-property write path goes through `WgpuViewOps`/`WgpuTextOps`-style
overrides on the web `Ops` types. The trait defaults are silent no-ops, and
a backend that skips the overrides drops every `AV.bind` write. Same hazard
as the wgpu backend (see `project_wgpu_viewops_animated`).
