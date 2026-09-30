# Proposal: the framework owns its web JS boundary

Idealyst's web backend reaches the browser through wasm-bindgen, web-sys
and js-sys. This proposal replaces them, inside the framework, with a
small binding layer the framework owns (`web-glue`) and a build pass that
turns it into the page's JS without post-processing the whole module.
Apps may keep using wasm-bindgen themselves; the framework stops relying
on it.

> **Status: phase 2 done — backend-web runs entirely on web-glue
> (2a: foundation; 2b: the DOM-operation surface, `Host::Node`, the dev
> tooling). The page is still hybrid because the SDKs are (phase 3).** Phase 1 (the proof of concept) is
> `crates/runtime/web-glue`, the passes `wasm_carve::{glue, glue_js,
> command_exports}` and `build_web::own_glue`, and the demos under
> `tests/own-glue/`, driven in headless Chrome by
> `crates/tools/build/web/tests/own_glue_e2e.rs` —
> [Phase 1 results](#phase-1-results). Phase 2a moved backend-web's
> scheduler, executor, time source, logger, panic hook, every event
> listener and its eight JS shims onto web-glue, and made the hybrid pass
> part of every `idealyst build --web` / `dev --web` —
> [Phase 2a results](#phase-2a-results).

---

## Decision

The framework owns the wasm ⇄ JS boundary. Every binding the framework
needs is declared in Rust with its JS written next to it; the JS travels
inside the linked wasm; one streaming build pass extracts it and writes
`pkg/<lib>.js` with the same entry contract wasm-bindgen's `--target web`
output has today. wasm-bindgen becomes something an app may bring
(hybrid mode), not something every build pays for.

## Why

**The boundary is ours and finite.** The framework calls a few hundred
distinct browser APIs, from 35 crates (~930 `web_sys::` sites, 274
`js_sys::`, 220 `wasm_bindgen::`). There is one hand-written
`#[wasm_bindgen] extern` block in the whole tree
(`backend/web/src/premint_guard.rs`) and two dev exports. The framework
already ships its own JS for the hot paths: the eight shims in
`crates/backend/web/runtime/js/` (1,645 lines: batching, node ids, text
and class bindings, the virtualizers), `include_str!`'d, evaluated with
`Function::new_no_args` and cached as `js_sys::Function` globals. This
proposal generalizes that pattern and removes the layer under it.

**wasm-bindgen's CLI costs time proportional to the whole program.** It
parses the module into walrus IR, walks every function body in its
externref / duplicate-import / exception-detection / GC passes, re-emits
the module and frees it. On CrewForge's hot-reload base — 376 k
functions, all rooted in the table for hot patching — that is **6–10 s
and 4.7 GB on every rebuild**, although 94% of those bodies never touch
JS. The own-bindings pass does not look at a function body: on the same
304 MB module it takes **0.45 s and 0.92 GB**, almost all of it file I/O
(see [Phase 1 results](#phase-1-results)).

**A lot of pipeline exists only to accommodate wasm-bindgen.** The hot
patch tier carries shim trampolines (`__idealyst_shim_*`), cast
forwarders for `wbg_cast`, stranded `__wbindgen_placeholder__` imports
(`wasm_carve::strand`), descriptor exclusions (`is_bindgen_internal`),
the `command_export` neutralize pass, JSTag support in wasm-carve, and the
rooting of every function before wasm-bindgen because its dead-code pass
ignores what the linker kept. The CLI and the crate must also match
exactly — an app linked against 0.2.126 is refused by a 0.2.128 CLI
(observed during phase 1 on CrewForge's modules, after the CLI had
already reached 2.3 GB of RSS parsing the 68 MB input).

**Found during phase 1, corrected in 2a: which export calls re-run static
constructors.** A wasm32 *bin* is linked by LLD as a command module, and
LLD wraps an export in a call to `__wasm_call_ctors` — re-running every
static constructor on each call — unless something unwraps it. Phase 1
saw this on the hybrid demo (a constructor-bumped counter read 15 after
boot and a few clicks) and counted 5,086 wrapped exports in CrewForge's
hot-reload base, and concluded every JS → Rust call in today's apps paid
a sweep. **Measured in a real app in 2a, that is wrong:** on
`examples/nav-showcase` built by the pre-port pipeline (master
`64306778`), a probe constructor ran **once at boot and zero more times
over 40 clicks** and the frames and navigation they caused. Only a
bare-named export (`#[no_mangle]`, `#[wasm_bindgen]` fns the page calls
directly) re-runs them — the probe's own reader did, once per read. The
5,086 were counted on the module *before* wasm-bindgen, whose output a
normal UI never calls through a wrapped export. So today's apps do not pay
a per-event sweep; `inventory` 0.3.24's idempotent `submit` covers the
remaining bare-export case. What IS affected is web-glue: its exports
(`__glue_invoke`, `__glue_alloc`, `__glue_microtask`, …) are bare-named
and on every event path, so the hybrid pass unwraps exactly those
(`unwrap_command_exports_where`, [Hybrid mode](#hybrid-mode)) and a
backend-web browser test pins it. The own-bindings loader links the
module as a reactor and runs constructors once (the E2E asserts it).

## What the framework needs from a binding layer

From the current code:

- **Long-lived JS object handles** in Rust structs (DOM nodes, option
  objects, cached functions), cloned and dropped freely.
- **Strings** both ways, including non-ASCII, on hot paths (text updates,
  attributes, class names).
- **Closures handed to JS**: event listeners stored in structs and
  detached before drop, a self re-arming `requestAnimationFrame`,
  fire-and-forget `once_into_js`.
- **Async**: a global executor (`wasm_bindgen_futures::spawn_local`
  today), Promises as futures (`JsFuture`), the scheduler's microtasks
  (`Promise.resolve().then`).
- **Typed arrays**: `Uint32Array` batches for the batch shims, `Uint8Array`
  copies.
- **Exceptions** as `Result<_, JsValue>`.
- **Reflection**: `Reflect` get/set of option objects.
- **A cross-crate native handle**: SDKs receive `Rc<dyn Any>` and downcast
  to `web_sys::Node` / `HtmlVideoElement` / … (svg, video, maps, form,
  webview SDKs).

## Mechanism

### Declaring a binding

A binding is a Rust function with its JS inline:

```rust
web_glue::import! {
    fn set_text(el: u32, p: usize, l: usize) =
        "(e, p, l) => { G.get(e).textContent = G.str(p, l); }";

    #[catch] // a JS throw becomes Err(JsError)
    fn parse(p: usize, l: usize) -> u32 = "(p, l) => G.add(JSON.parse(G.str(p, l)))";
}
```

Parameters and returns are raw wasm scalars. `G` is the JS runtime
(`crates/runtime/web-glue/js/runtime.js`), in scope in every snippet.
Typed, safe wrappers are written by hand on top — the `JsValue` methods,
and in phase 2 the backend's DOM layer. There is no generated web-sys:
the framework writes the few hundred operations it uses, each as the one
JS expression that does the job. That is also how the existing shims
already work.

### How the JS gets from the crate to the page

Two carriers, both inside the linked wasm:

1. **The import name carries the snippet.** Each `import!` item is a wasm
   import from module `./__idealyst_glue.js` whose name is
   `<key>\n<flags>\n<js>`. The key is `module_path::fn(types) -> ret`,
   unique per declaration so two same-JS, different-signature imports
   cannot be merged by LLD. The snippet is in the module exactly when
   the import is: LLD drops an unused import and its JS with it, so dead
   bindings cost nothing.
2. **A custom section carries whole JS modules.** The runtime and any
   crate's `js_module!` travel as self-delimiting records in the
   `__idealyst_glue` custom section, which LLD concatenates across
   crates in link order (arbitrary; the records are order-independent).

The obvious design — only the custom section — does not work, and phase 1
measured why. **LLD keeps a custom section only from object files it
loads, and it loads an rlib member only when a symbol in it is
referenced.** A dependency's `#[link_section]` static in a module nothing
calls was missing from both the debug and the release link, while one
nested inside a called function was present in both. So a module record
is nested in an `#[inline(never)]` anchor function (`js_module!`
generates it) whose code must run on every path that uses the module;
the runtime's record is nested in the exported `__glue_alloc`, which is
always linked. A second finding: `#[used]` on such a static makes rustc
emit the bytes **twice**, once in the custom section and once in the data
section (the first PoC build shipped the runtime twice), so the records
are deliberately not `#[used]`.

The build tool knows the record framing, the import-name grammar and three
runtime entry points (`attach`, `lazyAttach`, `module`) — never the heap
layout or the runtime's internals. The runtime is versioned with the
crate, which ends the CLI/crate lock-step. A framing change bumps a
version byte the pass checks.

### The runtime ABI

- **Handles.** A JS value is a `u32` index into a JS-side slab. The Rust
  type `JsValue` owns one slot: drop releases it, clone takes a second
  slot for the same value. Slot 0 is `undefined`, permanently, so
  undefined never allocates or crosses on drop. A released slot holds a
  sentinel; using or releasing it again throws naming the handle.
  `JsValue::live_count()` / `js_live_count()` expose both sides for
  tests.
- **Strings.** Rust → JS is a borrow: `(ptr, len)`, decoded with
  `TextDecoder` from a subarray view. JS → Rust uses a Rust-owned buffer:
  JS encodes, calls the exported `__glue_alloc(len)`, copies, and writes
  `[ptr, len]` into an out-slot; Rust adopts it as a `String`. That is one
  crossing plus a re-entry, where length-then-copy needs two crossings and
  a second encode or a JS-side stash (wasm-bindgen's `__wbindgen_malloc`
  is the same shape). **Invariant:** the alloc may grow memory, which
  detaches every JS view of the old buffer, so no view is ever held across
  a call into wasm; `G.u8()`/`G.u32()` re-create a detached view. The E2E
  forces growth inside the alloc and fails with `TypeError: Cannot perform
  %TypedArray%.prototype.set on a detached or out-of-bounds ArrayBuffer`
  if the runtime takes its view before the alloc — verified against a
  deliberately broken runtime.
- **Callbacks.** `Closure::new(FnMut)` / `Closure::once(FnOnce)` register
  the closure under a never-reused id and mint a JS function that calls
  the one exported entry point, `__glue_invoke(id, arg)`. Dropping the
  `Closure` unregisters it and marks the JS function dead; a later JS call
  throws `callback #N called after its Rust owner dropped it`, never a
  call into freed memory. The closure is taken out of the registry while
  it runs, so it may drop itself or other closures; a recursive call of
  the same closure is refused with an error (as wasm-bindgen does).
  `once_into_js` is the ownerless one-shot.
- **Futures.** `spawn_local` + `queue_microtask` run on a single-threaded
  executor drained by one export, `__glue_microtask`, scheduled with one
  `queueMicrotask` per burst. `JsFuture` wraps `then(ok, err)` with
  one-shot *silent* reactions, so dropping a future before its promise
  settles is a no-op, not an error in the page.
- **Exceptions.** A `#[catch]` snippet is wrapped in `G.catching`, which
  parks a throw in a two-word error slot in Rust memory (`[flag,
  handle]`) and returns 0. The generated wrapper checks the flag after the
  call: the success path is one crossing plus a load. The flag is separate
  from the handle because `throw undefined` is legal.

### The build pass and the entry contract

`build_web::own_glue::package_own` runs `wasm_carve::glue::extract` on
the linked module: it re-encodes the import section (glue imports renamed
`g0`, `g1`, …), drops the glue custom section, and copies every other
section — the code section included — as bytes. It then writes:

- `pkg/<lib>_bg.wasm`;
- `pkg/<lib>.js`: the runtime, the modules, each snippet as a literal
  function expression (no `eval`, so no CSP `unsafe-eval`), and the entry
  contract the rest of the build already relies on — `export default
  init(input?)` (URL / Request / Response / bytes / Module /
  `{ module_or_path }`), named `initSync(module?)`, and idempotent re-init
  that returns the raw exports (the `__wasm_split.js` loaders call
  `initSync(undefined, undefined)`). Import modules other than glue and
  `env` (e.g. `./__wasm_split.js`) are passed through as static ES
  imports, as wasm-bindgen does.

The entry is `G.attach(exports)`, then `__wasm_call_ctors()` once, then
`main(0, 0)`. That requires linking the bin as a **reactor**:
`-C link-arg=--export=__wasm_call_ctors` (`own_glue::link_args()`).
Without it LLD builds a command module and wraps every export in a ctor
call (see [Why](#why)). The pass refuses a module that was linked without
it.

## Hybrid mode

An app that uses wasm-bindgen (directly, or through wgpu) links both. The
glue namespace composes with wasm-bindgen instead of replacing it:

1. `glue::extract` renames and strips as above.
2. `command_exports::unwrap_command_exports_where` repoints web-glue's
   own exports (`__glue_*`) past LLD's ctor wrapper, so a listener
   dispatch does not re-run every static constructor. Every other export
   keeps its behaviour. Hybrid cannot link a reactor instead:
   wasm-bindgen's `__wbindgen_start` only calls `main(0, 0)`, so a reactor
   would never run constructors. (The phase-1 PoC's `package_hybrid`
   unwraps every export but `main`; the build does not.)
3. wasm-bindgen runs over the result as today. It passes the unknown
   import module through as `import * as … from "./__idealyst_glue.js"`.
4. The pass writes `pkg/__idealyst_glue.js`, which exports `g0…` and
   attaches `G` lazily through `initSync(undefined)` — the same trick the
   split loaders use to reach the instance. The import is circular
   (`<lib>.js` ⇄ glue); that is safe because `initSync` is a hoisted
   function declaration and is only called at glue-call time. The
   specifier must be the one the page itself imports wasm-bindgen's JS
   by — ES modules are keyed by URL, and a different spelling loads a
   second, uninitialized copy (found under wasm-bindgen-test-runner,
   which imports `./wasm-bindgen-test` without an extension). It also
   publishes `globalThis.__idealystGlue`: the HYBRID-BRIDGE
   (`web_glue::bridge`) and the hot-patch loader's `compileImport` /
   `registerRecords`.

In 2a this runs in every web build (`own_glue::hybrid_extract` /
`write_hybrid_glue_file`), in `idealyst export`, and — through the
workspace's wasm32 test runner (`scripts/wasm-glue-test-runner.sh`) — for
every wasm-bindgen browser test of a crate that links web-glue.

The same precedent already exists: `wasm-split-macro` emits
`#[link(wasm_import_module = "./__wasm_split.js")]` imports that
wasm-bindgen passes through.

## Pipeline: what goes, what stays

Once backend-web and the SDKs are off wasm-bindgen (phases 2–4), a
framework-only app builds with **cargo → `glue::extract` → (wasm-split)
→ (wasm-opt)**. What that removes from the default path:

- the wasm-bindgen CLI invocation and its flag matrix
  (`--keep-lld-exports`, `--no-demangle`);
- rooting every function in the table before wasm-bindgen so its
  dead-code pass keeps what a patch may call (hot-patch base prep keeps
  whatever rooting the patch tier itself needs);
- `__idealyst_shim_*` trampolines and `wbg_cast` forwarders;
- stranded `__wbindgen_placeholder__` imports (`wasm_carve::strand`) and
  the descriptor exclusions (`is_bindgen_internal`);
- the `command_export` neutralize pass (reactor linkage replaces it);
- JSTag handling in wasm-carve;
- the CLI/crate version lock-step, and the `wasm-bindgen` install
  requirement for building a framework-only app.

What stays, for hybrid only: the wasm-bindgen invocation,
`unwrap_command_exports`, and whichever of the above the app's own
wasm-bindgen use still triggers. The hot-patch and split machinery must
learn glue imports (see risks).

## Risks and mitigations

- **A throw in a non-`#[catch]` import unwinds through Rust frames.** The
  exception passes through wasm frames without running destructors, so a
  `RefCell` borrowed at the time stays borrowed. This is the same as a
  non-`catch` wasm-bindgen import today. Mitigation: `#[catch]` on
  anything that can throw, and phase 2 review of every snippet with that
  question.
- **Hot patching adds glue imports the base never saw.** Done in 2a: the
  patch plan's `Glue` tag makes the page compile each glue import from
  the JS in its name (`new Function`, dev only), and the patch's glue
  records are registered first — a record that CHANGES a module the page
  already runs fails the apply, i.e. reloads. Nothing is pre-declared.
- **Split chunks import glue too.** A chunk whose code uses a binding main
  does not use imports it from `./__idealyst_glue.js`; the split loader
  must supply the namespace to chunk instantiation. Phase 6.
- **Handle aliasing through raw indices.** Slots are reused, so a raw
  index kept past its release can alias a newer object. The Rust API
  makes that unrepresentable through RAII (`JsValue`); `from_raw` is
  `unsafe`. A debug-build generation tag on the slab is cheap if it is
  ever needed.
- **Single-threaded executor.** Wakers hold `Rc`s; like
  `wasm_bindgen_futures` without atomics. The Worker-based offload SDK
  gets its own bootstrap in phase 4.
- **Code size.** The generic `JsValue` reflect surface and the runtime
  cost a little: after `wasm-opt -Oz` and brotli the demo is 313 bytes
  (1.4%) larger than its web-sys twin. Measured, small, and it has not
  been optimized (the callback registry uses a SipHash `HashMap`, for
  instance).
- **Duplicate web-glue versions in one graph** would share one slab under
  two runtimes. The pass refuses two different runtime records at build
  time.
- **Debuggability.** Glue functions appear as `g<N>` in JS stack traces;
  the generated file comments each with its Rust key. Naming the
  functions after their key is an easy follow-up if it matters.

## Phased plan

1. **Proof of concept (done).** `web-glue`, the passes, the own and
   hybrid packaging, the demos, the E2E, the measurements.
   [Results below.](#phase-1-results)
2. **backend-web onto web-glue**, with a **typed handle family**
   (`web_glue::dom::{Node, Element, HtmlElement, HtmlInputElement, …}`,
   `instanceof`-checked casts) to replace web-sys types as the
   cross-crate native handle.
   - **2a (done):** the typed family; scheduler, render loop, executor,
     time source, logger and panic hook; every event listener
     (`TrackedListener` semantics kept); the eight shims as
     `js_module!`s (no run-time eval); boot; the hybrid pass in every
     build; hot-patch glue imports; the wasm32 test runner.
   - **2b (done):** every remaining DOM operation (creation, attribute /
     style / class / text writes, tree edits, measurement, CSSOM, history,
     ResizeObserver); the shim call sites and the virtualizer /
     virtual-grid callbacks (glue closures that take every argument and
     return a value); `Host::Node` = `web_glue::dom::Node`; the dev-only
     WebSocket transports, robot relay + DOM screenshot, overlay entry and
     hot-patch loader. backend-web names web-sys / wasm-bindgen ONLY in
     `src/bridge.rs` (HYBRID-BRIDGE): an un-ported SDK hands its web-sys
     element to the host (`node_from_web_sys`) and recovers one from the
     host node it receives as `&dyn Any` (`node_to_web_sys`); a dropped
     file still reaches the file-picker SDK as a `web_sys::File`. The
     SDK side of those seams (svg, video, maps, form, webview,
     canvas-native) changed only at the seam.
   - An own-mode switch in `BuildOptions` comes when a framework-only app
     links no wasm-bindgen at all (after phases 3–4).
3. **SDKs** (the 35 crates' remaining web-sys/js-sys use), each onto
   `import!` and the handle type.
4. **Third-party replacements**: fetch (gloo-net), IndexedDB (idb), the
   Worker bootstrap (wasmworker); drop plotters' web backend and
   web-time. (`console_error_panic_hook` went in 2a:
   `backend_web::install_panic_hook`.)
5. **Hybrid mode as a supported configuration** for wgpu (the GPU host's
   WebGPU canvas) and for apps that use wasm-bindgen themselves.
6. **Pipeline cleanup**: remove the wasm-bindgen-only machinery listed
   above from the default path; teach wasm-split chunks the glue
   namespace.

## Phase 1 results

Machine: Apple Silicon Mac, Rust 1.97.1, wasm-bindgen 0.2.128, headless
Chrome. Demos: `tests/own-glue/demo` (web-glue only, built
`--no-default-features` to leave out its self-checks) against
`tests/own-glue/websys-demo` (the same page and workload on web-sys).
Both release. Reproduce with
`cargo test -p build-web --test own_glue_e2e -- --ignored --nocapture --test-threads=1 measure`.

**Build time and size (demo):**

| | own glue | web-sys + wasm-bindgen |
|---|---:|---:|
| cargo build, cold target dir (deps included) | 0.92 s | 12.30 s |
| cargo build, after touching `main.rs` (3 runs) | 0.59–0.84 s | 1.01–1.10 s |
| post-cargo pass (glue pass vs wasm-bindgen CLI) | 0.5–1.5 ms | 13.7–31.0 ms |
| `_bg.wasm` | 54,294 B | 52,960 B |
| wasm + js | 67,617 B | 72,743 B |
| wasm + js, brotli q11 | 24,477 B | 24,215 B |
| after `wasm-opt -Oz`, brotli | 23,342 B | 23,029 B |

**Call overhead (demo, medians of 9 runs after a warm-up, 3 page loads
each):** creating 10,000 elements with 3 attributes and text each:
14.4 / 16.8 / 15.2 ms (own) vs 15.9 / 16.5 / 16.0 ms (web-sys). Updating
10,000 texts: 3.3 / 3.9 / 3.3 ms vs 3.3 / 3.5 / 3.3 ms. Parity within run
to run noise; headless Chrome's `performance.now()` is coarsened to
0.1 ms.

**Scaling on real modules (the pass alone, `wasm-carve/examples/glue_pass.rs`,
3 runs):**

| module | glue pass wall / peak RSS | wasm-bindgen |
|---|---|---|
| CrewForge main hot-reload base, 304.7 MB | 0.45–0.83 s / 0.92 GB (extract 38 ms, unwrap 50 ms; the rest is reading and writing the file) | 6–10 s / 4.7 GB (from the earlier CrewForge measurements; not re-run — the module is linked against 0.2.126 and the installed CLI refuses it) |
| CrewForge checkin hot-reload base, 67.9 MB | 0.04–0.05 s / 0.21 GB | refused (schema 0.2.126), after reaching 2.3 GB RSS |

**Behaviour, all asserted in headless Chrome by the E2E:** DOM creation,
attributes and text; a click listener mutating Rust state; an awaited
timer Promise and a rejected one; a caught `RangeError`; non-ASCII
strings both ways (up to ~450 KB); a JS → Rust string whose alloc grows
memory, three times in a row; 2,000 handles created and released with
both sides of the slab back to baseline; a listener detached, then its
`Closure` dropped, then the stale JS function called (throws, never
reaches Rust); a crate's `js_module!` reached through `G.m`; the reflect
surface and its error path; `initSync()`/`init()` after boot returning
the raw exports; the benchmark's 10,000 retained row handles all released; constructors run
exactly once. The crate graph and the output contain no wasm-bindgen.
Hybrid: both namespaces in one instance, glue callbacks and strings
through wasm-bindgen's instance, a `#[wasm_bindgen]` export callable from
the page, constructors run once (15 before the unwrap pass).

**Recommendation: GO for phase 2.** The mechanism works end to end
without wasm-bindgen, composes with it, costs nothing measurable per call,
and its build pass scales with file size rather than with the number of
function bodies. The size delta is small and unoptimized. The two open
design items that must be settled at the start of phase 2 are hot-patch
glue imports and the handle type.

## Phase 2a results

Machine and toolchain as phase 1; Chrome 154 with a matching
chromedriver.

**backend-web's browser suite: 101/101** (its wasm-bindgen tests, left on
wasm-bindgen-test — porting the runner is not 2a's scope), run through
`scripts/wasm-glue-test-runner.sh`, i.e. the whole test binary in hybrid
mode. Plus the new
`regression_glue_dispatch_does_not_rerun_static_constructors` (see
below).

**A real app, before and after** — `examples/nav-showcase`,
`idealyst build --web` (dev profile, split on), master `64306778`'s CLI vs
this branch's:

| | before (wasm-bindgen only) | after (hybrid) |
|---|---:|---:|
| cold build total | 33.02 s (cargo 32.69) | 32.91 s (cargo 32.54) |
| warm rebuild (touch `lib.rs`) | 1.29 s (cargo 0.99) | 1.05 s (cargo 0.67) |
| wasm-bindgen | 0.13–0.16 s | 0.14–0.15 s |
| glue-extract (new) | — | 0.04–0.06 s |
| constructor runs: boot / 40 clicks | 1 / 0 | 1 / 0 |

No regression: hybrid mode does not remove wasm-bindgen's cost yet (it
still runs), and the extraction adds tens of milliseconds. In Chrome the
after build boots, navigates, and logs no errors (a favicon 404 aside);
`pkg/__idealyst_glue.<hash>.js` is fingerprinted with the rest of the
bundle.

**Constructor count (decision 3's verification).** Before the port, 1
run at boot and 0 over 40 clicks — see [Why](#why) for what that
corrected. After the port the same, but only because the hybrid pass
unwraps web-glue's own exports: with them wrapped, every glue listener
dispatch re-runs every constructor, which the regression test above
shows (it fails with the runner's `IDEALYST_GLUE_KEEP_WRAPPERS=1`).

**Design changes from the phase-2 brief:**

- `Host::Node` stays `web_sys::Node` through 2a (see the plan above).
- A GC-owned closure (`web_glue::Closure::into_js_value`, via a
  `FinalizationRegistry` and a new `__glue_release` export): an
  element-lifetime listener hands its closure to the element instead of a
  backend `Vec`. That retired three never-cleared closure pools and two
  `.forget()`s.
- A re-entrant closure (`Closure::new_fn`): `scroll` handlers and the
  portal focus trap are re-entered by design, and a `FnMut` glue closure
  refuses re-entry the way wasm-bindgen's does.
- Test runner: wasm-bindgen-test-runner serves its output from a fresh
  tempdir but falls back to serving its working directory, so a thin
  wrapper supplies `__idealyst_glue.js` there. `wasm-pack test` bypasses
  it; `cargo test --target wasm32-unknown-unknown` is the supported way.

## Phase 2b results

**How the port was done.** `web_glue::dom` and `web_glue::js` bind
exactly what backend-web calls, with **web-sys's and js-sys's names and
signatures** (`Node::append_child -> Result<Node, _>`,
`Window::document -> Option<Document>`, `Reflect::get`,
`Uint32Array::from(&[u32])`, …), errors as `JsError` — which derefs to
its `JsValue`, as a wasm-bindgen error IS one. The port of ~19k lines was
then a path rewrite plus the compiler's list of missing members (541
errors → 0), and the phase-3 SDK ports get the same shape. Hot
operations (create / append / insert / remove, attributes, text, inline
style, classList, measurement) each have their own snippet; the long tail
of property reads and writes goes through generic accessors.

**web-glue additions:** `js` (Object, Array, Function, Reflect, Promise,
typed arrays, Set, Map, Date); the DOM classes above plus CSSOM, History,
Location, Navigator, ResizeObserver, WebSocket, XMLSerializer, XHR, the
2D canvas context, and event-init dictionaries; `Closure::new_with_args`
(every argument in, a value out — the return rides in `__glue_invoke`'s
status, `3 + handle`, so it costs no second call); `null` is a permanent
slot (1) next to `undefined` (0), so `JsValue::NULL` / `UNDEFINED` are
constants; every class derefs down to `JsValue`.

**Public API changes to web-glue (within this milestone):** to match
web-sys, `Window::document` returns `Option<Document>`,
`Window::inner_width/height` return `Result<JsValue, _>`,
`Window::match_media` returns `Result<Option<_>, _>`, and
`Node::contains` / `is_same_node` take `Option<&Node>`. The `Closure`
invoke protocol changed (runtime and Rust together; invisible to users).

**Found on the way:** backend-web used to enable ~60 web-sys features
for every crate in the graph. With it down to `Node` + `File` (bridge
only), crates that used web-sys types without declaring the features
stopped building alone (the stack navigator's hydration test used
`web_sys::window()` with only `History` enabled); each now declares its
own. The runtime-server transport attached a new window `resize`
listener (and leaked its closure) on every reconnect; it is now one
listener, replaced per connect.

### Phase 2b verification

- backend-web's browser suite **103/103** (all features), through the
  wasm32 runner; web-glue native tests; the codeblock soft-wrap and
  stack-navigator SSR-hydration browser tests.
- Every workspace crate that uses web-sys, checked **on its own** for
  wasm32 (so feature unification cannot hide a missing declaration).
- `dev_events_e2e` 1/1 and `wasm_hot_patch_e2e` 2/2 — including the body
  edit that adds a web-glue binding the base never had; `own_glue_e2e`
  3/3.
- Real apps in headless Chrome, built by `idealyst build --web`:
  `examples/nav-showcase` (tabs, a stack push, browser back via the
  `popstate` listener, URL sync) and `examples/whiteboard-demo`
  (canvas-native mounted through the HYBRID-BRIDGE seam, sized by the
  glue ResizeObserver, and a pointer-drawn stroke painted through the
  glue touch listeners). No console errors beyond a missing favicon.
- Build-time spot check, `nav-showcase`, warm rebuild after touching
  `lib.rs`: **0.99–1.00 s** (2a: 1.05 s; master before phase 2: 1.29 s);
  the glue extraction is 0.06–0.08 s of it. Hybrid mode still runs
  wasm-bindgen, so its cost is unchanged until phase 3.

**What remains in backend-web:** `src/bridge.rs` (and its `web-sys` —
`Node`, `File` — and `wasm-bindgen` dependencies) for the SDKs that still
use web-sys; the file-drop handoff of a `web_sys::File` to the
file-picker SDK; `wasm-bindgen-test` as the browser-test harness
(dev-dependency). Phase 3 ports those SDKs and deletes the bridge.

## Open questions

- **Unwrapping the remaining bare-named exports** (an app's own
  `#[wasm_bindgen]` / `#[no_mangle]` fns the page calls): measured in 2a,
  a normal UI never calls one, so this is now a small question rather
  than a per-event cost. Left as today.
- **Typed arrays:** the batch shims hand `Uint32Array`s built from Rust
  slices. With own bindings that is a `G.u32().subarray(...)` view read
  inside the snippet — no copy at all — but it must never be retained
  past the call (memory growth). Adopt that as the rule for the batch
  path in phase 2?
