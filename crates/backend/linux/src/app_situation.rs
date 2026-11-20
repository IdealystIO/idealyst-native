//! Host facts about the app's situation (`runtime_shared::app_state`):
//! focus, visibility, the reduce-motion preference and the primary
//! pointer, observed on the GTK window and reported as they change.
//!
//! | Fact | Source |
//! |---|---|
//! | `Focused` | the window's `is-active` (same signal the keyboard / pointer sources use for focus loss) |
//! | `Visible` | `map` / `unmap` of the window, and the `GdkToplevel`'s `state` (`MINIMIZED`) |
//! | `ReduceMotion` | `GtkSettings:gtk-enable-animations` (off → reduce motion), followed live |
//! | `PrimaryPointer` | the default `GdkSeat`'s capabilities (pointer → Fine, touch only → Coarse, neither → None), re-read on `device-added` / `device-removed` |
//!
//! `UserActivated` is never reported: a native window has no autoplay
//! gate (the default `true` is the truth).
//!
//! The color scheme is reported through `runtime_shared::host::report_color_scheme`
//! rather than `app_state`: the desktop portal's
//! `org.freedesktop.appearance` / `color-scheme` setting
//! ([`read_color_scheme`], `org.freedesktop.portal.Settings.ReadOne`),
//! followed through its `SettingChanged` signal. GNOME, KDE and the other
//! portal-backed desktops all publish it; GTK's own
//! `gtk-application-prefer-dark-theme` is the APP's request, not the
//! user's preference. Without a portal (a bare X server, a container) the
//! scheme stays `Auto`.
//!
//! Before the window is mapped, `is-active` and the toplevel state say
//! nothing meaningful (an unmapped window is never active), so focus and
//! visibility are first reported at `map` rather than at boot — reporting
//! them at boot would flash `focused = false` at every app start.
//!
//! Every report from a signal runs [`crate::newcore::schedule_flush`] so
//! the per-world signals commit.

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;

use runtime_shared::app_state::{self, AppStateChange};

use crate::pointer_policy;

/// The installed observers, kept so [`AppSituation::uninstall`] can
/// disconnect every one.
pub struct AppSituation {
    handlers: Vec<(glib::Object, glib::SignalHandlerId)>,
    /// The toplevel-state handler lives on the window's SURFACE, which
    /// exists only while the window is realized; shared with the
    /// realize/unrealize handlers that (re)connect it.
    surface_handler: std::rc::Rc<std::cell::RefCell<Option<(glib::Object, glib::SignalHandlerId)>>>,
}

const PORTAL_BUS_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_SETTINGS: &str = "org.freedesktop.portal.Settings";
const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";
const COLOR_SCHEME_KEY: &str = "color-scheme";
/// How long the boot read waits for the portal. A running portal answers
/// in a few milliseconds and a missing one fails at once (the bus says
/// `ServiceUnknown`); this bounds only a portal that hangs, which would
/// otherwise stall the first frame.
const PORTAL_TIMEOUT_MS: i32 = 250;

/// The session bus's settings portal, without autostarting it (a desktop
/// that has one runs it already); `None` without a session bus.
fn settings_portal() -> Option<gtk4::gio::DBusProxy> {
    gtk4::gio::DBusProxy::for_bus_sync(
        gtk4::gio::BusType::Session,
        gtk4::gio::DBusProxyFlags::DO_NOT_AUTO_START | gtk4::gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
        None,
        PORTAL_BUS_NAME,
        PORTAL_PATH,
        PORTAL_SETTINGS,
        gtk4::gio::Cancellable::NONE,
    )
    .ok()
}

/// The `u32` inside a portal reply: `ReadOne` answers `(v)`, the older
/// `Read` `(v)` holding another `v`, so unwrap tuples and variants down
/// to the number.
fn portal_u32(value: &glib::Variant) -> Option<u32> {
    if let Some(n) = value.get::<u32>() {
        return Some(n);
    }
    if let Some(inner) = value.as_variant() {
        return portal_u32(&inner);
    }
    if value.is_container() && value.n_children() == 1 {
        return portal_u32(&value.child_value(0));
    }
    None
}

/// The desktop's light / dark preference, read from the settings portal.
pub fn read_color_scheme() -> runtime_shared::ColorScheme {
    let Some(portal) = settings_portal() else {
        return runtime_shared::ColorScheme::Auto;
    };
    let args = (APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY).to_variant();
    let reply = ["ReadOne", "Read"].into_iter().find_map(|method| {
        portal
            .call_sync(
                method,
                Some(&args),
                gtk4::gio::DBusCallFlags::NONE,
                PORTAL_TIMEOUT_MS,
                gtk4::gio::Cancellable::NONE,
            )
            .ok()
    });
    pointer_policy::color_scheme_from_portal(reply.as_ref().and_then(portal_u32))
}

fn report(change: AppStateChange) {
    app_state::report(change);
    crate::newcore::schedule_flush();
}

/// `true` when the window is mapped and its toplevel is not minimized.
fn is_visible(window: &gtk4::Window) -> bool {
    if !window.is_mapped() {
        return false;
    }
    let minimized = window
        .surface()
        .and_then(|s| s.downcast::<gdk::Toplevel>().ok())
        .is_some_and(|t| t.state().contains(gdk::ToplevelState::MINIMIZED));
    !minimized
}

