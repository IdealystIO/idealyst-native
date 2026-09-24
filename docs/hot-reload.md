# Hot reload — what a save costs

`idealyst dev` sorts every save into one of three outcomes before it
starts any work. They differ by roughly two orders of magnitude each, so
the sorting is worth more than any of the individual mechanisms.

| Tier | What changed | What happens | Order of magnitude |
|---|---|---|---|
| **Overlay patch** | Only literals inside `ui!` bodies | The running app (the sidecar, or the page in `--local`) edits its own mounted tree. No compiler at all. | ~1–10 ms |
| **Hot patch** | Only function BODIES | The edited crate is re-emitted (with its dependents, for a library crate of the app's workspace), a patch (a dylib natively, a wasm side module on the web) is linked, a jump table rebinds the patched functions, and the mounted tree is re-run in place. No respawn, no reconnect, no page reload, and app state survives. | ~0.5–2 s on a small app, ~9–10 s on a large one ([measured](#what-a-save-costs-on-the-web)) |
| **Rebuild** | Anything that moves a file's SHAPE | Wire mode: `cargo build` + SIGKILL + respawn; clients keep their sockets and re-snapshot. `--local`: the bundle is rebuilt and the page reloads, so page state resets. | seconds to minutes |

The last boundary is a safety boundary, not a speed tier. See
[Why a shape change cannot be patched](#why-a-shape-change-cannot-be-patched).

Both live tiers exist in wire mode (also called runtime-server mode:
`idealyst dev` without `--local`, which is the default), where the app
runs in a native sidecar. `--local` mode, where the app is compiled to
wasm and runs in the browser, has the hot-patch tier too: the patch is a
wasm side module the page instantiates next to the running one. See
[The web save loop](#the-web-save-loop) for how, and
[What a save costs on the web](#what-a-save-costs-on-the-web) for
measured numbers.

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
compiler and no filesystem. Three digests per file, and a fourth for
premint (below)
(`dev_overlay::archive::FileDigest`):

- **content** — did this file change at all?
- **skeleton** — the file with every `ui!` body blanked, as TEXT.
  Unchanged ⇒ nothing outside the sites moved ⇒ overlay tier.
- **shape** — the file with every function body blanked, as TOKENS
  (`runtime_macros_parse::shape_of`). Unchanged ⇒ only statements inside
  functions moved ⇒ hot-patch tier.

Tokens rather than text for the shape, so reindenting a function is free
while reformatting a signature is not.

`stylesheet!` is the one macro the shape looks inside. It expands to
items (a builder, one enum per `variant` axis, a setter per axis and per
`override`, a `<name>_style()` fn), but every item comes from the sheet's
signature; the rule values land only inside `<name>_style()`'s body and in
string literals in the builder's methods. So `shape_of` blanks the
contents of every rules block (the brace group right after a
parenthesized binding: `base(t) { … }`, `small(t) { … }`,
`state hovered(t) { … }`) and the whole `transitions { … }` block (its
entries become `*_transition` values in the base rules, with no item per
entry), and keeps the rest. On the running page the
patched `<name>_style()` keys the stylesheet cache by the address of its
own function-local `static`, which the patch defines anew, so it builds
the sheet with the new rules instead of reusing the old one.

A premint session is the exception. Its class names were generated at
session start from a hash of the whole invocation, so there a value edit
still rebuilds: the archive keeps a fourth digest, **sheets** (the raw
`stylesheet!` tokens, `runtime_macros_parse::stylesheet_tokens`), and
`decide_with(.., premint = true)` rebuilds when it moves.

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
| A rule VALUE inside `stylesheet! { … }` (a number, a token, a rule added or removed in any block, including `state`/`breakpoint`/`container`/`compound` blocks) | Hot patch |
| Anything in a `stylesheet!`'s `transitions { … }` (a duration, an easing, an entry added or removed) | Hot patch |
| A `stylesheet!` signature: its name or vocabulary, an axis, an arm name, a `#[default]`, an `override`, a `state`/`breakpoint` key, a `container` threshold, a `compound` condition | Rebuild |
| Any `stylesheet!` edit in a premint session (`dev --premint`) | Rebuild |
| A function body in a library crate of the app's cargo workspace (a path dependency that is a member) | Hot patch — the library AND every workspace crate depending on it are re-emitted ([Workspace crates](#workspace-crates)) |
| A literal in a `ui!` body of such a library crate | Overlay patch |
| The body of a generic, `#[inline]`, `const`, `async` or `impl Trait` fn, or a trait's default method, in such a library crate | Rebuild — its dependents compile that body themselves |
| Any shape edit in such a library crate (a new prop on a shared component) | Rebuild |
| Edit a file of a local package outside the app's workspace (a `[patch]` checkout of the framework), or a `Cargo.toml` | Rebuild |

A few rows the table above does not make obvious, each a limitation of
the overlay tier rather than of hot reload as a whole:

| Edit | Tier |
|---|---|
| A `#[component]`'s literal prop (`Typography(content = "Hi")` → `"Hello"`) | Overlay patch — but it shows on that site's NEXT render, not immediately, unless the component's props are `Clone`. `#[component]` does not derive `Clone` on the props it generates (only `#[component(lazy, retryable)]` does), so this is the usual case; the page console says `overlay patch: 0 applied, 1 waiting for the next render` |
| Insert a sibling that carries a `style` (or any other dynamic prop) into a `ui!` body | Hot patch — the overlay would need to know the new node's expression is the same compiled code as a neighbour's ("slot aliasing"), which is not built |
| Any edit inside a `jsx!` body | Hot patch — `jsx!` is not on the overlay's split pass: its sites have no descriptor and no tags |

A save that carries BOTH a literal edit and a body edit is one hot
patch, not a patch plus a rebuild: the hot patch re-emits the crate from
source, so the literal arrives with it.

Some `ui!` edits the overlay refuses: a literal becoming a variable or a
closure (`text { "Title" }` → `text { title }`, a static slot turning
dynamic), or a `ui!` nested inside another site's body appearing or
vanishing (a new site has no compiled tag to address). Those are still
edits inside a function body, so they fall to the hot-patch tier when the
file's shape is unchanged, and rebuild only when it moved (or cannot be
computed). A `ui!` body that no longer parses rebuilds: it would not
compile as a patch either. After a hot patch the decision re-derives that
file's sites from the new source, keys included, because the remounted
tree is built by the patched code and carries the keys of the source it
was compiled from; the next literal edit is an overlay patch again.

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
since a wasm `fn` pointer IS a table index.

A JS shim (what an `extern "C"` block under `#[wasm_bindgen]` becomes)
is an IMPORT in the base, not a function, so it has no table slot. Base
prep gives each one a small forwarding body named
`__idealyst_shim_<name>`, roots that in the table, and does NOT export it
(see "No alias exports" below). A patch that calls the shim resolves to
that slot.

The prep prints a census, which is the first thing to read when a patch
fails on an unresolved import:

```text
[build-web] hot-patch base: … 26564 local fns = 2077 already in the table
  + 5851 bindgen-internal + 18636 promoted, + 4043 JS-shim trampolines
  → emitted 24756 slots over 30607 local fns; 4060 symbol aliases
```

"bindgen-internal" are wasm-bindgen's descriptor functions, left out on
purpose. "emitted" lower than the sum would mean walrus dropped something
it should not have. It does not: walrus 0.26 reaches a locally defined
table's segments through the table itself, and `--export-table` roots
the table.

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

**One function, several symbol names.** A patch resolves everything it
did not recompile by mangled name, and the base's names come from the
custom `name` section, which keeps ONE name per function. The linker
knew many functions by more than one: 949 of them in the lab's base, 4,060
extra names in all. Two causes, both in every ordinary app:

- `usize` is `u32` on wasm32, so the precompiled `core` defines
  `<usize as Display>::fmt` and `<u32 as Display>::fmt` as one function.
  No opt level changes this.
- LLVM's merge-functions pass folds identical bodies in dependencies
  built at `opt-level = 3`, which is what `--dev-opt optimized` gives
  them. `<Element as IntoElement>::into_element` and
  `<Element as IntoSceneElement>::into_scene_element` are both
  `{ self }`, so they become one function.

Looked up by name-section name alone, both looked absent, and every
ordinary body edit was refused. `build_web::hotpatch_aliases` reads
`alias → canonical` out of the `linking` section's symbol table (this is
what `--emit-relocs` on the base is for) and writes it next to the base as
`<app>.aliases.tsv`. It has to be read from the LINKED module, before base
prep: walrus and wasm-bindgen renumber functions, and the `linking`
section they carry forward keeps the old indices. The map is name to name,
so renumbering cannot make it stale. `BaseIndex` and the jump table both
consult it.

An import that still cannot be resolved is reported with which of the two
opposite fixes applies: "the base HAS this function but it is not in the
table" (a rooting bug) or "the base does not contain this function at
all" (it was never codegened, so no rooting pass could keep it).

**The patch keeps one start function.** wasm-ld makes
`__wasm_apply_global_relocs` the start function of a `--pie` module. It
adds `__table_base` to every `GOT.func.internal.*` global, which is the
address of a function the PATCH defines and can only be written relative
to the patch's own segment. The resolver drops any other start function
(it would run before the jump table is committed), and it used to drop
this one too. Then walrus's GC removed the function, those globals stayed
relative, and the first render against a patch trapped with `null
function` in `core::fmt::write`, where a `format!` argument's formatter
pointer was 0. That start function is kept now, and so is the wrapper
wasm-ld emits instead when the patch has any zero-initialized static:
`__wasm_start`, calling `__wasm_apply_global_relocs` and
`__wasm_init_memory` (which zero-fills the patch's own `.bss`). Any other
start is dropped, and `__wasm_apply_global_relocs` is then exported so the
loader still runs it.

### Each app gets its own web target dir while the tier is armed

Ordinary web builds share one `target/idealyst-web-<config>` directory
across every app of a framework checkout, so sibling apps reuse
dependency builds. With the hot-patch tier armed the key also carries
the project, and each app builds in a directory of its own.

A patch replays the app crate's captured rustc invocation minutes after
the build that captured it, outside cargo's lock, against whatever
`.rlib`/`.rmeta` files are on disk by then. In a shared directory another
app's build rewrites some of them in place: `backend-web` ships a cdylib,
so cargo gives its rlib no hash suffix and every app writes the same
`libbackend_web.rlib`. With the lab and CrewForge sharing a directory,
CrewForge's patches failed with "can't find crate for
`crewforge_ui_shared`" (rustc's locator log: `video -> backend_web:
Rejecting via hash`). The captured invocations live under the same
directory and separate with it. The cost is one cold framework build per
app the first time the tier is armed.

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
3. The builder replays the captured invocation of each crate the patch
   re-emits — the edited crate, its workspace dependents, and every crate
   patched since the last rebuild ([Workspace crates](#workspace-crates))
   — with `--emit=obj -Crelocation-model=pic`, concurrently, links those
   objects alone into one module with `wasm-ld --pie --experimental-pic`,
   resolves the imports against the running base, and pairs the two
   tables.
4. The patch module is written into the served bundle's
   `pkg/hotpatch/patch-N.wasm`, and the dev loop pushes an SSE `hot-patch`
   event carrying `{url, table}`.
5. The page's livereload script calls `window.__idealyst_hot_patch`, which
   applies the patch and rebuilds the tree against it.

Every step reports a failure that names what it could not do, and every
failure falls back to the rebuild-and-reload the save would otherwise have
got. A patch that half-applies is worse than no patch: the page keeps
running and dispatches through a table pointing somewhere arbitrary.

### What a save costs on the web

Measured 2026-09-23 on an M-series Mac, `--dev-opt optimized` (the
default), save to the new pixels on screen: the shell timestamp of the
write, against a `MutationObserver` (or a computed-style poll) in the
page that records when the new content appears. "Builder" is the
`[hotpatch]` log line's total: replay, link, resolve, jump table.

| App | Save | Save → on screen | Builder | Page state |
|---|---|---|---|---|
| Lab (small) | body edit, first of session | 1.57 s | 0.68 s | kept, no reload |
| Lab | body edit, warm | 0.84 – 1.17 s | 0.26 – 0.59 s | kept, no reload |
| Lab | `stylesheet!` value edit | 1.31 – 1.97 s | 0.36 – 0.83 s | kept, no reload |
| Lab | shape edit (add a prop) | 6.0 s (rebuild + reload) | – | reset |
| CrewForge (large) | body edit, first of session | 34 s | 29 s (rustc 26 s, cold replay cache) | kept, no reload |
| CrewForge | body edit, later | 8.6 – 9.8 s | 7.9 – 9.1 s (rustc 5.9 – 6.8 s, resolve 1.4 – 1.6 s, link 0.4 – 0.6 s, jump table 0.09 s, strip + write 0.02 s) | kept, no reload |
| CrewForge | rebuild with the tier armed, after patches | 38.6 s (cargo 15 s) | – | reset |

Workspace crates, measured 2026-09-24 on the same machine under heavier
load (a VM using ~1.4 cores, load average 8–12), so compare rows within
this table rather than with the one above:

| App | Save | Save → on screen | Builder | Page state |
|---|---|---|---|---|
| Lab + `lab-shared` | library body edit, first of session (library and app replayed side by side) | 1.24 s | 0.70 s (lab-shared 0.55 s, app 0.57 s) | kept, no reload |
| Lab + `lab-shared` | library body edit, later (app reused) | 0.93 s | 0.38 s | kept |
| Lab + `lab-shared` | library `stylesheet!` value edit (the app's `view(style = SharedPanel())` too) | 1.08 s | 0.56 s | kept |
| Lab + `lab-shared` | app edit after library patches (library carried, reused) | 0.95 s | 0.44 s | kept |
| Lab + `lab-shared` | literal in a library `ui!` body | 0.49 s | overlay, 2 ms | kept |
| Lab + `lab-shared` | library shape edit (new prop) | 7.7 s (rebuild + reload) | – | reset |
| CrewForge | `crewforge-ui-shared` body edit, first of session | 13.1 – 14.4 s | 12.3 – 13.6 s (ui-shared 2.1 – 3.6 s, app-main 8.8 – 9.5 s, concurrently) | kept, no reload |
| CrewForge | `crewforge-ui-shared` body edit, later (app-main reused) | 4.7 – 11.0 s | 3.9 – 10.0 s (ui-shared 1.3 – 5.6 s, resolve 2.1 – 3.1 s) | kept |
| CrewForge | `crewforge-main` body edit after ui-shared patches (ui-shared reused) | 15.9 – 20.9 s | 14.9 – 19.6 s (app-main 10.1 – 12.9 s, resolve 3.7 – 5.7 s) | kept |
| CrewForge | `crewforge-ui-shared` shape edit (new prop on `TeamAvatar`) | 59.4 s (rebuild + reload; cargo 29 s) | – | reset |
| CrewForge | rebuild after an app-main shape edit | ~40 s (cargo 13–15 s); only `crewforge-main` compiles | – | reset |

A ui-shared save is the cheaper of the two on CrewForge once the app
crate has been replayed once: the library is a third the size, and the
app's objects are reused.
| CrewForge | rebuild, tier not armed (reference) | ~23 s | – | reset |

On the page itself a patch applies in about 0.35 s even on CrewForge
(fetch, instantiate, 21,341 functions redirected, the tree rebuilt).
Most of the rest is the replayed rustc. Its floor is a no-edit replay,
about 5.4 s on CrewForge offline (`-Ztime-passes`): macro expansion 1.95 s
(`ui!` 0.61 s over 2,303 calls, `#[component]` 0.48 s over 213, now about
half that, `stylesheet!` 0.20 s), encoding this crate's metadata 0.9 –
1.25 s (an `rlib` always encodes it, and rustc has no switch to skip it;
a `staticlib` crate type skips it but costs more in codegen), persisting
the incremental cache 0.6 – 1.1 s, mono collection and partitioning
about 0.9 s. Type checking, borrow checking and codegen of the one
changed unit are each under 0.3 s.

The base is indexed once per base build, right after it is built, and
the builder keeps the base's slot map rather than re-reading the module
per patch.

**The replay cache is seeded after every base build.** A replay has its
own incremental directory, which the base build never fills, so the
first patch of a session used to compile the app crate cold (45.7 s of
a 49.4 s first save in a fresh devcontainer). Right after the base is
indexed, the app crate is replayed once in the background with no edit,
while the page loads:

```text
[hotpatch] replay cache for `crewforge_main` seeded in 42752 ms
```

A save that arrives mid-seed waits for it rather than starting a second
replay in the same incremental session. Its objects are kept under the
app crate's source digest, so a library save that only carries the app
reuses them. Measured on CrewForge with the replay cache cold (loaded
machine, as in the workspace table): first save 39.1 s save to screen
(rustc 34.4 s) without the seed, 18.3 s (rustc 11.7 s) with it.
`IDEALYST_HOTPATCH_NO_SEED=1` turns it off, for A/B timing.

**Before the replay starts** about 0.5 s passes: the file watcher's
50 ms debounce, then `settle`'s 400 ms quiet window, which coalesces a
multi-file save into one build, then reading and deciding. Measured save
to replay start on the lab: 499–515 ms. The patch itself is linked
`--strip-debug`, since the served module never carried DWARF.

**The served patch has no names.** A patch's `name` section is more than
half of it (33 of 60 MB on CrewForge; walrus already drops the DWARF on
emit). The jump table pairs BY NAME, so it is built first, and then the
`name` and any `.debug_*` sections are cut out of the bytes (3 ms; no
second walrus pass). The page fetches 26.6 MB instead of 59.7 MB: fetch
38 ms instead of 111–228 ms, compile 28 ms instead of 36–45 ms, and the
time from a built patch to the pixels drops from ~0.92 s to ~0.73 s.
Only the newest two served patches are kept (the older ones filled the
staging dir: 228 MB after four CrewForge saves).

What that costs is function names in a stack trace through the patch.
The named module is kept on disk as `last-patch.named.wasm` beside the
build's `idealyst-hotpatch/captures`, and `IDEALYST_HOTPATCH_KEEP_NAMES=1`
serves it instead, for a session where a trace has to be read in the
browser.

**Why the replay is not the whole crate.** rustc forces
`codegen-units=1` when the emit set contains an object-like output and no
count is given. Cargo's build (`--emit=link`) gets the incremental
default of 256, but a replay that just swapped in `--emit=obj` compiled
the crate as ONE unit, so any edit re-codegened all of it: 40–46 s for a
one-token edit, 63 s for one that shifts lines. `replay::replay_args`
passes `-Ccodegen-units=256` (7.4 s and 6.4 s). It also gives replays
their own incremental dir, `<dir>-hotpatch`: the base build and a replay
differ in tracked options, and rustc discards a cache whose options
differ, so a shared directory made each compile cold after the other.
Replay objects go to a private directory emptied first, because with many
units their names are hashed and would otherwise accumulate.

The rebuild with the tier armed is still slower than an ordinary one
because base prep, the stranded-import pass and a second walrus pass run
over a 223 MB module (320 MB linked), about 22 s on CrewForge.

**Memory.** On CrewForge the CLI's peak RSS during base prep is about
3.7 GB (one walrus parse of the module). The default memory cap is
4096 MB, and a session exceeded it twice. Run a large app with
`IDEALYST_MEMORY_LIMIT_MB=8192` while the tier is armed.

### Workspace crates

Most apps outgrow one crate: shared UI moves into a library crate of the
same cargo workspace (CrewForge's `crewforge-ui-shared` is 37k lines,
57 files with `ui!`). A body edit there is patched like one in the app
crate. The dev loop keeps one descriptor set per crate of the app's
closure of workspace members (`dev_overlay::Workspace`), routes a saved
file to the crate that owns it, and decides it against that crate's
archive. A library's sets live in
`target/idealyst/<app>/overlay/crates/<package>/`, inside the app's
staging tree. The session prints which crates it can patch:

```text
[dev-reload] workspace crates patchable with crewforge-main: crewforge-api, crewforge-core, crewforge-ui-shared
```

**A library patch re-emits the library's dependents too.** The jump
table only redirects calls that go through a table slot — a
`#[component]` body, reached through `__hot::call`. The app calls a
library's plain functions, its stylesheets' `<name>_style()`, its
builder methods DIRECTLY, and a direct `call` in the base's code cannot
be redirected. A patch of the library alone would change what the
library's own components run and leave every call from the app on the
old body. So a save in a library re-emits it and every workspace crate
depending on it, up to the app, and links all their objects into ONE
patch: the app's calls then resolve inside the patch, to the new bodies.
(`build-web`'s `wasm_patch_roundtrip` shows the difference on a real
two-crate build: the library alone pairs its own component slot and
none of the app's.)

One module rather than one per crate, because a patch REPLACES the
page's jump table: a second module holding only the app would send the
library's functions back to the base. For the same reason every crate
patched since the last rebuild is re-emitted by each later patch, even
when the save touched only the app — otherwise an app-only save would
silently undo the earlier library edit.

That is not a full recompile of the dependents' graph, for three
reasons:

- **The replays are concurrent.** Each reads its dependencies' METADATA
  from the base build, never another replay's output (a replay emits
  objects only), so the library and the app compile side by side and a
  save costs the slower one, not the sum.
- **Unchanged dependents are reused.** A crate carried only for
  another's edit, whose sources have not moved since its last replay
  against this base, compiles to the same objects; the builder keys each
  crate's objects by a digest of its sources and skips the replay. So the
  first library save of a session replays the app once; later ones do
  not.
- **Incremental.** A dependent that is replayed has an unchanged source
  and unchanged upstream metadata, so its codegen units come out of the
  cache — it pays the replay's floor (macro expansion, metadata
  encoding), not a compile.

**What a library patch cannot carry.** Because dependents are replayed
against the base's metadata, a body that rustc compiles INTO the
dependent from that metadata stays old there: a generic function (its
instantiations are compiled wherever the concrete types are named), an
`#[inline]`/`#[inline(always)]` function, a `const fn`, an `async fn`, a
function with `impl Trait` in its signature, a trait's default method.
The archive records a digest of each such function per file
(`runtime_macros_parse::downstream_bodies`), and a library save that
moves one rebuilds, naming it:

```text
[dev] rebuilding: crewforge-ui-shared/src/grid.rs changed the body of `fn sort_rows`, which the crates depending on it compile themselves (generic, `#[inline]`, `const`, `async` or `impl Trait`)
```

The check reads source, so it sees `#[inline]` and generics as written,
not as a macro might emit them. That is sound for the macros that matter
here: `#[component]` splits a body into an `#[inline(never)]`
`__<Name>_hot_impl`, and a generic component is caught by its own
signature; `stylesheet!`'s rule values land in a plain `<name>_style()`
fn. Two things make the rest of the library safe to replay alone
against old metadata:

- the decision already proved the crate's SHAPE unchanged, so its
  metadata (types, signatures, layouts) is still true;
- workspace members build at the dev profile's own `opt-level` — 0
  unless the app's manifest says otherwise; `--dev-opt optimized` raises
  only `package."*"` (registry and other non-member dependencies) to 3 —
  and with incremental compilation on, rustc infers no cross-crate
  inlining for unmarked functions, so a dependent holds no inlined copy
  of a plain function's body.

A local package that is NOT a workspace member — a `[patch]` pointing
the framework at a checkout — is watched but rebuilds: cargo builds it
with the `package."*"` profile, at `opt-level = 3` in the default
`--dev-opt optimized`, where dependents do inline its bodies.

**Statics.** As in the app crate, a patch defines its own copy of every
`static` and `thread_local!` of every crate it re-emits. A library's
world-lifetime caches (CrewForge's `TEAM_LOGO`) start empty in patched
code and fill again on first use; state that the app installed into a
library static at boot, once, is not there for the patched code. Keep
such installs inside a component, or expect the patched code to see the
static's initial value.

A body edit in a library costs more than one in the app on the FIRST
save of a session (two crates replay); see the costs in
[What a save costs on the web](#what-a-save-costs-on-the-web). Wire mode
(the native sidecar) still patches the app crate only; a library save
there respawns.

### One session per project

Run one `idealyst dev --web --local` session per project at a time. The
staged bundle, and the patches written into it, live in
`target/idealyst/<package name>/web` — keyed by the package NAME, not by
the project's path. Two sessions of one project overwrite each other's
bundle, and so does a copy of a project that kept its package name:
rename the package in the copy. The web target dir is keyed by the
project (above), so that part no longer collides.

Commands:

```sh
# the lab (a copy; any small app)
idealyst dev --web --local --port 8096

# CrewForge (full-stack; its own server serves the staged bundle)
cd crates/app-main
IDEALYST_MEMORY_LIMIT_MB=8192 RUSTUP_TOOLCHAIN=1.97.1-aarch64-apple-darwin \
  PATH=<wasm-bindgen 0.2.126>/bin:$PATH \
  idealyst dev --web --local --port 3150
```

### Rebuilding the tree without losing the page

Applying a patch changes nothing on screen — the DOM in front of the user
was built by the old bodies. So the page re-runs the app root, which is why
`backend_web::newcore::start_in_with` takes `impl Fn() -> Element` rather
than `FnOnce`.

State is carried by `runtime_world::hot_state`, and the order is not a
preference. Values are MOVED out of the tree's slots:

1. `harvest_owned()` while the tree is alive,
2. drop the TREE (`newcore::take_tree`), which frees every slot it owned,
3. `seed()` so each `signal()` call in the rebuild finds its predecessor's
   value, and mount into the SAME world.

Harvest after the drop finds nothing; seed before it seeds a run that has
not started.

**The world is kept.** On a page the world lives as long as the page, and
real apps rely on that: a `thread_local` holding a `Signal` (CrewForge has
several), a framework service published world-wide, the viewport signal
the `resize` listener writes into. The first version of this rebuild
dropped the world too, and the first render after a patch on CrewForge
panicked with "signal read after its World was dropped". So only the
tree is replaced, and only what the tree owned is harvested
(`harvest_owned`): a world-root signal outlives the tree and something may
still hold its handle.

Two consequences worth knowing:

- A signal created at the root level, outside any `#[component]`, is not
  carried. On the web the root has to be a component to be patchable at
  all, so app state lives in components anyway.
- A creation inside `runtime_world::unscoped` (a world-lifetime service)
  is not part of any component's state: it takes no carried value and
  does not count toward the position that matches a signal to its
  predecessor. Such a service is usually cached and not created again on
  the second run, and counting it shifted every later signal in the
  component onto its neighbour's value.

The native sidecar's `SessionMsg::Rerender` still drops its session's
world, with `harvest()`: a sidecar world is per session, and nothing
outlives a session there.

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

### The app root (native sidecar)

`fn app() -> Element` is not a `#[component]`, so it has no
`__*_hot_impl` symbol. The sidecar calls it through a fn pointer and the
host pairs it by ADDRESS instead: the sidecar reports the root's runtime
address on its `Hello` frame, the host subtracts the ASLR slide, and
looks up which symbol sits at that link-time address. Without this, an
app whose whole tree lives in `app()` would apply a patch that rebound
nothing. The web tier has no equivalent: there the root has to be a
component (see [The app root has to be a
`#[component]`](#the-app-root-has-to-be-a-component)).

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
- anything in a `static` or a `thread_local!` in a crate the patch
  re-emits (the app crate, and on the web any workspace library crate a
  save touched): the patch gets its own copy.

## Why a shape change cannot be patched

A patch dylib is spliced into a process where every other crate is still
the old build. Only the edited crates (and, for a workspace library, the
crates depending on it) are re-emitted. If a save changed a
props struct's fields, the patched code computes one layout while the
framework's own generic instantiations over that type — compiled into an
rlib that is never re-emitted — keep the other. The symptom is memory
corruption, not a stale render.

So a hot patch is attempted ONLY on an explicit body-only decision. A
missing or unreadable descriptor set means "cannot tell", and cannot-tell
respawns.

The archives the decision reads always describe what is RUNNING. After a
patch or a rebuild the dev loop installs archives scanned from the
sources as they were read just before that compile started (the scan
itself runs alongside the compile), so a save made while it compiled is
decided afterwards as the change it is; a failed rebuild leaves them
alone, so the next save is still decided against the build on screen.

## Escape hatches and diagnostics

- `IDEALYST_RUNTIME_SERVER_NO_HOTPATCH=1` forces every save through the
  respawn path. Useful for A/B timing and for isolating a suspected
  patch bug.
- `--split` on `dev --web --local` stands the web hot-patch tier down;
  body edits rebuild and reload.
- `IDEALYST_HOTPATCH_NO_SEED=1` skips the background replay that warms
  the replay cache after each base build (see
  [What a save costs on the web](#what-a-save-costs-on-the-web)).
- `IDEALYST_HOTPATCH_KEEP_NAMES=1` serves the web patch WITH its `name`
  section, so a stack trace through patched code is readable in the
  browser (see [What a save costs on the web](#what-a-save-costs-on-the-web)).
- `IDEALYST_MEMORY_LIMIT_MB=8192` raises the CLI's memory cap (default
  4096 MB), which a large app's base prep needs.
- `[hotpatch] …` lines from the web dev loop name each step of a patch
  build and, on failure, what it could not resolve; the rebuild it falls
  back to follows.
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

Off by default. Two things turn it on: `idealyst dev`'s generated
sidecar, and `idealyst dev --web --local` (the `hot-reload` framework
feature, which the `idealyst` facade forwards to the same crates plus
`backend-web`'s patch applier) unless `--split` is given. A production
build — `idealyst build` — carries none of it, and `runtime-macros`
emits the author's function verbatim.
