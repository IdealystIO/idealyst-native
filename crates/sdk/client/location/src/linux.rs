//! Linux (desktop) geolocation via **GeoClue2** over the D-Bus SYSTEM bus.
//!
//! GeoClue2 (`org.freedesktop.GeoClue2`) is the standard desktop location
//! provider — a system daemon that fuses WiFi/cell/GPS/GeoIP sources and hands
//! back fixes over D-Bus. It is *not* guaranteed to be present: on a headless
//! box, a minimal install, or a locked-down system it may be absent or
//! disabled. We map that cleanly onto the SDK's error model rather than
//! panicking (see [`map_zbus_error`]).
//!
//! ## Flow (both entry points)
//!
//! 1. `Manager.GetClient` → a private per-connection `Client` object path.
//! 2. Set the client's **`DesktopId`** property. GeoClue2 requires this before
//!    `Start` (it keys authorization + the on-screen agent prompt off it) and
//!    refuses clients that never set one — so this is not optional.
//! 3. Set `RequestedAccuracyLevel` (8 = `EXACT`) so GeoClue engages its most
//!    precise available source; it falls back to coarser sources on its own.
//! 4. Subscribe to the client's `LocationUpdated(o old, o new)` signal
//!    **before** `Start`, so a fast first fix can't race ahead of us.
//! 5. `Start`. Each `LocationUpdated` carries the object path of a
//!    `org.freedesktop.GeoClue2.Location` whose properties we read into a
//!    [`Position`] ([`read_location`]).
//!
//! - [`current_fix`] awaits the first `LocationUpdated` (bounded by
//!   [`FIX_TIMEOUT`]), reads it, then `Stop`s the client and tears down.
//! - [`start_watch`] runs the client on a dedicated worker thread, forwarding
//!   every `LocationUpdated` to the callback; the `WatchHandle`'s `Drop`
//!   signals the worker to `Stop` the client and joins it (RAII — no leak).
//!
//! ## Async driving
//!
//! `zbus`'s `Connection` runs an **internal executor thread** by default
//! (`ConnectionBuilder::internal_executor` defaults to `true`), so proxy
//! method calls / property reads / signal delivery self-drive their socket
//! I/O regardless of which executor polls our futures. That's why
//! [`current_fix`] (an `async fn` polled by whatever the app runs) and the
//! watch worker (a plain `async_io::block_on`) both work without a shared
//! runtime.
//!
//! ## Permission model
//!
//! Unlike iOS/Android there is no `permissions`-SDK-visible grant on Linux —
//! `permissions::request(LocationWhenInUse)` reports `Unsupported`, and
//! `crate::current` gates on `is_usable()` so it delegates here. GeoClue2 owns
//! authorization itself (its agent + the system location toggle); a denied or
//! disabled fix surfaces as a D-Bus `AccessDenied`, which we map to
//! [`LocationError::NotAuthorized`].
//!
//! VERIFICATION: the pure value mapping ([`read_location`]'s math via
//! [`build_position`]) and the D-Bus→[`LocationError`] classifier
//! ([`classify_dbus_error`]) are unit-tested. The live GeoClue path is
//! compile-checked and exercised behind an `#[ignore]`d integration test that
//! needs a running GeoClue2 daemon (see the crate's `tests/`), because a fix
//! depends on the host having location hardware/network + an authorizing
//! agent — not reproducible in CI.

use std::thread::JoinHandle;
use std::time::Duration;

use futures_util::future::{select, Either};
use futures_util::{pin_mut, StreamExt};
use zbus::zvariant::OwnedObjectPath;
use zbus::{proxy, Connection};

use crate::{BoxedCallback, LocationError, Position};

/// The `DesktopId` reported to GeoClue2. GeoClue keys authorization + its
/// agent prompt off this and rejects a `Start` from a client that never set
/// one, so it must be non-empty; the value need not match an installed
/// `.desktop` file for basic operation.
const DESKTOP_ID: &str = "org.idealyst.app";

/// GeoClue2 `AccuracyLevel::EXACT`. The enum is sparse
/// (NONE=0, COUNTRY=1, CITY=4, NEIGHBORHOOD=5, STREET=6, EXACT=8); we request
/// the most precise and let GeoClue fall back to coarser sources itself.
const ACCURACY_EXACT: u32 = 8;

