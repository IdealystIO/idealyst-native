# Fiddle

Write Idealyst code in the browser and see it run. The fiddle server builds
submitted code with **`idealyst build --web`** — the command a real app ships
with — so a preview is not an approximation of the code: it *is* the code,
compiled and running on the web backend.

It serves two front ends:

- **The course** (`websites/tutorial`): every lesson's editable example posts
  its `app.rs` here and frames the result.
- **The standalone editor** (`webapp/`): a multi-file project with a file
  tree, built the same way.

```
 editor (course or webapp/)                     fiddle server (this crate)
 ── POST /compile {files} ───────────────────▶  write files into .work/src/
                                                 idealyst build --web .work
 ◀── {status: built, url} | {failed, diagnostics}
 ── <iframe src=/compiled/<hash>/> ──────────▶  serve the bundle
```

## Run it

```sh
cargo build -p idealyst-cli          # a CLI built from this checkout
IDEALYST_BIN=target/debug/idealyst cargo run -p fiddle
# [fiddle] serving on http://127.0.0.1:8787
```

`IDEALYST_BIN` defaults to `idealyst` on `PATH`; point it at a CLI built from
the same checkout as the framework source the fiddle builds against.
`FIDDLE_ADDR` changes the listen address (the default port is 8787, clear of
the 8080 / 8081 that `idealyst dev` takes).

The standalone editor is an idealyst app too. Build it once, then open
<http://127.0.0.1:8787>:

```sh
target/debug/idealyst build --web examples/fiddle/webapp
```

## How a compile works

1. `POST /compile` with `{ "files": { "app.rs": "...", "widgets.rs": "..." } }`.
   `app.rs` is required and must define `pub fn app() -> Element`.
2. The server hashes the files together with a stamp of the framework source
   (`HEAD` plus uncommitted changes under `crates/`) and of the CLI binary. A
   bundle already built for that hash is answered at once.
3. Otherwise it writes `app.rs` to `.work/src/app.rs` and every other file to
   `.work/src/app/<path>` — where `mod widgets;` inside `app.rs` resolves, as
   in a scaffolded project — and runs
   `idealyst build --web --no-split --events-file … .work`.
4. On success the bundle moves to `compiled/<hash>/` (its `index.html`
   rewritten to load from there, with a small script that forwards runtime
   errors to the embedding page as `postMessage`). The reply is
   `{ "status": "built", "url": "/compiled/<hash>/", "ms", "cached", "diagnostics" }`.
5. On a compile error the reply is `{ "status": "failed", "diagnostics": [...] }`:
   rustc's messages for the submitted files, read from the build's
   `--events-file` as data, with their file, line and column in the
   submitted names.

`.work/` is a copy of `template/`, a real idealyst app (`src/main.rs` is
`idealyst::entry!`, `src/lib.rs` declares `mod app;`). Submitted code can use
what the template depends on: `runtime_core` and `idea_ui`. It renders the
builtin primitives and idea-ui; an SDK whose payload needs a registered
handler needs a line in the template's `register_scene_extensions`.

A rebuild after a one-file edit takes about a second on an M-series Mac: the
framework stays compiled in the CLI's per-workspace target directory, and
only the snippet crate rebuilds. Builds are serialized — there is one working
copy.

## Limits

- **It runs what it is sent.** Submitted code compiles with your user's
  permissions, and a proc macro or `include_str!` in it runs on the host.
  Serve it on localhost, or behind a sandbox.
- **Web only, for now.** The preview is the web backend. Native previews
  (the wgpu simulator, a device) are not wired in yet.
- `compiled/` keeps the 400 most recent bundles. `POST /clear-cache` empties it.
