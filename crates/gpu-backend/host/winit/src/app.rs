//! winit `ApplicationHandler` shim + the public `run` entry.
//!
//! - Spins up the winit event loop + window + wgpu surface.
//! - Installs `render_wgpu::install_redraw_hook` so the
//!   core can wake the event loop when an animation or signal
//!   change needs another paint.
//! - Translates `winit::event::WindowEvent` values into the
//!   normalized `render_wgpu::input` event types and
//!   forwards them to the core's `Host`.
//! - Drives `RedrawRequested` through the core's `Renderer`.

use std::rc::Rc;
use std::sync::Arc;

use render_api::{
    DeviceProfile, Key, KeyEvent, KeyModifiers, PointerButton, PointerEvent, PointerId,
    ScrollEvent,
};
use render_wgpu::{install_redraw_hook, Host, Renderer, Painter};
use runtime_core::Element;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalSize};

// macOS-only: OS-level aspect-locked drag via a custom
// `NSWindowDelegate`. Its `windowWillResize:toSize:` callback
// fires for every proposed resize (interactive or programmatic)
// and returns an aspect-corrected size — AppKit then enforces
// it before painting the next frame, so drags track smoothly
// in both axes with no snap-back. All obj-c lives in this
// module, gated behind `cfg(target_os = "macos")`.
#[cfg(target_os = "macos")]
mod mac {
    use std::cell::Cell;

    use objc2::declare_class;
    use objc2::msg_send;
    use objc2::msg_send_id;
    use objc2::mutability;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::ClassType;
    use objc2::DeclaredClass;
    use objc2_app_kit::{NSWindow, NSWindowDelegate};
    use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    /// Conservative title-bar height for a standard macOS window
    /// with traffic-light controls. AppKit's actual chrome can
    /// vary by 1–2 pt across versions, but the content aspect
    /// stays within a fraction of a percent of the target —
    /// invisible at typical window sizes.
    const TITLE_BAR_HEIGHT: f64 = 28.0;

    pub struct AspectLockIvars {
        /// Target ratio (content width / content height).
        aspect: Cell<f64>,
    }

    declare_class!(
        /// `NSWindowDelegate` subclass that constrains
        /// `windowWillResize:toSize:` to the device's aspect.
        pub struct AspectLock;

        unsafe impl ClassType for AspectLock {
            type Super = NSObject;
            type Mutability = mutability::MainThreadOnly;
            const NAME: &'static str = "IdealystAspectLock";
        }

        impl DeclaredClass for AspectLock {
            type Ivars = AspectLockIvars;
        }

        unsafe impl NSObjectProtocol for AspectLock {}

        unsafe impl NSWindowDelegate for AspectLock {
            #[method(windowWillResize:toSize:)]
            unsafe fn window_will_resize(
                &self,
                _sender: &NSWindow,
                proposed_frame: NSSize,
            ) -> NSSize {
                let aspect = self.ivars().aspect.get();

                // `proposed_frame` is the *frame* size AppKit
                // wants us to take, including title bar. Work in
                // content coords; the title bar is fixed-height
                // chrome that doesn't participate in the lock.
                let pw = proposed_frame.width;
                let ph = (proposed_frame.height - TITLE_BAR_HEIGHT).max(1.0);

                // Project `(pw, ph)` onto the aspect line
                // `h = w / aspect`. The closest point on that
                // line is what gives a smooth, anchor-agnostic
                // resize — both axes shift together along the
                // line, so corner drags don't ping-pong between
                // "lock width" and "lock height" decisions.
                //
                // Line direction (un-normalized): (aspect, 1).
                // t = ((aspect, 1) · (pw, ph)) / ((aspect, 1) · (aspect, 1))
                //   = (aspect * pw + ph) / (aspect² + 1)
                // Closest point: (t * aspect, t).
                let denom = aspect * aspect + 1.0;
                let t = (aspect * pw + ph) / denom.max(f64::EPSILON);
                let content_w = (t * aspect).max(1.0);
                let content_h = t.max(1.0);

                NSSize::new(content_w, content_h + TITLE_BAR_HEIGHT)
            }

            // We override NSWindow's delegate to install the
            // aspect lock above, which means winit's own
            // delegate — the one that normally translates
            // `windowWillClose:` into `WindowEvent::CloseRequested`
            // — is no longer attached. Without an explicit close
            // handler here, clicking the red traffic-light just
            // hid the window and left the process running.
            // Forwarding to winit's delegate is the "proper" fix but would
            // require holding its `Retained` and `super`-style
            // chaining; for the single-window simulator this
            // direct hook is simpler and correct.
            #[method(windowWillClose:)]
            unsafe fn window_will_close(&self, _notification: &objc2_foundation::NSNotification) {
                // OS reclaims every thread (owner park) so no
                // manual cleanup is strictly required. Multi-window will need to
                // skip the exit and instead post a Rust-side
                // notification that decrements an active-window
                // counter, calling exit only on the last close.
                //
                // `_exit` rather than `std::process::exit`:
                // the latter runs Rust's thread-local destructors
                // before exiting, and one of them ends up
                // accessing a TLS slot that's already been torn
                // down, panicking with "cannot access a Thread
                // Local Storage value during or after
                // destruction". `_exit(2)` is the raw POSIX
                // syscall — it terminates the process
                // immediately, skipping every destructor and
                // atexit hook. The OS still reclaims threads,
                // memory, and file descriptors.
                extern "C" {
                    fn _exit(code: i32) -> !;
                }
                _exit(0);
            }
        }
    );

