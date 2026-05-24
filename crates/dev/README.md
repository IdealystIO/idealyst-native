# `dev/` — runtime-locality and the dev-mode path

The Runtime doesn't have to live in the same process as the Backend.
The `Backend` trait is fine-grained enough that its calls can be
*serialized as messages* instead of executed in-process, sent over a
wire, and replayed against a remote Backend. That's what the crates
here enable.

Two flavors of remote execution use the same machinery:

- **Hot reload** — author edits app code; the dev server re-evaluates
  the tree, diffs it against the previous version, and ships the
  minimal sequence of Backend calls to the live device.
- **Runtime server** — the Runtime runs on the host machine; the
  device-side process is a thin replayer. Same architectural shape
  as Phoenix LiveView or Blazor Server, expressed against the
  `Backend` trait rather than HTML/DOM.

| Crate | Path | Role |
| --- | --- | --- |
| `wire` | [`wire/`](./wire) | The wire protocol. Pure data — a `Command` enum and three id namespaces (nodes, handlers, styles). No `runtime-core` dep; usable by any consumer of the protocol. |
| `dev-hot` | [`hot/`](./hot) | Diff-and-patch for hot reload. Compares two `Primitive` trees by identity hash and emits the minimal wire command sequence. |
| `dev-client` | [`client/`](./client) | App-side replayer. Receives wire commands and applies them to the local Backend. Bundled into the running app. |
| `dev-server` | [`server/`](./server) | The dev server itself. Watches sources, drives recompiles, ships diffs over the wire. |
| `dev-reload` | [`reload/`](./reload) | The reload loop logic — what changes survive a patch, what forces a full rebuild. |
| `dev-http` | [`http/`](./http) | HTTP transport for the dev server (bundles, source maps, browser refresh signals). |
| `web-dev-host` | [`web-host/`](./web-host) | Browser-side host that bootstraps a web app under `idealyst dev`. |
| `runtime-server-shell-native` | [`runtime-server-shell/`](./runtime-server-shell) | The device-side shell that runs when an app is launched in runtime-server mode — it boots the Backend, opens the connection, and feeds incoming wire commands into `dev-client`. |

## How the pieces connect

```
       host machine                          target device
  ┌────────────────────┐                ┌────────────────────┐
  │  Author source     │                │                    │
  │       │            │                │                    │
  │       ▼            │                │                    │
  │  dev-hot           │                │   dev-client       │
  │  (diff trees)      │   wire cmds    │   (replay into     │
  │       │            │  ───────────►  │    Backend)        │
  │       ▼            │   dev-http /   │       │            │
  │  wire::Command     │   AAS shell    │       ▼            │
  │       │            │                │   Backend trait    │
  │       ▼            │                │  (UIKit / Views /  │
  │  dev-server        │                │   DOM / wgpu)      │
  └────────────────────┘                └────────────────────┘
```

`runtime-server-shell-native` is the device-side piece for the
runtime-server flavor; for hot reload over HTTP, `web-dev-host` or
the in-app `dev-client` listens directly.
