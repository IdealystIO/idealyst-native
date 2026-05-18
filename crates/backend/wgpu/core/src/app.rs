//! winit `ApplicationHandler` shim + the public `run` entry point.
//!
//! Everything platform-agnostic — interaction state, focus, drag,
//! keystroke handling — lives in [`crate::Host`]. This module's
//! only job is:
//!
//! 1. Spin up the winit event loop + window + wgpu surface.
//! 2. Translate `winit::event::WindowEvent` values into the
//!    normalized [`crate::input::PointerEvent`] /
//!    [`crate::input::KeyEvent`] vocabulary.
//! 3. Forward those to the `Host`.
//! 4. Drive `RedrawRequested` through the renderer walk.
//!
//! A future browser / iOS / Android shell builds its own version of
//! this file with the same shape: own a `Host`, translate native
//! events to the input types, call `Host::pointer_*` / `Host::key`.

use std::sync::Arc;

use framework_core::{ColorScheme, Primitive};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WKey, NamedKey};
use winit::window::{Window, WindowId};

use std::time::Instant;

use crate::animation::{AnimProperty, TweenKey};
use crate::gpu::Gpu;
use crate::host::{hit_test_node, Host};
use crate::input::{Key, KeyEvent, KeyModifiers, PointerButton, PointerEvent, PointerId};
use crate::node::{NodeKind, WgpuNode};
use crate::pipeline::{Instance as RectInstance, RectPipeline};
use crate::scheduler::{install_proxy, AppEvent};
use crate::style_convert::srgb_rgba_to_linear;
use crate::text::{render_text, StagedText, TextCtx, TextStore};
use crate::widgets;

/// Which mobile OS the simulator should mimic. Drives the rendered
/// look of native widgets (UISwitch vs Material switch, UISlider
/// vs Material slider, etc.). The framework's primitive tree is
/// the same across platforms — what differs is how the backend
/// paints native-feeling controls.
///
/// Currently only `Ios` is fully implemented; `Android` is a stub
/// that falls through to iOS styling pending a Material 3 pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimulatedPlatform {
    Ios,
    Android,
}

/// Device-frame description for a preview variant. Passed by
/// `backend-wgpu-phone` / `-tablet` / `-tv` into [`run`].
#[derive(Clone, Debug)]
pub struct DeviceProfile {
    /// Logical width × height in CSS px. The actual window may be
    /// larger if the platform forces it; we pin our `WgpuBackend`
    /// viewport to this so the rendered layout always matches the
    /// target device's coordinate space.
    pub logical_size: (u32, u32),
    /// Window title (shown in the title bar / dock).
    pub title: String,
    /// Initial color scheme reported to the app on init. Phone /
    /// tablet default to `Auto`; the TV variant pins `Dark`.
    pub color_scheme: ColorScheme,
    /// OS skin the simulator should mimic. Variants default to
    /// `Ios`; expose a switcher later via a `run_for(platform, app)`
    /// wrapper on each variant crate.
    pub platform: SimulatedPlatform,
}

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

/// Render a small magenta dot at the cached pointer position on
/// every frame. Useful for cross-checking that the hit-test
/// coordinate space matches the rendered scene. Flip to `true`
/// while diagnosing input-coordinate issues.
const DEBUG_POINTER_DOT: bool = false;

/// Run the preview window until the user closes it.
///
/// `build_ui` is invoked exactly once, after the backend is ready,
/// to construct the framework's root `Primitive`.
pub fn run<F>(profile: DeviceProfile, build_ui: F) -> Result<(), RunError>
where
    F: FnOnce() -> Primitive + 'static,
{
    let event_loop: EventLoop<AppEvent> = EventLoop::with_user_event()
        .build()
        .map_err(|e| RunError::EventLoop(e.to_string()))?;
    install_proxy(event_loop.create_proxy());
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::new(profile, Box::new(build_ui));
    event_loop
        .run_app(&mut app)
        .map_err(|e| RunError::EventLoop(e.to_string()))
}