    impl AspectLock {
        fn new(mtm: MainThreadMarker, aspect: f64) -> Retained<Self> {
            let this = mtm.alloc::<Self>().set_ivars(AspectLockIvars {
                aspect: Cell::new(aspect),
            });
            // SAFETY: `init` is the no-arg designated initializer
            // of `NSObject`; our subclass uses the same.
            unsafe { msg_send_id![super(this), init] }
        }
    }

    fn ns_window_ptr(window: &Window) -> Option<*mut AnyObject> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::AppKit(h) = handle.as_raw() else {
            return None;
        };
        let ns_view = h.ns_view.as_ptr() as *mut AnyObject;
        if ns_view.is_null() {
            return None;
        }
        // SAFETY: ns_view is a live NSView for the duration of
        // the surrounding Window borrow; `-[NSView window]`
        // returns a borrowed NSWindow pointer that's also alive.
        let win: *mut AnyObject = unsafe { msg_send![ns_view, window] };
        if win.is_null() { None } else { Some(win) }
    }

    /// Install an aspect-lock delegate on `window`. The returned
    /// `Retained<AspectLock>` must be held by the caller for the
    /// lifetime of the window — NSWindow only weak-references
    /// its delegate, so dropping this would unhook the lock.
    pub fn install_aspect_lock(
        window: &Window,
        content_w: f64,
        content_h: f64,
    ) -> Option<Retained<AspectLock>> {
        let mtm = MainThreadMarker::new()?;
        let aspect = content_w / content_h.max(1.0);
        let delegate = AspectLock::new(mtm, aspect);

        let ns_window = ns_window_ptr(window)?;
        let proto: &ProtocolObject<dyn NSWindowDelegate> =
            ProtocolObject::from_ref(&*delegate);
        // SAFETY: `ns_window` is a live NSWindow; the protocol
        // object is a valid `id<NSWindowDelegate>` whose lifetime
        // is at least as long as the `Retained` we return.
        unsafe {
            let _: () = msg_send![ns_window, setDelegate: proto];
        }
        Some(delegate)
    }

    /// Cap the window's content area (in points = logical px) so
    /// it can't exceed the screen's visible work area. Works
    /// alongside the delegate — `setContentMaxSize:` bounds the
    /// rect, the delegate keeps it aspect-correct within those
    /// bounds.
    pub fn set_content_max(window: &Window, max_w: f64, max_h: f64) {
        let Some(ns_window) = ns_window_ptr(window) else { return };
        let size = NSSize::new(max_w, max_h);
        // SAFETY: see `install_aspect_lock`.
        unsafe {
            let _: () = msg_send![ns_window, setContentMaxSize: size];
        }
    }

    /// Aspect-correct content minimum, set alongside the
    /// delegate so the user can't drag a single axis below what
    /// the aspect lock can honor.
    pub fn set_content_min(window: &Window, min_w: f64, min_h: f64) {
        let Some(ns_window) = ns_window_ptr(window) else { return };
        let size = NSSize::new(min_w, min_h);
        // SAFETY: see `install_aspect_lock`.
        unsafe {
            let _: () = msg_send![ns_window, setContentMinSize: size];
        }
    }
}
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WKey, NamedKey};
use winit::window::{Window, WindowId};

use crate::gpu::Gpu;

#[derive(Debug)]
pub enum RunError {
    EventLoop(String),
    Render(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::EventLoop(s) => write!(f, "event loop: {s}"),
            RunError::Render(s) => write!(f, "render: {s}"),
        }
    }
}

impl std::error::Error for RunError {}

/// Custom event the redraw hook posts to wake the winit loop.
#[derive(Debug, Clone, Copy)]
pub(crate) enum AppEvent {
    Redraw,
    /// Posted by the scheduler worker thread whenever a registered
    /// `after_ms` deadline has expired or a 60 Hz raf pulse is due.
    /// The `user_event` handler calls
    /// [`crate::scheduler::drain_due`] to fire the matching
    /// closures on the main thread.
    SchedTick,
}

/// Run the preview window until the user closes it. `skin`
/// drives every per-frame widget + keyboard paint call.
/// Runtime-server variant of [`run`]. Sets up the winit event
/// loop + wgpu surface + Host the same way, but instead of
/// mounting a local `app()` it spawns a
/// `RuntimeServerShell<WgpuBackend>` against `host.backend()`
/// (shared `Rc<RefCell<>>`) so the shell's `apply_batch` writes
/// and the renderer's per-frame reads land on the same backend.
///
/// Wired into [`RedrawRequested`] so the shell ticks once per
/// frame — pulling inbound commands, sending `RequestFrame` to
/// drive the sidecar's animation clock, reporting the viewport
/// so the sidecar's `page_ref.frame()` reads track the actual
/// window size.
///
/// `url` is the dev-server WebSocket URL the CLI bakes into the
/// wrapper at `idealyst dev` time via `IDEALYST_DEV_ENDPOINT`.
#[cfg(feature = "runtime-server")]
pub fn run_runtime_server(
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    url: String,
) -> Result<(), RunError> {
    let event_loop: EventLoop<AppEvent> = EventLoop::with_user_event()
        .build()
        .map_err(|e| RunError::EventLoop(e.to_string()))?;
    let proxy = event_loop.create_proxy();
    install_redraw_hook(Box::new({
        let proxy = proxy.clone();
        move || {
            let _ = proxy.send_event(AppEvent::Redraw);
        }
    }));
    crate::scheduler::install(proxy);
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::new_runtime_server(profile, skin, url);
    event_loop
        .run_app(&mut app)
        .map_err(|e| RunError::EventLoop(e.to_string()))
}

