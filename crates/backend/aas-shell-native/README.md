# aas-shell-native

Native shell for **AAS — Application-as-a-Service**, the framework's
server-driven-UI mode. Built from the sync blocking WebSocket transport
([`tungstenite`](https://crates.io/crates/tungstenite)), mDNS service
discovery ([`mdns-sd`](https://crates.io/crates/mdns-sd)), and the
worker-thread `AasShell` that ties them together.

In AAS mode, the production UI runs on a dev-controlled server and the
device is a thin replayer. The same transport powers hot-reload during
development. Together with [`framework/wire`](../../framework/wire) (the
protocol) and [`framework/dev-client`](../../framework/dev-client) (the
replay engine), this crate is what lets a native iOS / Android / desktop
app participate in either mode.

The web equivalent is `backend-web`'s `dev_transport` module
(`web_sys::WebSocket` + rAF outbound pump).

## Why this is under `crates/backend/` rather than `crates/framework/`

The framework layer declares the protocol (`wire`) and the replay engine
(`dev-client`); both are platform-pure. This crate is the *implementation*
of "drive that engine over a native socket" — concrete choices about which
WebSocket crate to use, which discovery mechanism, which threading model.
Those are platform decisions, so the crate lives next to the other
backends.

## Public surface

```rust
pub use aas_shell::AasShell;
pub use discover::{discover, discover_blocking, SERVICE_TYPE};
pub use transport::{connect_and_run, ClientError};
```

- **`AasShell`** — the worker-thread shell. Spawns a background thread that
  owns the WebSocket, pumps commands to the main thread, and pumps events
  back. Hosts call into `AasShell` from the main thread; the shell does the
  blocking I/O off-thread.
- **`discover` / `discover_blocking`** — mDNS service discovery. Hosts that
  want to advertise themselves on a dev network use the matching emitter
  on the dev side.
- **`connect_and_run`** — the lower-level entry point if you want to bring
  your own threading model.

## Feature gate

```toml
[features]
default = []
aas-shell = []
```

The crate compiles to *nothing* without `aas-shell` enabled. Production
builds opt out by default — the dev-only transport doesn't end up in
shipped binaries.