/// winit application handler. Holds the wgpu state and a `Host`
/// that handles all interaction logic platform-agnostically.
struct App {
    profile: DeviceProfile,
    /// Consumed on first `resumed`. None afterward.
    build_ui: Option<Box<dyn FnOnce() -> Primitive>>,
    gpu: Option<Gpu>,
    rect: Option<RectPipeline>,
    text_ctx: Option<TextCtx>,
    host: Host,
    /// Logical→physical scale factor. Pointer events arrive in
    /// physical px; the host expects logical px (Taffy / layout
    /// space). Updated on `resumed` / `ScaleFactorChanged`.
    scale_factor: f32,
    /// Cached modifier state. winit 0.30 delivers modifiers via a
    /// separate `ModifiersChanged` event, so we track them
    /// alongside the keyboard handler.
    modifiers: KeyModifiers,
    /// winit reports the pointer position via `CursorMoved` and the
    /// button state via `MouseInput` (positionless). Cache the last
    /// move so we can supply an authoritative position to every
    /// `PointerEvent` we hand to the host.
    last_pointer: (f32, f32),
}

impl App {
    fn new(profile: DeviceProfile, build_ui: Box<dyn FnOnce() -> Primitive>) -> Self {
        let host = Host::new(profile.platform, profile.color_scheme);
        Self {
            profile,
            build_ui: Some(build_ui),
            gpu: None,
            rect: None,
            text_ctx: None,
            host,
            scale_factor: 1.0,
            modifiers: KeyModifiers::default(),
            last_pointer: (0.0, 0.0),
        }
    }