pub fn run<F>(
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    build_ui: F,
) -> Result<(), RunError>
where
    F: FnOnce() -> Element + 'static,
{
    // No app-supplied extension handlers — the common case (a leaf-only
    // app, or one whose SDKs self-register via `inventory`).
    run_with(profile, skin, |_| {}, build_ui)
}

/// As [`run`], but invokes `register` on the wgpu backend before the app
/// tree mounts. Use this when the app depends on `Element::Navigator` /
/// `Element::External` SDKs whose handlers must be registered explicitly
/// (e.g. `drawer_navigator::chrome::register`, `table::register`) — the
/// generic-backend registrars that the AppKit/web hosts call from their
/// `register_extensions` glue. `register` receives a mutable borrow of
/// the `WgpuBackend` after it is built and before the first mount.
pub fn run_with<R, F>(
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    register: R,
    build_ui: F,
) -> Result<(), RunError>
where
    R: FnOnce(&mut render_wgpu::WgpuBackend) + 'static,
    F: FnOnce() -> Element + 'static,
{
    let event_loop: EventLoop<AppEvent> = EventLoop::with_user_event()
        .build()
        .map_err(|e| RunError::EventLoop(e.to_string()))?;
    // Install the core's redraw hook to point at our event loop.
    // Any `render_wgpu::request_redraw()` call from inside
    // `apply_style`, the animator, etc. now wakes us up.
    let proxy = event_loop.create_proxy();
    install_redraw_hook(Box::new({
        let proxy = proxy.clone();
        move || {
            let _ = proxy.send_event(AppEvent::Redraw);
        }
    }));
    // Install the native scheduler BEFORE we start running the
    // event loop (and therefore before `resumed` mounts the user
    // tree). The welcome page — and most non-trivial apps — fire
    // `runtime_core::after_ms` / `raf_loop` during their
    // `effect!` block; if the scheduler isn't installed by then,
    // those calls fall into the inert / synchronous fallbacks and
    // every author-driven animation freezes.
    crate::scheduler::install(proxy);
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::new(profile, skin, Box::new(register), Box::new(build_ui));
    event_loop
        .run_app(&mut app)
        .map_err(|e| RunError::EventLoop(e.to_string()))
}

/// Per-axis scale from physical surface px to content-logical
/// px. The macOS host pins the window's aspect ratio via the
/// NSWindowDelegate, so this is effectively a uniform scale
/// (with a sub-percent skew from the title-bar approximation);
/// on other platforms where the window isn't aspect-locked,
/// the axes can diverge and content stretches non-uniformly.
#[derive(Clone, Copy)]
struct ViewportScale {
    /// Content's logical (CSS-px) size — constant per session,
    /// taken from the device profile.
    logical: (f32, f32),
    /// Most recent physical surface size, in pixels.
    phys: (f32, f32),
}

impl ViewportScale {
    fn new(phys_surface: (u32, u32), logical: (f32, f32)) -> Self {
        Self {
            logical,
            phys: (phys_surface.0.max(1) as f32, phys_surface.1.max(1) as f32),
        }
    }

    /// Map physical pixel coords (winit's `PhysicalPosition`)
    /// directly to content-logical px. No bezel offset because
    /// content fills the full surface.
    fn physical_to_logical(&self, phys: (f32, f32)) -> (f32, f32) {
        (
            phys.0 * self.logical.0 / self.phys.0,
            phys.1 * self.logical.1 / self.phys.1,
        )
    }

    /// Full-surface viewport rect. Returned as
    /// `(x, y, w, h)` in physical px so it can be handed to
    /// `RenderPass::set_viewport` unchanged.
    fn surface_rect(&self) -> (f32, f32, f32, f32) {
        (0.0, 0.0, self.phys.0, self.phys.1)
    }
}

