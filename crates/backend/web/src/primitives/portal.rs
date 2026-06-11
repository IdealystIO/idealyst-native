//! `Element::Portal` — a floating subtree mounted directly to `<body>`,
//! escaping the parent's layout and clipping context.
//!
//! # Why we portal
//!
//! Modals, popovers, dropdowns, etc. need to render *above* every
//! other element in the layout, including parents with `overflow:
//! hidden`, `transform`, or `position` styles that establish new
//! stacking contexts. The browser's z-index doesn't reach across
//! those boundaries. The standard fix on the web is to "portal" —
//! render the floating content outside its logical parent, directly
//! under `<body>`, where no ancestor can clip or restack it.
//!
//! Backdrops are no longer the backend's concern. The framework's
//! composition layer (`primitives::overlay`) renders a backdrop
//! primitive as a child of the portal — the backend just sees a portal
//! whose children include whatever scrim the caller put there.
//!
//! # The insert hijack
//!
//! The render walker calls `Backend::insert(parent, child)` to
//! parent every primitive into its surrounding container. For
//! portals we don't want that — the portal element is already attached
//! to `<body>`. To keep the walker simple, we stamp every portal-
//! created `<div>` with `data-overlay-skip-insert` and the global
//! `view::insert` checks the attribute and skips the `append_child`
//! call.