/// How long [`current_fix`] waits for the first `LocationUpdated` before giving
/// up. GeoClue can be slow on a cold start (scanning WiFi / warming GPS);
/// mirrors the web backend's 30 s `getCurrentPosition` timeout.
const FIX_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// D-Bus proxies for the three GeoClue2 interfaces we touch. `zbus`'s `#[proxy]`
// generates the async method wrappers, property getters/setters, and the
// `receive_location_updated()` signal stream.
// ---------------------------------------------------------------------------

#[proxy(
    interface = "org.freedesktop.GeoClue2.Manager",
    default_service = "org.freedesktop.GeoClue2",
    default_path = "/org/freedesktop/GeoClue2/Manager"
)]
trait Manager {
    /// Hand back a fresh per-connection `Client` object path.
    fn get_client(&self) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(
    interface = "org.freedesktop.GeoClue2.Client",
    default_service = "org.freedesktop.GeoClue2"
)]
trait Client {
    /// Begin delivering `LocationUpdated` signals. Fails with `AccessDenied`
    /// when location is disabled system-wide or the agent denies us.
    fn start(&self) -> zbus::Result<()>;
    /// Stop delivery. Best-effort on teardown.
    fn stop(&self) -> zbus::Result<()>;

    /// Required before `Start` — GeoClue authorizes per desktop id.
    #[zbus(property)]
    fn set_desktop_id(&self, id: &str) -> zbus::Result<()>;

    /// Bias GeoClue toward a precise source (it falls back on its own).
    #[zbus(property)]
    fn set_requested_accuracy_level(&self, level: u32) -> zbus::Result<()>;

    /// Fires once per new fix; `new` is the `Location` object path to read.
    #[zbus(signal)]
    fn location_updated(&self, old: OwnedObjectPath, new: OwnedObjectPath)
        -> zbus::Result<()>;
}

