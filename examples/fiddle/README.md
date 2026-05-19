# Fiddle

Interactive Rust playground for Idealyst snippets. One binary serves
the editor UI **and** runs the compilations — no separate backend, no
toolchain in the browser.

```
┌──────────────────┐        ┌───────────────────────────┐
│ editor (webapp/) │  HTTP  │  fiddle server (this bin) │
│  ── source ──── ─┼───────▶│  /compile  → wasm-pack    │
│  ◀── hash ───────┼────────│            ↳ template/    │
│  ── iframe ─────▶│  HTTP  │  /compiled/<hash>/        │
└──────────────────┘        └───────────────────────────┘
        ▲                                  │
        │      (iframe loads)              │
        └──────────────────────────────────┘
```

## One-time setup

```sh
cargo install wasm-pack       # the compile worker shells out to it
wasm-pack build examples/fiddle/webapp --target web --dev
```

The second line builds the editor UI itself (Idealyst-built; see
`webapp/src/lib.rs`). It produces `examples/fiddle/webapp/pkg/`,
which the server serves at `/pkg/...`.

## Run

```sh
cargo run -p fiddle
# → [fiddle] serving on http://0.0.0.0:8081
```

Open <http://127.0.0.1:8081>. Edit the source, hit **Run**, the
iframe reloads with the freshly-compiled snippet.

## How the round trip works

1. Editor `POST`s `{ "source": "..." }` to `/compile`.
2. Server hashes the source. If `compiled/<hash>/index.html` exists,
   responds immediately with `{ "hash": "..." }`.
3. On cache miss: writes the source into
   `template/src/snippet.rs`, runs `wasm-pack build template/`, copies
   the output to `compiled/<hash>/pkg/`, writes a tiny `index.html`
   shim that imports `./pkg/snippet.js`.
4. Editor flips the iframe `src` to `/compiled/<hash>/?t=<bust>`.
5. Iframe loads the snippet wasm, which mounts a `host-web` simulator
   on a canvas and renders the user's `pub fn app() -> Primitive`
   inside the iOS skin.

The cache key is `sha256(source)[:8]`, so re-running unchanged source
is basically instant.

## Writing a snippet

The user code lives inside a synthetic `mod snippet` wrapping. The
ambient prelude (injected by the compile worker) is:

```rust
use crate::__rt::*;
```

which re-exports the symbols most snippets reach for —
`framework_core::{view, button, text, signal!, ui!, ...}`, `idea_ui::{card, heading, ...}`,
and `std::rc::Rc`. Anything outside that prelude needs the usual
fully-qualified path. Snippets must define:

```rust
pub fn app() -> Primitive { /* ... */ }
```

## Caveats (v1)

- **Editor is single-line.** The framework's `TextInput` renders as
  `<input type="text">`. Paste works; pressing Enter doesn't insert
  a newline. A multi-line `TextArea` primitive is the next obvious
  thing to add.
- **One process, one user.** Compilation is serialized through a
  `Mutex` — the template's `target/` directory is shared, so
  parallel compiles would race.
- **Trusts the snippet.** Snippets are compiled inside the template
  crate's normal cargo invocation. `include_str!("/etc/passwd")` and
  similar build-time macros would read the host filesystem. Fine for
  a localhost dev fiddle; not fine if you ever expose this to the
  open internet.
- **wgpu navigator dispatchers** are still WIP in `render-wgpu` —
  a snippet whose `app()` uses a navigator (`DrawerNavigator`,
  `TabNavigator`, …) will render the initial screen but log
  "navigator push not yet wired" on transitions. Static layouts,
  buttons, switches, sliders, scroll views all work.

## Why the `[workspace]` markers in `template/` and `webapp/`

Each of those sub-crates carries an empty `[workspace]` section so
the parent workspace's cargo doesn't try to merge them in. The
template gets its own warm `target/` (per-compile rebuilds stay
incremental); the webapp's wasm-pack build doesn't trip over the
workspace's shared lockfile.
