//! Zero-copy Android display: the [`MediaStream`](crate::MediaStream)'s
//! `native_source` when a camera can push straight into a display's `Surface`.
//!
//! Apple and web hand a consumer a frame handle to show. Camera2 works the other
//! way round: a capture session pushes into consumer `Surface`s, and which ones a
//! frame reaches is decided by the session's repeating request. So this source
//! carries no frames. It is a control object: a display (the `video` SDK) claims
//! it, hands over its `Surface`, and frames go from the camera to that surface
//! without passing through the process's memory. A later GPU importer would ask
//! the same producer for an output of its own beside the display's.
//!
//! The stream's CPU tap is separate and on demand
//! ([`FrameWriter::on_cpu_demand_change`](crate::FrameWriter::on_cpu_demand_change)),
//! so a display holding the surface costs no RGBA conversion.
//!
//! ## One display at a time
//!
//! Only [`PreviewClaim`]'s holder can attach. A preview plus a second preview plus
//! the camera's CPU reader is not a stream combination a device has to support, so
//! a second display gets `None` from [`PreviewSource::try_claim`], shows the CPU
//! path, and takes the surface over when the first lets go.
//!
//! ## Refusal
//!
//! A device can refuse the combination of a preview and the CPU reader, or accept
//! it and deliver nothing. Either side calls refuse (the camera through
//! [`PreviewProducer::refuse`], a display that never saw a frame through
//! [`PreviewClaim::refuse`]) and the refusal is permanent for this source: displays
//! use the CPU path rather than show black.
//!
//! ## Threading
//!
//! The source and its claims live on the thread displaying them. The producer half
//! is `Send`, for the camera's callback thread. `PreviewDriver` calls come from the
//! display's thread and must not block on the camera.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// How a preview's buffers relate to what is upright on screen. Everything a
/// display needs to place them, and nothing about the camera beyond it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewGeometry {
    /// Buffer width in pixels, in the sensor's orientation. A display sets its
    /// `SurfaceTexture`'s default buffer size to this before making the `Surface`.
    pub width: u32,
    /// Buffer height in pixels, in the sensor's orientation.
    pub height: u32,
    /// `CameraCharacteristics.SENSOR_ORIENTATION`, in degrees. Camera2 rotates
    /// buffers by this on the way to a `SurfaceTexture`.
    pub sensor_orientation: i32,
    /// `LENS_FACING == FRONT`. Camera2 also mirrors such a camera's buffers on the
    /// way to a `SurfaceTexture`. The framework's frames are not mirrored, so a
    /// display undoes it.
    pub front_facing: bool,
}

/// The producer's half of [`PreviewClaim::attach`] and [`PreviewClaim::detach`]:
/// what makes frames reach a surface or stop.
pub trait PreviewDriver: Send + Sync + 'static {
    /// Start delivering frames into `surface`, in place of any previous one.
    ///
    /// # Safety
    /// `surface` is a JNI reference to an `android.view.Surface`, valid for this
    /// call only and on the calling thread only. Keep your own reference to it.
    unsafe fn attach(&self, surface: *mut c_void);

    /// Stop delivering frames to the attached surface. A display releases that
    /// surface shortly after this returns, so requests must stop naming it.
    fn detach(&self);
}

struct Shared {
    driver: Box<dyn PreviewDriver>,
    geometry: PreviewGeometry,
    claimed: AtomicBool,
    refused: AtomicBool,
}

/// The stream's [`native_source`](crate::MediaStream::native_source) on Android: a
/// display downcasts to this and claims it. Cheap to clone.
#[derive(Clone)]
pub struct PreviewSource {
    shared: Arc<Shared>,
}

/// The camera's half: lets it report that a device refused the preview.
#[derive(Clone)]
pub struct PreviewProducer {
    shared: Arc<Shared>,
}

