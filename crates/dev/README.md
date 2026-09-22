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
| `dev-overlay` | [`overlay/`](./overlay) | A build's descriptor set, and the patch-or-rebuild decision for a save. Its own crate because BOTH dev shapes need the decision and they share nothing else — the web watcher pulls the bundler, the runtime-server host pulls the wire protocol. |
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

## What a save does

A rebuild is seconds. Most saves during UI work change a label, a
number, a child order — data, not code — and those do not need one. So
before anything expensive starts, `dev-reload` asks whether this save is
a PATCH:

```
  save
   │
   ▼
  dev-reload ── read the changed files
   │
   ├─ skeleton changed?  (the file with `ui!` bodies blanked)  ──► rebuild
   ├─ a site moved / appeared / vanished?                      ──► rebuild
   ├─ does not parse?                                          ──► rebuild
   │
   ▼  diff each changed site against the build's archived descriptor set
      (target/idealyst/<app>/overlay/<build>.json)
   │
   ├─ runtime_template::diff refuses  ──────────────────────────► rebuild
   │
   ▼  Patch
   ├─ web --local (static):     dev-http's SSE `patch` event
   ├─ web --local (full-stack): the same SSE, from a stream running
   │                            ALONE on a CLI-owned port beside the
   │                            app's own server (absolute URL in the
   │                            staged index.html, CORS on the route)
   │        │
   │        ▼  the page's `__idealyst_overlay_patch` → stage + apply live
   │
   └─ runtime-server: host → `SidecarIn::OverlayPatch` → the sidecar
                      applies it to its OWN tree, so the resulting Host
                      calls reach every client as ordinary wire commands
                      and update the recorder's scene mirror (which is
                      what a late-joining client is snapshotted from)

  [dev] patched 1 site(s) in 18 ms, no rebuild
```

Measured on CrewForge (2050 `ui!` sites across 260 files): 18 ms for a
patched literal, against a ~5 s rebuild.

The decision is conservative and the asymmetry is the design. A wrong
rebuild costs seconds. A wrong patch means the screen and the source
disagree with nothing to say so, and every edit after it is reasoned
about against a program that is not there. So a file carrying both a
`ui!` edit and a logic edit rebuilds, and every refusal names its
reason:

```
[dev] rebuilding: src/screens/login.rs changed outside its `ui!` bodies
[dev] rebuilding: src/screens/auth/brand_lockup.rs: the site's compiled
      expressions changed (5 slots before, 6 after)
```

The full-stack stream is why a full-stack page now gets **livereload**
too: before this there was no push channel to it at all — dev-http's SSE
and its injected `EventSource` existed only on the static path, and a
full-stack project's own server hands out `index.html`.

See [docs/ui-layer.md](../../docs/ui-layer.md) for what patches and what
rebuilds, and `crates/dev/overlay/src/decide.rs` for the decision table
itself.