use crate::WebBackend;
use runtime_core::primitives::portal::{
    AnchorTarget, ElementAlign, ElementSide, PortalHandle, PortalOps, PortalTarget,
    ViewportPlacement, ViewportRect,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

/// Per-portal runtime state. Stored in `WebBackend::portal_instances`,
/// keyed by the `data-portal-id` attribute stamped on the portal
/// element. Holds the JS closures so they stay alive while the portal
/// is mounted; dropping the instance fires `Closure::Drop` which
/// destroys the wasm-bindgen handle and frees its JS-side allocation.
pub(crate) struct PortalInstance {
    /// The outer `<div>` parented to `<body>`. Removing this from
    /// `<body>` on `release` tears down the entire portal subtree.
    /// We look up by `data-portal-id` from the framework-supplied
    /// node in `release`, so this stored reference is unused at
    /// runtime — it's held purely for debugging (introspection from
    /// the instance map).
    #[allow(dead_code)]
    portal_root: web_sys::Element,
    /// Escape-key handler attached to the window. Removed in
    /// `release_portal`. Only populated when the portal has an
    /// `on_dismiss` callback.
    #[allow(dead_code)]
    escape_handler: Option<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
    /// Scroll + resize handlers that re-measure the anchor target
    /// and rewrite the content's inline `top`/`left` so the portal
    /// keeps tracking the trigger as the page scrolls or the
    /// viewport resizes. Only populated for `PortalTarget::Anchor`
    /// portals — viewport-anchored portals pin via `inset` /
    /// `transform` already.
    ///
    /// `scroll` is registered with `capture: true` because scroll
    /// events from nested scroll containers don't bubble. Held here
    /// so `release` can `removeEventListener` with the same closure
    /// references — otherwise the browser keeps firing them against
    /// freed Rust state.
    #[allow(dead_code)]
    reposition_scroll_handler: Option<Closure<dyn FnMut(web_sys::Event)>>,
    #[allow(dead_code)]
    reposition_resize_handler: Option<Closure<dyn FnMut(web_sys::Event)>>,
    /// First-paint reposition task. The walker calls
    /// `create_portal` *before* it inserts the portal's children,
    /// so the content element has no size when we install the
    /// initial inline `top`/`left`. We schedule a one-shot
    /// `requestAnimationFrame` here that fires once the children
    /// have mounted and a paint has measured them.
    #[allow(dead_code)]
    initial_measure_task: Option<runtime_core::ScheduledTask>,
    /// Focus-trap `focusin` handler attached to the window. Only
    /// populated when `trap_focus = true`. Routes focus back to the
    /// first focusable child of the portal when it tries to leave
    /// the subtree.
    ///
    /// `Fn`, not `FnMut`: the handler's own `.focus()` call
    /// synchronously re-dispatches `focusin`, re-entering this very
    /// closure. wasm-bindgen's exclusive-borrow guard would throw
    /// "closure invoked recursively or after being dropped" on a
    /// `FnMut`; an `Fn` closure is re-entry-safe, so the inner call
    /// runs the body and the `in_progress` flag short-circuits it.
    /// See [`install_focus_trap`].
    #[allow(dead_code)]
    focus_trap_handler: Option<Closure<dyn Fn(web_sys::Event)>>,
}

/// All live portal instances, keyed by `data-portal-id`.
pub(crate) type PortalInstances = HashMap<u32, PortalInstance>;

/// Base inline style applied to every portal root before the
/// target-specific positioning rules are layered on top. `pointer-
/// events: auto` lets the portal's content (including any
/// backdrop child the caller provides) receive clicks; the framework
/// no longer special-cases backdrops at the backend level.
const PORTAL_ROOT_BASE_STYLE: &str =
    "position: fixed; pointer-events: auto; z-index: 1000;";

/// Gutter (CSS px) between the portal and the viewport edges when
/// the anchor-positioning clamp kicks in.
const EDGE_GAP: f32 = 8.0;

pub(crate) fn create(
    b: &mut WebBackend,
    target: PortalTarget,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
) -> Node {
    let id = b.next_portal_id;
    b.next_portal_id += 1;

    // ---- Portal root: viewport-pinned, parented to <body> ----
    let portal_root = b
        .doc
        .create_element("div")
        .expect("create_element div failed");
    let _ = portal_root.set_attribute("data-portal-id", &id.to_string());
    // `data-overlay-skip-insert` keeps the framework's later
    // `insert(parent, this)` call from yanking the portal back into
    // the surrounding layout tree. Shared with the old overlay path
    // — see `view::insert`.
    let _ = portal_root.set_attribute("data-overlay-skip-insert", "1");

    // Apply target-specific positioning on top of the base style.
    let placement_style = position_styles_for_target(&target);
    let _ = portal_root.set_attribute(
        "style",
        &format!("{} {}", PORTAL_ROOT_BASE_STYLE, placement_style),
    );

    // ---- Attach to <body> ----
    if let Some(body) = b.doc.body() {
        let _ = body.append_child(&portal_root);
    }

    // ---- Escape-key handler on window (platform dismissal) ----
    //
    // For viewport-rooted portals this is the canonical "user wants
    // out" signal. Anchor portals (popovers, dropdowns) also honor
    // Escape — the caller flips their open-state signal in response.
    let escape_handler = on_dismiss.as_ref().and_then(|dismiss| {
        // Only install if dismissal is wanted; Named portals have no
        // window mounted, so skip them too.
        if matches!(target, PortalTarget::Named(_)) {
            return None;
        }
        let window = web_sys::window()?;
        let dismiss = dismiss.clone();
        let closure = Closure::wrap(Box::new(move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" {
                (dismiss)();
            }
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);
        let _ = window.add_event_listener_with_callback(
            "keydown",
            closure.as_ref().unchecked_ref(),
        );
        Some(closure)
    });

    // ---- Scroll / resize reposition for anchored portals ----
    //
    // Anchor portals measure the trigger and re-place themselves on
    // every scroll + resize. `capture: true` on scroll is the
    // standard trick: scroll events from nested scroll containers
    // don't bubble (spec disables bubbling for `scroll`), so a
    // `window` listener with `useCapture=true` is the only way to
    // catch ancestor scrolls without attaching one listener per
    // container.
    //
    // Viewport portals don't need re-measurement — `inset`/`transform`
    // already pin them to the viewport regardless of scroll.
    let (reposition_scroll_handler, reposition_resize_handler, initial_measure_task) =
        if let PortalTarget::Anchor { target: anchor_target, side, align, offset } = &target {
            install_anchor_reposition(
                &portal_root,
                anchor_target.clone(),
                *side,
                *align,
                *offset,
            )
        } else {
            (None, None, None)
        };

    // ---- Focus trap ----
    //
    // When `trap_focus` is on, install a `focusin` listener on
    // `document` that catches focus moving outside the portal's
    // subtree and routes it back to the first focusable child. Not a
    // full WAI-ARIA dialog implementation, but enough that Tab
    // doesn't immediately escape into the page underneath.
    let focus_trap_handler = if trap_focus {
        install_focus_trap(&b.doc, portal_root.clone())
    } else {
        None
    };

    // ---- Stash the instance for cleanup ----
    b.portal_instances.insert(
        id,
        PortalInstance {
            portal_root: portal_root.clone(),
            escape_handler,
            reposition_scroll_handler,
            reposition_resize_handler,
            initial_measure_task,
            focus_trap_handler,
        },
    );

    portal_root.unchecked_into::<Node>()
}

/// Tear down a portal. Removes it from `<body>`, drops the
/// closures (Escape, focus trap, scroll/resize reposition), and
/// removes the instance entry.
pub(crate) fn release(b: &mut WebBackend, node: &Node) {
    let portal_root = match node.dyn_ref::<web_sys::Element>() {
        Some(el) => el,
        None => return,
    };
    let id_str = portal_root
        .get_attribute("data-portal-id")
        .unwrap_or_default();
    let id: u32 = id_str.parse().unwrap_or(u32::MAX);

    // Detach from <body>. Browsers automatically remove the
    // element's event listeners as part of GC once nothing else
    // holds a reference, but our wasm-bindgen Closure handles are
    // kept alive by the PortalInstance entry — dropping that
    // entry below is what actually frees them.
    if let Some(body) = b.doc.body() {
        let _ = body.remove_child(portal_root.unchecked_ref());
    }

    // Every document- or window-level listener installed during
    // `create` has to be explicitly removed here. Dropping the
    // PortalInstance below frees the wasm-bindgen `Closure` handle,
    // which destroys the JS-side wrapper — but the EventTarget (the
    // `document` / `window`) still has the wrapper registered as a
    // listener. The next matching event will invoke a freed closure
    // and throw "closure invoked recursively or after being dropped".
    if id != u32::MAX {
        if let Some(inst) = b.portal_instances.get(&id) {
            if let Some(window) = web_sys::window() {
                if let Some(closure) = inst.escape_handler.as_ref() {
                    let _ = window.remove_event_listener_with_callback(
                        "keydown",
                        closure.as_ref().unchecked_ref(),
                    );
                }
                if let Some(closure) = inst.reposition_scroll_handler.as_ref() {
                    let _ = window.remove_event_listener_with_callback_and_bool(
                        "scroll",
                        closure.as_ref().unchecked_ref(),
                        true,
                    );
                }
                if let Some(closure) = inst.reposition_resize_handler.as_ref() {
                    let _ = window.remove_event_listener_with_callback(
                        "resize",
                        closure.as_ref().unchecked_ref(),
                    );
                }
            }
            if let Some(closure) = inst.focus_trap_handler.as_ref() {
                let _ = b.doc.remove_event_listener_with_callback(
                    "focusin",
                    closure.as_ref().unchecked_ref(),
                );
            }
        }
        b.portal_instances.remove(&id);
    }
}

pub(crate) fn make_handle(node: &Node) -> PortalHandle {
    let el: web_sys::HtmlElement = node
        .clone()
        .dyn_into()
        .expect("portal node is not an HtmlElement");
    PortalHandle::new(Rc::new(el), &WebPortalOps)
}

struct WebPortalOps;
impl PortalOps for WebPortalOps {}

// ---------------------------------------------------------------------------
// Positioning
// ---------------------------------------------------------------------------

/// Inline CSS that positions the portal root for the given target.
/// For element anchors we attempt a one-shot measurement of the
/// target's viewport rect — if the target hasn't been mounted yet
/// (its `Ref` is still empty), we fall back to centering as a safe
/// default. The scroll/resize handlers re-measure once children
/// mount.
fn position_styles_for_target(target: &PortalTarget) -> String {
    match target {
        PortalTarget::Viewport(p) => viewport_placement_styles(*p).to_string(),
        PortalTarget::Anchor { target: anchor_target, side, align, offset } => {
            // Initial mount only — `requestAnimationFrame` will
            // remeasure and re-place after children have laid out.
            let Some(rect) = anchor_target.rect() else {
                return centered_fallback_styles().to_string();
            };
            anchor_styles(rect, *side, *align, *offset)
        }
        PortalTarget::Named(_) => {
            // No consumer yet — anything that tries to mount a named
            // portal should know it's unimplemented.
            unimplemented!("PortalTarget::Named not implemented on web")
        }
    }
}

/// CSS for the six viewport placements. The `Center` case uses
/// `top: 50%; left: 50%; transform: translate(-50%, -50%)` instead of
/// the `inset: 0; margin: auto` centering trick because the trick
/// requires explicit `width` / `height` and the portal root has
/// neither — we let it auto-size to its content.
fn viewport_placement_styles(placement: ViewportPlacement) -> &'static str {
    match placement {
        ViewportPlacement::Center => {
            "top: 50%; left: 50%; right: auto; bottom: auto; transform: translate(-50%, -50%);"
        }
        ViewportPlacement::Top => "top: 0; left: 0; right: 0;",
        ViewportPlacement::Bottom => "bottom: 0; left: 0; right: 0;",
        ViewportPlacement::Left => "top: 0; bottom: 0; left: 0;",
        ViewportPlacement::Right => "top: 0; bottom: 0; right: 0;",
        ViewportPlacement::FullScreen => "inset: 0;",
    }
}

