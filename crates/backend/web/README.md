# backend-web

Web backend: drives DOM nodes. The reference backend, with the most complete
primitive coverage. Every framework test and example targets it first.

## Its JS boundary: web-glue

Since own-web-bindings phase 2b
([`docs/proposals/own-web-bindings.md`](../../../docs/proposals/own-web-bindings.md))
every DOM operation, listener, timer, shim call and dev-tooling entry
point in this crate goes through `web-glue`: `web_glue::dom` classes with
web-sys's method names and signatures, `web_glue::js` for the JS
built-ins. `Host::Node` is `web_glue::dom::Node`. The eight `runtime/js/`
shims ship as `web_glue::js_module!`s (no run-time eval).

Nothing in this crate names web-sys or wasm-bindgen (`wasm-bindgen-test`
stays as the browser-test harness, a dev-dependency): a dropped file reaches
the file-picker SDK as a `web_glue::dom::File` (the type its
`picked_from_dropped` downcasts `DroppedFile.source` to), and SDKs return /
downcast `web_glue::dom::Node` as the host node — the generic node bridge used
during the port (`backend_web::bridge::{node_from_web_sys, node_to_web_sys}`)
and the dropped-file `HYBRID-BRIDGE` crossing are gone (own-web-bindings
phase 3).

A web page is still **hybrid** while any crate in it uses wasm-bindgen:
`idealyst build --web` extracts the glue before wasm-bindgen runs and writes
`pkg/__idealyst_glue.js` after (`build_web::own_glue`).

A crate that uses web-sys types must enable the web-sys features it uses
itself: this crate no longer enables ~60 of them for everyone, and code
that compiled only because of that unification fails to build alone.

Listener ownership, which the port made uniform:

- a listener a node's teardown must detach → `WebBackend::track_listener`
  (a `web_glue::dom::Listener`: detaches BEFORE its closure drops);
- a `window` / `document` listener → a `Listener` held by its owner;
- a listener that lives exactly as long as its element →
  `glue_dom::listen_for_element_lifetime` (the element owns it; the Rust
  closure is released when JS collects the element). Never a backend-held
  `Vec` of closures — that pinned them for the life of the page.
- a handler that may be re-entered (`scroll` re-fired by its own layout
  write, a focus trap's own `.focus()`) → the `_fn` variants
  (`web_glue::Closure::new_fn`); `FnMut` refuses re-entry loudly.

**Running the browser tests:** `cargo test -p backend-web --target
wasm32-unknown-unknown` with `CHROMEDRIVER` set. The workspace's wasm32
runner (`scripts/wasm-glue-test-runner.sh`, `.cargo/config.toml`) supplies
`__idealyst_glue.js` to the test page; `wasm-pack test` bypasses it and the
tests cannot load.

## Bootstrap: every web host must do this

Before constructing a `WebBackend` or booting a tree, the host **must**
call:

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
is non-obvious. `newcore::start_in(...)` — the standard web boot entry —
performs both installs itself (they are idempotent); a host that wires
the backend up by hand must call them.

## File layout

- **`style.rs`**: CSS converters (`rules_to_css` + per-enum helpers),
  stylesheet rule-index bookkeeping (`insert_rule` / `delete_rule` on
  `WebBackend`), and the register/apply inherent methods that live next to
  the data they mutate (the `caps::StyleOps` impl delegates to them).
- **`defaults.rs`**: global baselines, including the `.ui-default` class,
  spinner keyframes, the JS shims (as web-glue modules), and dynamic-slot
  teardown.
- **`glue_dom.rs`**: listener helpers, element-lifetime callbacks, the shim callback shapes.
- **`primitives/`**: one module per primitive. Each owns its
  create/update functions, any `Ops` impl, and the `make_*_handle` builder.
- **`newcore.rs`**: the `impl Host for WebBackend` block plus all ~30
  `impl caps::*Ops for WebBackend` blocks. A thin delegation layer over
  the inherent `*_impl` methods and the `primitives/` modules.
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