struct App {
    profile: DeviceProfile,
    /// Consumed on first `resumed`. None afterward. Mutually
    /// exclusive with `runtime_server_url`: local-mount mode
    /// supplies a `build_ui` closure, runtime-server mode supplies
    /// an app id and the shell is wired up post-resume.
    build_ui: Option<Box<dyn FnOnce() -> Element>>,
    /// Set in runtime-server mode. On `resumed()` we spawn a
    /// `RuntimeServerShell<WgpuBackend>` against `host.backend()`
    /// (the same `Rc<RefCell<>>` the renderer reads from) so the
    /// shell's `apply_batch` and the renderer's per-frame reads
    /// stay in sync. The shell tick is driven from
    /// `RedrawRequested` further down. None in local-mount mode.
    #[cfg(feature = "runtime-server")]
    runtime_server_url: Option<String>,
    #[cfg(feature = "runtime-server")]
    runtime_server_shell: Option<std::rc::Rc<runtime_server_shell_native::RuntimeServerShell<render_wgpu::WgpuBackend>>>,
    gpu: Option<Gpu>,
    renderer: Option<Renderer>,
    host: Host,
    /// Most recent physical→logical mapping for the surface.
    /// Refreshed on every `Resized` so the renderer's viewport
    /// and pointer translation always reflect the live window.
    viewport: ViewportScale,
    /// Cached modifier state. winit 0.30 delivers modifiers via a
    /// separate `ModifiersChanged` event, so we track them
    /// alongside the keyboard handler.
    modifiers: KeyModifiers,
    /// winit reports the pointer position via `CursorMoved` and
    /// the button state via `MouseInput` (positionless). Cache
    /// the last move so every `PointerEvent` we hand to the host
    /// has an authoritative position.
    last_pointer: (f32, f32),
    /// Most recent physical size we accepted, used to detect
    /// which dimension the user is dragging during a resize so
    /// the aspect-lock snaps along the *other* axis instead of
    /// fighting the drag.
    last_size: Option<PhysicalSize<u32>>,
    /// macOS-only: retained `NSWindowDelegate` that enforces
    /// the device-aspect ratio on every interactive resize. The
    /// retain has to live as long as the window — NSWindow holds
    /// its delegate weakly.
    #[cfg(target_os = "macos")]
    _aspect_lock: Option<objc2::rc::Retained<mac::AspectLock>>,
    /// Optional AccessKit bridge — only present when the `a11y`
    /// feature is enabled. Constructed inside `resumed` once the
    /// window exists (AccessKit's adapter needs the window's
    /// platform handles + a live `&ActiveEventLoop`). Synced after
    /// every frame so the platform AX layer sees the wgpu
    /// backend's parallel semantics tree.
    #[cfg(feature = "a11y")]
    a11y_bridge: Option<host_wgpu_accesskit::WgpuAccessKitBridge>,
}

impl App {
    fn new(
        profile: DeviceProfile,
        skin: Rc<dyn Painter>,
        register: Box<dyn FnOnce(&mut render_wgpu::WgpuBackend)>,
        build_ui: Box<dyn FnOnce() -> Element>,
    ) -> Self {
        let host = Host::new(skin, profile.color_scheme);
        // Register app-supplied External / Navigator handlers on the
        // freshly-built backend BEFORE the tree mounts in `resumed`.
        // `Host::new` constructs the `WgpuBackend` eagerly (no GPU device
        // needed — registration only touches the handler registries), so
        // the registry is populated by the time `build_ui` runs and
        // `Element::Navigator` / `Element::External` leaves resolve their
        // handler instead of hitting the "not registered" panic. This is
        // the wgpu equivalent of the per-backend `register_extensions`
        // call the AppKit/web hosts make before mount.
        register(&mut host.backend().borrow_mut());
        let logical = (profile.logical_size.0 as f32, profile.logical_size.1 as f32);
        // Seeded with the profile's logical size at 1×; the
        // actual surface size is plugged in inside `resumed`.
        let viewport = ViewportScale::new(profile.logical_size, logical);
        Self {
            profile,
            build_ui: Some(build_ui),
            gpu: None,
            renderer: None,
            host,
            viewport,
            modifiers: KeyModifiers::default(),
            last_pointer: (0.0, 0.0),
            last_size: None,
            #[cfg(feature = "runtime-server")]
            runtime_server_url: None,
            #[cfg(feature = "runtime-server")]
            runtime_server_shell: None,
            #[cfg(target_os = "macos")]
            _aspect_lock: None,
            #[cfg(feature = "a11y")]
            a11y_bridge: None,
        }
    }

    /// Runtime-server variant of [`Self::new`]. The host wraps a
    /// fresh `WgpuBackend`; we'll spawn a
    /// `RuntimeServerShell<WgpuBackend>` against `host.backend()`
    /// (shared `Rc<RefCell<>>`) inside `resumed()` so the shell's
    /// `apply_batch` writes and the renderer's per-frame reads
    /// land on the same backend instance.
    #[cfg(feature = "runtime-server")]
    fn new_runtime_server(
        profile: DeviceProfile,
        skin: Rc<dyn Painter>,
        url: String,
    ) -> Self {
        let host = Host::new(skin, profile.color_scheme);
        let logical = (profile.logical_size.0 as f32, profile.logical_size.1 as f32);
        let viewport = ViewportScale::new(profile.logical_size, logical);
        Self {
            profile,
            build_ui: None,
            gpu: None,
            renderer: None,
            host,
            viewport,
            modifiers: KeyModifiers::default(),
            last_pointer: (0.0, 0.0),
            last_size: None,
            runtime_server_url: Some(url),
            runtime_server_shell: None,
            #[cfg(target_os = "macos")]
            _aspect_lock: None,
            #[cfg(feature = "a11y")]
            a11y_bridge: None,
        }
    }

