//! winit `ApplicationHandler` shim + the public `run` entry.
//!
//! - Spins up the winit event loop + window + wgpu surface.
//! - Installs `backend_wgpu_core::install_redraw_hook` so the
//!   core can wake the event loop when an animation or signal
//!   change needs another paint.
//! - Translates `winit::event::WindowEvent` values into the
//!   normalized `backend_wgpu_core::input` event types and
//!   forwards them to the core's `Host`.
//! - Drives `RedrawRequested` through the core's `Renderer`.

use std::sync::Arc;
use std::time::Instant;

use backend_wgpu_api::{
    DeviceProfile, Key, KeyEvent, KeyModifiers, PointerButton, PointerEvent, PointerId,
    ScrollEvent,
};
use backend_wgpu_core::{install_redraw_hook, Host, Renderer};
use framework_core::Primitive;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
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
enum AppEvent {
    Redraw,
}

/// Run the preview window until the user closes it.
pub fn run<F>(profile: DeviceProfile, build_ui: F) -> Result<(), RunError>
where
    F: FnOnce() -> Primitive + 'static,
{
    let event_loop: EventLoop<AppEvent> = EventLoop::with_user_event()
        .build()
        .map_err(|e| RunError::EventLoop(e.to_string()))?;
    // Install the core's redraw hook to point at our event loop.
    // Any `backend_wgpu_core::request_redraw()` call from inside
    // `apply_style`, the animator, etc. now wakes us up.
    let proxy = event_loop.create_proxy();
    install_redraw_hook(Box::new(move || {
        let _ = proxy.send_event(AppEvent::Redraw);
    }));
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::new(profile, Box::new(build_ui));
    event_loop
        .run_app(&mut app)
        .map_err(|e| RunError::EventLoop(e.to_string()))
}

struct App {
    profile: DeviceProfile,
    /// Consumed on first `resumed`. None afterward.
    build_ui: Option<Box<dyn FnOnce() -> Primitive>>,
    gpu: Option<Gpu>,
    renderer: Option<Renderer>,
    host: Host,
    /// Logical→physical scale factor reported by winit.
    scale_factor: f32,
    /// Cached modifier state. winit 0.30 delivers modifiers via a
    /// separate `ModifiersChanged` event, so we track them
    /// alongside the keyboard handler.
    modifiers: KeyModifiers,
    /// winit reports the pointer position via `CursorMoved` and
    /// the button state via `MouseInput` (positionless). Cache
    /// the last move so every `PointerEvent` we hand to the host
    /// has an authoritative position.
    last_pointer: (f32, f32),
}

impl App {
    fn new(profile: DeviceProfile, build_ui: Box<dyn FnOnce() -> Primitive>) -> Self {
        let host = Host::new(profile.platform, profile.color_scheme);
        Self {
            profile,
            build_ui: Some(build_ui),
            gpu: None,
            renderer: None,
            host,
            scale_factor: 1.0,
            modifiers: KeyModifiers::default(),
            last_pointer: (0.0, 0.0),
        }
    }

    fn render_frame(&mut self) -> Result<(), wgpu::SurfaceError> {
        let Some(gpu) = self.gpu.as_mut() else { return Ok(()) };
        let Some(renderer) = self.renderer.as_mut() else { return Ok(()) };

        let surface_tex = gpu.surface.get_current_texture()?;
        let view = surface_tex
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let logical = (
            self.profile.logical_size.0 as f32,
            self.profile.logical_size.1 as f32,
        );
        renderer.render(&self.host, &gpu.device, &gpu.queue, &view, logical);
        surface_tex.present();

        // If any tween is still in flight, request another frame.
        if self.host.tick(Instant::now()) {
            gpu.window.request_redraw();
        }
        Ok(())
    }
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
        let attrs = Window::default_attributes()
            .with_title(self.profile.title.clone())
            .with_inner_size(LogicalSize::new(
                self.profile.logical_size.0 as f64,
                self.profile.logical_size.1 as f64,
            ))
            .with_resizable(false);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("create_window failed: {e}");
                event_loop.exit();
                return;
            }
        };
        self.scale_factor = window.scale_factor() as f32;
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
        // Hand the host the logical viewport size so the
        // on-screen keyboard can lay out against the bottom edge.
        self.host.set_viewport(
            self.profile.logical_size.0 as f32,
            self.profile.logical_size.1 as f32,
        );
        // Now that the renderer is up, build the framework tree.
        if let Some(build_ui) = self.build_ui.take() {
            self.host.mount(build_ui);
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Redraw => {
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
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(size.width, size.height);
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
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
                let sf = self.scale_factor.max(0.001);
                let p = (position.x as f32 / sf, position.y as f32 / sf);
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
                // Translate winit's wheel delta into logical pixels.
                // LineDelta uses platform-defined "lines" so we
                // multiply by an empirical px/line (matches Cocoa
                // default). PixelDelta arrives in physical px on
                // macOS HiDPI; divide by scale factor.
                //
                // winit's convention is "positive y = wheel up =
                // reveal content above". We invert here so wheel
                // down scrolls down (reveals content below) —
                // matches the example's requested feel. (Flip
                // these signs to switch the whole app to
                // natural-scroll.)
                const LINE_HEIGHT_PX: f32 = 24.0;
                let sf = self.scale_factor.max(0.001);
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (-x * LINE_HEIGHT_PX, -y * LINE_HEIGHT_PX)
                    }
                    MouseScrollDelta::PixelDelta(p) => {
                        (-(p.x as f32) / sf, -(p.y as f32) / sf)
                    }
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
                match self.render_frame() {
                    Ok(()) => {}
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        if let Some(gpu) = self.gpu.as_mut() {
                            let w = gpu.config.width;
                            let h = gpu.config.height;
                            gpu.resize(w, h);
                        }
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                    Err(wgpu::SurfaceError::Timeout) => {}
                }
            }
            _ => {}
        }
    }
}