/// Create a paired [`PreviewSource`] and [`PreviewProducer`] for a capture driven
/// through `driver`.
pub fn preview_channel(
    geometry: PreviewGeometry,
    driver: impl PreviewDriver,
) -> (PreviewSource, PreviewProducer) {
    let shared = Arc::new(Shared {
        driver: Box::new(driver),
        geometry,
        claimed: AtomicBool::new(false),
        refused: AtomicBool::new(false),
    });
    (PreviewSource { shared: shared.clone() }, PreviewProducer { shared })
}

impl PreviewSource {
    /// How this source's buffers are shaped and oriented.
    pub fn geometry(&self) -> PreviewGeometry {
        self.shared.geometry
    }

    /// Whether the preview was refused, for good: use the CPU path.
    pub fn is_refused(&self) -> bool {
        self.shared.refused.load(Ordering::Acquire)
    }

    /// Become the one display of this source. `None` while another display holds
    /// it (try again later) or once it is [refused](Self::is_refused).
    pub fn try_claim(&self) -> Option<PreviewClaim> {
        if self.is_refused() || self.shared.claimed.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some(PreviewClaim { shared: self.shared.clone(), attached: Cell::new(false) })
    }

    /// Whether `other` is a handle to this same source. Lets a display notice a
    /// swapped stream without holding a clone of it, which would keep capture on.
    pub fn is_same_source(&self, other: &PreviewSource) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl PreviewProducer {
    /// The device will not do this preview. Displays fall back to the CPU path.
    pub fn refuse(&self) {
        self.shared.refused.store(true, Ordering::Release);
    }
}

/// One display's hold on a [`PreviewSource`]. Dropping it detaches any surface and
/// frees the source for another display. Holds no reference to the stream, so a
/// claim does not keep capture running.
pub struct PreviewClaim {
    shared: Arc<Shared>,
    attached: Cell<bool>,
}

impl PreviewClaim {
    /// Hand over the surface to display in. Ignored once the source is refused.
    ///
    /// # Safety
    /// `surface` is a JNI reference to an `android.view.Surface`, valid for this
    /// call on this thread.
    pub unsafe fn attach(&self, surface: *mut c_void) {
        if self.shared.refused.load(Ordering::Acquire) {
            return;
        }
        // SAFETY: the caller's guarantee, passed on unchanged.
        unsafe { self.shared.driver.attach(surface) };
        self.attached.set(true);
    }

    /// Stop displaying in the surface, which is going away. Keeps the claim, so a
    /// new surface for the same view can attach. A no-op with nothing attached.
    pub fn detach(&self) {
        if self.attached.replace(false) {
            self.shared.driver.detach();
        }
    }

