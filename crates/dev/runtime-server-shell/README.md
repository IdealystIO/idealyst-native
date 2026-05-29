# runtime-server-shell-native

Native shell for **runtime-server**, the framework's server-driven-UI mode.
Built from the sync blocking WebSocket transport
([`tungstenite`](https://crates.io/crates/tungstenite)) and the worker-
thread `RuntimeServerShell` that drives it.

In runtime-server mode, the production UI runs on a dev-controlled server
and the device is a thin replayer. The same transport powers hot-reload
during development. Together with [`crates/dev/wire`](../wire) (the
protocol) and [`crates/dev/client`](../client) (the replay engine), this
crate is what lets a native iOS / Android / desktop app participate in
either mode.

The web equivalent is `backend-web`'s `dev_transport` module
(`web_sys::WebSocket` + rAF outbound pump).

## Why this is under `crates/dev/` rather than `crates/runtime/`

The runtime layer declares the protocol (`wire`) and the replay engine
(`dev-client`); both are platform-pure. This crate is the *implementation*
of "drive that engine over a native socket": concrete choices about which
WebSocket crate to use, which threading model. Those are platform decisions,
so the crate lives next to the other dev-time tools.

## Discovery: there isn't any

The shell does not discover the dev-server itself. The CLI bakes the
endpoint URL into the wrapper build at `idealyst dev` time via the
`IDEALYST_DEV_ENDPOINT` env var and the wrapper passes the resolved
URL straight to [`RuntimeServerShell::spawn`]. Use
[`resolve_endpoint`] / [`endpoint_or_panic`] in your `main.rs`.

This replaced an earlier mDNS / Bonjour browse layer that was unreliable
across networks (corporate Wi-Fi, VPN, multicast filtering) and required
platform-specific permissions (iOS `NSBonjourServices`, Android
`MulticastLock`). Going through the CLI for endpoint resolution moves
those concerns out of the device build entirely.

## Public surface

```rust
pub use shell::{RuntimeServerShell, RuntimeServerShellOptions, ENDPOINT_ENV,
                resolve_endpoint, endpoint_or_panic};
pub use transport::{connect_and_run, ClientError};
```

- **`RuntimeServerShell`**: the worker-thread shell. Spawns a background
  thread that owns the WebSocket, pumps commands to the main thread, and
  pumps events back. Hosts call into it from the main thread; the shell
  does the blocking I/O off-thread.
- **`endpoint_or_panic` / `resolve_endpoint`**: read `IDEALYST_DEV_ENDPOINT`
  at runtime; panic / return `None` respectively when unset.
- **`connect_and_run`**: the lower-level entry point if you want to bring
  your own threading model.

## Feature gate

```toml
[features]
default = []
runtime-server = ["dep:tungstenite", "dev-client/dev-hot-reload"]
```

The crate compiles to *nothing* without `runtime-server` enabled.
Production builds opt out by default; the dev-only transport doesn't end
up in shipped binaries.
