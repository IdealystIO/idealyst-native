# `connectivity`

Cross-platform **network reachability**. Ask whether the device currently has
network access and over what transport, and subscribe to be told when that
changes. One small author API — `current()` for a synchronous snapshot and
`watch(cb)` for a change subscription — maps to each platform's native
reachability monitor.

It reports a coarse reachability *category*, not a precise link descriptor:
online/offline plus a transport bucket (wifi / cellular / ethernet / other).

```rust
use connectivity::{current, watch, Transport};

// Synchronous best-effort snapshot.
let net = current();
if !net.online {
    // skip the request, show an offline banner, etc.
}
let _ = net.transport == Transport::Cellular; // e.g. defer a large download

// Subscribe to changes. Keep `sub` alive to keep receiving them; drop it to
// stop. Don't mem::forget it — store it in your component/app state.
let sub = watch(|net| {
    println!("changed: online={} via {:?}", net.online, net.transport);
});
// ... later: drop(sub) to unregister.
```

## What you get

- `Connectivity { online: bool, transport: Transport }` — a snapshot. The
  pair is always consistent: offline is `{ online: false, transport: None }`;
  online carries a non-`None` transport (`Other` when the medium is unknown).
- `Transport` — `Wifi | Cellular | Ethernet | Other | None`.
- `current() -> Connectivity` — a synchronous, best-effort snapshot.
  Platforms with no cheap synchronous query return
  `{ online: true, transport: Other }` (`Connectivity::ASSUME_ONLINE`) rather
  than blocking — assuming reachable is the safe default for a "should I
  attempt the request?" check.
- `watch(cb) -> ConnectivitySubscription` — registers `cb`, fired with a fresh
  snapshot on every change. The returned `ConnectivitySubscription` is an RAII
  guard that unregisters (and tears down the native monitor) on drop. It does
  *not* fire immediately with the current state — call `current()` to seed
  initial UI, then let `watch` deliver changes.

Every backend delivers the **same shape** — the platforms diverge in
mechanism, not in the API you call.

## Per-platform mechanism

| Target | Mechanism | Status |
| --- | --- | --- |
| web (wasm32) | `navigator.onLine` + `online`/`offline` window events; `navigator.connection` (NetworkInformation) for the transport hint where present | runnable on web |
| iOS / macOS / tvOS | `NWPathMonitor` (Network framework C API): `currentPath.status` for online-ness, `usesInterfaceType:` for the transport; the path-update block bridges to `watch`; the monitor lives in the subscription and is `cancel`led on drop | compile-checked only ⚠️ |
| Android | `ConnectivityManager`: `getActiveNetwork` + `getNetworkCapabilities` (`NET_CAPABILITY_VALIDATED`, `hasTransport(TRANSPORT_*)`) for `current()` | compile-checked only ⚠️ |
| other native (Windows / Linux / tests) | no platform monitor — `current()` returns the best-effort `ASSUME_ONLINE` snapshot; `watch` never fires | inert |

### Android `watch` seam

`current()` is fully implemented on Android. `watch` is structured around
`registerDefaultNetworkCallback`, but that requires a
`ConnectivityManager.NetworkCallback` **Java subclass** to receive change
events, and pure JNI cannot synthesize a new Java class at runtime. Delivering
change events therefore needs a small host-provided Java/Kotlin shim that
subclasses `NetworkCallback` and forwards each event back across JNI (the same
host-shim pattern the `camera` SDK uses). Until that shim is wired, Android
`watch` returns an inert subscription; a host that needs change notifications
today can poll `current()`. The public API is unchanged when the native driver
lands.

## Permissions

Only **Android** needs a permission: `ACCESS_NETWORK_STATE`. iOS / macOS / web
need none. This crate declares the capability `network_state` in its
`[package.metadata.idealyst]`; the CLI maps it to the Android manifest
permission and injects nothing on the other platforms.

## Scope

Reachability + coarse transport type only. Bandwidth estimation, the
metered / expensive-network flags (`NetworkCapabilities.NET_CAPABILITY_*` /
`NWPath.isExpensive`), and a reactive `Signal` binding are deliberately left
to higher-level layers rather than baked in here — this crate establishes the
raw capability via a plain snapshot + callback.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p connectivity` — snapshot self-consistency, the named constants, `watch` register+drop, fallback `ASSUME_ONLINE`
- [ ] `cargo build -p connectivity --features catalog` — recipes/docs compile
- [ ] `cargo build -p connectivity --target wasm32-unknown-unknown` — web target

**Behavior**
- [ ] **Web** — `current()` reports `online`/`transport` from `navigator.onLine`/`connection`; toggle the network (DevTools offline / real Wi-Fi off) and confirm `watch()` fires with the new `online` value.
- [ ] **iOS** — `current()` reflects the live `NWPathMonitor` state; toggle airplane mode / Wi-Fi and `watch()` fires with the new `online`/`transport`; the monitor is cancelled on drop.
- [ ] **macOS** — same `NWPathMonitor` flow: pull the ethernet/Wi-Fi and `watch()` reports the change.
- [ ] **Android** — `current()` is authoritative (`getActiveNetwork` + capabilities). `watch()` requires the host `NetworkCallback` Java-subclass shim forwarding events back across JNI — currently inert; poll `current()` until that shim is wired, then verify `watch()` fires.

**Permissions**
- [ ] Android only: confirm `ACCESS_NETWORK_STATE` (from the `network_state` capability) is in the merged manifest; iOS/macOS/web need none.
