//! Media handlers: `image`, `icon`, `link`.

use std::rc::Rc;

use runtime_shared::primitives::link::LinkConfig;
use runtime_scene::{Element, MountCx};
use runtime_world::{effect, Value};

use crate::caps::{AssetOps, IconOps, ImageOps, IntrospectionOps, LinkOps};
use crate::prims::{IconPrim, ImagePrim, LinkPrim};
use crate::style_attach::{attach_style, StyleServices};

use super::{bind_dyn, bind_value, initial_of};

/// Mount an `image` — port of `walker/image.rs::build`.
///
/// Sequence: (`register_asset`) → `create_image(initial src, static
/// alt)` → attach_style → load/error handler installs (before the src
/// binding, so a cached decode can't race the handler) → src binding
/// (one `update_image_src` at mount for Const AND Dyn — the walker's
/// unconditional effect) → `Dyn` alt binding → ref-fill.
pub fn mount_image<H>(cx: &mut MountCx<'_, H>, prim: ImagePrim, _children: Vec<Element>) -> H::Node
where
    H: ImageOps + StyleServices + AssetOps + IntrospectionOps,
{
    let backend = cx.backend().clone();
    if let Some(asset) = prim.asset {
        backend
            .borrow_mut()
            .register_asset(asset.id, asset.tag, &asset.source);
    }
    let initial_src = initial_of(&prim.src);
    let (initial_alt, dyn_alt) = match prim.alt {
        Value::Const(alt) => (alt, None),
        Value::Dyn(f) => (None, Some(f)),
    };
    let node = backend
        .borrow_mut()
        .create_image(&initial_src, initial_alt.as_deref(), &prim.a11y);
    #[cfg(feature = "robot")]
    let _robot = crate::robot::register_mount(
        &backend,
        &node,
        crate::robot::ElementKind::Image,
        prim.test_id,
        None,
        None,
        crate::robot::MountActions::default(),
    );
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }
    if let Some(h) = prim.on_load {
        backend.borrow_mut().install_image_load_handler(&node, h);
    }
    if let Some(h) = prim.on_error {
        backend.borrow_mut().install_image_error_handler(&node, h);
    }
    {
        let b = backend.clone();
        let n = node.clone();
        bind_value(prim.src, move |url| {
            b.borrow_mut().update_image_src(&n, url);
        });
    }
    if let Some(f) = dyn_alt {
        let b = backend.clone();
        let n = node.clone();
        let _binding = effect(move || {
            let alt = f();
            b.borrow_mut().update_image_alt(&n, alt.as_deref());
        });
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_image_handle(&node);
        fill(handle);
    }
    node
}

/// Mount an `icon` — port of `walker/icon.rs::build`.
///
/// Sequence: `create_icon(initial data, initial color)` → attach_style
/// → `Dyn` data binding → `Dyn` color binding → stroke (inline apply +
/// `Dyn` binding — the walker applies inline THEN installs the effect,
/// so a `Dyn` stroke emits two `update_icon_stroke` at mount) →
/// draw-in (snap to `from`, animate on the next microtask) → ref-fill.
///
/// Microtask scheduling rides `runtime_shared::schedule_microtask` during
/// the transition (same host-installed scheduler every backend already
/// wires; synchronous fallback off-web without one, exactly like the
/// walker).
pub fn mount_icon<H>(cx: &mut MountCx<'_, H>, prim: IconPrim, _children: Vec<Element>) -> H::Node
where
    H: IconOps + StyleServices + IntrospectionOps,
{
    let backend = cx.backend().clone();
    let (initial_data, dyn_data) = match prim.data {
        Value::Const(d) => (d, None),
        Value::Dyn(f) => (f(), Some(f)),
    };
    let (initial_color, dyn_color) = match prim.color {
        None => (None, None),
        Some(Value::Const(c)) => (Some(c), None),
        Some(Value::Dyn(f)) => (Some(f()), Some(f)),
    };
    let node = backend
        .borrow_mut()
        .create_icon(&initial_data, initial_color.as_ref(), &prim.a11y);
    // Passive/visual primitive: no actions, but findable by test_id +
    // kind (see `mount_activity_indicator`).
    #[cfg(feature = "robot")]
    let _robot = crate::robot::register_mount(
        &backend,
        &node,
        crate::robot::ElementKind::Icon,
        prim.test_id,
        None,
        None,
        crate::robot::MountActions::default(),
    );
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }
    if let Some(f) = dyn_data {
        let b = backend.clone();
        let n = node.clone();
        let _binding = effect(move || {
            let data = f();
            b.borrow_mut().update_icon_data(&n, &data);
        });
    }
    if let Some(f) = dyn_color {
        let b = backend.clone();
        let n = node.clone();
        let _binding = effect(move || {
            let color = f();
            b.borrow_mut().update_icon_color(&n, &color);
        });
    }
    if let Some(stroke) = prim.stroke {
        let initial = initial_of(&stroke);
        backend.borrow_mut().update_icon_stroke(&node, initial);
        let b = backend.clone();
        let n = node.clone();
        bind_dyn(stroke, move |&progress| {
            b.borrow_mut().update_icon_stroke(&n, progress);
        });
    }
    if let Some(anim) = prim.draw_in {
        backend.borrow_mut().update_icon_stroke(&node, anim.from);
        let b = backend.clone();
        let n = node.clone();
        runtime_shared::schedule_microtask(move || {
            b.borrow_mut().animate_icon_stroke(
                &n,
                anim.from,
                anim.to,
                anim.duration_ms,
                anim.easing,
                anim.infinite,
                anim.autoreverses,
            );
        });
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_icon_handle(&node);
        fill(handle);
    }
    node
}

