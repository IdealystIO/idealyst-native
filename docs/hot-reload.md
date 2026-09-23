# Hot reload — what a save costs

`idealyst dev` sorts every save into one of three outcomes before it
starts any work. They differ by roughly two orders of magnitude each, so
the sorting is worth more than any of the individual mechanisms.

| Tier | What changed | What happens | Order of magnitude |
|---|---|---|---|
| **Overlay patch** | Only literals inside `ui!` bodies | The sidecar edits its own mounted tree. No compiler at all. | ~1–10 ms |
| **Hot patch** | Only function BODIES | The user crate is re-emitted, a patch dylib is linked, a jump table rebinds the patched functions, and the mounted tree is re-run in place. No respawn, no reconnect, no page reload, and app state survives. | ~0.5–2 s |
| **Rebuild** | Anything that moves a file's SHAPE | `cargo build` + SIGKILL + respawn. Clients keep their sockets and re-snapshot. | seconds to minutes |

The last boundary is a safety boundary, not a speed tier. See
[Why a shape change cannot be patched](#why-a-shape-change-cannot-be-patched).

Both live tiers exist **only in wire mode** (also called runtime-server
mode) — `idealyst dev` without `--local`, which is the default. In
`--local` mode the app is compiled to wasm and runs in the browser;
there is no process to patch, so a body edit rebuilds and reloads the
page.

## Wire mode

The app runs NATIVELY, in a sidecar process on your machine. The
browser is a thin client: it opens a WebSocket, replays the commands the
sidecar sends, and sends events back. It runs none of your code.

That is what makes hot patching possible at all — there is a live
process holding your reactive tree, and a jump table can be applied to
it. It also means the app is subject to the sidecar's environment, not
the browser's; see [What runs where](#what-runs-where).

### Full-stack projects

A project that declares a server (`server_bin` / `server_manifest`) gets
wire mode too. The two run side by side:

- **the project's own server** keeps serving the bundle and the API
  same-origin, exactly as in `--local`;
- **the sidecar** holds the reactive tree and talks to the browser on
  its own WebSocket port.

Three things differ from `--local` on the same project:

- The bundle is built ONCE, as a thin client (features: just
  `runtime-server`), and is not watched. Source saves are the sidecar's
  business now.
- The sidecar's URL is written into the STAGED `index.html` rather than
  spliced in at serve time — the project's server hands that file out
  verbatim, so there is no serve-time hook.
- No reload/overlay SSE stream: the WebSocket is the push channel.

The SERVER watcher still runs. A `#[server]` fn's signature is a
contract between two binaries, and no jump table spans them — a server
edit rebuilds and restarts the server, as before.

### `#[server]` calls come from the sidecar

This is the consequence most worth internalising. In `--local` the
browser makes the call and the browser's cookie jar authenticates it. In
wire mode the SIDECAR makes the call.

- **Base URL.** The dev host is spawned with `IDEALYST_SERVER_URL`, so
  the documented native pattern — `server::dev_base_url()` inside your
  own `configure_server()` — resolves to the project's dev server. An
  app with no native arm in `configure_server` will fall back to whatever
  it hardcodes, or fail with "configure was never called".
- **Credentials.** On macOS the `net` SDK's transport is NSURLSession
  with the process-wide shared `HTTPCookieStorage`, so a login performed
  through the app authenticates every later call from that sidecar. You
  sign in through the browser as usual; the keystrokes travel over the
  wire, the sidecar performs the login, and the sidecar holds the
  session. **The browser never needs the cookie.**

  The jar is PROCESS-wide, not session-wide: two browser tabs on one
  sidecar share one identity. For one developer on their own machine
  that is invisible; it is not a property to rely on. Per-session jars
  need an `NSURLSessionConfiguration` per session thread.

### What runs where

| Thing | In wire mode |
|---|---|
| Your components, signals, effects | Sidecar (native) |
| `#[server]` calls | Sidecar → your dev server |
| DOM, layout, paint, input | Browser |
| `web_sys` / `js_sys` code | **Not run** — the native arm of your `cfg` runs instead |
| `localStorage` | Not available; the native arm runs |
| A file picker / clipboard / camera SDK | The SIDECAR's — i.e. your machine's, not the viewer's |
| `#[component(lazy)]` | No split natively; the body resolves immediately |

An app with a `#[cfg(target_arch = "wasm32")]` / `#[cfg(not(...))]` pair
takes the NATIVE arm. If that arm was written for a phone or for an SSR
prerender, it may be wrong here — a boot probe skipped "because SSR has
no reactor" will also be skipped in the sidecar, which does have one.

### Parity with `--local` — what is still different

Wire mode moves the app out of the browser, so anything the browser's
own engine was doing has to be reproduced over the protocol. Most of it
is; some is not yet. Measured against the same app in `--local`:

**Fixed (was wrong, now matches):**

| | |
|---|---|
| Window size | The sidecar had no viewport at all, so every breakpoint classified as `Xs` and the app painted its MOBILE layout at any width. |
| Platform identity | The recorder answered `Custom("")`, whose `is_apple()`/`is_mobile()`/`is_tv()` predicates are all `false`. It now reports the CLIENT's platform. |
| Borders, shadows, cursor, leading | 63 of 109 style properties did not cross. The block that makes an app look like an app now does. |

**Still different, in rough order of how much you notice:**

| | Why |
|---|---|
| No hover / pressed / focus styling | The recorder reports `handles_states_natively() == false`, so the engine takes the event-driven path — and the browser's `attach_states` is an intentional no-op, because on web CSS pseudo-classes normally do this job. The loop is built and broken at one point. |
| Nothing eases; everything snaps | The 34 `*_transition` style fields still do not cross. |
| Anchored overlays appear centred | `CreatePortal` collapses an `Anchor` target to viewport-centre — the sidecar cannot hand the client a live anchor node. |
| Closed overlays stay in the DOM | `ReleaseNode` does not call `release_portal`, so backdrops keep swallowing clicks. |
| Virtualized lists render nothing | `apply_create_virtualizer` is a stub; the node is never registered, so every later op about it is skipped. |
| Navigator chrome missing | No header, title, back button or transition — a navigator is a `div` that swaps its child. |
| Grid layouts collapse to flex | `display` and the grid placement family do not cross. |
| Canvas / chart surfaces are blank | Nothing populates the client's graphics registry. |
| Dark-mode switch does nothing | `ColorSchemeChanged` is a no-op in the sidecar. |
| No programmatic scroll or focus | Those capability handles resolve to their no-op defaults. |

If one of these is in your way, `--local` is the escape hatch for that
session.

### Two sessions of one project

The port sentinel is keyed by the CLI's pid, so two sessions no longer
hand each other's browser tabs to the wrong sidecar. They DO still
share the sidecar/host binaries under the framework's `target/` and the
staged bundle under the project's — so two sessions of the same project
will rebuild over each other. Use one at a time, or different projects.

### Navigation is partly wired

The sidecar has no address bar. What works and what does not:

| | Wire mode |
|---|---|
| In-app navigation (links, pushes) | **Works** — it is ordinary tree mutation over the wire |
| Deep link / initial URL | **Works** — the browser reports it on `Hello` and the session parks it where `resolve_initial` looks |
| A push updating the address bar | **No** — nothing writes `history.pushState` |
| Browser Back / Forward | **No** — no `popstate` listener, and no history entries were pushed, so Back leaves the page |
| Query-param screen state in the URL | **No** — the state changes, the URL does not |

The missing half is a wire message pair (sidecar → browser "the URL is
now X"; browser → sidecar "the user pressed Back"). Until it lands,
wire mode is for iterating on a screen you can reach by clicking, and
`--local` remains the way to exercise URL-driven behavior.

## What the decision looks at

`dev-overlay` holds it, and it is pure: source text in, decision out, no
compiler and no filesystem. Three digests per file
(`dev_overlay::archive::FileDigest`):

- **content** — did this file change at all?
- **skeleton** — the file with every `ui!` body blanked, as TEXT.
  Unchanged ⇒ nothing outside the sites moved ⇒ overlay tier.
- **shape** — the file with every function body blanked, as TOKENS
  (`runtime_macros_parse::shape_of`). Unchanged ⇒ only statements inside
  functions moved ⇒ hot-patch tier.

Tokens rather than text for the shape, so reindenting a function is free
while reformatting a signature is not.

A decision table, by example:

| Edit | Tier |
|---|---|
| `text { "hello" }` → `text { "goodbye" }` | Overlay patch |
| Add / remove / reorder `text` lines in a `ui!` body | Overlay patch |
| `count.get() * 2` → `count.get() * 3` | Hot patch |
| Add a statement, a `let`, a closure | Hot patch |
| Add a `signal()` call | Hot patch (that component's state resets — see below) |
| Edit a free function's body | Hot patch |
| Add a comment or blank line outside a body | Hot patch |
| Change a prop's type, or add a prop | Rebuild |
| Add or rename any item | Rebuild |
| Change a `const` / `static` initializer | Rebuild |
| Change an attribute or a doc comment | Rebuild |
| Edit inside `stylesheet! { … }` | Rebuild |
| Edit a file outside the app crate | Rebuild |

A save that carries BOTH a literal edit and a body edit is one hot
patch, not a patch plus a rebuild: the hot patch re-emits the crate from
source, so the literal arrives with it.

## The hot-patch pipeline

1. The initial sidecar build is a "fat" build: `RUSTFLAGS` keeps
   `-Csave-temps=true -Clink-dead-code`, and a rustc wrapper captures
   each crate's exact invocation to disk.
2. On a body-only save the host replays the USER crate's captured
   invocation with `--emit=obj`. Framework crates stay cached, so this
   is one crate's codegen, not the graph's.
3. A stub object is synthesized: every undefined reference in those
   objects is resolved back into the running sidecar's text as an
   absolute-address trampoline (plus TLV descriptors for thread-locals).
4. `cc -dynamiclib` links tip objects + stub into `libpatch-N.dylib`.
5. Symbols named `__*_hot_impl` — the inner halves of the
   `#[component]` split — are paired between the sidecar binary and the
   dylib into a `JumpTable`, along with the app root (matched by
   ADDRESS; see below).
6. The table crosses the host↔sidecar pipe; the sidecar calls
   `dev_hot::apply_patch` and tells every live session to re-run.

Everything from step 2 lives in
`crates/tools/build/runtime-server/src/hotpatch/`; the runtime side is
`crates/dev/hot`.

### On the web the base module has to be prepared first

A wasm patch is a PIC side module: everything it does not define, it
imports, and those imports resolve against the module already running in
the page. Making a function resolvable there is not the same problem as
on a native target, for two measured reasons.

**wasm-bindgen garbage-collects, and it does not care what the linker
kept.** On the probe crate, rustc linked 2993 functions under
`--no-gc-sections`; wasm-bindgen's own pass cut that to 560, keeping only
what the 12 exports and the element segment reach.

**Exporting more does not help.** `--export-dynamic` takes a module from
~70 exports to ~1700 and still exports no `__<Name>_hot_impl`, because a
private Rust `fn` has internal linkage and is not a dynamic symbol.

So the mechanism is the element table, not the export table.
`build_web::hotpatch_base::prepare_base_module` runs between cargo and
wasm-bindgen and appends every local function to
`__indirect_function_table`. A table entry is a GC root, so wasm-bindgen
keeps the function — and a table index is what the patch wants anyway,
since a wasm `fn` pointer IS a table index. It also copies each
`__wbindgen*` intrinsic into a trampoline named `__saved_wbg_<name>`,
because wasm-bindgen deletes those by name-match whether they are used or
not.

The prepared module is written to a sibling file rather than over
cargo's artifact. Cargo's freshness check is on mtime and does not hash
what it produced, so overwriting in place would leave a later
non-hot-patch build reusing a module with thousands of extra table
entries, considering it fresh, and shipping it.

### `-Clink-dead-code` is not available on wasm

It is the flag that would make rustc emit every monomorphization rather
than only the reachable ones, which is what the native pipeline uses. On
wasm it panics wasm-bindgen 0.2.128 outright:

```
wasm-bindgen-cli-support-0.2.128/src/descriptor.rs:324
index out of bounds: the len is 0 but the index is 0
```

Measured both with and without `--export-dynamic`; the flag alone is
enough to trigger it. The consequence is that a patch referencing a
function rustc never codegened fails to link — which is loud, and falls
back to a rebuild.

### What the base module has to survive

Rooting every function in the element table is what makes the tier work,
and it drags three wasm-bindgen behaviours along with it. Each cost a
non-booting page before it was understood, so each is worth naming.

**wasm-bindgen's descriptor imports.** Keeping every function alive keeps
the descriptor machinery alive, and wasm-bindgen emits no JS binding for
the `__wbindgen_placeholder__.__wbindgen_describe` it calls — the page
dies with "Import #0 __wbindgen_placeholder__: module is not an object or
function". Leaving the functions that reach it unrooted does not work:
transitively that is a third of the module (`Closure::wrap` calls a
`describe` function, so every event handler goes with it) and patches
then fail to link against `<u32 as Display>::fmt`; one hop misses the
survivors. So everything is rooted and the leftover import is given a
local `unreachable` body afterwards. A descriptor function is never
called at run time — wasm-bindgen has already read what it describes — so
the trap is unreachable, and loud if it ever is not.

**No alias exports.** An earlier design exported each `__wbindgen*`
function a second time as `__saved_wbg_<name>` so a patch could import it.
wasm-bindgen deletes the ORIGINAL export and then looks up "the export
name for this function" to generate its JS, found the alias, and emitted
`wasm.__saved_wbg___wbindgen_exn_store.command_export(idx)` — not a
function. Every handled error threw, during boot. The table is the single
mechanism; a patch reaches these through their slot.

**walrus 0.26.** wasm-bindgen's catch-wrapper transform emits real
exception handling once its machinery is kept alive, and walrus 0.23's
parser has no `EXCEPTIONS` feature — the next pass could not read the
module at all. 0.26 parses with `WasmFeatures::default()`, and is the
version wasm-bindgen itself uses.

### The session pins the `idealyst` binary

A hot-patch build sets `RUSTC_WRAPPER` to the running `idealyst`
executable so every crate's rustc invocation is captured. Cargo re-runs
that path for each crate, so rebuilding or deleting the binary while a
session is live breaks the session's next build with

```text
could not execute process /…/target/debug/idealyst
```

Restart the session after rebuilding the CLI. The same applies to
`cargo clean` on a workspace whose `target/` holds both the binary and
the session's web target directory.

### The app root has to be a `#[component]`

A jump table redirects a function by its slot in
`__indirect_function_table`, and only a function whose address something
took has one. `#[component]` takes it — that is what the
`__<Name>_hot_impl` split is for.

A root written as a plain `pub fn app()` has no slot, so it cannot be
redirected, and nothing below it is reached through the patch either: the
patch holds a fresh copy of the whole crate, but only a redirected
function is ever entered. Keep `app()` thin and put the tree in a
component:

```rust
#[component]
fn Root() -> Element { /* … */ }

pub fn app() -> Element {
    ui! { Root() }
}
```

### The jump table is indices, not addresses

A native entry pairs two addresses. A wasm entry pairs two
`__indirect_function_table` indices, found by composing the custom `name`
section (function index → name) with the element segments (table index →
function index). `build_web::hotpatch_wasm::build_jump_table` does that.

A key is an ABSOLUTE index in the base's table; a value is an index
RELATIVE to the patch's own element segment, because `apply_patch` grows
the table and rebases the patch's entries by the grow's return — which is
what the `__table_base` global the patch imports resolves to. Which is
also why a segment whose offset is `global.get` (every PIC patch) folds
to zero rather than being skipped.

### The web save loop

Armed by `idealyst dev --web --local`, and only when the splitter is off
(`--split` stands the tier down: it moves functions into lazy chunks and
renumbers the main module's table, so a patch built against the base pairs
with the wrong slots or with nothing). Splitting is off by default in dev,
so the ordinary session has it.

1. The base build sets `RUSTC_WRAPPER` to the `idealyst` binary, so every
   crate's exact rustc invocation is captured to
   `target/…/idealyst-hotpatch/captures/`.
2. A save that `overlay_decide` calls `HotPatch` — a change inside function
   bodies — goes to `build_web::hotpatch_build::WasmPatchBuilder` instead of
   a rebuild.
3. The builder replays the user crate's captured invocation with
   `--emit=obj -Crelocation-model=pic`, links those objects alone with
   `wasm-ld --pie --experimental-pic`, resolves the imports against the
   running base, and pairs the two tables.
4. The patch module is written into the served bundle's
   `pkg/hotpatch/patch-N.wasm`, and the dev loop pushes an SSE `hot-patch`
   event carrying `{url, table}`.
5. The page's livereload script calls `window.__idealyst_hot_patch`, which
   applies the patch and rebuilds the tree against it.

Every step reports a failure that names what it could not do, and every
failure falls back to the rebuild-and-reload the save would otherwise have
got. A patch that half-applies is worse than no patch: the page keeps
running and dispatches through a table pointing somewhere arbitrary.

### Rebuilding the tree without losing the page

Applying a patch changes nothing on screen — the DOM in front of the user
was built by the old bodies. So the page re-runs the app root, which is why
`backend_web::newcore::start_in_with` takes `impl Fn() -> Element` rather
than `FnOnce`.

State is carried by `runtime_world::hot_state`, and the order is not a
preference. Values are MOVED out of the dying world's arena:

1. `harvest()` while the arena is alive,
2. drop the tree and the world,
3. `seed()` so each `signal()` call in the rebuild finds its predecessor's
   value.

Harvest after the drop finds nothing; seed before it seeds the world about
to die. This is the same sequence the native sidecar's `SessionMsg::Rerender`
runs, deliberately: one hot-patch semantics, two backends.

The rebuild rides `subsecond::register_handler` rather than following the
`apply_patch` call, because on wasm `apply_patch` finishes asynchronously —
it awaits a fetch and an instantiate. Rebuilding at the call site would
rebuild against the old code and show nothing changed.

### Why components are split

Subsecond rebinds function ADDRESSES. For an entry to do anything the
running program has to reach that function through an indirect call that
consults the table — a direct `bl` in already-linked code cannot be
redirected. So under `runtime-core/hot-reload`, `#[component]` emits:

```rust
#[doc(hidden)]
#[inline(never)]
fn __Counter_hot_impl(props: &CounterProps) -> Element { /* body */ }

pub fn Counter(props: &CounterProps) -> Element {
    let __idealyst_hot_inner: fn(&CounterProps) -> Element = __Counter_hot_impl;
    ::runtime_core::__hot::call(__idealyst_hot_inner, (props,))
}
```

The outer keeps the author's name, visibility, signature and attributes,
so a component's SHAPE — props struct, `Tag = TagProps` alias,
`BuildElement` impl, every call site — is identical with the feature on
or off. `runtime-macros`' frozen `goldens/component_*.txt` prove the
feature-off emission byte for byte.

A component whose signature has no fn-pointer spelling is emitted
unsplit and is simply not on the fast path: generics, arity above nine
(subsecond's widest `HotFunction` impl), `impl Trait`, a destructuring
parameter, `async` / `unsafe` / `extern`.

### The app root

`fn app() -> Element` is not a `#[component]`, so it has no
`__*_hot_impl` symbol. The sidecar calls it through a fn pointer and the
host pairs it by ADDRESS instead: the sidecar reports the root's runtime
address on its `Hello` frame, the host subtracts the ASLR slide, and
looks up which symbol sits at that link-time address. Without this, an
app whose whole tree lives in `app()` would apply a patch that rebound
nothing.

## What survives a patch, and what does not

A patch rebinds functions; it does not re-render. The sidecar therefore
re-runs each mounted session's tree in place. A fresh run would normally
build a fresh world with fresh signals — so the kernel carries the
values across (`runtime_world::hot_state`).

Values MOVE, they are not cloned: `harvest` takes each recorded signal's
boxed payload straight out of the dying arena and `seed` hands it to the
matching `signal()` call in the next run as its initial value. That is
why no `Clone` bound appears on `signal`.

Matching is by POSITION, the only identity the kernel has: the component
path (each `#[component]` body opens a frame; a frame folds its parent's
identity with the component's name and which occurrence of that name it
is under that parent), the ordinal within that frame, and the value's
`TypeId`.

**Preserved:**

- signals created during the mount walk — the app root's own state, and
  every `#[component]` body reached from it.

**Not preserved, by design:**

- state whose layout changed. The first ordinal whose type disagrees
  with the recording poisons that frame: everything after it in the same
  body gets fresh state. A body that gained or reordered a `signal()`
  therefore RESETS rather than quietly receiving another variable's
  value.
- signals created after the mount walk — inside an event handler, or
  inside a reactive region (`if` / keyed `for` / a navigator screen)
  that mounts later. Those creations happen in whatever frame is open at
  the time, which for a driver effect is the root frame, so two regions
  building the same component would compete for one ordinal. Handing one
  region another's state is a worse failure than resetting it, and it
  would be invisible.
- anything in a `static` or a `thread_local!` in the user crate: the
  patch dylib gets its own copy.

## Why a shape change cannot be patched

A patch dylib is spliced into a process where every other crate is still
the old build. Only the user crate is re-emitted. If a save changed a
props struct's fields, the patched code computes one layout while the
framework's own generic instantiations over that type — compiled into an
rlib that is never re-emitted — keep the other. The symptom is memory
corruption, not a stale render.

So a hot patch is attempted ONLY on an explicit body-only decision. A
missing or unreadable descriptor set means "cannot tell", and cannot-tell
respawns.

## Escape hatches and diagnostics

- `IDEALYST_RUNTIME_SERVER_NO_HOTPATCH=1` forces every save through the
  respawn path. Useful for A/B timing and for isolating a suspected
  patch bug.
- `[dev] …` lines name the tier a save took and, on a rebuild, why.
- `[hotpatch] timing: rustc …ms stub …ms link …ms jt …ms` breaks the
  patch build down.
- `[dev-hot] call #N … jt_entries=… jt_hit=…` prints the first few
  dispatches after each patch. `jt_hit=None` on a function you expected
  to change means the table has no entry for it.
- `[runtime-server-app] … carrying N signal value(s) across the patch`
  is the state handoff.

## Turning it off

The whole substrate is behind one cargo feature,
`runtime-core/hot-reload`, which forwards to:

- `runtime-macros/hot-reload` — the emission (the split);
- `runtime-vocabulary/hot-reload` — the dispatch anchor (`glue::__hot`,
  a `dev-hot` re-export) and, through it, `runtime-world/hot-reload` —
  the kernel's state carry.

Off by default. `idealyst dev`'s generated sidecar is the only thing
that turns it on; a production build carries none of it, and
`runtime-macros` emits the author's function verbatim.