    /// This display waited and no frame came. Same effect as
    /// [`PreviewProducer::refuse`], for a device that accepted the session but
    /// delivers nothing.
    pub fn refuse(&self) {
        self.shared.refused.store(true, Ordering::Release);
        self.detach();
    }
}

impl Drop for PreviewClaim {
    fn drop(&mut self) {
        self.detach();
        self.shared.claimed.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MediaStream;
    use std::rc::Rc;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct Recording(Arc<Mutex<Vec<&'static str>>>);

    impl Recording {
        fn take(&self) -> Vec<&'static str> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl PreviewDriver for Recording {
        unsafe fn attach(&self, _surface: *mut c_void) {
            self.0.lock().unwrap().push("attach");
        }
        fn detach(&self) {
            self.0.lock().unwrap().push("detach");
        }
    }

    const GEOMETRY: PreviewGeometry = PreviewGeometry {
        width: 1920,
        height: 1080,
        sensor_orientation: 270,
        front_facing: true,
    };

    fn source() -> (PreviewSource, PreviewProducer, Recording) {
        let driver = Recording::default();
        let (source, producer) = preview_channel(GEOMETRY, driver.clone());
        (source, producer, driver)
    }

    /// Any non-null stand-in; the recording driver never dereferences it.
    fn surface() -> *mut c_void {
        std::ptr::dangling_mut::<u8>().cast()
    }

    #[test]
    fn a_second_display_gets_no_claim_until_the_first_lets_go() {
        let (source, _producer, _driver) = source();
        let first = source.try_claim().expect("free");
        assert!(source.try_claim().is_none(), "held");
        drop(first);
        assert!(source.try_claim().is_some(), "free again");
    }

    #[test]
    fn attach_and_detach_reach_the_driver_once_each() {
        let (source, _producer, driver) = source();
        let claim = source.try_claim().unwrap();
        claim.detach();
        assert!(driver.take().is_empty(), "nothing attached, nothing to detach");
        unsafe { claim.attach(surface()) };
        assert_eq!(driver.take(), ["attach"]);
        claim.detach();
        claim.detach();
        assert_eq!(driver.take(), ["detach"], "a repeated detach is one detach");
    }

    #[test]
    fn dropping_a_claim_detaches_its_surface() {
        let (source, _producer, driver) = source();
        let claim = source.try_claim().unwrap();
        unsafe { claim.attach(surface()) };
        driver.take();
        drop(claim);
        assert_eq!(driver.take(), ["detach"]);
    }

    /// A view's surface can go and come back while the view stays mounted.
    #[test]
    fn a_claim_survives_its_surface_going_away() {
        let (source, _producer, driver) = source();
        let claim = source.try_claim().unwrap();
        unsafe { claim.attach(surface()) };
        claim.detach();
        unsafe { claim.attach(surface()) };
        assert_eq!(driver.take(), ["attach", "detach", "attach"]);
    }

    /// The device said no: a black preview is the one outcome ruled out.
    #[test]
    fn regression_a_refused_source_sends_displays_to_the_cpu_path() {
        let (source, producer, driver) = source();
        let held = source.try_claim().unwrap();
        producer.refuse();
        assert!(source.is_refused());
        unsafe { held.attach(surface()) };
        assert!(driver.take().is_empty(), "no attach to a refused source");
        drop(held);
        assert!(source.try_claim().is_none(), "refused for good");
    }

    /// The device said yes and then showed nothing.
    #[test]
    fn a_display_that_saw_no_frame_refuses_and_detaches() {
        let (source, _producer, driver) = source();
        let claim = source.try_claim().unwrap();
        unsafe { claim.attach(surface()) };
        driver.take();
        claim.refuse();
        assert_eq!(driver.take(), ["detach"]);
        assert!(source.is_refused());
    }

    #[test]
    fn sameness_is_identity() {
        let (a, _, _) = source();
        let (b, _, _) = source();
        assert!(a.is_same_source(&a.clone()));
        assert!(!a.is_same_source(&b));
        assert_eq!(a.geometry(), GEOMETRY);
    }

    /// `stream.native_source().downcast` is how `video` finds it; and a compositor
    /// asking for importable frames has to be told this holds none, or it binds a
    /// texture layer to nothing where the CPU path works.
    #[test]
    fn a_stream_carries_it_as_a_source_with_no_importable_frames() {
        let (stream, _writer) = MediaStream::new();
        assert!(!stream.has_importable_source(), "no source at all");
        let (preview, _producer, _driver) = source();
        stream.set_native_source(Rc::new(preview));
        let found = stream.native_source().expect("set");
        assert!(found.downcast_ref::<PreviewSource>().is_some());
        assert!(!stream.has_importable_source());
        stream.set_native_source(Rc::new(5_u8));
        assert!(stream.has_importable_source(), "any other source holds frames");
    }

    /// Capture stops with the last stream clone, whatever a display holds.
    #[test]
    fn a_claim_does_not_keep_the_stream_alive() {
        let (stream, _writer) = MediaStream::new();
        let stopped = Rc::new(Cell::new(false));
        let flag = stopped.clone();
        stream.attach_stopper(move || flag.set(true));
        let (preview, _producer, _driver) = source();
        stream.set_native_source(Rc::new(preview.clone()));
        let claim = preview.try_claim().unwrap();
        drop(stream);
        assert!(stopped.get(), "the claim holds the driver, not the stream");
        drop(claim);
    }
}
