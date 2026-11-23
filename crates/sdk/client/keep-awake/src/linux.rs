//! Linux: an idle inhibitor from the desktop, over the session bus.
//!
//! - `org.freedesktop.portal.Inhibit` first: the sandbox-friendly one, which
//!   GNOME and KDE both back.
//! - `org.freedesktop.ScreenSaver` where the portal has none.
//!
//! Neither (a bare window manager) means nothing to ask; `is_supported` says
//! so. The bus calls block, so one worker thread makes them, in order, and
//! keeps the connection: both services drop an inhibitor when the connection
//! that asked for it goes.

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, Value};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_INHIBIT: &str = "org.freedesktop.portal.Inhibit";
const PORTAL_REQUEST: &str = "org.freedesktop.portal.Request";
/// The portal's `Idle` inhibit flag.
const INHIBIT_IDLE: u32 = 8;
const SCREEN_SAVER: &str = "org.freedesktop.ScreenSaver";
const SCREEN_SAVER_PATH: &str = "/org/freedesktop/ScreenSaver";

/// How long `is_supported` waits for the desktop to answer. A portal that is
/// not running yet is started by the call.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// A live inhibitor.
enum Hold {
    Portal(OwnedObjectPath),
    ScreenSaver(u32),
}

pub(crate) fn set(on: bool) {
    static WORKER: OnceLock<Mutex<Sender<bool>>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<bool>();
        // The thread ends with the process: the sender is a static.
        let _ = std::thread::Builder::new().name("keep-awake".into()).spawn(move || {
            let mut bus: Option<Connection> = None;
            let mut held: Option<Hold> = None;
            while let Ok(on) = rx.recv() {
                // Only the latest state matters.
                let on = rx.try_iter().last().unwrap_or(on);
                if bus.is_none() {
                    bus = Connection::session().ok();
                }
                let Some(bus) = &bus else { continue };
                match (on, held.take()) {
                    (true, None) => held = inhibit(bus),
                    (false, Some(hold)) => release(bus, hold),
                    (_, kept) => held = kept,
                }
            }
        });
        Mutex::new(tx)
    });
    let _ = worker.lock().unwrap_or_else(|e| e.into_inner()).send(on);
}

fn inhibit(bus: &Connection) -> Option<Hold> {
    let labels = crate::keep_awake_labels();
    portal_inhibit(bus, &labels.reason)
        .map(Hold::Portal)
        .or_else(|_| screen_saver_inhibit(bus, &labels.reason).map(Hold::ScreenSaver))
        .ok()
}

fn portal_inhibit(bus: &Connection, reason: &str) -> zbus::Result<OwnedObjectPath> {
    let options = HashMap::from([("reason", Value::from(reason))]);
    // No parent window: the request has no dialog to place.
    Proxy::new(bus, PORTAL, PORTAL_PATH, PORTAL_INHIBIT)?.call("Inhibit", &("", INHIBIT_IDLE, options))
}

fn screen_saver_inhibit(bus: &Connection, reason: &str) -> zbus::Result<u32> {
    let app = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    Proxy::new(bus, SCREEN_SAVER, SCREEN_SAVER_PATH, SCREEN_SAVER)?.call("Inhibit", &(app, reason))
}

fn release(bus: &Connection, hold: Hold) {
    let _: zbus::Result<()> = match hold {
        Hold::Portal(request) => {
            Proxy::new(bus, PORTAL, request, PORTAL_REQUEST).and_then(|p| p.call("Close", &()))
        }
        Hold::ScreenSaver(cookie) => Proxy::new(bus, SCREEN_SAVER, SCREEN_SAVER_PATH, SCREEN_SAVER)
            .and_then(|p| p.call("UnInhibit", &(cookie,))),
    };
}

pub(crate) fn is_supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    if let Some(known) = SUPPORTED.get() {
        return *known;
    }
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("keep-awake-probe".into()).spawn(move || {
        let _ = tx.send(probe());
    });
    if spawned.is_err() {
        return false;
    }
    // An answer that came too late is not remembered: the next call asks
    // again.
    match rx.recv_timeout(PROBE_TIMEOUT) {
        Ok(found) => *SUPPORTED.get_or_init(|| found),
        Err(_) => false,
    }
}

fn probe() -> bool {
    let Ok(bus) = Connection::session() else { return false };
    // A portal without an inhibit backend does not export the interface, so
    // reading its version fails there.
    let portal = Proxy::new(&bus, PORTAL, PORTAL_PATH, PORTAL_INHIBIT)
        .and_then(|p| p.get_property::<u32>("version"))
        .is_ok();
    portal
        || Proxy::new(&bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus")
            .and_then(|p| p.call::<_, _, bool>("NameHasOwner", &(SCREEN_SAVER,)))
            .unwrap_or(false)
}
