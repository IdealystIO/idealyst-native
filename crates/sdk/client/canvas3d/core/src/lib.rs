//! `canvas3d-core` — the renderer-agnostic half of the `canvas3d` SDK.
//!
//! A `Canvas3d` view has three layers, the way game engines structure a frame:
//!
//! 1. **`draw`** — the 3D scene ([`Scene3d`]): models, lights, the camera, and
//!    world-space [`Lines`] (grids, selection boxes — anything attached to the
//!    world belongs here, depth-tested or on top).
//! 2. **`overlay`** — a 2D `canvas::Scene` painter (labels, HUD), composited
//!    over the 3D image *in the same frame* by the renderer. Use
//!    [`Scene3d::project`] / [`Canvas3dHandle`] to anchor 2D to 3D points.
//! 3. **Ordinary views** stacked over the canvas, for interactive UI.
//!
//! The painters run inside the renderer's reactive effect: any signal they
//! read (an [`OrbitCamera`], a selection) re-renders the view.
//!
//! ```ignore
//! let model = Model::from_gltf(include_bytes!("../assets/helmet.glb"))?;
//! let orbit = OrbitCamera::new(OrbitConfig::default());
//! let view = Canvas3dHandle::new();
//! ui! {
//!     view(on_touch = orbit.touch_handler(&view), on_wheel = orbit.wheel_handler()) {
//!         { Canvas3d(Canvas3dProps {
//!             draw: canvas3d::draw(move |s: &mut Scene3d| {
//!                 s.camera(orbit.camera());
//!                 s.light(Light::directional(Vec3::new(-1.0, -2.0, -1.0), Color::WHITE, 3.0));
//!                 s.model(&model, Mat4::IDENTITY).pick_id(1);
//!             }),
//!             handle: Some(view.clone()),
//!             ..Default::default()
//!         }) }
//!     }
//! }
//! ```
#![allow(missing_docs)]

mod camera;
pub use camera::{Camera, Projection, Ray};

mod color;
pub use color::{linear_to_srgb, srgb_to_linear, Color};

mod model;
pub use model::{Aabb, AlphaMode, Material, MeshData, Model, ModelError, Part, Texture};

mod gltf_load;

mod scene;
pub use scene::{Light, LineBatch, LineDepth, Lines, ModelItem, Scene3d};

mod pick;
pub use pick::{pick, ray_aabb, ray_triangle, PickHit};

mod orbit;
pub use orbit::{OrbitCamera, OrbitConfig, OrbitState};

mod prim;
pub use prim::{register_ssr, Canvas3d, Canvas3dBound, Canvas3dHandle, Canvas3dPrim, Frame3d};

pub use glam::{Mat4, Quat, Vec2, Vec3, Vec4};

use runtime_core::IdealystSchema;

/// A 3D scene painter: called with a fresh [`Scene3d`] each time it runs.
pub type DrawFn3d = Box<dyn Fn(&mut Scene3d)>;

/// Wrap a closure as a [`DrawFn3d`].
pub fn draw<F: Fn(&mut Scene3d) + 'static>(f: F) -> DrawFn3d {
    Box::new(f)
}

/// Author props for a [`Canvas3d`] view.
#[derive(IdealystSchema)]
pub struct Canvas3dProps {
    /// The 3D scene painter. Reactive: signals read inside re-run it.
    #[schema(constraint = "a Fn(&mut Scene3d) painter — build with canvas3d::draw(...)")]
    pub draw: DrawFn3d,
    /// Optional 2D overlay painter, composited over the 3D image in the same
    /// frame. Reactive like `draw`.
    #[schema(constraint = "optional canvas::draw(...) painter for a 2D overlay")]
    pub overlay: Option<canvas_core::DrawFn>,
    /// Optional handle that tracks the last painted scene (for picking and
    /// projecting from input handlers).
    #[schema(constraint = "optional Canvas3dHandle for picking")]
    pub handle: Option<Canvas3dHandle>,
}

impl Default for Canvas3dProps {
    fn default() -> Self {
        Canvas3dProps { draw: Box::new(|_| {}), overlay: None, handle: None }
    }
}

/// Everything a screen using `canvas3d` usually needs.
pub mod prelude {
    pub use crate::{
        draw, Aabb, Camera, Canvas3d, Canvas3dHandle, Canvas3dProps, Color, Light, LineDepth, Lines,
        Mat4, Material, MeshData, Model, OrbitCamera, OrbitConfig, PickHit, Projection, Quat, Scene3d,
        Vec2, Vec3,
    };
}
