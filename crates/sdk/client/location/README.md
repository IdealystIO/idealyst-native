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
  `timestamp_ms` is when the fix was made, which tells a fresh one from a
  remembered one.
- `LocationError::{ NotAuthorized, ServicesOff, Unavailable(String),
  NotSupported }`. `ServicesOff` is Location switched off for the whole
  device, told apart from "no fix yet" on Android, iOS and macOS.

The permission **grant** flows through the shared `permissions` SDK rather than
being re-implemented here; only the position-**data** APIs are called directly.
Every backend delivers the **same shape** — the platforms diverge in mechanism,
not in the functions you call.

## Per-platform mechanism

| Target | Mechanism |
| --- | --- |
| web (wasm32) | `navigator.geolocation` `getCurrentPosition` / `watchPosition` / `clearWatch` |
| iOS / macOS / tvOS | `CLLocationManager` + a `CLLocationManagerDelegate` (objc2): `requestLocation` for `current`, `startUpdatingLocation` for `watch`, `stopUpdatingLocation` on drop |
| Android | framework `LocationManager` (no Play Services) through a Kotlin `LocationListener` shim. `current` asks the fused provider (API 31+), or GPS and network together, for a fix made after the call; the rest are asked too after 10 s, and after 30 s the newest fix the device knows is the answer. `watch` streams at most one a second |
| Linux | GeoClue2 over the D-Bus system bus |
| Windows | WinRT `Geolocator`: `GetGeopositionAsync` (30 s bound) for `current`, `PositionChanged` for `watch` (thread-pool thread); authorization through `permissions` (Windows' location prompt) |
| other native | unsupported — `current` returns `NotSupported`; `watch` installs nothing |

**Verification status.** The **web** path is exercised by browser tests. The
Apple paths are **compile-checked only**. Android is exercised on an API 34
emulator with test providers (its GPS gave no reports): the checklist has the
cases. Below API 31, and real Wi-Fi location on a tablet without GPS, are not.

## Permissions

This crate declares the `location` capability
(`[package.metadata.idealyst] capabilities = ["location"]`). The CLI walks the
app's dependency graph, finds it, and injects the right artifacts per platform.
The app must declare the platform usage strings / manifest entries:

- **iOS** — `NSLocationWhenInUseUsageDescription`.
- **macOS** — `NSLocationUsageDescription`.
- **Android** — `android.permission.ACCESS_FINE_LOCATION` +
  `android.permission.ACCESS_COARSE_LOCATION`.
- **web** — none; the browser prompts implicitly on the first
  `getCurrentPosition` / `watchPosition`.

The runtime grant itself is requested by `current()` through the `permissions`
SDK (`Permission::LocationWhenInUse`); you can also call
`permissions::request(Permission::LocationWhenInUse)` yourself before `watch()`,
which doesn't await a grant of its own.

The user may grant approximate location only, on Android and iOS; `current()`
then returns a fix with a large `accuracy_m`.

### In the background

`location` declares foreground location only. An app that reads its position
in the background opts in, in its **own** `Cargo.toml`:

```toml
[package.metadata.idealyst]
capabilities = ["location_background"]

[package.metadata.idealyst.app.permissions]
location_background = "Find a lost tablet"
```

That adds Android's `ACCESS_BACKGROUND_LOCATION` and iOS's
`NSLocationAlwaysAndWhenInUseUsageDescription`, and
`permissions::request(Permission::LocationAlways)` asks for the grant.

On Android `current()` and `watch()` need no Activity. An Android device owner
(the `device-admin` SDK) needs nobody at the device either:

```rust,ignore
use device_admin::{permission, set_permission_grant_state, PermissionGrantState};

device_admin::set_location_enabled(true)?;
for p in [
    permission::ACCESS_FINE_LOCATION,
    permission::ACCESS_COARSE_LOCATION,
    permission::ACCESS_BACKGROUND_LOCATION,
] {
    set_permission_grant_state(MY_PACKAGE, p, PermissionGrantState::Granted)?;
}
// Keeps the process running. It carries no location access of its own: a fix
// in the background comes from the background grant above.
device_admin::start_keep_alive("Device management", "Connected")?;

let here = location::current().await?;
```

## Scope

Raw position + updates — the unopinionated capability. Background tracking
(schedules, batching), geofencing, geocoding, distance/region math, map rendering (`maps`), and
reactive `Signal` bindings are deliberately left to higher-level SDKs built on
top of this one rather than baked in here.

## Robot test controls

For E2E tests (`idealyst test`), a robot (dev) build can fake the position.
Register the controls once at boot — a no-op in release builds:

```rust
location::register_robot_controls();
```

| Function | Effect |
|---|---|
| `location.set {latitude, longitude, accuracy_m?, altitude?, heading?, speed?}` | `current()` resolves this fix at once (no permission prompt) and every live `watch` receives it; the platform's own updates are ignored until `location.clear`. `accuracy_m` defaults to 5 m. |
| `location.clear` | Back to the platform's real position. |

```rust
app.call_function("location.set", json!({ "latitude": 52.52, "longitude": 13.405 }));
app.test_id("city").assert_text("Berlin");
```

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p location` — portable logic (`Position` value, error `Display`, `watch` install+drop on the host stub)
- [ ] `cargo build -p location --features catalog` — recipes/docs compile
- [ ] `cargo build -p location --target wasm32-unknown-unknown` — web target
- [x] `cargo test -p location --target wasm32-unknown-unknown` (headless Chrome through the workspace runner) — `tests/web_geolocation.rs`: `current()` decodes a fix (nullable fields, NaN heading) and passes the high-accuracy options; position errors map to `NotAuthorized` / `Unavailable`; no geolocation → `NotSupported`; `watch` delivers positions and its guard clears the watch

**Behavior**
- [ ] **Web** — `current()` prompts the browser; returns a plausible lat/long; the demo updates as `watchPosition` fires; deny → `NotAuthorized` with no crash.
- [ ] **iOS** — the permission prompt shows the app's `NSLocationWhenInUseUsageDescription`; `current()` returns a plausible fix (`accuracy_m` set, optional fields where supplied); `watch()` streams updates and drops the `CLLocationManager` cleanly; deny → `NotAuthorized`.
- [ ] **macOS** — same flow against `CLLocationManager` with `NSLocationUsageDescription`.
- [x] **Android** (API 34 emulator, test providers) — `current()` returns a fix dated after the call; with the fused provider silent, one from the network provider after 10 s; with nothing fresh, the remembered one after 30 s; with Location off, `ServicesOff`. `watch()` streams and stops on drop. In the background (`kiosk-agent`'s **Locate** in `device-admin/examples`: device owner, another app in lock task, screen asleep): the same, with the agent's Activity destroyed too; with only the foreground grant, no fix.
- [ ] **Android hardware** — the background case on a tablet without GPS, on Wi-Fi.

**Permissions**
- [ ] `current()` requests `LocationWhenInUse` via the `permissions` SDK — confirm the prompt text matches the app's declared usage string, and a deny yields `NotAuthorized` (not a fake fix). For `watch()`, grant the permission first (it doesn't await its own grant).