/// Fallback when the anchor's ref isn't filled yet at create time.
/// Matches the `Center` viewport placement so the portal lands
/// somewhere sensible until the first-paint rAF re-measures.
fn centered_fallback_styles() -> &'static str {
    "top: 50%; left: 50%; right: auto; bottom: auto; transform: translate(-50%, -50%);"
}

/// Initial-mount anchor styles. We don't know the portal's rendered
/// size yet, so we use `transform: translate(...)` to flip the
/// origin based on `side`/`align`. The reposition tick on the next
/// animation frame replaces this with a measured-and-clamped
/// inline `top`/`left`.
fn anchor_styles(rect: ViewportRect, side: ElementSide, align: ElementAlign, offset: f32) -> String {
    let (top, left) = anchor_origin_position(rect, side, align, offset);
    let transform = match side {
        ElementSide::Below | ElementSide::End => match align {
            ElementAlign::Start => "",
            ElementAlign::Center => match side {
                ElementSide::Below => "transform: translateX(-50%);",
                _ => "transform: translateY(-50%);",
            },
            ElementAlign::End => match side {
                ElementSide::Below => "transform: translateX(-100%);",
                _ => "transform: translateY(-100%);",
            },
        },
        ElementSide::Above => match align {
            ElementAlign::Start => "transform: translateY(-100%);",
            ElementAlign::Center => "transform: translate(-50%, -100%);",
            ElementAlign::End => "transform: translate(-100%, -100%);",
        },
        ElementSide::Start => match align {
            ElementAlign::Start => "transform: translateX(-100%);",
            ElementAlign::Center => "transform: translate(-100%, -50%);",
            ElementAlign::End => "transform: translate(-100%, -100%);",
        },
    };
    format!("top: {}px; left: {}px; {}", top, left, transform)
}