    /// Aspect-lock the window: when winit reports a resize that
    /// doesn't match the device profile's aspect, request the
    /// nearest correct size along whichever axis the user wasn't
    /// dragging. Returns `true` if a correction was issued (the
    /// caller should then wait for the following `Resized` event
    /// rather than treating the off-aspect size as the new
    /// truth).
    ///
    /// On macOS this is a no-op — NSWindow.contentAspectRatio
    /// already constrains the drag at the OS level, so no
    /// post-hoc correction is needed (and fighting it would
    /// cause exactly the jank this method tries to fix).
    #[cfg_attr(target_os = "macos", allow(unused_variables))]
    fn enforce_aspect(&mut self, size: PhysicalSize<u32>) -> bool {
        #[cfg(target_os = "macos")]
        {
            return false;
        }
        #[cfg(not(target_os = "macos"))]
        {
        const ASPECT_TOLERANCE: f32 = 0.005;
        let logical = (
            self.profile.logical_size.0 as f32,
            self.profile.logical_size.1 as f32,
        );
        let target_aspect = logical.0 / logical.1.max(1.0);
        let actual_aspect = size.width as f32 / size.height.max(1) as f32;
        if (actual_aspect - target_aspect).abs() <= ASPECT_TOLERANCE {
            return false;
        }
        // Pick the axis to preserve. If we have a previous size,
        // preserve the dimension the user changed the most — i.e.
        // the axis they were actively dragging. Otherwise default
        // to preserving width.
        let primary_is_width = match self.last_size {
            Some(prev) => {
                let dw = (size.width as i32 - prev.width as i32).abs();
                let dh = (size.height as i32 - prev.height as i32).abs();
                dw >= dh
            }
            None => true,
        };
        let (new_w, new_h) = if primary_is_width {
            let h = ((size.width as f32) / target_aspect).round() as u32;
            (size.width, h.max(1))
        } else {
            let w = ((size.height as f32) * target_aspect).round() as u32;
            (w.max(1), size.height)
        };
        if let Some(gpu) = self.gpu.as_ref() {
            let _ = gpu.window.request_inner_size(PhysicalSize::new(new_w, new_h));
        }
        true
        }
    }

    fn refresh_viewport(&mut self) {
        if let Some(gpu) = self.gpu.as_ref() {
            let logical = (
                self.profile.logical_size.0 as f32,
                self.profile.logical_size.1 as f32,
            );
            self.viewport =
                ViewportScale::new((gpu.config.width, gpu.config.height), logical);
        }
    }

    fn render_frame(&mut self) -> FrameOutcome {
        let Some(gpu) = self.gpu.as_mut() else { return FrameOutcome::Ok };
        let Some(renderer) = self.renderer.as_mut() else { return FrameOutcome::Ok };

        // wgpu 29: `get_current_texture` returns a `CurrentSurfaceTexture`
        // *enum* (was `Result<SurfaceTexture, SurfaceError>`). Map every
        // non-Success variant we care about to our own outcome so the
        // caller's match doesn't need to know wgpu internals.
        let surface_tex = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated
            | wgpu::CurrentSurfaceTexture::Lost => return FrameOutcome::Reconfigure,
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return FrameOutcome::Skip,
        };
        let view = surface_tex
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let logical = self.viewport.logical;
        renderer.render(
            &self.host,
            &gpu.device,
            &gpu.queue,
            &view,
            logical,
            self.viewport.surface_rect(),
        );
        surface_tex.present();

        // Project the wgpu backend's parallel a11y tree into the
        // platform AX layer. Cheap on no-change frames (the bridge
        // hashes the tree and skips identical pushes). Also drains
        // any pending live-region announcements (`announce_for_
        // accessibility` calls from the framework's effect handlers)
        // so screen readers speak them right after the visual frame
        // they correspond to.
        //
        // The bridge's `drain_announcements` API takes `&mut S` +
        // `&B` so the source and backend can be different objects.
        // When they're the same WgpuBackend instance, we'd need
        // simultaneous `&mut` and `&` to the same RefCell borrow,
        // which Rust rejects. Workaround: pre-drain into a local
        // `Vec`, wrap it in a tiny `AnnouncementSource` that emits
        // the pre-drained payload, then call the bridge with a
        // disjoint borrow.
        #[cfg(feature = "a11y")]
        if let Some(bridge) = self.a11y_bridge.as_mut() {
            use host_wgpu_accesskit::AnnouncementSource;
            use runtime_core::accessibility::LiveRegionPriority;

            struct PreDrained(Vec<(String, LiveRegionPriority)>);
            impl AnnouncementSource for PreDrained {
                fn drain(&mut self) -> Vec<(String, LiveRegionPriority)> {
                    std::mem::take(&mut self.0)
                }
            }

            let backend_rc = self.host.backend();
            // Mut-borrow: drain announcements into a local Vec.
            let pending = {
                let mut be = backend_rc.borrow_mut();
                be.drain_pending_announcements()
            };
            // Re-borrow as `&` for the tree dump + adapter push.
            let backend = backend_rc.borrow();
            bridge.sync(&*backend);
            if !pending.is_empty() {
                let mut source = PreDrained(pending);
                bridge.drain_announcements(&mut source, &*backend);
            }
        }

