# `location`

Cross-platform **device geolocation** — the device's raw position, as a
one-shot fix or a continuous stream of updates. One small API that maps to each
platform's native location stack and hands back a uniform `Position`
(lat/long + accuracy/altitude/heading/speed/timestamp).

It is deliberately the *raw capability*. Geocoding (address ↔ coordinate), map
rendering (the `maps` SDK), background tracking, geofencing, and reactive
`Signal` bindings are separate, higher-level layers — this crate just
establishes the position feed.

```rust
use location::{current, watch};

# async fn demo() -> Result<(), location::LocationError> {
// One fix (prompts for the location permission via the `permissions` SDK).
let here = current().await?;
println!("{}, {} (±{} m)", here.latitude, here.longitude, here.accuracy_m);

// Continuous updates — keep the guard alive while you want them.
let watcher = watch(|pos| {
    println!("moved to {}, {}", pos.latitude, pos.longitude);
});
// Dropping the guard stops updates and releases the native location manager.
drop(watcher);
# Ok(())
# }
```

## What you get

Two functions and one value type:

- `current() -> Result<Position, LocationError>` — async; requests
  `Permission::LocationWhenInUse` through the `permissions` SDK first
  (returns `LocationError::NotAuthorized` if denied), then reads a single fix.
- `watch(|pos| { .. }) -> LocationWatch` — continuous updates into the
  callback. `LocationWatch` is an **RAII guard**: it holds the native location
  manager and **stops updates on drop**. Hold it in your app state for as long
  as you want updates; never `mem::forget` it (that leaks the manager and
  leaves the location hardware running).
- `Position { latitude, longitude, accuracy_m, altitude, heading, speed,
  timestamp_ms }` — a plain `Copy` value. `latitude`/`longitude` are WGS-84
  degrees and `accuracy_m` (68 % confidence radius) is always present;
  `altitude`/`heading`/`speed` are `Option` (present only when the hardware
  supplied them on that fix).
- `LocationError::{ NotAuthorized, Unavailable(String), NotSupported }`.

The permission **grant** flows through the shared `permissions` SDK rather than
being re-implemented here; only the position-**data** APIs are called directly.
Every backend delivers the **same shape** — the platforms diverge in mechanism,
not in the functions you call.

## Per-platform mechanism

| Target | Mechanism |
| --- | --- |
| web (wasm32) | `navigator.geolocation` `getCurrentPosition` / `watchPosition` / `clearWatch` |
| iOS / macOS / tvOS | `CLLocationManager` + a `CLLocationManagerDelegate` (objc2): `requestLocation` for `current`, `startUpdatingLocation` for `watch`, `stopUpdatingLocation` on drop |
| Android | framework `LocationManager` via JNI: `getLastKnownLocation` for `current`; `requestLocationUpdates` for `watch` needs a Java `LocationListener` host shim (see below) |
| Windows / Linux / other native | unsupported — `current` returns `NotSupported`; `watch` installs nothing |

**Verification status.** The **web** path is genuinely runnable and is treated
as exercised. The **iOS / macOS / Android** native paths are **compile-checked
only** — exercising them needs a real device / simulator with the matching
usage-description / manifest entries and a granted runtime permission. Their
delegate-class / JNI structure mirrors the verified `permissions` /
`microphone` SDKs, with the lifetime and platform invariants documented inline.

**Android `watch` seam.** The one-shot `current()` (`getLastKnownLocation`) is
fully implemented over JNI. Continuous `watch()` needs
`requestLocationUpdates(provider, minTime, minDist, LocationListener)`, and
`LocationListener` is a Java *interface* — pure JNI can't implement it at
runtime without a host stub. So the Android host must supply a small
`LocationListener` that forwards `onLocationChanged` back into the SDK. Until
that shim lands, `watch` installs nothing and the callback never fires
(documented, not faked).

## Permissions

This crate declares the `location` capability
(`[package.metadata.idealyst] capabilities = ["location"]`). The CLI walks the
app's dependency graph, finds it, and injects the right artifacts per platform.
The app must declare the platform usage strings / manifest entries:

- **iOS** — `NSLocationWhenInUseUsageDescription` (and
  `NSLocationAlwaysAndWhenInUseUsageDescription` for always-on background).
- **macOS** — `NSLocationUsageDescription`.
- **Android** — `android.permission.ACCESS_FINE_LOCATION` +
  `android.permission.ACCESS_COARSE_LOCATION`.
- **web** — none; the browser prompts implicitly on the first
  `getCurrentPosition` / `watchPosition`.

The runtime grant itself is requested by `current()` through the `permissions`
SDK (`Permission::LocationWhenInUse`); you can also call
`permissions::request(Permission::LocationWhenInUse)` yourself before `watch()`,
which doesn't await a grant of its own.

## Scope

Raw position + updates — the unopinionated capability. Background location,
geofencing, geocoding, distance/region math, map rendering (`maps`), and
reactive `Signal` bindings are deliberately left to higher-level SDKs built on
top of this one rather than baked in here.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p location` — portable logic (`Position` value, error `Display`, `watch` install+drop on the host stub)
- [ ] `cargo build -p location --features catalog` — recipes/docs compile
- [ ] `cargo build -p location --target wasm32-unknown-unknown` — web target

**Behavior**
- [ ] **Web** — `current()` prompts the browser; returns a plausible lat/long; the demo updates as `watchPosition` fires; deny → `NotAuthorized` with no crash.
- [ ] **iOS** — the permission prompt shows the app's `NSLocationWhenInUseUsageDescription`; `current()` returns a plausible fix (`accuracy_m` set, optional fields where supplied); `watch()` streams updates and drops the `CLLocationManager` cleanly; deny → `NotAuthorized`.
- [ ] **macOS** — same flow against `CLLocationManager` with `NSLocationUsageDescription`.
- [ ] **Android** — `current()` (`getLastKnownLocation`) returns a fix after `ACCESS_FINE_LOCATION` is granted. `watch()` requires the host `LocationListener` shim forwarding `onLocationChanged` — currently inert (callback never fires); verify once that shim is wired.

**Permissions**
- [ ] `current()` requests `LocationWhenInUse` via the `permissions` SDK — confirm the prompt text matches the app's declared usage string, and a deny yields `NotAuthorized` (not a fake fix). For `watch()`, grant the permission first (it doesn't await its own grant).
