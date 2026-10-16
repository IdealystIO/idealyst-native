//! Per-primitive capability traits: one Ops trait per primitive family,
//! grouped by which handler calls the method.
//!
//! Every trait is a subtrait of [`runtime_scene::Host`], which supplies
//! the shared `Node` associated type and the structural operations
//! (`insert`, `insert_many`, `insert_at`, `remove_child`,
//! `clear_children`, `create_anchor`, `supports_splice`) — those are
//! therefore NOT redeclared here.
//!
//! Naming note: several traits intentionally share a name with a
//! runtime-shared *handle-ops* trait (`TextOps`, `ButtonOps`, `ViewOps`,
//! `primitives::toggle::ToggleOps`, …). Those are the per-handle
//! imperative-ref surfaces (`.bind()` handles); these are the backend
//! capability traits the design names. Different crates, different jobs —
//! call sites disambiguate by path.

mod a11y;
mod app;
mod batch;
mod external;
mod input;
mod media;
mod nav_overlay;
mod noop;
mod scroll;
mod style;
mod text;
mod widgets;
mod wire;

pub use a11y::{A11yOps, AnimationOps, IntrospectionOps};
pub use app::{AppEnvOps, LifecycleOps};
pub use batch::BatchOps;
pub use external::{DocumentOps, ExternalOps};
pub use input::{InputOps, PressableOps, ViewOps};
pub use media::{IconOps, ImageOps, LinkOps};
pub use nav_overlay::{GraphicsOps, NavigatorOps, PortalOps, PresenceOps};
pub use scroll::{GridOps, SafeAreaOps, ScrollOps, VirtualizerOps};
pub use style::{AssetOps, StyleOps};
pub use text::{ButtonOps, TextOps};
pub use widgets::{ActivityIndicatorOps, SliderOps, TextInputOps, ToggleOps};
pub use wire::WireBindingOps;

/// Umbrella capability: everything the full built-in vocabulary needs.
/// `register_builtins::<B: AllCaps>()` bounds on this; the blanket impl
/// means any type implementing every Ops trait is `AllCaps`
/// automatically.
pub trait AllCaps:
    AppEnvOps
    + LifecycleOps
    + ViewOps
    + InputOps
    + PressableOps
    + TextOps
    + ButtonOps
    + ImageOps
    + IconOps
    + LinkOps
    + TextInputOps
    + ToggleOps
    + SliderOps
    + ActivityIndicatorOps
    + ScrollOps
    + SafeAreaOps
    + VirtualizerOps
    + GridOps
    + GraphicsOps
    + PortalOps
    + PresenceOps
    + NavigatorOps
    + ExternalOps
    + DocumentOps
    + StyleOps
    + AssetOps
    + A11yOps
    + AnimationOps
    + IntrospectionOps
    + BatchOps
    + WireBindingOps
{
}

impl<T> AllCaps for T where
    T: AppEnvOps
        + LifecycleOps
        + ViewOps
        + InputOps
        + PressableOps
        + TextOps
        + ButtonOps
        + ImageOps
        + IconOps
        + LinkOps
        + TextInputOps
        + ToggleOps
        + SliderOps
        + ActivityIndicatorOps
        + ScrollOps
        + SafeAreaOps
        + VirtualizerOps
        + GridOps
    + GridOps
        + GraphicsOps
        + PortalOps
        + PresenceOps
        + NavigatorOps
        + ExternalOps
        + DocumentOps
        + StyleOps
        + AssetOps
        + A11yOps
        + AnimationOps
        + IntrospectionOps
        + BatchOps
        + WireBindingOps
{
}
