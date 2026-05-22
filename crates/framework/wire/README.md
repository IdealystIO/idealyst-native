# wire

The dev-mode wire protocol that connects a Rust dev process to a running app
on a device, simulator, or browser. Used by:

- **Hot reload** — the dev process re-runs the user's component tree on each
  edit and ships the resulting `Backend` calls as `Command`s to the app.
- **Server-driven UI (AAS, "Application-as-a-Server")** — the same protocol
  drives apps where the *production* UI runs on a dev-controlled server and
  the device is a thin replayer.

Both modes share this crate's data definitions; transport + replay live
elsewhere (see "Where transport / replay live" below).

## Design

The dev side runs the user's components in a normal Rust process. A
`WireRecordingBackend` (in `dev-server`) translates every `Backend` trait
call the render walker makes into a serializable [`Command`]. The app side
receives the command stream and a [`WireBackend<B>`] (in
[`../dev-client`](../dev-client)) replays each command against the real
platform backend (`WebBackend`, `IosBackend`, `AndroidBackend`, …).

The protocol is **pure data** — this crate has no `framework-core`
dependency. Conversion to/from in-memory framework types is the
caller's job (`dev-server` for the dev side, `dev-client` for the app side).

## ID namespaces

Three id namespaces are minted on the dev side and held opaquely on the app
side:

- **`NodeId`** — backend nodes (every `create_*` call mints one).
- **`HandlerId`** — closures (every primitive callback gets one). Most
  resolve back to dev-side closures via the reverse channel; GPU-bound
  callbacks resolve to app-local registered renderers.
- **`StyleId`** — pre-registered styles. The dev side ships the rule body
  once via `Command::RegisterStyle`; subsequent `Command::ApplyStyle`s
  reference by id.

Additional id namespaces (`StylesheetId`, `ScopeId`, `AssetId`, `TypefaceId`)
follow the same pattern.

## Versioning

`PROTOCOL_VERSION` is bumped on **any** breaking wire change. Dev and app
versions must match exactly — this is a dev-mode tool, so the protocol does
not pay for backward compatibility. Mismatched versions fail loudly at the
`DevToApp::Hello` exchange.

## Adding a new wire command

A new `Backend` trait method that needs to traverse the wire requires:

1. A new variant in `Command` (or the relevant child enum).
2. A matching encode on the dev side in `dev-server`'s `WireRecordingBackend`.
3. A matching decode + dispatch on the app side in `dev-client`'s
   `WireBackend`.
4. A bump of `PROTOCOL_VERSION`.
5. For AAS-mode catchup: a corresponding addition to `SceneModel` so fresh
   clients can be brought up to the current state without replaying the
   command log (see `project_aas_state_snapshot` in memory).

## Where transport / replay live

This crate is data-only. The pieces that move bytes:

- **App-side replay engine**: [`../dev-client`](../dev-client) — wraps any
  `framework_core::Backend` and feeds it `Command`s.
- **App-side native transport**: [`../../backend/aas-shell-native`](../../backend/aas-shell-native)
  — sync WebSocket + mDNS discovery. Used on iOS / Android / desktop.
- **App-side web transport**: `backend-web`'s `dev_transport` module —
  `web_sys::WebSocket` + rAF outbound pump.
- **Dev side**: [`../../dev/server`](../../dev/server) (`dev-server`) — runs
  the user's component tree, owns the `WireRecordingBackend`, manages all
  three id namespaces, ships commands over the chosen transport.