/// Compute the unmeasured `(top, left)` *anchor point* relative to
/// the trigger. The final visual top-left of the portal is then
/// derived via a CSS `transform: translate(...)`. Used only on
/// initial mount, before the portal has been measured.
fn anchor_origin_position(
    rect: ViewportRect,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
) -> (f32, f32) {
    match side {
        ElementSide::Below => (rect.y + rect.height + offset, anchor_horizontal(rect, align)),
        ElementSide::Above => (rect.y - offset, anchor_horizontal(rect, align)),
        ElementSide::Start => (anchor_vertical(rect, align), rect.x - offset),
        ElementSide::End => (anchor_vertical(rect, align), rect.x + rect.width + offset),
    }
}

fn anchor_horizontal(rect: ViewportRect, align: ElementAlign) -> f32 {
    match align {
        ElementAlign::Start => rect.x,
        ElementAlign::Center => rect.x + rect.width / 2.0,
        ElementAlign::End => rect.x + rect.width,
    }
}

fn anchor_vertical(rect: ViewportRect, align: ElementAlign) -> f32 {
    match align {
        ElementAlign::Start => rect.y,
        ElementAlign::Center => rect.y + rect.height / 2.0,
        ElementAlign::End => rect.y + rect.height,
    }
}

/// Install the scroll/resize + first-paint reposition pipeline for an
/// anchored portal. Returns the scroll/resize closures (so they stay
/// alive until `release_portal` removes them) and the one-shot rAF
/// task for the initial measurement.
fn install_anchor_reposition(
    portal_root: &web_sys::Element,
    target: AnchorTarget,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
) -> (
    Option<Closure<dyn FnMut(web_sys::Event)>>,
    Option<Closure<dyn FnMut(web_sys::Event)>>,
    Option<runtime_core::ScheduledTask>,
) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return (None, None, None),
    };
    let portal_html: web_sys::HtmlElement = portal_root.clone().unchecked_into();

    // Measure-based reposition: read the portal's *rendered* rect via
    // `getBoundingClientRect`, pick the side with enough room for it,
    // compute the visual top-left directly (no transform-translate
    // trick — we know the size, so we just shift the box manually),
    // and clamp the top-left so the whole rendered rect stays inside
    // the viewport.
    let reposition: Rc<dyn Fn()> = Rc::new(move || {
        let Some(trigger) = target.rect() else {
            return;
        };
        let viewport = viewport_size();
        let portal_size = measure_portal_size(&portal_html);
        // Shared, host-tested placement resolver (side-flip + measured
        // position + viewport clamp). Lives in runtime_core so web/iOS/
        // Android can't drift (CLAUDE.md §7).
        let placement = runtime_core::primitives::portal::resolve_anchored_placement(
            trigger, portal_size, viewport, side, align, offset, EDGE_GAP,
        );
        let style = portal_html.style();
        let _ = style.remove_property("transform");
        let _ = style.set_property("top", &format!("{}px", placement.y));
        let _ = style.set_property("left", &format!("{}px", placement.x));
    });

    let reposition_scroll = reposition.clone();
    let scroll_closure: Closure<dyn FnMut(web_sys::Event)> =
        Closure::wrap(Box::new(move |_ev: web_sys::Event| {
            (reposition_scroll)();
        }) as Box<dyn FnMut(web_sys::Event)>);
    let _ = window.add_event_listener_with_callback_and_bool(
        "scroll",
        scroll_closure.as_ref().unchecked_ref(),
        true, // useCapture — catch nested scroll containers
    );

    let reposition_resize = reposition.clone();
    let resize_closure: Closure<dyn FnMut(web_sys::Event)> =
        Closure::wrap(Box::new(move |_ev: web_sys::Event| {
            (reposition_resize)();
        }) as Box<dyn FnMut(web_sys::Event)>);
    let _ = window.add_event_listener_with_callback(
        "resize",
        resize_closure.as_ref().unchecked_ref(),
    );

    // First-paint reposition: the walker hasn't inserted our
    // children yet, so the portal element has no measurable size
    // right now. The next animation frame runs after the walker's
    // `insert_children` call completes and the browser has laid the
    // children out.
    let reposition_initial = reposition.clone();
    let initial_measure_task = runtime_core::after_animation_frame(move || {
        (reposition_initial)();
    });

    (Some(scroll_closure), Some(resize_closure), Some(initial_measure_task))
}