    fn render_frame(&mut self) -> Result<(), wgpu::SurfaceError> {
        let Some(gpu) = self.gpu.as_mut() else { return Ok(()) };
        let Some(rect) = self.rect.as_mut() else { return Ok(()) };
        let Some(text_ctx) = self.text_ctx.as_mut() else { return Ok(()) };

        // Logical-pixel viewport. The surface is configured in
        // physical pixels (gpu.config.width/height are physical),
        // but we lay out + draw + glyph-position in *logical* CSS
        // pixels so that pointer events (also logical, after
        // dividing by scale_factor) match Taffy's frames exactly.
        // The shader maps to NDC via this viewport, so as long as
        // unit consistency holds, geometry covers the right
        // fraction of the physical surface — Retina just gets a
        // sharper rasterization for free.
        let viewport = [
            self.profile.logical_size.0 as f32,
            self.profile.logical_size.1 as f32,
        ];

        // Run Taffy layout first. Brief mut borrow.
        let root = self.host.backend().borrow().root();
        if let Some(root) = root.as_ref() {
            let mut backend = self.host.backend().borrow_mut();
            let root_layout = root.borrow().layout;
            backend.layout.compute(root_layout, viewport[0], viewport[1]);
        }

        // Hold shared borrows for the draw pass. `text_store` is a
        // separate Rc<RefCell<>> from backend, so glyphon buffer
        // refs in `texts` live there without conflicting with
        // backend access.
        let backend = self.host.backend().borrow();
        let text_store = self.host.text_store().borrow();
        let platform = self.host.platform();
        let focused_layout = self.host.focused_input_layout();
        let mut rects: Vec<RectInstance> = Vec::new();
        let mut texts: Vec<StagedText<'_>> = Vec::new();
        let now = Instant::now();
        if let Some(root) = root.as_ref() {
            walk(
                &backend,
                &text_store,
                platform,
                focused_layout,
                now,
                root,
                0.0,
                0.0,
                &mut rects,
                &mut texts,
            );
        }

        // Debug overlay: a small magenta dot at the last logical
        // pointer position. Lets the eye verify the hit-test space
        // matches the rendered scene. Toggle via the constant.
        if DEBUG_POINTER_DOT {
            const DOT: f32 = 10.0;
            let (px, py) = self.last_pointer;
            rects.push(RectInstance {
                rect: [px - DOT * 0.5, py - DOT * 0.5, DOT, DOT],
                bg: srgb_rgba_to_linear([1.0, 0.0, 1.0, 0.9]),
                corner_radius: [DOT * 0.5; 4],
                border_color: [0.0; 4],
                border_width: 0.0,
                _pad: [0.0; 3],
            });
        }

        let frame = gpu.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("idealyst-frame"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("idealyst-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 1.0,
                            g: 1.0,
                            b: 1.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rect.render(&gpu.device, &gpu.queue, &mut pass, viewport, &rects);
            let mut fs = self.host.font_system().borrow_mut();
            // glyphon's `Resolution` is its NDC mapping space —
            // same as our rect pipeline's `viewport`. Logical px so
            // text positions line up with the rest of the scene.
            let _ = render_text(
                text_ctx,
                &mut fs,
                &gpu.device,
                &gpu.queue,
                &mut pass,
                [viewport[0] as u32, viewport[1] as u32],
                &texts,
            );
        }

        gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        drop(text_store);
        drop(backend);

        // Drive the animation loop: if any tween is still in
        // flight after this frame, ask winit for another. The
        // host's tick purges completed tweens so this stops
        // firing as soon as everything settles.
        if self.host.tick_animations(now) {
            if let Some(gpu) = self.gpu.as_ref() {
                gpu.window.request_redraw();
            }
        }
        Ok(())
    }
}

/// Recursive tree walk. Accumulates draw commands into `rects` and
/// `texts` in tree order (back-to-front). `texts` holds glyphon
/// buffer refs borrowed from `text_store`; hence both share `'a`.
/// `node` is short-lived. `now` is the frame's reference clock for
/// sampling the animator — passed through so every node in the
/// frame sees the same time and avoids one-pixel jitter from
/// timestamp drift across the walk.
#[allow(clippy::too_many_arguments)]
fn walk<'a>(
    backend: &crate::backend_impl::WgpuBackend,
    text_store: &'a TextStore,
    platform: SimulatedPlatform,
    focused_input_layout: Option<native_layout::LayoutNode>,
    now: Instant,
    node: &WgpuNode,
    parent_x: f32,
    parent_y: f32,
    rects: &mut Vec<RectInstance>,
    texts: &mut Vec<StagedText<'a>>,
) {
    let data = node.borrow();
    let frame = backend.layout.frame_of(data.layout);
    let x = parent_x + frame.x;
    let y = parent_y + frame.y;
    let w = frame.width;
    let h = frame.height;

    // Form inputs paint their own background/border via the
    // platform-skinned widget renderer; skip the generic
    // background path for them.
    let is_native_widget = matches!(
        data.kind,
        NodeKind::Toggle { .. } | NodeKind::Slider { .. } | NodeKind::TextInput { .. }
    );

    let r = &data.render;
    if !is_native_widget {
        let has_bg = r.background.is_some();
        let any_border = r.border_width.iter().any(|w| *w > 0.0);
        if has_bg || any_border {
            // Sample the animator for background + top border
            // color, falling back to the resolved `RenderStyle`
            // value when no tween is active. Any stylesheet that
            // didn't declare a transition for that property pays
            // the same hash-lookup cost (HashMap::get) plus the
            // no-op return — the snap-to-new path is preserved.
            let bg_rest = r.background.unwrap_or([0.0; 4]);
            let bg = backend.animator.sample_color(
                TweenKey::new(data.layout, AnimProperty::BackgroundColor),
                bg_rest,
                now,
            );
            let bw = r.border_width[0];
            let bc = backend.animator.sample_color(
                TweenKey::new(data.layout, AnimProperty::BorderTopColor),
                r.border_color[0],
                now,
            );
            // sRGB → linear: the surface is sRGB-encoded so we ship
            // linear values and the hardware encodes back to sRGB
            // on write. Authoring side keeps CSS-style hex codes.
            let bg_lin = srgb_rgba_to_linear([bg[0], bg[1], bg[2], bg[3] * r.opacity]);
            let bc_lin = srgb_rgba_to_linear(bc);
            rects.push(RectInstance {
                rect: [x, y, w, h],
                bg: bg_lin,
                corner_radius: r.corner_radius,
                border_color: bc_lin,
                border_width: bw,
                _pad: [0.0; 3],
            });
        }
    }

    match &data.kind {
        NodeKind::Text { .. } | NodeKind::Button { .. } => {
            if let Some(entry) = text_store.buffers.get(&data.layout) {
                let color = backend.animator.sample_color(
                    TweenKey::new(data.layout, AnimProperty::TextColor),
                    r.color,
                    now,
                );
                texts.push(StagedText {
                    buffer: &entry.buffer,
                    x,
                    y,
                    color,
                    clip: glyphon::TextBounds {
                        left: x as i32,
                        top: y as i32,
                        right: (x + w) as i32,
                        bottom: (y + h) as i32,
                    },
                });
            }
        }
        NodeKind::Toggle { value, .. } => {
            // Animator-driven thumb position: sample the tween,
            // fall back to the rest position from `value`.
            let rest = if *value { 1.0 } else { 0.0 };
            let t = backend.animator.sample(
                TweenKey::new(data.layout, AnimProperty::ToggleThumb),
                rest,
                now,
            );
            widgets::paint_toggle(platform, x, y, w, h, t, rects);
        }
        NodeKind::Slider { value, min, max, .. } => {
            widgets::paint_slider(platform, x, y, w, h, *value, *min, *max, rects);
        }
        NodeKind::TextInput { value, .. } => {
            let is_focused = focused_input_layout == Some(data.layout);
            let is_placeholder = value.is_empty();
            if let Some(entry) = text_store.buffers.get(&data.layout) {
                let caret_local = entry
                    .buffer
                    .layout_runs()
                    .next()
                    .map(|r| r.line_w)
                    .unwrap_or(0.0);
                widgets::paint_text_input(
                    platform,
                    x,
                    y,
                    w,
                    h,
                    is_focused,
                    is_placeholder,
                    &entry.buffer,
                    caret_local,
                    r.color,
                    rects,
                    texts,
                );
            }
        }
        _ => {}
    }

    let children: Vec<WgpuNode> = data.children.clone();
    drop(data);
    for child in &children {
        walk(
            backend,
            text_store,
            platform,
            focused_input_layout,
            now,
            child,
            x,
            y,
            rects,
            texts,
        );
    }
}

