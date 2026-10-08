//! Render-elsewhere and animated-lifecycle primitives: portal,
//! presence, navigator extensions, and the graphics surface.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives;
use runtime_shared::{Easing, StyleRules};

use super::noop;
use super::{ExternalOps, ViewOps};

/// The `overlay` / portal primitive — render children at a window-level
/// target, escaping layout and clipping. Serves `walker/portal.rs` +
/// `walker/cleanup.rs`.
pub trait PortalOps: ExternalOps {
    /// Stand up the platform's render-elsewhere mount at `target`.
    #[allow(unused_variables)]
    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.missing_primitive_placeholder("portal (backend compiled without `prim-portal`)")
    }

    /// Tear down the portal's backend-side state.
    #[allow(unused_variables)]
    fn release_portal(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Show/hide a mounted portal WITHOUT tearing it down (overlays on
    /// inactive navigator screens).
    #[allow(unused_variables)]
    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        // default no-op
    }

    /// Imperative-ref handle for a portal. Default: no-op.
    #[allow(unused_variables)]
    fn make_portal_handle(&self, node: &Self::Node) -> primitives::portal::PortalHandle {
        primitives::portal::PortalHandle::new(Rc::new(()), &noop::NoopPortalOps)
    }
}

/// The `presence` primitive — enter/exit transforms around mount and
/// deferred unmount. Serves `handlers/presence.rs`.
pub trait PresenceOps: ViewOps {
    /// Create the mount point a presence subtree is inserted into.
    ///
    /// It MUST be layout-transparent — a reactive anchor
    /// ([`Host::create_anchor`](runtime_scene::Host::create_anchor): `display: contents` on web, a
    /// `runtime_layout` contents node on Taffy hosts). The author never
    /// wrote this node, so presence's child has to lay out exactly as if
    /// it were the direct child of presence's parent. A plain view here
    /// is a real flex item, and Taffy resolves an absolute child's insets
    /// against its DIRECT layout parent: a `position: absolute; bottom:
    /// md` child of a plain-view placeholder was placed against a
    /// full-width, ZERO-height box at the top of the positioned ancestor
    /// and landed above it, off screen (the reported iOS "alert never
    /// appears" bug; web hid it because a static `<div>` is not a CSS
    /// containing block). The same box also hugged a `flex_grow` child
    /// to nothing. Pinned by `tests/presence_layout.rs`.
    ///
    /// The enter/exit animation never targets this node — the handler
    /// applies [`apply_presence`](Self::apply_presence) to the CHILD's
    /// nodes — so a box-less placeholder loses nothing.
    ///
    /// `a11y` is the author's bag for the presence. The default ignores
    /// it: the handler applies a non-default bag through
    /// [`A11yOps::update_accessibility`](super::A11yOps::update_accessibility)
    /// after creation, so an override need not either. An override
    /// exists only to pick a different native view class (macOS: a
    /// deep-descent hit-test view) — it must still register a contents
    /// layout node.
    #[allow(unused_variables)]
    fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        self.create_anchor()
    }

    /// Apply a presence transform (opacity + translate + scale):
    /// pre-mount snap (`transition = None`), animate-to-rest, exit,
    /// or reversal.
    #[allow(unused_variables)]
    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        // default: no-op
    }

    /// Imperative-ref handle for a presence. Default: no-op.
    #[allow(unused_variables)]
    fn make_presence_handle(&self, node: &Self::Node) -> primitives::presence::PresenceHandle {
        primitives::presence::PresenceHandle::new(Rc::new(()), &noop::NoopPresenceOps)
    }
}

/// Navigator extensions — the unified registry-dispatched entry for
/// stack/swap/drawer/tab navigator kinds. Serves `walker/navigator.rs`.
pub trait NavigatorOps: ExternalOps {
    // `create_navigator` was DELETED with the old core, not re-defaulted.
    // Its `NavigatorHost` closed over the pre-v2 `Element` (build_node /
    // build_layout_with_outlet), so it only ever served old-`Backend`
    // interop. Navigators mount through `handlers::navigator`
    // (SwapNav/StackNav over the Lifecycle/View caps); nothing calls it.

    /// Tear down a navigator extension.
    #[allow(unused_variables)]
    fn release_navigator(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Apply a slot-style update (e.g. `"header"`, `"tab_bar"`).
    #[allow(unused_variables)]
    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<StyleRules>,
    ) {
        // default no-op
    }

    /// Imperative-ref handle for a navigator. Default: no-op.
    #[allow(unused_variables)]
    fn make_navigator_handle(&self, node: &Self::Node) -> primitives::navigator::NavigatorHandle {
        primitives::navigator::NavigatorHandle::new(Rc::new(()), &noop::NoopNavigatorOps)
    }

    /// Attach the framework-realized initial screen outside the
    /// `create_navigator` borrow window. Placeholder backends drop the
    /// screen instead of panicking.
    #[allow(unused_variables)]
    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn std::any::Any>,
    ) {
        // Feature-mismatch / no-navigator backends: nothing to attach the
        // screen to — the navigator node is already the placeholder. Drop
        // the screen instead of panicking.
        let _ = (navigator, screen, scope_id, options);
    }
}

/// The `graphics` primitive — a raw GPU-drawable surface. Serves
/// `walker/graphics.rs` + `walker/cleanup.rs`.
pub trait GraphicsOps: ExternalOps {
    /// Stand up the native drawable widget and wire its surface
    /// lifecycle to `on_ready` / `on_resize` / `on_lost`.
    #[allow(unused_variables)]
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.missing_primitive_placeholder("graphics (backend compiled without `prim-graphics`)")
    }

    /// Tear down the surface (device, queue, user render state).
    #[allow(unused_variables)]
    fn release_graphics(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Imperative-ref handle for a graphics surface. Default: no-op.
    #[allow(unused_variables)]
    fn make_graphics_handle(&self, node: &Self::Node) -> primitives::graphics::GraphicsHandle {
        primitives::graphics::GraphicsHandle::new(Rc::new(()), &noop::NoopGraphicsOps)
    }
}