/// Read the portal content's rendered `(width, height)` from the
/// DOM. Returns the bounding client rect's size; if the element has
/// no layout yet, returns `(0, 0)` and the caller's clamp will keep
/// the top-left at the gutter.
fn measure_portal_size(el: &web_sys::HtmlElement) -> (f32, f32) {
    let rect = el.get_bounding_client_rect();
    (rect.width() as f32, rect.height() as f32)
}

/// Viewport size in CSS pixels.
fn viewport_size() -> (f32, f32) {
    let Some(window) = web_sys::window() else { return (1024.0, 768.0) };
    let w = window
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1024.0) as f32;
    let h = window
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(768.0) as f32;
    (w, h)
}

// ---------------------------------------------------------------------------
// Focus trap
// ---------------------------------------------------------------------------

/// CSS selector matching every element that's natively keyboard-
/// focusable. Lifted from the standard focus-trap library set; the
/// `:not([tabindex="-1"])` filter excludes elements explicitly opted
/// out via `tabindex="-1"`.
const FOCUSABLE_SELECTOR: &str = concat!(
    "a[href]:not([tabindex=\"-1\"]),",
    "button:not([disabled]):not([tabindex=\"-1\"]),",
    "input:not([disabled]):not([tabindex=\"-1\"]),",
    "select:not([disabled]):not([tabindex=\"-1\"]),",
    "textarea:not([disabled]):not([tabindex=\"-1\"]),",
    "[tabindex]:not([tabindex=\"-1\"])"
);

