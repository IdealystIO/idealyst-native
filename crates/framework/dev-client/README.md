# dev-client

The app-side replay engine. Wraps a real platform `Backend` and applies an
incoming stream of [`wire::Command`]s against it. This is the crate every
platform target imports to participate in hot-reload or AAS (server-driven
UI).

The dev-side counterpart is [`dev-server`](../../dev/server).

## What this crate does

- Owns the app-side `NodeId → B::Node` table; every `Command::Create*`
  inserts, every `Command::Destroy` removes.
- Owns the `StyleId → Rc<StyleRules>` table; `Command::RegisterStyle`
  inserts, `Command::ApplyStyle` looks up + forwards to
  `Backend::apply_style`.
- Owns the `HandlerId → Closure` table; when a `Command::Create*` carries
  callbacks, the replayer installs a thin closure that pushes
  `AppToDev::Event` onto the outbound channel.
- Dispatches every wire `Command` to one `Backend` trait call (or a small
  cluster) — see `imp/mod.rs` for the full mapping.

Re-exported as `AasClient` for consumers of the AAS (Application-as-a-Service) path:

```text
UI tree → AasBackend → Wire → AasClient<PlatformBackend> → Native
```

## What this crate is *not*

- **Not a transport.** This crate is platform-pure: protocol + replay only.
  Bytes-on-the-wire live in [`../../backend/aas-shell-native`](../../backend/aas-shell-native)
  for native targets (sync WebSocket + mDNS) and in `backend-web`'s
  `dev_transport` module for web (`web_sys::WebSocket` + rAF pump).
- **Not the dev side.** Components run in [`dev-server`](../../dev/server);
  this crate has no idea what the dev process is doing — it just applies
  commands.

## Catchup on connect (AAS mode)

For fresh connections, the dev side ships a `SceneModel` snapshot rather
than the full command log. The replay engine decodes the snapshot through
the same code path as a live command stream so the apply logic stays in
one place.

When you add a new wire command, you also need to add the corresponding
state to `SceneModel` — otherwise a reconnecting client won't see anything
that command set up. See `project_aas_state_snapshot` in memory.

## Graphics callbacks

GPU primitives (`Graphics`) carry `OnReady` / `OnResize` / `OnLost`
callbacks. Those can't be round-tripped over the wire — they have to run
synchronously on the device thread that owns the GPU surface. Instead, the
app registers concrete renderers locally and the wire carries an opaque
*name*; the replayer resolves the name through a `GraphicsRegistry` at
apply time.

This is also why AAS mode currently emits a *placeholder* for
`Primitive::Graphics` rather than a real surface — see
`project_aas_graphics_unsupported` in memory.