        // If any tween / drawer needs another frame, route the
        // wake-up through the event-loop proxy
        // (`render_wgpu::request_redraw()` → `AppEvent::Redraw`
        // → `user_event` → `window.request_redraw`). Calling
        // `gpu.window.request_redraw()` directly from inside
        // the `RedrawRequested` handler is silently coalesced
        // on macOS — a slow animation source would break the
        // chain and stall the redraw loop.
        if self.host.tick() {
            render_wgpu::request_redraw();
        }
        FrameOutcome::Ok
    }
}

/// Outcome of a single render-frame pump. wgpu 29 collapsed the
/// old `Result<SurfaceError>` into a per-call enum (`CurrentSurfaceTexture`);
/// we surface the subset the redraw loop needs to act on without
/// leaking wgpu types up to the event-handler arm.
enum FrameOutcome {
    /// Frame rendered (or no-op because gpu / renderer not initialized yet).
    Ok,
    /// Surface needs reconfigure (outdated / lost).
    Reconfigure,
    /// Frame skipped (timeout / occluded / validation). Drop & try again.
    Skip,
}

// ---------------------------------------------------------------------------
// winit → normalized event translation
// ---------------------------------------------------------------------------

fn winit_button_to_pointer(b: MouseButton) -> Option<PointerButton> {
    match b {
        MouseButton::Left => Some(PointerButton::Primary),
        MouseButton::Right => Some(PointerButton::Secondary),
        MouseButton::Middle => Some(PointerButton::Middle),
        MouseButton::Back => Some(PointerButton::Other(3)),
        MouseButton::Forward => Some(PointerButton::Other(4)),
        MouseButton::Other(n) => Some(PointerButton::Other(n)),
    }
}

fn winit_key(event: &winit::event::KeyEvent, modifiers: KeyModifiers) -> KeyEvent {
    let key = match &event.logical_key {
        WKey::Named(NamedKey::Backspace) => Key::Backspace,
        WKey::Named(NamedKey::Delete) => Key::Delete,
        WKey::Named(NamedKey::Enter) => Key::Enter,
        WKey::Named(NamedKey::Escape) => Key::Escape,
        WKey::Named(NamedKey::Tab) => Key::Tab,
        WKey::Named(NamedKey::ArrowLeft) => Key::ArrowLeft,
        WKey::Named(NamedKey::ArrowRight) => Key::ArrowRight,
        WKey::Named(NamedKey::ArrowUp) => Key::ArrowUp,
        WKey::Named(NamedKey::ArrowDown) => Key::ArrowDown,
        WKey::Named(NamedKey::Home) => Key::Home,
        WKey::Named(NamedKey::End) => Key::End,
        WKey::Character(_) => Key::Character,
        _ => Key::Unknown,
    };
    KeyEvent {
        key,
        text: event.text.as_ref().map(|s| s.to_string()),
        modifiers,
        pressed: event.state.is_pressed(),
    }
}

