# `dev/` — runtime-locality and the dev-mode path

The Runtime doesn't have to live in the same process as the Backend.
The `Host` seam + capability (`*Ops`) traits are fine-grained enough
that their calls can be *serialized as messages* instead of executed
in-process, sent over a wire, and replayed against a remote Backend.
That's what the crates here enable.

Two flavors of remote execution use the same machinery:

- **Hot reload** — author edits app code; the dev server re-evaluates
  the tree, diffs it against the previous version, and ships the
  minimal sequence of capability calls to the live device.
- **Runtime server** — the Runtime runs on the host machine; the
  device-side process is a thin replayer. Same architectural shape
  as Phoenix LiveView or Blazor Server, expressed against the
  capability traits rather than HTML/DOM.

| Crate | Path | Role |
| --- | --- | --- |
| `wire` | [`wire/`](./wire) | The wire protocol. Pure data — a `Command` enum and three id namespaces (nodes, handlers, styles). No runtime dep; usable by any consumer of the protocol. |
| `dev-hot` | [`hot/`](./hot) | Thin facade over [`subsecond`](https://docs.rs/subsecond): the `#[component]` call wrapper and `apply_patch` (jump-table install). Compiles out entirely when the `hot` feature is off. |
| `dev-client` | [`client/`](./client) | App-side replayer. Receives wire commands and applies them to the local backend's `Host` + capability surface. Bundled into the running app. |
| `dev-server` | [`server/`](./server) | The dev side. Its `WireRecordingBackend` implements `Host` + the capability traits with `Node = NodeId`, turning every call the realize pass makes into a `Command`. |
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
  │  realize pass      │                │   dev-client       │
  │       │            │   wire cmds    │   (replay into     │
  │       ▼            │  ───────────►  │    the backend)    │
  │  wire::Command     │   dev-http /   │       │            │
  │       │            │   AAS shell    │       ▼            │
  │       ▼            │                │  Host + caps Ops   │
  │  dev-server        │                │  (UIKit / Views /  │
  │  (WireRecording)   │                │   DOM / wgpu)      │
  └────────────────────┘                └────────────────────┘
```

`runtime-server-shell-native` is the device-side piece for the
runtime-server flavor; for hot reload over HTTP, `web-dev-host` or
the in-app `dev-client` listens directly.
