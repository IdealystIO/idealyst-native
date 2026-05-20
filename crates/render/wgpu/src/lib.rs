//! wgpu render backend — implements `framework_core::Backend` and
//! the [`render_api::EventSink`] contract.
//!
//! **No winit. No browser deps.** Any native shell that translates
//! its platform events into the `render_api` event vocabulary
//! and provides a wgpu surface can drive this backend.
//!
//! # Architecture
//!
//! - [`backend_impl::WgpuBackend`] — `framework_core::Backend` trait
//!   impl. Builds and mutates the node tree + Taffy layout tree.
//!   Owns the animator and the shared text + font-system stores.
//! - [`Host`] — interaction state (focus, press, drag, momentum,
//!   keyboard slide) + the `EventSink` impl. The native shell
//!   talks to the render side only through this trait.
//! - [`Renderer`] — wgpu pipeline + tree walker. Render one frame
//!   into a `wgpu::TextureView`.
//! - [`animation::Animator`] — tween engine used by both widget
//!   animations (toggle thumb) and style-driven transitions
//!   (theme crossfade).
//! - [`Skin`] — the pluggable platform skin contract. Concrete
//!   skins (`ios-sim`, `android-sim`) live in their own
//!   crates; the renderer holds an `Rc<dyn Skin>` and dispatches
//!   every widget + keyboard paint call through it.
//! - [`scheduler::install_redraw_hook`] — the shell installs its
//!   redraw closure here; render-side state changes call
//!   `request_redraw()` to wake it.

#![allow(clippy::new_without_default)]

mod animation;
mod backend_impl;
mod device_frame_pipeline;
mod host;
mod image_pipeline;
pub mod keyboard;
pub mod nav_anim;
mod node;
pub mod pipeline;
mod renderer;
mod video;
mod audio;
mod scheduler;
mod skin;
mod style_convert;
pub mod text;
pub mod widgets;

// Re-export the api vocabulary so consumers of this crate
// don't have to depend on `render-api` separately for
// the common types.
pub use render_api as api;
pub use render_api::{
    DeviceProfile, EventSink, Key, KeyEvent, KeyModifiers, PointerButton, PointerEvent,
    PointerId, ScrollEvent,
};

pub use animation::{AnimProperty, Animator, TweenKey, lerp_color};
pub use backend_impl::{graphics_with_drawer, register_graphics_drawer, WgpuBackend};
pub use host::Host;
pub use nav_anim::{
    clear_transition_override, default_transition, with_transition, InstantTransition,
    ScreenTransition, ScreenXform, SlideFromBottom, SlideFromRight, TransitionDirection,
    TransitionFrame,
};
pub use node::{
    GraphicsDrawer, GraphicsFrame, WgpuNode, KEYBOARD_KEY_FONT_SIZE, KEYBOARD_KEY_GAP,
    KEYBOARD_KEY_RADIUS, KEYBOARD_ROW_GAP, KEYBOARD_SIDE_MARGIN, KEYBOARD_VERT_MARGIN,
    NAV_HEADER_HEIGHT, SLIDER_THUMB_SIZE, SLIDER_TRACK_HEIGHT, TEXT_INPUT_CARET_WIDTH,
    TOGGLE_THUMB_INSET,
};
pub use renderer::{paint_icon, Renderer};
pub use scheduler::{install_redraw_hook, request_redraw};
pub use skin::{
    ButtonPressVisual, NavigatorHeaderAction, NavigatorHeaderChrome, NavigatorHeaderHit, Skin,
};
