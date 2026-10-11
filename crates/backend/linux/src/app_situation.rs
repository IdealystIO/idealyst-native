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
