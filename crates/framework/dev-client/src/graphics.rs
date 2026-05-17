//! App-local registry for Graphics renderers.
//!
//! The `Graphics` primitive's `on_ready` / `on_resize` / `on_lost`
//! closures are intrinsically device-local — they accept a raw window
//! handle that cannot exist on the dev machine. The wire ships a
//! *name* identifying which app-side renderer to bind; this registry
//! is where the app's startup code registers each name's closures.
//!
//! Usage from app startup:
//!
//! ```ignore
//! let mut registry = GraphicsRegistry::new();
//! registry.register("main_scene", || {
//!     (
//!         Box::new(|evt: OnReadyEvent| { /* wgpu init */ }),
//!         Box::new(|evt: OnResizeEvent| { /* resize */ }),
//!         Box::new(|| { /* on_lost */ }),
//!     )
//! });
//! ```
//!
//! When `Command::CreateGraphics { renderer }` arrives, the replay
//! engine looks up `renderer` and hands the resulting closures to
//! `Backend::create_graphics`.

use std::collections::HashMap;

use framework_core::primitives::graphics::{OnLost, OnReady, OnResize};

/// Factory that builds a fresh `OnReady` closure on each binding.
/// Wrapped in a factory so multiple Graphics mounts of the same
/// renderer get independent closures (each has its own captured
/// state).
pub type OnReadyFactory = Box<dyn Fn() -> OnReady>;
pub type OnResizeFactory = Box<dyn Fn() -> OnResize>;
pub type OnLostFactory = Box<dyn Fn() -> OnLost>;

/// Bundle of factories registered under a single name.
pub struct GraphicsRendererBundle {
    pub on_ready: OnReadyFactory,
    pub on_resize: OnResizeFactory,
    pub on_lost: OnLostFactory,
}

/// App-side registry of named Graphics renderers. Holds the closures
/// that the wire `CreateGraphics { renderer }` command resolves against.
#[derive(Default)]
pub struct GraphicsRegistry {
    bundles: HashMap<String, GraphicsRendererBundle>,
}

impl GraphicsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a named renderer. The supplied factories are called
    /// each time a Graphics primitive with this name mounts.
    pub fn register(&mut self, name: impl Into<String>, bundle: GraphicsRendererBundle) {
        self.bundles.insert(name.into(), bundle);
    }

    /// Look up a renderer and produce a fresh `(OnReady, OnResize, OnLost)`
    /// triple for a mount. Returns `None` if the name isn't registered;
    /// the caller falls back to no-op closures so the surface still
    /// mounts and the layout stays correct.
    pub fn lookup(&self, name: &str) -> Option<(OnReady, OnResize, OnLost)> {
        let bundle = self.bundles.get(name)?;
        Some(((bundle.on_ready)(), (bundle.on_resize)(), (bundle.on_lost)()))
    }
}

impl std::fmt::Debug for GraphicsRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphicsRegistry")
            .field("renderers", &self.bundles.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// No-op closures, used when a wire `CreateGraphics { renderer }`
/// names a renderer that hasn't been registered. The surface still
/// mounts (so the layout stays correct); the GPU side is silent.
pub fn no_op_graphics_handlers() -> (OnReady, OnResize, OnLost) {
    (Box::new(|_| {}), Box::new(|_| {}), Box::new(|| {}))
}