/// Mount a `link` — port of `walker/link.rs::build`.
///
/// Sequence: build [`LinkConfig`] (external links default activation to
/// the platform URL opener, the walker's port; route links resolve the
/// ambient [`LinkActivator`](crate::prims::LinkActivator) AT MOUNT —
/// the new-core counterpart of the old construction-time ambient
/// capture, and mounted-outside-any-navigator links silently no-op on
/// activation, the old `link()` posture; links with none of
/// callback/external/route panic) → `create_link` → children →
/// attach_style → `Dyn` url binding (`update_link_url`, first fire at
/// mount) → ref-fill.
pub fn mount_link<H>(cx: &mut MountCx<'_, H>, prim: LinkPrim, children: Vec<Element>) -> H::Node
where
    H: LinkOps + StyleServices + IntrospectionOps,
{
    let backend = cx.backend().clone();
    let initial_url = initial_of(&prim.url);
    let route_name = prim.route_link.as_ref().map(|r| r.name).unwrap_or("");
    let on_activate: Rc<dyn Fn()> = match (prim.on_activate, prim.route_link) {
        (Some(cb), _) => cb,
        (None, Some(route_link)) => {
            // Capture the enclosing navigator's activator NOW (mount
            // runs inside the navigator's provide window); each
            // activation reproduces fresh params and stages the
            // navigator-kind command (swap Select / stack Push).
            let ambient = runtime_world::inject::<crate::prims::LinkActivator>();
            let url = initial_url.clone();
            Rc::new(move || {
                if let Some(activator) = &ambient {
                    activator.activate(route_link.name, url.clone(), (route_link.make_params)());
                }
            })
        }
        (None, None) if prim.external => {
            let url = initial_url.clone();
            Rc::new(move || runtime_shared::open_url(&url))
        }
        (None, None) => panic!(
            "link(): a non-external link needs .route(...) or .on_activate(...) — a link \
             that silently does nothing is a footgun"
        ),
    };
    #[cfg(feature = "robot")]
    let robot_click = on_activate.clone();
    let config = LinkConfig {
        route: route_name,
        url: initial_url,
        external: prim.external,
        on_activate,
    };
    let mut node = backend.borrow_mut().create_link(config, &prim.a11y);
    // Links register WITHOUT a test_id (`LinkPrim` carries no slot —
    // the old core's `Element::Link` has none either; `with_test_id`
    // silently no-ops there, and the glue setter mirrors that). The
    // robot's `click` is the same activation the backend wires on tap.
    #[cfg(feature = "robot")]
    let _robot = crate::robot::register_mount(
        &backend,
        &node,
        crate::robot::ElementKind::Link,
        None,
        None,
        None,
        crate::robot::MountActions {
            click: Some(robot_click),
            ..Default::default()
        },
    );
    cx.realize_children_into(&mut node, children);
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }
    {
        let b = backend.clone();
        let n = node.clone();
        bind_dyn(prim.url, move |url| {
            b.borrow_mut().update_link_url(&n, url);
        });
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_link_handle(&node);
        fill(handle);
    }
    node
}