#[proxy(
    interface = "org.freedesktop.GeoClue2.Location",
    default_service = "org.freedesktop.GeoClue2"
)]
trait GLocation {
    #[zbus(property)]
    fn latitude(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn longitude(&self) -> zbus::Result<f64>;
    /// Horizontal accuracy radius, metres.
    #[zbus(property)]
    fn accuracy(&self) -> zbus::Result<f64>;
    /// Metres above the ellipsoid, or a large-negative sentinel if unknown.
    #[zbus(property)]
    fn altitude(&self) -> zbus::Result<f64>;
    /// Ground speed m/s, or `-1` if unknown.
    #[zbus(property)]
    fn speed(&self) -> zbus::Result<f64>;
    /// Heading degrees from true north, or `-1` if unknown.
    #[zbus(property)]
    fn heading(&self) -> zbus::Result<f64>;
    /// `(seconds, microseconds)` since the Unix epoch.
    #[zbus(property)]
    fn timestamp(&self) -> zbus::Result<(u64, u64)>;
}

// ---------------------------------------------------------------------------
// Pure mapping helpers (unit-tested — no live bus).
// ---------------------------------------------------------------------------

/// GeoClue marks an unknown `Altitude` with a huge-negative sentinel
/// (`-DBL_MAX`, i.e. `f64::MIN`). Any value below this threshold is "no
/// altitude"; a genuine sub-sea-level altitude (e.g. the Dead Sea, ~ -430 m)
/// is nowhere near it.
const ALTITUDE_UNKNOWN_THRESHOLD: f64 = -1.0e308;

/// Assemble a [`Position`] from the raw GeoClue2 `Location` property values.
/// Split out from the D-Bus reads so the sentinel handling is unit-testable
/// without a bus.
///
/// - `altitude`: below [`ALTITUDE_UNKNOWN_THRESHOLD`] → `None`.
/// - `speed` / `heading`: GeoClue uses `-1` for "unknown" → `None`.
/// - `timestamp` `(secs, micros)` → milliseconds since the Unix epoch.
#[allow(clippy::too_many_arguments)]
fn build_position(
    latitude: f64,
    longitude: f64,
    accuracy_m: f64,
    altitude: f64,
    speed: f64,
    heading: f64,
    timestamp: (u64, u64),
) -> Position {
    let (secs, micros) = timestamp;
    Position {
        latitude,
        longitude,
        accuracy_m,
        altitude: (altitude > ALTITUDE_UNKNOWN_THRESHOLD).then_some(altitude),
        heading: (heading >= 0.0).then_some(heading),
        speed: (speed >= 0.0).then_some(speed),
        timestamp_ms: (secs as f64) * 1000.0 + (micros as f64) / 1000.0,
    }
}

/// Classify a D-Bus error (by its error *name* + detail message) into a
/// [`LocationError`]. Split from the `zbus::Error` matching so the routing —
/// the part that's genuinely ours — is unit-testable without constructing live
/// bus errors.
///
/// - Service genuinely absent (`ServiceUnknown` / `NameHasNoOwner`) → the one
///   honest `NotSupported`: GeoClue2 isn't installed / can't be activated.
/// - `AccessDenied` / `AuthFailed` → `NotAuthorized`: location disabled
///   system-wide, or the GeoClue agent denied us.
/// - Everything else (timeouts, no-fix, transient daemon errors) →
///   `Unavailable`, carrying the detail.
fn classify_dbus_error(error_name: &str, detail: &str) -> LocationError {
    match error_name {
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner" => LocationError::NotSupported,
        "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.AuthFailed" => LocationError::NotAuthorized,
        _ => {
            let msg = if detail.is_empty() {
                error_name.to_string()
            } else {
                format!("{error_name}: {detail}")
            };
            LocationError::Unavailable(msg)
        }
    }
}

/// Map a `zbus::Error` to a [`LocationError`], routing D-Bus method errors
/// through [`classify_dbus_error`].
fn map_zbus_error(err: zbus::Error) -> LocationError {
    match err {
        zbus::Error::MethodError(name, detail, _) => {
            classify_dbus_error(name.as_str(), detail.as_deref().unwrap_or(""))
        }
        // A well-known `org.freedesktop.DBus.Error.*` surfaced as an fdo error.
        zbus::Error::FDO(fdo) => match *fdo {
            zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_) => {
                LocationError::NotSupported
            }
            zbus::fdo::Error::AccessDenied(_) | zbus::fdo::Error::AuthFailed(_) => {
                LocationError::NotAuthorized
            }
            other => LocationError::Unavailable(other.to_string()),
        },
        // The interface/proxy couldn't be resolved — treat as "no GeoClue".
        zbus::Error::InterfaceNotFound => LocationError::NotSupported,
        // Bus unreachable, handshake failure, I/O, etc. — the service might be
        // fine; the transport isn't. Report as transient-unavailable, not a
        // hard NotSupported (that's reserved for a genuinely absent GeoClue).
        other => LocationError::Unavailable(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Client bring-up shared by one-shot + watch.
// ---------------------------------------------------------------------------

/// Connect to the system bus, `GetClient`, set `DesktopId` +
/// `RequestedAccuracyLevel`. Returns the live connection (kept alive so its
/// internal executor keeps running) and the configured client proxy — but does
/// NOT `Start`; callers subscribe to the signal first.
async fn make_client(
    conn: &Connection,
) -> Result<ClientProxy<'static>, LocationError> {
    let manager = ManagerProxy::new(conn).await.map_err(map_zbus_error)?;
    let client_path = manager.get_client().await.map_err(map_zbus_error)?;
    let client = ClientProxy::builder(conn)
        .path(client_path)
        .map_err(map_zbus_error)?
        .build()
        .await
        .map_err(map_zbus_error)?;

    // DesktopId is mandatory before Start (see module docs).
    client
        .set_desktop_id(DESKTOP_ID)
        .await
        .map_err(map_zbus_error)?;
    // Best-effort: an older GeoClue that rejects the property shouldn't sink
    // the whole request — accuracy is an optimization, not a requirement.
    let _ = client.set_requested_accuracy_level(ACCURACY_EXACT).await;

    Ok(client)
}

/// Read the `Location` object at `path` on `conn` into a [`Position`].
async fn read_location(
    conn: &Connection,
    path: &OwnedObjectPath,
) -> Result<Position, LocationError> {
    let loc = GLocationProxy::builder(conn)
        .path(path.clone())
        .map_err(map_zbus_error)?
        .build()
        .await
        .map_err(map_zbus_error)?;

    let latitude = loc.latitude().await.map_err(map_zbus_error)?;
    let longitude = loc.longitude().await.map_err(map_zbus_error)?;
    let accuracy_m = loc.accuracy().await.map_err(map_zbus_error)?;
    // Altitude/speed/heading/timestamp are always present on the interface but
    // carry GeoClue's sentinels for "unknown"; a read failure on any of them is
    // non-fatal — fall back to the sentinel so `build_position` maps it away.
    let altitude = loc.altitude().await.unwrap_or(f64::MIN);
    let speed = loc.speed().await.unwrap_or(-1.0);
    let heading = loc.heading().await.unwrap_or(-1.0);
    let timestamp = loc.timestamp().await.unwrap_or((0, 0));

    Ok(build_position(
        latitude, longitude, accuracy_m, altitude, speed, heading, timestamp,
    ))
}

// ---------------------------------------------------------------------------
// One-shot fix.
// ---------------------------------------------------------------------------

pub(crate) async fn current_fix() -> Result<Position, LocationError> {
    let conn = Connection::system().await.map_err(map_zbus_error)?;
    let client = make_client(&conn).await?;

    // Subscribe BEFORE Start so a quick first fix can't outrun us.
    let mut updates = client
        .receive_location_updated()
        .await
        .map_err(map_zbus_error)?;

    client.start().await.map_err(map_zbus_error)?;

    // Await the first fix, bounded by FIX_TIMEOUT. `async_io::Timer` rides
    // async-io's global reactor, so it fires regardless of the outer executor.
    let timeout = async_io::Timer::after(FIX_TIMEOUT);
    let next = updates.next();
    pin_mut!(next);

    let result = match select(next, timeout).await {
        Either::Left((Some(signal), _)) => match signal.args() {
            Ok(args) => read_location(&conn, args.new()).await,
            Err(e) => Err(map_zbus_error(e)),
        },
        Either::Left((None, _)) => Err(LocationError::Unavailable(
            "GeoClue2 location stream ended before a fix".into(),
        )),
        Either::Right(_) => Err(LocationError::Unavailable(
            "timed out waiting for a GeoClue2 location fix".into(),
        )),
    };

    // Best-effort stop; the client is per-connection and dies with `conn`
    // regardless, so a failed Stop can't leak the daemon-side hardware.
    let _ = client.stop().await;
    result
}

// ---------------------------------------------------------------------------
// Continuous watch.
//
// `start_watch` is synchronous (installer contract), but GeoClue is async and
// the client must stay alive for the watch's lifetime. We run the whole client
// on a dedicated worker thread driving its own `async_io::block_on`, and use a
// oneshot channel as the Drop→worker "please stop" signal. The worker owns the
// connection (and thus zbus's internal executor), so nothing leaks when the
// handle drops.
// ---------------------------------------------------------------------------

pub(crate) struct WatchHandle {
    /// Sending (or dropping) this tells the worker to Stop the client and exit.
    stop_tx: Option<futures_channel::oneshot::Sender<()>>,
    /// The worker thread; joined on drop so teardown is synchronous.
    worker: Option<JoinHandle<()>>,
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        // Dropping the sender resolves the worker's shutdown future; the worker
        // then Stops the client and returns.
        drop(self.stop_tx.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(crate) fn start_watch(callback: BoxedCallback) -> WatchHandle {
    let (stop_tx, stop_rx) = futures_channel::oneshot::channel::<()>();

    // The worker thread owns the callback exclusively (it's `Send` but not
    // `Sync`, so it can be moved to the thread but not shared) and calls it for
    // every fix.
    let worker = std::thread::Builder::new()
        .name("location-geoclue-watch".into())
        .spawn(move || {
            async_io::block_on(watch_loop(callback, stop_rx));
        })
        .ok();

    WatchHandle {
        stop_tx: Some(stop_tx),
        worker,
    }
}

/// The watch's async body: bring up the client, then forward every
/// `LocationUpdated` to `callback` until the shutdown signal fires (or the
/// stream ends). Errors during bring-up leave the callback simply never firing
/// — consistent with the SDK's best-effort `watch` contract (the grant /
/// GeoClue availability is the caller's concern; a hard failure here isn't
/// surfaced through the sync installer).
async fn watch_loop(
    callback: BoxedCallback,
    stop_rx: futures_channel::oneshot::Receiver<()>,
) {
    let Ok(conn) = Connection::system().await else {
        return;
    };
    let Ok(client) = make_client(&conn).await else {
        return;
    };
    let Ok(mut updates) = client.receive_location_updated().await else {
        return;
    };
    if client.start().await.is_err() {
        return;
    }

    // `stop_rx` is `Unpin`; polled repeatedly via `&mut` until it resolves
    // (sender sent, or — the usual path — dropped by `WatchHandle::drop`).
    let mut stop_rx = stop_rx;
    loop {
        let next = updates.next();
        pin_mut!(next);
        match select(next, &mut stop_rx).await {
            Either::Left((Some(signal), _)) => {
                if let Ok(args) = signal.args() {
                    // A transient read failure on one fix shouldn't tear the
                    // watch down — GeoClue keeps delivering; skip and continue.
                    if let Ok(pos) = read_location(&conn, args.new()).await {
                        callback(pos);
                    }
                }
            }
            // Stream ended, or shutdown requested — either way, stop.
            Either::Left((None, _)) | Either::Right(_) => break,
        }
    }

    let _ = client.stop().await;
}

// ---------------------------------------------------------------------------
// Unit tests — the pure logic that's ours, no live bus.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_position_maps_present_fields() {
        // secs=1_700_000_000, micros=500_000 → 1_700_000_000_500 ms.
        let p = build_position(
            37.7749,
            -122.4194,
            5.0,
            16.0,   // altitude present
            1.5,    // speed present
            90.0,   // heading present
            (1_700_000_000, 500_000),
        );
        assert_eq!(p.latitude, 37.7749);
        assert_eq!(p.longitude, -122.4194);
        assert_eq!(p.accuracy_m, 5.0);
        assert_eq!(p.altitude, Some(16.0));
        assert_eq!(p.speed, Some(1.5));
        assert_eq!(p.heading, Some(90.0));
        assert_eq!(p.timestamp_ms, 1_700_000_000_500.0);
    }

    #[test]
    fn build_position_altitude_sentinel_is_none() {
        // GeoClue's -DBL_MAX "unknown altitude" sentinel maps to None...
        let p = build_position(0.0, 0.0, 10.0, f64::MIN, -1.0, -1.0, (0, 0));
        assert_eq!(p.altitude, None);
        // ...while a genuine below-sea-level altitude survives.
        let dead_sea = build_position(31.5, 35.5, 10.0, -430.0, -1.0, -1.0, (0, 0));
        assert_eq!(dead_sea.altitude, Some(-430.0));
    }

    #[test]
    fn build_position_unknown_speed_and_heading_are_none() {
        // GeoClue uses -1 for unknown speed/heading.
        let p = build_position(0.0, 0.0, 10.0, f64::MIN, -1.0, -1.0, (0, 0));
        assert_eq!(p.speed, None);
        assert_eq!(p.heading, None);
        // Zero is a valid value (stationary / due-north), not "unknown".
        let stationary = build_position(0.0, 0.0, 10.0, f64::MIN, 0.0, 0.0, (0, 0));
        assert_eq!(stationary.speed, Some(0.0));
        assert_eq!(stationary.heading, Some(0.0));
    }

    #[test]
    fn absent_geoclue_service_maps_to_not_supported() {
        assert_eq!(
            classify_dbus_error("org.freedesktop.DBus.Error.ServiceUnknown", "no such name"),
            LocationError::NotSupported
        );
        assert_eq!(
            classify_dbus_error("org.freedesktop.DBus.Error.NameHasNoOwner", ""),
            LocationError::NotSupported
        );
    }

    #[test]
    fn denied_or_disabled_maps_to_not_authorized() {
        assert_eq!(
            classify_dbus_error(
                "org.freedesktop.DBus.Error.AccessDenied",
                "location disabled"
            ),
            LocationError::NotAuthorized
        );
        assert_eq!(
            classify_dbus_error("org.freedesktop.DBus.Error.AuthFailed", ""),
            LocationError::NotAuthorized
        );
    }

    #[test]
    fn other_errors_map_to_unavailable_with_detail() {
        match classify_dbus_error("org.freedesktop.DBus.Error.TimedOut", "no reply") {
            LocationError::Unavailable(msg) => {
                assert!(msg.contains("TimedOut"), "detail preserved: {msg}");
                assert!(msg.contains("no reply"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
        // Empty detail falls back to the bare error name.
        match classify_dbus_error("org.freedesktop.GeoClue2.Error.Whatever", "") {
            LocationError::Unavailable(msg) => {
                assert_eq!(msg, "org.freedesktop.GeoClue2.Error.Whatever")
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }
}
