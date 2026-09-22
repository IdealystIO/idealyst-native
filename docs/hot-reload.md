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

Both live tiers exist **only in runtime-server mode** — `idealyst dev`
without `--local`, which is the default. In `--local` mode the app is
compiled to wasm and runs in the browser; there is no process to patch,
so a body edit rebuilds and reloads the page.

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
