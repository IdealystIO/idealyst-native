//! Bridge between the framework's reactive flush and winit's redraw
//! request.
//!
//! When a `Signal` flips, the framework wants to re-run the
//! associated `Effect`s, then re-draw the next frame. On native
//! desktop we satisfy "re-draw" by asking winit to fire a
//! `RedrawRequested` for our window, which the `App` event handler
//! services.
//!
//! Wired into the framework via [`install_redraw_handle`]. The
//! event-loop owner stores its `EventLoopProxy` here at startup;
//! the framework's effect-flush hook (or the backend itself, after
//! mutating its own state) calls [`request_redraw`] to wake the
//! loop.
//!
//! NOTE: winit's `EventLoopProxy` is `Send`, but we keep redraw
//! requests on the calling thread by funneling them through a
//! thread-local. A future cross-thread integration (audio thread,
//! network callback) would need to broaden this to a `Mutex` or
//! a channel.

use std::cell::RefCell;
use winit::event_loop::EventLoopProxy;

/// Custom event type the app event loop receives.
#[derive(Debug, Clone, Copy)]
pub enum AppEvent {
    Redraw,
}

thread_local! {
    static PROXY: RefCell<Option<EventLoopProxy<AppEvent>>> = const { RefCell::new(None) };
}

pub fn install_proxy(proxy: EventLoopProxy<AppEvent>) {
    PROXY.with(|cell| {
        *cell.borrow_mut() = Some(proxy);
    });
}

pub fn request_redraw() {
    PROXY.with(|cell| {
        if let Some(proxy) = cell.borrow().as_ref() {
            // send_event can fail only if the loop has exited; we
            // don't care in that case.
            let _ = proxy.send_event(AppEvent::Redraw);
        }
    });
}