// Keep the `hit_test_node` re-export accessible for callers that
// imported it from `app` historically. Forwarded to `host` so the
// implementation lives in one place.
#[allow(dead_code)]
pub(crate) fn hit_test_node_at(
    backend: &crate::backend_impl::WgpuBackend,
    node: &WgpuNode,
    parent_x: f32,
    parent_y: f32,
    point: (f32, f32),
) -> Option<(WgpuNode, f32, f32, f32, f32)> {
    hit_test_node(backend, node, parent_x, parent_y, point)
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
        let rect = RectPipeline::new(&gpu.device, gpu.config.format);
        let text_ctx = TextCtx::new(&gpu.device, &gpu.queue, gpu.config.format);
        self.gpu = Some(gpu);
        self.rect = Some(rect);
        self.text_ctx = Some(text_ctx);
        // Build the framework tree now that the GPU is ready.
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
                if DEBUG_POINTER_DOT {
                    if let Some(gpu) = self.gpu.as_ref() {
                        gpu.window.request_redraw();
                    }
                }
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
            WindowEvent::KeyboardInput { event, .. } => {
                let ke = winit_key(&event, self.modifiers);
                self.host.key(&ke);
            }
            WindowEvent::Focused(false) => {
                // OS-level focus loss → cancel any active drag.
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