/// Install a `focusin` listener on `document` that bounces focus
/// back to the portal's first focusable child whenever it escapes
/// the portal subtree.
///
/// ## Why this is an `Fn` closure, not `FnMut`
///
/// The handler's own `.focus()` call (below) synchronously
/// re-dispatches a `focusin` event, which re-enters THIS closure
/// before the outer invocation returns. wasm-bindgen guards
/// `Closure<dyn FnMut>` with an exclusive-borrow check at the FFI
/// boundary: a re-entrant call throws "closure invoked recursively
/// or after being dropped" *before* the Rust body runs — so the
/// `in_progress` flag never gets a chance to short-circuit it, and
/// the throw surfaces as an uncaught console error during any focus
/// bounce (and on teardown, when removing a focused portal moves
/// focus and triggers the handler). An `Fn` closure carries no such
/// guard — re-entry is memory-safe through the `RefCell` interior
/// mutability — so the inner call runs the body and the
/// `in_progress` flag cleanly bails it out.
pub(crate) fn install_focus_trap(
    doc: &web_sys::Document,
    portal_root: web_sys::Element,
) -> Option<Closure<dyn Fn(web_sys::Event)>> {
    let in_progress: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let portal_root_for_listener = portal_root.clone();
    let closure: Closure<dyn Fn(web_sys::Event)> =
        Closure::wrap(Box::new(move |ev: web_sys::Event| {
            // Re-entry guard: our own `.focus()` triggers another
            // `focusin`. Bail without recursing.
            if *in_progress.borrow() {
                return;
            }
            let target_node: Option<web_sys::Node> =
                ev.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
            let Some(target_node) = target_node else {
                return;
            };
            // Focus is already inside the portal subtree — nothing
            // to do. `Node.contains` returns true for the node
            // itself, so this also accepts focus landing on the
            // portal root.
            let portal_node: &web_sys::Node = portal_root_for_listener.as_ref();
            if portal_node.contains(Some(&target_node)) {
                return;
            }
            // Focus escaped. Find the first focusable descendant
            // and route focus back to it. If nothing is focusable,
            // focus the portal root itself (its `tabindex` is set
            // below).
            let first_focusable = portal_root_for_listener
                .query_selector(FOCUSABLE_SELECTOR)
                .ok()
                .flatten();
            *in_progress.borrow_mut() = true;
            if let Some(el) = first_focusable {
                if let Ok(h) = el.dyn_into::<web_sys::HtmlElement>() {
                    let _ = h.focus();
                }
            } else if let Ok(h) = portal_root_for_listener.clone().dyn_into::<web_sys::HtmlElement>() {
                let _ = h.focus();
            }
            *in_progress.borrow_mut() = false;
        }) as Box<dyn Fn(web_sys::Event)>);

    // `tabindex="-1"` lets the portal root itself receive focus
    // programmatically (so the fallback `portal_root.focus()` works)
    // without inserting it into the natural Tab order.
    let _ = portal_root.set_attribute("tabindex", "-1");

    let _ = doc.add_event_listener_with_callback(
        "focusin",
        closure.as_ref().unchecked_ref(),
    );
    Some(closure)
}
