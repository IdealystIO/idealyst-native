//! Per-primitive capability traits — the split of the `Backend`
//! mega-trait (design §5), signatures frozen to today's.
//!
//! Grouping is derived mechanically from which walker module calls which
//! Backend method (each trait's doc names its walker module / caller).
//! Every trait is a subtrait of [`runtime_scene::Host`], which supplies
//! the shared `Node` associated type and the seven structural operations
//! extracted in P1 (`insert`, `insert_many`, `insert_at`, `remove_child`,
//! `clear_children`, `create_anchor`, `supports_splice`) — those seven
//! Backend methods are therefore NOT redeclared here; see `COVERAGE.md`.
//!
//! Naming note: several traits intentionally share a name with a
//! runtime-core *handle-ops* trait (`TextOps`, `ButtonOps`, `ViewOps`,
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
pub use scroll::{SafeAreaOps, ScrollOps, VirtualizerOps};
pub use style::{AssetOps, StyleOps};
pub use text::{ButtonOps, TextOps};
pub use widgets::{ActivityIndicatorOps, SliderOps, TextInputOps, ToggleOps};
pub use wire::WireBindingOps;

/// Umbrella capability: everything the full built-in vocabulary needs.
/// `register_builtins::<B: AllCaps>()` (P2b) bounds on this; the blanket
/// impl means any type implementing every Ops trait — notably
/// [`LegacyBridge`](crate::bridge::LegacyBridge) around any existing
/// `Backend` — is `AllCaps` automatically.
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