// ---------------------------------------------------------------------------
// ApplicationHandler
// ---------------------------------------------------------------------------

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title(self.profile.title.clone())
            .with_inner_size(LogicalSize::new(
                self.profile.logical_size.0 as f64,
                self.profile.logical_size.1 as f64,
            ))
            // Intentionally no `with_min_inner_size` here. winit
            // installs that min in non-aspect-correct shape and
            // macOS, when forced to honor both a free-shape min
            // and a content aspect ratio, lets one axis drift
            // off-aspect to satisfy both constraints — which
            // breaks vertical drags. The macOS branch below
            // installs an aspect-correct content-min directly.
            .with_resizable(true);
        if let Some((x, y)) = self.profile.position {
            attrs = attrs.with_position(LogicalPosition::new(x as f64, y as f64));
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("create_window failed: {e}");
                event_loop.exit();
                return;
            }
        };
        let gpu = match Gpu::new(window.clone()) {
            Ok(g) => g,
            Err(e) => {
                log::error!("gpu init failed: {e}");
                event_loop.exit();
                return;
            }
        };
        let renderer = Renderer::new(&gpu.device, &gpu.queue, gpu.config.format);
        self.gpu = Some(gpu);
        self.renderer = Some(renderer);
        self.refresh_viewport();

        // AccessKit bridge — must be constructed before the window
        // is visible. The bridge owns its `accesskit_winit::Adapter`
        // and routes the framework's parallel a11y tree into the
        // platform AX layer (UIA / AT-SPI / NSAccessibility). The
        // activation callback fires the first time a screen reader
        // connects; we just log so the dev sees AT engagement, but
        // the next `sync` call will push the tree regardless.
        #[cfg(feature = "a11y")]
        {
            let window_ref = self.gpu.as_ref().expect("gpu set above").window.clone();
            self.a11y_bridge = Some(host_wgpu_accesskit::WgpuAccessKitBridge::new(
                event_loop,
                &window_ref,
                || {
                    log::debug!("[a11y] AccessKit activation handler fired — AT connected");
                },
            ));
        }

        // macOS: pin the window's content aspect ratio so every
        // drag tracks smoothly — the OS will constrain the
        // mouse, so we never see an off-aspect Resized event.
        // Also cap the content height to ~90% of the visible
        // screen so a phone profile can't be stretched taller
        // than the user's display.
        #[cfg(target_os = "macos")]
        {
            let logical_w = self.profile.logical_size.0 as f64;
            let logical_h = self.profile.logical_size.1 as f64;
            // Install the resize-time aspect lock. The delegate
            // is retained on `self` so AppKit's weak ref stays
            // valid for the window's lifetime.
            self._aspect_lock = mac::install_aspect_lock(&window, logical_w, logical_h);
            // Aspect-correct minimum so neither axis can be
            // dragged into a shape the aspect lock can't honor.
            // Pick a sensible content-min at ~25% of the device
            // size; below that the chrome looks broken anyway.
            const MIN_RATIO: f64 = 0.25;
            mac::set_content_min(
                &window,
                (logical_w * MIN_RATIO).floor(),
                (logical_h * MIN_RATIO).floor(),
            );
            if let Some(monitor) = window.current_monitor() {
                let sf = monitor.scale_factor().max(0.001);
                let mon_logical_h = monitor.size().height as f64 / sf;
                let mon_logical_w = monitor.size().width as f64 / sf;
                // Leave room for the menu bar + dock; 88% is the
                // usual rule of thumb across macOS screen layouts.
                const SCREEN_USE_RATIO: f64 = 0.88;
                let max_h = (mon_logical_h * SCREEN_USE_RATIO).floor();
                let aspect = logical_w / logical_h.max(1.0);
                let max_w_for_h = max_h * aspect;
                let max_w = max_w_for_h.min(mon_logical_w * SCREEN_USE_RATIO).floor();
                let max_h = (max_w / aspect).floor();
                mac::set_content_max(&window, max_w, max_h);
            }
        }
        // Hand the host the logical viewport size so the
        // on-screen keyboard can lay out against the bottom edge.
        // The logical viewport is fixed by the profile and never
        // changes on resize; only the letterbox transform does.
        self.host.set_viewport(
            self.profile.logical_size.0 as f32,
            self.profile.logical_size.1 as f32,
        );
        // Now that the renderer is up, build the framework tree.
        if let Some(build_ui) = self.build_ui.take() {
            self.host.mount(build_ui);
        }
        // Runtime-server mode: spawn the shell against the same
        // backend `Rc<RefCell<>>` the renderer reads from. The
        // worker thread starts discovering / connecting to the
        // dev-server immediately; first commands arrive a tick
        // later and are applied through `shell.tick(...)` from
        // `RedrawRequested` below. No local `app()` mount.
        #[cfg(feature = "runtime-server")]
        if let Some(url) = self.runtime_server_url.take() {
            let backend = self.host.backend().clone();
            let initial_viewport = Some(runtime_server_shell_native::WireViewport {
                width: self.profile.logical_size.0 as f32,
                height: self.profile.logical_size.1 as f32,
            });
            let shell = std::rc::Rc::new(
                runtime_server_shell_native::RuntimeServerShell::<render_wgpu::WgpuBackend>::spawn_with_shared_backend(
                    backend,
                    url,
                    runtime_server_shell_native::RuntimeServerShellOptions {
                        platform: runtime_server_shell_native::WirePlatform::Other,
                        device_label: Some(format!(
                            "sim ({:.0}×{:.0})",
                            self.profile.logical_size.0 as f32,
                            self.profile.logical_size.1 as f32,
                        )),
                        viewport: initial_viewport,
                    },
                ),
            );
            self.runtime_server_shell = Some(shell);
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Redraw => {
                if let Some(gpu) = self.gpu.as_ref() {
                    gpu.window.request_redraw();
                }
            }
            AppEvent::SchedTick => {
                // Fire any due `after_ms` closures + tick every
                // active `raf_loop` client. Drains run on the main
                // thread (closures aren't `Send`), so this must
                // stay inside the winit event handler.
                crate::scheduler::drain_due();
                // The drain almost certainly touched
                // `AnimatedValue`s or queued more work — wake the
                // renderer so it picks up the new values.
                if let Some(gpu) = self.gpu.as_ref() {
                    gpu.window.request_redraw();
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        // Diag: surface every window event so we can tell what
        // actually fires when the user clicks the red X. Strip
        // once `CloseRequested` is confirmed routing.
        if matches!(event, WindowEvent::CloseRequested | WindowEvent::Destroyed) {
            eprintln!("[diag] window_event: {:?}", event);
        }

        // Route every event through the AccessKit bridge first.
        // The bridge needs to see focus/activation events to keep
        // its window-status mirror current; today
        // `process_event` returns `()` (no event consumption), so
        // we always fall through to the app's own handling. The
        // bool return shape is reserved for future AccessKit
        // versions that may consume gesture events.
        #[cfg(feature = "a11y")]
        if let (Some(bridge), Some(gpu)) =
            (self.a11y_bridge.as_mut(), self.gpu.as_ref())
        {
            let _ = bridge.handle_event(&gpu.window, &event);
        }

        match event {
            WindowEvent::CloseRequested => {
                eprintln!("[close] CloseRequested fired — shutting down");
                event_loop.exit();
                // Force the process to exit. On macOS, NSApp
                // does NOT terminate when the last window
                // closes — `event_loop.exit()` returns control
                // from `run_app`, but reactive-scope statics
                // can keep the process alive.
                // When the run loop is single-window today, the
                // user expectation is "X button kills the app",
                // matching how virtually every macOS preview /
                // simulator behaves. Multi-window support will
                // need a window registry that calls exit only
                // on the last close — until that lands, this
                // unconditional exit is the right behavior.
                std::process::exit(0);
            }
            WindowEvent::Resized(size) => {
                // First, snap back to the locked aspect ratio if
                // the user dragged us off it. `enforce_aspect`
                // schedules another `Resized` with the corrected
                // size; ignore this off-aspect frame so we don't
                // briefly distort the content.
                if self.enforce_aspect(size) {
                    return;
                }
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(size.width, size.height);
                }
                self.last_size = Some(size);
                // Refresh pointer + render-viewport scales so
                // they track the new surface size. With the
                // window aspect-locked by the OS, both axes
                // scale uniformly; on platforms without the
                // lock, the axes diverge and content stretches.
                self.refresh_viewport();
                if let Some(gpu) = self.gpu.as_ref() {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                // The underlying surface gets reconfigured via the
                // following Resized event; nothing to do here. The
                // viewport is derived from physical px so it
                // adapts automatically.
            }
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                self.modifiers = KeyModifiers {
                    shift: s.shift_key(),
                    ctrl: s.control_key(),
                    alt: s.alt_key(),
                    meta: s.super_key(),
                };
            }
            WindowEvent::CursorMoved { position, .. } => {
                // `position` is in physical pixels (winit 0.30).
                // Convert to content-logical px via the active
                // surface-scale; with the window aspect-locked
                // by the macOS delegate this is uniform across
                // both axes.
                let p = self
                    .viewport
                    .physical_to_logical((position.x as f32, position.y as f32));
                self.last_pointer = p;
                self.host.pointer_move(PointerEvent {
                    id: PointerId::MOUSE,
                    button: PointerButton::Primary,
                    position: p,
                });
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some(b) = winit_button_to_pointer(button) else { return };
                let pe = PointerEvent {
                    id: PointerId::MOUSE,
                    button: b,
                    position: self.last_pointer,
                };
                match state {
                    ElementState::Pressed => self.host.pointer_down(pe),
                    ElementState::Released => self.host.pointer_up(pe),
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Translate winit's wheel delta into content-logical
                // pixels. LineDelta uses platform-defined "lines" so
                // we multiply by an empirical px/line (matches Cocoa
                // default). PixelDelta arrives in physical px; scale
                // it to logical so a 100-px wheel turn always moves
                // the same amount of *content* regardless of window
                // size.
                //
                // winit's convention is "positive y = wheel up =
                // reveal content above". We invert here so wheel
                // down scrolls down (reveals content below).
                const LINE_HEIGHT_PX: f32 = 24.0;
                let logical_per_phys_x =
                    self.viewport.logical.0 / self.viewport.phys.0.max(1.0);
                let logical_per_phys_y =
                    self.viewport.logical.1 / self.viewport.phys.1.max(1.0);
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (-x * LINE_HEIGHT_PX, -y * LINE_HEIGHT_PX)
                    }
                    MouseScrollDelta::PixelDelta(p) => (
                        -(p.x as f32) * logical_per_phys_x,
                        -(p.y as f32) * logical_per_phys_y,
                    ),
                };
                self.host.scroll(ScrollEvent {
                    position: self.last_pointer,
                    delta: (dx, dy),
                });
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let ke = winit_key(&event, self.modifiers);
                self.host.key(&ke);
            }
            WindowEvent::Focused(false) => {
                self.host.pointer_cancel();
            }
            WindowEvent::RedrawRequested => {
                // Runtime-server tick: pulls inbound commands +
                // applies them through the shared backend the
                // renderer is about to read, sends RequestFrame so
                // the sidecar advances its animation clock, and
                // reports the viewport so welcome's planet-orbit
                // math (and anything reading `page_ref.frame()`)
                // tracks window resizes. Has to happen BEFORE
                // `render_frame` so this frame paints the post-
                // apply scene. No-op in local-mount mode.
                #[cfg(feature = "runtime-server")]
                if let Some(shell) = &self.runtime_server_shell {
                    let viewport = Some(runtime_server_shell_native::WireViewport {
                        width: self.profile.logical_size.0 as f32,
                        height: self.profile.logical_size.1 as f32,
                    });
                    shell.tick(viewport);
                    // The shell may have applied new SetAnimated
                    // values; keep the redraw loop alive so the
                    // animator's next frame paints too.
                    if let Some(gpu) = self.gpu.as_ref() {
                        gpu.window.request_redraw();
                    }
                }
                match self.render_frame() {
                    FrameOutcome::Ok => {}
                    FrameOutcome::Reconfigure => {
                        if let Some(gpu) = self.gpu.as_mut() {
                            let w = gpu.config.width;
                            let h = gpu.config.height;
                            gpu.resize(w, h);
                        }
                    }
                    FrameOutcome::Skip => {}
                }
            }
            _ => {}
        }
    }
}
