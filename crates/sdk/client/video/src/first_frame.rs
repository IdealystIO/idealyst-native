//! [`FirstFrame`] — the shared bookkeeping behind
//! [`VideoProps::on_first_frame`](crate::VideoProps::on_first_frame).
//!
//! Every backend learns "a frame of the current source is now on screen" its
//! own way (a DOM `loadeddata`, an `AVSampleBufferDisplayLayer` enqueue, a
//! `TextureView` update, a swapchain present, …). What happens next is the
//! same everywhere and lives here, once:
//!
//! - **Once per source.** The backend reports the source it is populating
//!   ([`FirstFrame::track`]); a different URL or a different stream (pointer
//!   identity) re-arms the callback, so swapping the camera reports again.
//!   Repeated frames of the same source report nothing.
//! - **Deferred to the scheduler.** The callback runs from a 0 ms scheduler
//!   timer, never inside the platform callback or render pass that noticed the
//!   frame. Platform callbacks arrive outside the framework's dispatch, where a
//!   signal write the author makes would sit uncommitted; every backend
//!   flushes after a scheduler callback, so this is the one host-agnostic way
//!   to commit it (the canvas `SizeReporter` does the same).
//! - **Scoped.** The callback is skipped once the `Video`'s scope is torn
//!   down, and a pending call is cancelled when the source changes or the
//!   `Video` drops.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_shared::scheduling::{after_ms, ScheduledTask};
use runtime_vocabulary::glue::ScopeAlive;

use crate::{MediaContent, VideoProps};

/// The source a backend is populating, compared to spot a change. Holds a
/// clone of the stream, released on the first `track` after the source moves
/// on, so it never keeps a dropped source's capture running.
#[derive(PartialEq)]
enum Shown {
    Nothing,
    Url(String),
    Stream(media_stream::MediaStream),
}

impl Shown {
    fn of(content: &MediaContent) -> Self {
        match content {
            MediaContent::Url(u) if !u.is_empty() => Shown::Url(u.clone()),
            MediaContent::Stream(s) => Shown::Stream(s.clone()),
            _ => Shown::Nothing,
        }
    }
}

/// One mounted `Video`'s first-frame state. Build it in the mount handler (so
/// it takes the mount's scope), call [`track`](Self::track) wherever the
/// backend resolves the source, and [`frame_painted`](Self::frame_painted)
/// wherever it puts a frame on screen. Main thread only.
pub(crate) struct FirstFrame {
    callback: Option<Rc<dyn Fn()>>,
    alive: ScopeAlive,
    shown: RefCell<Shown>,
    painted: Cell<bool>,
    pending: RefCell<Option<ScheduledTask>>,
}

impl FirstFrame {
    pub(crate) fn new(props: &VideoProps) -> Rc<Self> {
        Rc::new(FirstFrame {
            callback: props.on_first_frame.clone(),
            alive: ScopeAlive::current(),
            shown: RefCell::new(Shown::Nothing),
            painted: Cell::new(false),
            pending: RefCell::new(None),
        })
    }

    /// Record the source the backend is populating now. Returns `true` when
    /// it differs from the last one: the next frame painted is a first frame
    /// again, and the backend should treat its surface as empty.
    pub(crate) fn track(&self, content: &MediaContent) -> bool {
        let next = Shown::of(content);
        if *self.shown.borrow() == next {
            return false;
        }
        *self.shown.borrow_mut() = next;
        self.painted.set(false);
        // A first frame reported for the old source must not land as the new
        // source's.
        self.pending.borrow_mut().take();
        true
    }

    /// A frame of the current source is on screen. The first one since the
    /// source was tracked schedules `on_first_frame`; the rest are no-ops, so
    /// a backend may call this on every frame.
    pub(crate) fn frame_painted(&self) {
        if self.painted.get() || *self.shown.borrow() == Shown::Nothing {
            return;
        }
        self.painted.set(true);
        let Some(callback) = self.callback.clone() else { return };
        let alive = self.alive.clone();
        let task = after_ms(0, move || {
            if alive.get() {
                callback();
            }
        });
        *self.pending.borrow_mut() = Some(task);
    }

    /// Whether a frame of the current source has been painted. The Apple
    /// backends poll a readiness flag until it is.
    #[cfg_attr(not(any(test, target_os = "ios", target_os = "macos")), allow(dead_code))]
    pub(crate) fn painted(&self) -> bool {
        self.painted.get()
    }
}

#[cfg(test)]
mod tests {
    //! Without an installed scheduler `after_ms` runs its callback
    //! synchronously on native, so a report is observable right after
    //! `frame_painted`.

    use super::*;

    fn counting() -> (Rc<FirstFrame>, Rc<Cell<u32>>) {
        let calls = Rc::new(Cell::new(0));
        let props = VideoProps {
            on_first_frame: Some(Rc::new({
                let calls = calls.clone();
                move || calls.set(calls.get() + 1)
            })),
            ..Default::default()
        };
        (FirstFrame::new(&props), calls)
    }

    #[test]
    fn reports_once_per_source() {
        let (first, calls) = counting();
        assert!(first.track(&MediaContent::Url("a.mp4".into())));
        first.frame_painted();
        first.frame_painted();
        assert_eq!(calls.get(), 1, "repeated frames of one source report once");
        assert!(first.painted());

        assert!(!first.track(&MediaContent::Url("a.mp4".into())), "same source");
        first.frame_painted();
        assert_eq!(calls.get(), 1);

        assert!(first.track(&MediaContent::Url("b.mp4".into())));
        assert!(!first.painted(), "a new source starts unpainted");
        first.frame_painted();
        assert_eq!(calls.get(), 2, "a new source reports again");
    }

    #[test]
    fn a_different_stream_is_a_new_source_and_a_clone_is_not() {
        let (first, calls) = counting();
        let (stream, _w) = media_stream::MediaStream::new();
        first.track(&MediaContent::Stream(stream.clone()));
        first.frame_painted();
        assert!(!first.track(&MediaContent::Stream(stream.clone())), "a clone is the same stream");
        let (other, _w2) = media_stream::MediaStream::new();
        assert!(first.track(&MediaContent::Stream(other)));
        first.frame_painted();
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn nothing_to_show_never_reports() {
        let (first, calls) = counting();
        first.frame_painted();
        first.track(&MediaContent::None);
        first.frame_painted();
        first.track(&MediaContent::Url(String::new()));
        first.frame_painted();
        assert_eq!(calls.get(), 0);
    }

    /// Releasing the stream matters on Android, where the last clone dropping
    /// is what turns the camera off.
    #[test]
    fn moving_off_a_stream_releases_its_clone() {
        let (first, _calls) = counting();
        let stopped = Rc::new(Cell::new(false));
        let (stream, _w) = media_stream::MediaStream::new();
        stream.attach_stopper({
            let stopped = stopped.clone();
            move || stopped.set(true)
        });
        first.track(&MediaContent::Stream(stream));
        assert!(!stopped.get(), "tracked: still the source");
        first.track(&MediaContent::None);
        assert!(stopped.get(), "the last clone went with the source");
    }

    #[test]
    fn no_callback_is_fine() {
        let first = FirstFrame::new(&VideoProps::default());
        first.track(&MediaContent::Url("a.mp4".into()));
        first.frame_painted();
        assert!(first.painted());
    }
}
