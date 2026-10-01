# Proposal: the framework owns its web JS boundary

Idealyst's web backend reaches the browser through wasm-bindgen, web-sys
and js-sys. This proposal replaces them, inside the framework, with a
small binding layer the framework owns (`web-glue`) and a build pass that
turns it into the page's JS without post-processing the whole module.
Apps may keep using wasm-bindgen themselves; the framework stops relying
on it.

> **Status: all six phases done. A framework-only app builds in OWN mode
> — no wasm-bindgen anywhere, CLI included — and an app that still links
> wasm-bindgen (wgpu, its own web-sys) builds in the supported HYBRID
> mode; the build decides per linked module
> ([Phases 5 + 6](#phases-5--6--own-mode-by-default-hybrid-supported)).**
> Phase 1 (the proof of concept) is
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
  JS calls the exported `__glue_alloc(s.length)`, writes ASCII straight
  into it (`charCodeAt`), and — from the first non-ASCII unit — grows it to
  the worst case with `__glue_realloc`, `encodeInto`s the rest in place and
  shrinks it to the written length; then it writes `[ptr, len]` into an
  out-slot and Rust adopts the buffer as a `String`. An ASCII string is
  one crossing plus one re-entry and no temporary array (the first version
  `TextEncoder.encode`d into a temporary and copied it in: ~8× slower per
  short string, visible on the theme toggle). Length-then-copy would need
  two crossings and a second encode or a JS-side stash (wasm-bindgen's
  `__wbindgen_malloc` / `__wbindgen_realloc` is the same shape).
  **Invariant:** the alloc and the realloc may grow memory, which
  detaches every JS view of the old buffer, so no view is ever held across
  a call into wasm; `G.u8()`/`G.u32()` re-create a detached view. The E2E
  forces growth inside the alloc and fails with `TypeError: Cannot perform
  %TypedArray%.prototype.set on a detached or out-of-bounds ArrayBuffer`
  if the runtime takes its view before the alloc — verified against a
  deliberately broken runtime.
- **Property access.** Every DOM property read or write is its own
  import with the name in its JS (`(o) => G.add(G.get(o)["cssRules"])`),
  generated by `dom_api.rs`'s `prop_*!` / `set_*!` / `call0!` macros —
  the shape wasm-bindgen emits as `__wbg_cssRules_<hash>`. The first
  version shared one generic import per kind that took the name as
  `(ptr, len)`, so every access `TextDecoder`-decoded its name; that was
  the theme toggle's remaining gap to web-sys (see
  [Runtime performance against web-sys](#runtime-performance-against-web-sys)).
  `JsValue::get` / `set` / `call_method` keep the by-name form for names
  only known at run time.
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

A release build writes either file (`<lib>.js`, or hybrid's
`__idealyst_glue.js`) in `wasm_carve::glue_js::JsLayout::Minified`:
web-glue's runtime and the loader boilerplate go through the same
comment/whitespace minifier backend-web runs over its shims, the
per-import `// key` comments and the indentation are dropped, and import
snippets and `js_module!` sources are emitted verbatim (the minifier
cannot tokenize a regex literal, and those come from any crate). On the
benchmark variant that took the glue file from 13.0 KB to 9.7 KB brotli,
which with the smaller wasm leaves the bundle ~1% under the web-sys build.
Dev builds stay readable. The wasm32 test runner writes the minified shape,
so every browser test runs it.

The entry is `G.attach(exports)`, then `__wasm_call_ctors()` once if the
module exports it, then `main(0, 0)`. Phase 1 linked the bin as a
**reactor** (`-C link-arg=--export=__wasm_call_ctors`,
`own_glue::link_args()`), where that order runs constructors once. The
build (phase 6) instead links every web app as the command module LLD
makes by default — own or hybrid is only known after the link, and hybrid
cannot be a reactor — and the pass points every export but `main` past
LLD's constructor wrapper; `main`'s wrapper is then the one constructor
run. Both are tested (`own_glue_e2e`).

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

Since phase 6 the build takes this path only for a module that carries
wasm-bindgen metadata (`own_glue::extract_for_build`); everything else is
own mode. `idealyst export` (its bridge is `#[wasm_bindgen]` classes) and
— through the workspace's wasm32 test runner
(`scripts/wasm-glue-test-runner.sh`) — every wasm-bindgen browser test of
a crate that links web-glue always take it.

The same precedent already exists: `wasm-split-macro` emits
`#[link(wasm_import_module = "./__wasm_split.js")]` imports that
wasm-bindgen passes through.

## Pipeline: what goes, what stays

Done in phase 6 ([below](#phases-5--6--own-mode-by-default-hybrid-supported)):
a framework-only app builds with **cargo → `glue::extract` → (hot-patch
base prep) → (wasm-split) → (wasm-opt)**. What that removed from the
default path:

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

What stays, for hybrid only: the wasm-bindgen invocation and every item
above except the install requirement for framework-only apps. Own mode
keeps the table rooting (the patch tier needs a slot for every function),
`__idealyst_shim_*` trampolines for `./__wasm_split.js` imports only, and
`unwrap_command_exports` (every export but `main`).

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
- **Split chunks import glue too.** Did not materialize: wasm-carve gives
  a split module no function imports at all — every import, glue ones
  included, stays in main and a chunk reaches it through a table
  trampoline — so a chunk-only binding is an import of main's module and
  its JS is in main's `pkg/<lib>.js`. Pinned in a browser by
  `tests/lazy-chunk-handoff` (phase 6).
- **Handle aliasing through raw indices.** Slots are reused, so a raw
  index kept past its release can alias a newer object. The Rust API
  makes that unrepresentable through RAII (`JsValue`); `from_raw` is
  `unsafe`. A debug-build generation tag on the slab is cheap if it is
  ever needed.
- **Single-threaded executor.** Wakers hold `Rc`s; like
  `wasm_bindgen_futures` without atomics. Workers are separate instances
  sharing no memory (`web_glue::worker`, phase 4), each with its own
  executor.
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
4. **Third-party replacements (done)**: fetch (gloo-net), IndexedDB (idb),
   the Worker bootstrap (wasmworker → `web_glue::worker`); plotters and
   web-time dropped — see [fetch / IndexedDB](#phase-4--fetch--indexeddb)
   and [Workers / charts / web-time](#phase-4--workers--charts--web-time).
   (`console_error_panic_hook` went in 2a: `backend_web::install_panic_hook`.)
5. **Hybrid mode as a supported configuration (done)** for wgpu (the GPU
   host's WebGPU canvas, canvas-vello) and for apps that use wasm-bindgen
   themselves — chosen per module, covered by `hybrid_web_e2e`.
6. **Pipeline cleanup (done)**: own mode by default; the
   wasm-bindgen-only machinery listed above is skipped for it; split
   chunks needed nothing. [Results below.](#phases-5--6--own-mode-by-default-hybrid-supported)

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

## Phase 3 — DOM-mounting SDKs

**svg, video, maps (+ maps-web), form, webview, codeblock (its browser
test) and canvas-native run on web-glue.** Each handler builds its element
with `web_glue::dom`, returns it as the host node, and its ops downcast the
`&dyn Any` host node to `web_glue::dom::Node`; what `web_glue::dom` does not
bind is a crate-local `import!` / `js_class!` (`HTMLFormElement`, media
playback, `postMessage` / `eval`, the whole Canvas2D surface in
`canvas-native/src/web_ctx.rs`). backend-web's node bridge
(`backend_web::bridge`, 2b, never released) is deleted. No public API
changed; maps-web keeps a `#[deprecated]` `build_map_iframe ->
web_sys::Element` next to the new `build_map_element`.

**What still crosses `web_glue::bridge`** (each site marked
`HYBRID-BRIDGE`):

- `native_source` media streams stay `web_sys::MediaStream` until the media
  SDKs switch in one change — video's `srcObject`, canvas-native's texture
  layers and its `captureStream` self-capture;
- backend-web's dropped file stays a `web_sys::File` for file-picker
  (`file_drop.rs::file_for_picker`) — backend-web keeps `wasm-bindgen` and
  web-sys `File` for that site only;
- canvas-native's public `make_2d_rasterizer` / `publish_capture_stream`
  take a `web_sys::HtmlCanvasElement` because canvas-vello (wgpu) calls
  them.

**Bugs found on the way:** form and webview parked their listener closures
behind an `Rc::into_raw` number on the element that nothing reclaimed —
every mount leaked, and webview's `message` listener on `window` outlived
its iframe; both are now `Listener`s dropped at teardown. maps-web pinned
`width/height: 100%` inline, which beat the author's style classes, so a
map could not be sized on web. web-sys binds JS `MediaStream.clone()` as
an inherent `clone()`, so `Rc::new(stream.clone())` publishes a new stream
with cloned tracks (camera / microphone still do this).

**Verification:** each crate's new wasm32 browser suite through the
workspace runner (svg 2, maps 1, form 2, webview 3, video 2, codeblock 1,
canvas-native 3 — pixel read-back), host tests, iOS / Android `cargo
check`; backend-web's browser suite 118/118; all 35 web-sys-using crates
`cargo check` alone for wasm32; `wasm_hot_patch_e2e` 2/2,
`dev_events_e2e` 1/1. Real apps built with `idealyst build --web` in
headless Chrome: whiteboard-demo (a pointer stroke painted through
canvas-native, repainted on resize) and a throwaway app mounting svg,
maps, form, webview, video and codeblock together (reactive markup and
intrinsic size, author-sized map, handle submit and Enter-to-submit,
webview load + `postMessage` from `execute_js`, video mute op, editor
typing) — no console errors besides a deliberately missing clip.

## Phase 3 — media/file switch

**The three crossings the DOM-mounting phase left are gone.** One coherent
change switched every producer and consumer of a web `native_source` stream
to `web_glue::dom::MediaStream` (new, with `MediaStreamTrack`, in web-glue's
`dom`): camera, microphone, screen-recorder, canvas-native's self-capture,
video-compose and media-stream's synthetic-audio bridge publish one; video,
canvas-native's texture layers, canvas-vello, video-compose, media-writer and
media-stream's `screenshot()` downcast one. backend-web hands a dropped file
to file-picker as a `web_glue::dom::File`, so backend-web depends on neither
wasm-bindgen nor web-sys any more (`wasm-bindgen-test` stays as its test
harness). camera, microphone, media-stream, media-writer, screen-recorder,
video-compose and file-picker run entirely on web-glue (crate-local
`import!`s for getUserMedia / getDisplayMedia, the canvas pumps, WebAudio,
MediaRecorder, `requestVideoFrameCallback`, `showOpenFilePicker` and the
Blob reader); `files` stayed on idb until phase 4 ([below](#phase-4--fetch--indexeddb)).

**What still crosses `web_glue::bridge`** is wgpu's, marked
`HYBRID-BRIDGE: wgpu`: canvas-native's public `make_2d_rasterizer` /
`publish_capture_stream` keep their `web_sys::HtmlCanvasElement` signatures
(canvas-vello calls them) and convert at the boundary, and canvas-vello's
texture-layer `<video>` stays a web-sys element for wgpu's
`ExternalImageSource`, so a layer's glue stream crosses out to become its
`srcObject`. No public signature changed.

**Bug fixed on the way:** web-sys binds the JS `MediaStream.clone()` as an
inherent `clone()`, so camera and microphone published
`Rc::new(stream.clone())` — a new stream with cloned tracks — and
screen-recorder kept such a copy for teardown. Stopping a capture ended one
stream while consumers kept showing (or sharing) the other. A glue handle's
`Clone` is the same JS object; each SDK's browser test stops the capture and
asserts the consumer's tracks ended (each fails against a JS `clone()`).

**Test harness:** the wasm32 runner now carries a crate's `webdriver.json`
into the directory it runs wasm-bindgen-test-runner from, so a crate can give
headless Chrome fake media devices (`--use-fake-device-for-media-stream`) or
relax the autoplay policy (camera, microphone, media-stream, media-writer).


**Found by verification — the build still needs wasm-bindgen's runtime.**
With backend-web off wasm-bindgen, an app with no other wasm-bindgen user
(both hot-patch E2E apps; a plain framework app without files / net / wgpu)
linked none of it. Every web build still runs the wasm-bindgen CLI, whose
externref pass (on whenever the module has `reference-types`, rustc's wasm32
default) needs the runtime's `__externref_table_alloc` exports and failed with
"failed to find intrinsics to enable `clone_ref` function" — the dev session
never served. `WebBackend::new_in` now calls
`wasm_bindgen::__rt::link_mem_intrinsics()` (the hook wasm-bindgen's own
generated code uses), so backend-web depends on wasm-bindgen again for the
build only; no wasm-bindgen type crosses its API. Own mode (phase 6) removed
it again — such a module no longer reaches the CLI. Also fixed on the way: backend-web's host tests did not compile on macOS
(a wasm-only `.init_array` probe), and screen-recorder's host tests (a
misplaced dev-dependency, and a stale `Element::External` test).

**Verification:** new wasm32 browser suites through the workspace runner
(headless Chrome 154) — web-glue 3, media-stream 4, camera 2, microphone 2,
screen-recorder 2, video 3, video-compose 1, media-writer 3, canvas-native 3,
file-picker 2; backend-web's browser suite 118/118 (incl.
`regression_dropped_file_source_is_the_glue_file_file_picker_reads`); host
`cargo test` of every touched crate (screen-recorder and
backend-web's lib tests after the fixes above); `cargo check` for wasm32 of
every touched crate and every workspace dependent alone (denoise-demo fails
only on its un-fetched model asset); iOS / Android `cargo check` of the nine
SDKs; `wasm_hot_patch_e2e` 2/2 (0/2 before the runtime fix) and
`dev_events_e2e` 1/1. Real apps built with `idealyst build --web` and driven
in headless Chrome over WebDriver: camera-preview-demo with fake media
devices (Start → the `<video>` plays the 640×480 feed; Stop → the tracks the
preview showed are `ended` and `srcObject` is cleared) and whiteboard-demo (a
pointer stroke painted on the Canvas2D board, still painted after a window
resize shrank the canvas's backing store). No console errors beyond a missing
favicon.

## Phase 4 — fetch / IndexedDB

**net's HTTP arm and files' IndexedDB store run on web-glue; gloo-net,
gloo-utils and idb are out of the graph.** Each is one crate-local
`js_module!` that owns the whole browser exchange and settles one promise,
so the Rust side is a single `JsFuture` and one error match:

- `net/fetch` (`crates/sdk/client/net/src/web.rs`) builds the `Headers`,
  calls `fetch` with an `AbortController` signal, buffers the body and
  resolves `{ status, headers, body }`, or rejects `{ kind, message }` with
  `kind` = `abort` / `timeout` / `network`. A cancel token, an expired
  timeout and a dropped `send` future all abort the browser request.
- `files/idb` (`crates/sdk/client/files/src/web.rs`) opens the database at
  its current version, adds the `blobs` store by a one-version upgrade only
  when it is missing, runs one transaction to completion and closes the
  connection. Connections close on `versionchange`; a blocked upgrade is an
  error, not a hang.

net and files depend on web-glue alone for the browser (wasm-bindgen-test
stays as the browser-test harness). No public API changed, and nothing was
added to web-glue core. None of net's dependents (graphql, auto-update,
i18n, server, server-kit, jobs, sync) used gloo or web-sys for HTTP.

**Bugs fixed on the way** (each with a browser regression test that fails
on the old arm):

- net ignored `timeout` on web, so a request to a server that never
  answered hung forever. It is now `Error::Timeout`, and the deadline
  covers the body read, as with reqwest.
- net sent only the last value of a repeated request header, because
  gloo's `Headers` wrapper only had `set`.
- net threw out of `send` on an invalid header name (gloo's
  `unwrap_throw`). It is now `Error::Network`.
- Dropping net's `send` future left the browser request running.
- files opened at a fixed version 1. A database past version 1 failed with
  `VersionError`, and one without the `blobs` store never got it.
- files read a value that was not bytes as `Ok(Some(empty))`.

**Found here, fixed after:** net's iOS (NSURLSession) and Android
(HttpURLConnection) arms ignored `timeout` too. Both now apply it with the
same meaning (one deadline on the whole exchange, body included,
`Error::Timeout`): NSURLSession through a per-request session's
`timeoutIntervalForResource` (an `NSURLRequest.timeoutInterval` alone is an
idle timeout a trickling body never trips), HttpURLConnection through a
deadline watchdog that disconnects the connection, with
`setConnectTimeout` / `setReadTimeout` as per-phase backstops. Covered by
`net/tests/timeout.rs` (host, and the iOS simulator via `simctl spawn`)
and `net/tests/android-device/run.sh` (an emulator, under `app_process`).

**Verification:** net's browser suites through the workspace runner
(headless Chrome 154): `web_fetch` 16 (real `fetch` against `blob:` /
`data:` URLs, the runner's own server for a 404, and loopback port 1 for a
refused connection; a stand-in `fetch` that normalises through
`new Request` and honours the `AbortSignal` for the sent request and for a
server that never answers), plus `web_socket_glue` 3 and
`web_closure_lifetime` 2. files' browser suite 9 (`src/web/tests.rs`). Host
`cargo test` for both crates; `cargo check` for aarch64-apple-ios and
aarch64-linux-android for both; media-writer's browser suite (it writes
through files); wasm32 `cargo check` of net's dependents.

## Phase 4 — Workers / charts / web-time

**Workers: `web_glue::worker`, and offload on it.** `worker::spawn(entry:
fn())` starts a module Worker that instantiates the same wasm (no shared
memory) and calls `entry` there. A Rust `fn` pointer is a function-table
index, and every instance of one module has the same table, so the index
is the whole message; the engine type-checks `call_indirect`, so a stale
index traps rather than misbehaving. The generated JS that embeds the
runtime (own: `<lib>.js`; hybrid: `__idealyst_glue.js`) reports its
`import.meta.url` to the runtime (`G.entry`) and exports
`__glueWorkerInit(module?)`; the runtime's blob-URL bootstrap imports that
URL, instantiates, and calls the new `__glue_worker_start(entry)` export.
Own mode hands the worker the compiled `WebAssembly.Module`; hybrid mode
gives wasm-bindgen's `init` an explicit `"<stem>_bg.wasm"` URL, which
`fingerprint_pkg` rewrites like any other wasm name. Nothing new is
emitted: a build without offload is unchanged except for those two lines of
JS. The runtime itself already avoided `window`/`document`; the new browser
tests prove it in a `WorkerGlobalScope`.

offload (`crates/sdk/client/offload`) drops `wasmworker` and
`serde-wasm-bindgen`. A job crosses as two table indices — the job fn and
a monomorphized `dispatch::<T, R>` — and postcard bytes (transferred, not
copied twice). Workers start lazily up to `hardwareConcurrency`, one job
each, the rest queued. Public API unchanged (`#[job]`, `handle!`, `run`,
`OffloadError`); `Handle<T, R>` is now the handle type on web too (it was
wasmworker's `WebWorkerFn`, nameable only through a wasmworker dependency),
`handle!` accepts a path on web as it did natively, and `#[job]` is a no-op
marker everywhere, so a job-defining crate no longer needs `wasmworker` /
`wasm-bindgen` dependencies. offload's wasm32 graph has no wasm-bindgen.

**Bug fixed:** a panicking job hung its caller forever under wasmworker
(its worker script awaited the job export with no `catch`, so no response
was ever posted). Now a panic — reported by a worker panic hook before the
trap — or any other trap (the Worker's `error` event) resolves the caller
with `OffloadError::Canceled`, the same variant native returns, logs the
job's name and cause, and replaces the worker. A worker that never starts
cancels the queued jobs instead of respawning in a loop.

**Limits (traps → `Canceled`, documented in offload's README):** a job only
reachable from a wasm-split chunk the worker never loaded, and a job a dev
hot patch added (an edited body of an existing job keeps its index, so
workers run the base build's body until a reload — as under wasmworker).

**web-time:** render-wgpu, ios-sim and android-sim read a crate-local
`render_wgpu::time::Instant` over `runtime_shared::time::now_micros()`
(and `epoch_millis()` for the sim status-bar clock). `Host::new` installs
the platform default source (first install wins) so the clock can never
read a frozen 0; host-web installs backend-web's sources before it. The
`Painter` trait's `now` parameter type changed accordingly.

**Charts:** charts-core took only tick selection from plotters (linear key
points, `LogCoord`, the `DateTime<Utc>` range, the f64 label printer), but
on wasm32 plotters links wasm-bindgen + web-sys unconditionally and its
`datetime` feature turns on chrono's `clock` + `wasmbind`. The new
`charts_core::ticks` (linear / log / time) is a line-for-line port of
plotters 0.3.7's algorithms; chrono stays for UTC calendar math and
strftime labels with `default-features = false, features = ["alloc"]`. Over
2,976 harness inputs (every axis kind, spans from 1 ms to 1,000 years,
budgets 0–20, zero / negative / reversed / tiny / huge / NaN / inf bounds,
plus `scale::resolve` over 416 combinations) every input on which plotters
returned gives byte-identical ticks and labels; the 229 that differ are
inputs where plotters hung or panicked (non-finite linear spans, a log
bound of 0 or ∞, `max_ticks = 0` on a time axis under ~292 years,
a time range reversed by ≥ a week), where the port returns no ticks.
`tests/ticks.rs` replays 1,409 recorded plotters outputs and pins each
divergence. *Later:* the port also stopped reproducing three plotters label
quirks — linear ranges under ~1e-5 labelled `"0.0"` / `"-0.0"`, log axes
repeating a decade tick (`"10", "10"`) and labelling small decades `"0"`,
and time axes labelled by span rather than step (a multi-year axis stepped
in weeks read `"2024", "2024", "2025"`; sub-minute steps read `"13:47"`
repeatedly). Adjacent labels are now always distinct and never `-0`; 454 of
the 1,409 corpus lines were rewritten for it, each category listed in the
corpus header.

**What remains on wasm-bindgen** (normal edges, wasm32): offload, charts,
charts-core, web-glue — none (wasm-bindgen-test is a dev-dependency only).
render-wgpu — wgpu's own (phase 5, hybrid).

**Verification:** browser suites through the workspace runner (headless
Chrome 154): web-glue `web_worker` 3 (+ `web_media` 3), offload
`web_worker` 5 (a job's result from a worker, a 4 MB payload, a panicking
and a trapping job each `Canceled` with the pool still serving, parallel
workers with results routed to the right callers), backend-web 118/118.
`own_glue_e2e` 2/2 (own mode now spawns a worker that reports
`in_worker=true window=false ctors=1`), `wasm_hot_patch_e2e` 2/2,
`dev_events_e2e` 1/1. A real app: `examples/offload-demo` (under
`crates/sdk/client/offload/examples`) built by `idealyst build --web` and
driven in headless Chrome — 148,933 primes below 2·10⁶ counted in a worker
while the page animated, a panicking job answered with `Canceled` and the
console naming it, a later job still served; no console errors besides a
missing favicon. Host tests of offload, web-glue, wasm-carve, build-web
(fingerprint), render-wgpu (46), charts-core / charts; iOS / Android
`cargo check` of offload, offload-macro, web-glue, render-wgpu / ios-sim /
android-sim, charts; wasm32 checks of render-wgpu + host-web.

## Phases 5 + 6 — own mode by default, hybrid supported

**The build decides per linked module.** `build_web::own_glue::extract_for_build`
reads cargo's artifact once (the glue extraction it always ran) and reports
wasm-bindgen use (`wasm_carve::glue::Glue::wasm_bindgen`). Either signal makes
the module HYBRID; neither makes it OWN:

- an import from a `__wbindgen*` module (`__wbindgen_placeholder__`,
  `__wbindgen_externref_xform__`) — the module cannot instantiate without
  wasm-bindgen's generated JS;
- the `__wasm_bindgen_unstable` custom section — a `#[wasm_bindgen]` item
  survived the link. An exported fn takes no import, so the import test
  alone would miss it.

Having wasm-bindgen in the crate graph is not evidence: LLD loads an rlib
member only when it is referenced, so a dependency that names wasm-bindgen
without calling it links neither signal. The decision is logged
(`own mode: … no wasm-bindgen CLI` / `hybrid mode: … (5 __wbindgen* imports
and a … section)`) and the stages differ (`glue-package` vs `wasm-bindgen`),
which is what the E2Es assert.

**Own mode** writes `pkg/<lib>.js` (`loader_js`) and `pkg/<lib>_bg.wasm`
itself, with wasm-bindgen's entry contract (default `init`, `initSync`,
idempotent re-init returning the raw exports), so `index.html`, SSR/SSG,
fingerprinting and the split loaders are unchanged. Every app is linked the
same way (a command module) — the mode is known only after the link, a
reactor would break hybrid's start, and one RUSTFLAGS set keeps the cargo
cache shared — and own mode unwraps every export but `main` past LLD's
constructor wrapper, which is a reactor's behaviour (constructors once, in
`main`'s wrapper; once in a worker too). It strips `linking`, `reloc.*` and
DWARF as wasm-bindgen used to (the splitter and the hot-patch alias map read
cargo's artifact, not this one), and removes a previous hybrid build's glue
file, `.d.ts` files and `snippets/` from `pkg/`. The loader now also publishes
`globalThis.__idealystGlue`, which the hot-patch loader needs in both modes.

**What own mode skips, and what hybrid keeps:**

| | own | hybrid |
|---|---|---|
| wasm-bindgen CLI (`--keep-lld-exports`, `--no-demangle`) | — | yes |
| command-export unwrap | every export but `main` | web-glue's `__glue_*` |
| `command_export` neutralize pass (0.2.122) | — | yes |
| hot-patch table rooting | yes | yes (before wasm-bindgen's GC) |
| `__idealyst_shim_*` trampolines | `./__wasm_split.js` imports only | every JS-shim import |
| `wbg_cast` forwarders, `is_bindgen_internal` exclusions | — (`hotpatch_base::Flavor::Own`) | yes |
| stranded `__wbindgen_placeholder__` imports (`wasm_carve::strand`) | — | yes |
| JSTag support in wasm-carve | unused | yes |

An edit that makes an own-mode app start using wasm-bindgen cannot be hot
patched (its `__wbindgen_*` imports resolve to nothing); it falls back to a
rebuild, which builds hybrid. The patch link keeps `--no-demangle` — that is
wasm-ld's flag, pairing the patch's mangled names with the base's.

**Lazy chunks need nothing.** wasm-carve gives a split module no function
imports — every import, glue ones included, stays in main, and a chunk reaches
it through a table trampoline — so the split loader supplies no glue namespace
and `initSync(undefined, undefined)` answers it in both modes.
`tests/lazy-chunk-handoff` now calls a `web_glue::import!` only its chunk
uses (marker `glue in chunk: 42`).

**Dependencies removed.** backend-web's `wasm-bindgen` dependency and its
`link_mem_intrinsics` hook (3f9131e8, which existed only so the CLI could
process an app that links no wasm-bindgen). canvas-native's `wasm-bindgen` /
`web-sys` are optional behind a new `web-sys-canvas` feature that canvas-vello
enables: its two web-sys-typed entry points (HYBRID-BRIDGE: wgpu) put
wasm-bindgen into every canvas app, so `examples/whiteboard-demo`, which has
no GPU renderer, still built hybrid. `idealyst doctor` lists the wasm-bindgen
CLI as optional (hybrid builds and `idealyst export` only).

**Bugs found and fixed (each with a regression test):**

- **Every GPU canvas on the DOM backend was blank since 2b.** The Graphics
  surface handed out a `WebCanvasWindowHandle` pointing at the canvas's
  WEB-GLUE handle; raw-window-handle defines that pointer as a wasm-bindgen
  `JsValue`, and canvas-vello, host-web and wgpu read it as one — an index
  into a different heap. canvas-vello found no canvas and drew nothing. The
  surface now hands out `WebWindowHandle` (the canvas carries
  `data-raw-handle`, page-unique and non-zero), which wgpu resolves itself;
  canvas-vello and host-web look it up the same way. Limitation, as with
  wgpu's own lookup: a canvas inside a shadow root is not found. Found by
  `hybrid_web_e2e`; pinned by backend-web's
  `regression_graphics_window_handle_names_the_canvas_by_its_raw_handle_id`.
- **`--data-prune` did nothing in own mode.** The splitter read the
  data-symbol table from the packaged module's `linking` section, which
  wasm-bindgen carried through and own mode strips. It now falls back to the
  rustc module's table (the same table — packaging never renumbers symbols
  or moves data). lazy-payload-split's lazy main: 1409 KiB before the fix,
  897 KiB after (eager 1408; the pre-change hybrid pipeline gave 898 / 1410).
  Pinned by wasm-carve's
  `regression_data_prune_works_on_a_packaged_module_without_linking`.

**What still builds hybrid** (and so still needs the CLI at the version in
the app's `Cargo.lock`): anything linking wgpu — canvas-vello (charts-demo
`--features vello`, pdf-demo), the GPU host (`host-web`, the website
simulator); apps calling web-sys / `#[wasm_bindgen]` themselves
(examples/inspector, login-demo, CrewForge); `idealyst export`; the dev smoke
crates; and, likely, any app on the `maps` SDK, whose `maps-web` still exports
the deprecated `build_map_iframe -> web_sys::Element` (no example app uses
maps on web, so this was not measured).

### Phases 5 + 6 verification

Machine and toolchain as phase 1; Chrome 154 with a matching chromedriver.

- Host tests: wasm-carve (incl. the detection, the own loader's
  `__idealystGlue`, the data-prune fallback), build-web 154 (incl. own vs
  hybrid extraction on command modules, stale-file cleanup, the own hot-patch
  flavor), the CLI's doctor tests, canvas-native's graph test.
- `own_glue_e2e` 3/3 — the demo packaged as `idealyst build` links it
  (command module; constructors once in the page and in a worker, thousands
  of calls) and as a reactor, plus the hybrid demo.
- `wasm_hot_patch_e2e` 2/2 and `dev_events_e2e` 1/1, both own mode: no
  `wasm-bindgen` / `hotpatch-strand-imports` / `command-export-neutralize`
  stage in any build of either session (asserted). The base-prep stage is
  0.03 s.
- `hybrid_web_e2e` 1/1 (new): a canvas-vello app builds hybrid (wasm-bindgen
  stage, fingerprinted glue file), three glue-listener clicks render through
  wasm-bindgen's instance, canvas-vello logs `web GPU (WebGPU)` in headless
  Chrome on Apple Silicon (`--enable-unsafe-webgpu`; WebGPU's own output is
  not read back), and the forced Canvas2D fallback reads back the scene's red
  through the HYBRID-BRIDGE into canvas-native. It failed before the
  window-handle fix (no renderer marker, transparent canvas).
- `wasm_patch_roundtrip` 2/2 (the hybrid hot-patch path), backend-web's
  browser suite 119/119, canvas-native's browser suite 3/3 with
  `web-sys-canvas` and 2/2 without.
- `prune-regression --browser` (release, `--data-prune`): every app builds in
  own mode and renders; lazy main 511 KiB smaller than eager.
- Real apps in headless Chrome: `examples/nav-showcase` with `idealyst build
  --web` and `dev --web --local` (own mode; tabs, a stack push, browser back,
  settings → about, wizard; no console errors), `examples/whiteboard-demo`
  (own mode after the canvas-native change; a pointer stroke paints the
  board), `offload-demo` (own mode; 148,933 primes in a worker while the page
  animates, a panicking job answered `Canceled` and a later job served — the
  only console error is that job's deliberate panic report),
  `tests/lazy-chunk-handoff` in a dev build (the chunk's glue binding
  renders, its button works).

**Build time.** `examples/nav-showcase`, `idealyst build --web` (dev profile,
split on), warm rebuild after touching `lib.rs`, 3 runs each; hybrid is
master `4a42486d`'s CLI on that commit's tree (this tree's nav-showcase can no
longer be built hybrid — it links no wasm-bindgen runtime, which is the
point):

| | own (this change) | hybrid (`4a42486d`) |
|---|---:|---:|
| total | 0.66–0.68 s (first run 0.98) | 0.81–0.82 s |
| cargo | 0.52–0.54 s | 0.55–0.57 s |
| wasm-bindgen | — | 0.09–0.10 s |
| glue-extract + glue-package | 0.01 + 0.00 s | 0.01 s (+ neutralize 0.00) |
| wasm-split | 0.11–0.12 s | 0.12 s |
| `_bg.wasm` + JS | 3,065,258 + 59,038 B | 3,066,540 + 15,754 + 58,164 B |

On that small app wasm-bindgen was ~0.1 s, so own mode saves ~0.15 s (18%)
per warm rebuild and ships one JS file fewer. On a large app the CLI's cost
is the one this proposal started from. CrewForge (the `crewforge-hotpatch`
bench copy, framework `[patch]`ed to this checkout) still calls web-sys /
`#[wasm_bindgen]` in 15 app files, so it builds HYBRID — 67 `__wbindgen*`
imports and the section — and shows what own mode would remove once those
move to SDKs (`crates/app-main`, `idealyst build --web`, dev profile, split
on, Rust 1.97.1 aarch64, wasm-bindgen 0.2.126):

| | cold | warm (touch `app.rs`, 3 runs) |
|---|---:|---:|
| total | 71.0 s | 10.7–10.9 s |
| cargo | 61.2 s | 2.2 s |
| wasm-split | 4.7 s | 4.3–4.4 s |
| **wasm-bindgen** | **4.3 s** | **3.2–3.4 s** |
| stage + fingerprint | 0.66 s | 0.66–0.68 s |
| command-export neutralize + glue-extract | 0.13 + 0.09 s | 0.11–0.13 + 0.09–0.11 s |
| peak RSS (`/usr/bin/time -l`, the build's process tree) | 4.6 GB | 4.6 GB |

wasm-bindgen is a third of every warm CrewForge rebuild. The own-mode
replacement is the glue pass, measured in phase 1 at 0.45–0.83 s / 0.92 GB
on CrewForge's 305 MB hot-reload base and 0.04–0.05 s / 0.21 GB on a 68 MB
module, most of it file I/O (the build now keeps the module in memory
between extraction and `pkg/`). Porting
CrewForge's 15 web-sys files onto the SDKs is app work, not framework work.

The web-sys files are not the only thing keeping CrewForge hybrid. A
dependency can link wasm-bindgen for the app: chrono's `Utc::now()` /
`Local::now()` reach the browser through chrono's `wasmbind` feature
(wasm-bindgen + js-sys), and without it they panic on
`SystemTime::now()`. CrewForge reads the clock this way in app-checkin
(clock, overrides, roster) and in the shared api/core domains
(attendance, safety tips). The framework had no date/time API, so this
was a gap; it is now the `datetime` SDK (`crates/sdk/client/datetime`:
`now_utc()`, `local_offset_at(t)`, `local_timezone()`, and
`now_chrono_utc()` / `now_chrono_local()` behind its `chrono` feature),
bound through web-glue. Once CrewForge switches those calls and drops
chrono's `clock` and `wasmbind` features from its web-compiled crates,
chrono no longer links wasm-bindgen. The full list of browser
capabilities and the framework API for each is
[`docs/web-platform-coverage.md`](../web-platform-coverage.md).

## Runtime performance against web-sys

The `benchmark/` suites, the framework variants built at the pre-port
commit (`ab21341b`, web-sys through wasm-pack) and on this tree
(web-glue in hybrid mode through `bench-pack-web`), run interleaved in
one headless Chrome 154 (M3 Max), the order rotating every round, and
compared per round (median of each run, and minimum).

Three per-operation costs separated the port from web-sys, each found
with a CPU profile of the suite's operation in a tight loop and fixed
in web-glue rather than at the call sites:

- **Checked casts** decoded the class name per call (`3419eb69`): one
  `instanceof` import per `js_class!` now.
- **JS → Rust strings** went through a temporary `TextEncoder` array
  (`3d3bf085`): encoded in place now (see the Strings bullet above).
- **Property access** decoded the property name per call: one import
  per property now (see the Property access bullet above).

The suspicion that handles cost more because they live in a JS slab did
not hold: the pre-port build used wasm-bindgen without reference types,
so its handles were a JS heap as well. Counting every import and export
call per theme toggle (each wrapped at `WebAssembly.instantiate`):

| per toggle | web-sys (`ab21341b`) | web-glue |
|---|---:|---:|
| boundary crossings | 34 | 31 |
| of which handle drops / clones | 9 / 2 | 9 / 1 |
| property reads by name (`G.str` decode) | 0 | 5 → 0 |
| tight loop, µs per toggle (JIT) | 6.1–6.3 | 6.7–7.3 → 6.0–6.3 |
| tight loop, µs per toggle (interpreter only) | 11.8 | 13.0–13.4 → 12.0 |

In the profile the five name decodes were the whole difference: native
`decode` 1945 against 1443 ns per toggle (21 decodes against 16),
`G.str` 620 against 483 ns, the generic getter's own 192 ns.

The suites, paired Δ% of the per-round medians [90% bootstrap CI],
12 rounds at load 3–13 for toggle / rebuild / anim-bounce and 14 rounds
at load 5–110 (other builds on the machine) for hierarchy /
reactive-style:

| suite | bucket | web-sys | web-glue before the property fix | web-glue now |
|---|---|---:|---:|---:|
| toggle | L→D | 0.120 ms | +9.7% [+6.4…+12.9] | +12.8% [+4.0…+16.7] |
| toggle | D→L | 0.120 ms | +10.3% [+3.8…+14.6] | +0.0% [−4.2…+4.3] |
| rebuild | 1k | 5.77 ms | +5.9% [−3.5…+35.9] | +6.8% [−3.4…+27.7] |
| rebuild | 10k | 14.8 ms | +1.7% [−8.6…+6.6] | +4.3% [−0.4…+9.6] |
| hierarchy | global | 0.61 ms | −7.2% [−10.5…−2.0] | −8.9% [−14.2…+2.7] |
| reactive-style | point | 0.054 ms | +6.5% [+0.0…+12.7] | +10.3% [+3.9…+20.6] |
| anim-bounce | 1k | 117 ms | −2.3% [−33.3…+8.6] | −2.6% [−15.3…+7.5] |
| anim-bounce | 10k | 952 ms | +0.3% [−30.7…+37.5] | −1.1% [−9.7…+3.3] |

The fix closes D→L (−7.5% [−12.0…−4.3] against the unfixed build).
rebuild, hierarchy, reactive-style and anim-bounce show no difference
the noise does not cover; reactive-style's point update is 0.05 ms, a
few timer ticks (5 µs resolution, cross-origin isolated).

The toggle rows above are from the suite as it was then, and their
L→D +12.8% is the suite's warm-up, not the toggle. Both directions make
the same 31 imports, with the same string lengths, against the same DOM
(1009 elements) and the same `:root` rule. With 100 iterations, neither
build shows a direction gap past toggle ~30.

V8 compiles a function up a tier after it has run a fixed amount, so
the one-off compiles land on the same toggle numbers in every page load.
A browser trace of three loads per build gave the same schedule each
time: lazy Liftoff compiles at toggle 1, a wasm TurboFan unit at 6, a
Sparkplug batch finalized on the main thread at 9 (web-glue only), and
Maglev at 19 (web-glue) or 22 (web-sys). The page starts light, so a
toggle is light→dark exactly when its number is odd. With two warmup
toggles, the ten measured toggles were 3–12, all inside that schedule;
toggle 9 is iteration 7, an L→D sample. Starting the same suite dark
(one extra unmeasured toggle) moves web-glue's excess off L→D: L→D
against D→L goes from +5.8% [+0.0…+8.1] to −1.9% [−6.9…+2.5] (mean of
the middle 3 of each direction's 5 samples, 30 rounds, 95% CI). So the
gap came from where V8's compiles fell, which both builds go through;
no direction is slower. Which toggles the compiles hit depends on the
build's code, which is why the gap showed only on web-glue.

The suite now runs `jitWarmup` (default 50) untimed toggles, one frame
each, before the paced warmup. Paired against web-sys, 95% CI:

| toggle, `jitWarmup` 50 | web-sys | web-glue | L→D vs D→L (web-glue) |
|---|---:|---:|---:|
| 10 iterations, 30 rounds, L→D (median) | 0.115 ms | −4.3% [−8.3…+2.2] | −1.4% [−4.1…+4.1] |
| 10 iterations, 30 rounds, D→L (median) | 0.118 ms | +0.0% [−4.2…+2.2] | |
| 100 iterations, 12 rounds, L→D (10% trimmed mean) | 0.118 ms | −0.8% [−3.3…+2.1] | +0.5% [−1.5…+1.7] |
| 100 iterations, 12 rounds, D→L (10% trimmed mean) | 0.115 ms | +0.2% [−2.1…+2.8] | |

A toggle's ~115 µs in the suite is mostly cold code, and Blink's share
is smaller than it looks. Timing every import and the export per toggle
(iterations 21–100, 6 rounds) gives an export of 114–120 µs in both
builds: `setProperty` ×7 ≈ 38 µs, `removeProperty` ×2 ≈ 7–9 µs, the
other 20 imports ≈ 15 µs, and ≈ 52 µs inside the wasm itself. The same
toggle takes 6 µs in a tight loop. In the suite, each toggle runs after
350 ms of frames and verification, so the Rust and the Blink calls both
start cold.
