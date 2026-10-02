//! A fake camera, shaped like a real camera SDK would be for streamed
//! components:
//!
//! - the preview is a NATIVE view the bundle mounts by name (the app
//!   exports `CameraPreview`; frames never enter wasm);
//! - actions are `#[host_fn]`s: [`battery_level`] (sync) and
//!   [`take_photo`] (async — resolves after a shutter delay).
//!
//! The `Wire` impls below are hand-written; a `#[derive(Wire)]` is the
//! production answer once props and results carry real structs.

use stream_abi::Wire;
use stream_macros::host_fn;

#[derive(Debug, Clone, PartialEq)]
pub struct PhotoOptions {
    /// `"back"` or `"front"`. Anything else fails with `NoSuchCamera`.
    pub camera: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Photo {
    pub sequence: u32,
    pub width: u32,
    pub height: u32,
    pub camera: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CameraError {
    NoSuchCamera(String),
}

impl Wire for PhotoOptions {
    fn type_tag() -> String {
        "PhotoOptions".into()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        self.camera.encode(out);
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        Some(PhotoOptions { camera: String::decode(input)? })
    }
}

impl Wire for Photo {
    fn type_tag() -> String {
        "Photo".into()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        self.sequence.encode(out);
        self.width.encode(out);
        self.height.encode(out);
        self.camera.encode(out);
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        Some(Photo {
            sequence: u32::decode(input)?,
            width: u32::decode(input)?,
            height: u32::decode(input)?,
            camera: String::decode(input)?,
        })
    }
}

impl Wire for CameraError {
    fn type_tag() -> String {
        "CameraError".into()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            CameraError::NoSuchCamera(name) => {
                out.push(0);
                name.encode(out);
            }
        }
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        match u8::decode(input)? {
            0 => Some(CameraError::NoSuchCamera(String::decode(input)?)),
            _ => None,
        }
    }
}

/// Battery charge, 0.0–1.0. Sync: the bundle gets the value back inline.
/// The fake drains 1% per call so repeated calls are visibly live.
#[host_fn]
pub fn battery_level() -> f64 {
    imp::drain_battery()
}

/// Take a photo. Async: in a bridged bundle this returns a future
/// (`HostFuture`) for the framework's `spawn_then`, in a model A bundle a
/// `HostCall` for `stream_guest::spawn_then`; in the app it is an ordinary
/// future.
#[host_fn]
pub async fn take_photo(opts: PhotoOptions) -> Result<Photo, CameraError> {
    if opts.camera != "back" && opts.camera != "front" {
        return Err(CameraError::NoSuchCamera(opts.camera));
    }
    imp::Shutter::new(imp::SHUTTER_MS).await;
    Ok(Photo { sequence: imp::next_sequence(), width: 4032, height: 3024, camera: opts.camera })
}

/// App-side machinery behind the fake. Not compiled into bundles.
#[cfg(not(idealyst_stream_guest))]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};

    use runtime_shared::scheduling::{after_ms, ScheduledTask};

    /// Long enough to see "capturing…" in the demo.
    pub const SHUTTER_MS: i32 = 600;

    thread_local! {
        static BATTERY: Cell<f64> = const { Cell::new(0.87) };
        static SEQUENCE: Cell<u32> = const { Cell::new(0) };
    }

    pub fn drain_battery() -> f64 {
        BATTERY.with(|b| {
            let v = b.get();
            b.set((v - 0.01).max(0.0));
            v
        })
    }

    pub fn next_sequence() -> u32 {
        SEQUENCE.with(|s| {
            s.set(s.get() + 1);
            s.get()
        })
    }

    #[derive(Default)]
    struct State {
        fired: bool,
        waker: Option<Waker>,
    }

    /// Resolves after `ms` on the platform scheduler. Dropping it cancels
    /// the timer (the `ScheduledTask` it holds cancels on drop).
    pub struct Shutter {
        state: Rc<RefCell<State>>,
        timer: Option<ScheduledTask>,
        ms: i32,
    }

    impl Shutter {
        pub fn new(ms: i32) -> Self {
            Shutter { state: Rc::default(), timer: None, ms }
        }
    }

    impl Future for Shutter {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.state.borrow().fired {
                return Poll::Ready(());
            }
            self.state.borrow_mut().waker = Some(cx.waker().clone());
            // Armed on first poll, not at construction: a future that is
            // never polled never touches the scheduler.
            if self.timer.is_none() {
                let state = self.state.clone();
                self.timer = Some(after_ms(self.ms, move || {
                    let waker = {
                        let mut s = state.borrow_mut();
                        s.fired = true;
                        s.waker.take()
                    };
                    if let Some(w) = waker {
                        w.wake();
                    }
                }));
            }
            Poll::Pending
        }
    }
}