fn seat_pointer_kind(display: &gdk::Display) -> runtime_shared::app_state::PointerKind {
    let caps = display
        .default_seat()
        .map(|s| s.capabilities())
        .unwrap_or(gdk::SeatCapabilities::empty());
    pointer_policy::pointer_kind(
        caps.intersects(gdk::SeatCapabilities::POINTER | gdk::SeatCapabilities::TABLET_STYLUS),
        caps.contains(gdk::SeatCapabilities::TOUCH),
    )
}

fn enable_animations() -> bool {
    gtk4::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
}

impl AppSituation {
    /// Connect every observer to `window` and return the facts readable
    /// right now. The caller reports those (with no backend borrow held —
    /// [`app_state::report`] runs world observers).
    pub fn install(window: &gtk4::Window) -> (Self, Vec<AppStateChange>) {
        let mut handlers: Vec<(glib::Object, glib::SignalHandlerId)> = Vec::new();
        let surface_handler = std::rc::Rc::new(std::cell::RefCell::new(None));

        // Focus.
        let h = window.connect_is_active_notify(|w| {
            if w.is_mapped() {
                report(AppStateChange::Focused(w.is_active()));
            }
        });
        handlers.push((window.clone().upcast(), h));

        // Visibility: map / unmap, plus minimize via the toplevel state.
        let h = window.connect_map(|w| {
            report(AppStateChange::Visible(is_visible(w)));
            report(AppStateChange::Focused(w.is_active()));
        });
        handlers.push((window.clone().upcast(), h));
        let h = window.connect_unmap(|_| report(AppStateChange::Visible(false)));
        handlers.push((window.clone().upcast(), h));

        let hook_surface = {
            let slot = surface_handler.clone();
            move |w: &gtk4::Window| {
                if let Some(old) = slot.borrow_mut().take() {
                    let (obj, id): (glib::Object, glib::SignalHandlerId) = old;
                    obj.disconnect(id);
                }
                let Some(toplevel) = w.surface().and_then(|s| s.downcast::<gdk::Toplevel>().ok()) else {
                    return;
                };
                let weak = w.downgrade();
                let id = toplevel.connect_state_notify(move |_| {
                    if let Some(w) = weak.upgrade() {
                        report(AppStateChange::Visible(is_visible(&w)));
                    }
                });
                *slot.borrow_mut() = Some((toplevel.upcast(), id));
            }
        };
        if window.is_realized() {
            hook_surface(window);
        }
        let h = window.connect_realize(hook_surface);
        handlers.push((window.clone().upcast(), h));
        let slot = surface_handler.clone();
        let h = window.connect_unrealize(move |_| {
            if let Some((obj, id)) = slot.borrow_mut().take() {
                obj.disconnect(id);
            }
        });
        handlers.push((window.clone().upcast(), h));

        // Reduce motion, followed live.
        if let Some(settings) = gtk4::Settings::default() {
            let h = settings.connect_gtk_enable_animations_notify(|s| {
                report(AppStateChange::ReduceMotion(pointer_policy::reduce_motion(
                    s.is_gtk_enable_animations(),
                )));
            });
            handlers.push((settings.upcast(), h));
        }

        // Color scheme, followed through the portal's `SettingChanged`
        // (namespace, key, value). The proxy lives in `handlers`, which
        // keeps the subscription for as long as the situation is installed.
        if let Some(portal) = settings_portal() {
            let h = portal.connect_g_signal(|_, _sender, signal, params| {
                if signal != "SettingChanged" || params.n_children() != 3 {
                    return;
                }
                let namespace = params.child_value(0);
                let key = params.child_value(1);
                if namespace.str() != Some(APPEARANCE_NAMESPACE) || key.str() != Some(COLOR_SCHEME_KEY) {
                    return;
                }
                let scheme = pointer_policy::color_scheme_from_portal(portal_u32(&params.child_value(2)));
                runtime_shared::backend::report_color_scheme(scheme);
                crate::newcore::schedule_flush();
            });
            handlers.push((portal.upcast(), h));
        }

        // Primary pointer, re-read when devices come and go.
        let display = WidgetExt::display(window);
        if let Some(seat) = display.default_seat() {
            for added in [true, false] {
                let d = display.clone();
                let cb = move |_: &gdk::Seat, _: &gdk::Device| {
                    report(AppStateChange::PrimaryPointer(seat_pointer_kind(&d)))
                };
                let h = if added { seat.connect_device_added(cb) } else { seat.connect_device_removed(cb) };
                handlers.push((seat.clone().upcast(), h));
            }
        }

        let mut boot = vec![
            AppStateChange::ReduceMotion(pointer_policy::reduce_motion(enable_animations())),
            AppStateChange::PrimaryPointer(seat_pointer_kind(&display)),
        ];
        if window.is_mapped() {
            boot.push(AppStateChange::Visible(is_visible(window)));
            boot.push(AppStateChange::Focused(window.is_active()));
        }
        (AppSituation { handlers, surface_handler }, boot)
    }

    /// Disconnect every observer.
    pub fn uninstall(self) {
        for (obj, id) in self.handlers {
            obj.disconnect(id);
        }
        if let Some((obj, id)) = self.surface_handler.borrow_mut().take() {
            obj.disconnect(id);
        }
    }
}
