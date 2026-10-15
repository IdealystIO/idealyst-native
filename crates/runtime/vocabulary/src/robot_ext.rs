//! Robot registration for nodes a scene-`Registry` extension mounts.
//!
//! The builtin handlers register every primitive with the robot registry
//! (`robot::register_mount`), but a third-party payload mounted through
//! `runtime_scene::Registry` had no way in: its nodes were invisible to
//! `find_element` / `get_parent`, and an `on_touch` it installed could not be
//! clicked. The idea-ui Table's web lowering is the case that surfaced it —
//! its `<td>` cells carry the clickable-row tap recognizer, and the robot saw
//! neither the cells nor the handler.
//!
//! Always compiled, so an SDK calls it unconditionally: without the
//! vocabulary's `robot` feature [`register_extension_node`] returns an inert
//! guard and nothing is recorded (the `robot_methods` stub pattern).
//!
//! ```ignore
//! let mut node = backend.borrow_mut().create_element("td");
//! let robot = robot_ext::register_extension_node(&backend, &node, None);
//! cx.realize_children_into(&mut node, children); // children parent to `node`
//! if let Some(h) = on_touch {
//!     robot.attach_touch(&h);
//!     backend.borrow_mut().install_touch_handler(&node, h);
//! }
//! drop(robot); // end of the mount handler
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use crate::caps::IntrospectionOps;

/// Keeps the node on the robot's parent stack while the mount handler
/// realizes its children; drop it when the handler returns (or let it fall
/// out of scope). Inert without the `robot` feature.
#[must_use = "children realized while this guard lives are parented to the node"]
pub struct ExtensionNode {
    #[cfg(feature = "robot")]
    prim: crate::robot::RegisteredPrim,
}

/// Register an extension-mounted node as a robot `View`. Call right after
/// the node exists and BEFORE realizing its children, exactly like the
/// builtin handlers.
#[allow(unused_variables)]
pub fn register_extension_node<H: IntrospectionOps>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    test_id: Option<&'static str>,
) -> ExtensionNode {
    ExtensionNode {
        #[cfg(feature = "robot")]
        prim: crate::robot::register_mount(
            backend,
            node,
            crate::robot::ElementKind::View,
            test_id,
            None,
            None,
            crate::robot::MountActions::default(),
        ),
    }
}

impl ExtensionNode {
    /// Expose the touch handler installed on the node, so robot `click` /
    /// `drag` can drive it. Pass the SAME handler the backend receives.
    #[allow(unused_variables)]
    pub fn attach_touch(&self, handler: &runtime_shared::TouchHandler) {
        #[cfg(feature = "robot")]
        crate::robot::attach_touch_action(&self.prim, handler.clone());
    }
}
