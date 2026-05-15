//! `Primitive::Overlay` — a floating subtree portaled to `<body>`.
//!
//! # Why we portal
//!
//! Modals, popovers, dropdowns, etc. need to render *above* every
//! other element in the layout, including parents with `overflow:
//! hidden`, `transform`, or `position` styles that establish new
//! stacking contexts. The browser's z-index doesn't reach across
//! those boundaries. The standard fix on the web is to "portal" —
//! render the overlay outside its logical parent, directly under
//! `<body>`, where no ancestor can clip or restack it.
//!
//! React, Vue, Solid, and friends all expose this as
//! `<Portal>` / `Teleport`. The framework's overlay primitive bakes
//! it in.
//!
//! # The insert hijack
//!
//! The render walker calls `Backend::insert(parent, child)` to
//! parent every primitive into its surrounding container. For
//! overlays we don't want that — the overlay is already attached
//! to `<body>`. To keep the walker simple, we stamp every
//! overlay-created `<div>` with `data-overlay-root="1"` and the
//! global `view::insert` checks the attribute and skips the
//! `append_child` call. The child stays where we put it.
//!
//! # Layout
//!
//! Three pieces per mount:
//!
//! 1. The **portal root** — a `position: fixed; inset: 0;
//!    pointer-events: none` `<div>` parented to `<body>`. Provides
//!    a viewport-sized hit-test surface; doesn't itself absorb
//!    clicks (children re-enable pointer events).
//! 2. The **backdrop** — child `<div>` of the portal root, sized
//!    to the viewport, holds the scrim color + optional
//!    click-to-dismiss handler. Hidden entirely when `BackdropMode::None`.
//! 3. The **content container** — the second child of the portal
//!    root, holds the user's overlay children. Positioned according
//!    to the `anchor`: centered/edge for viewport anchors,
//!    measured-then-placed for element anchors.
//!
//! The framework's `insert_children` call after `create_overlay`
//! returns parents the user's children to the **content container**,
//! not the portal root. We return the content container as the
//! "node" the framework sees, so its `insert` calls naturally hit
//! the right element.

use crate::WebBackend;
use framework_core::primitives::overlay::{
    BackdropMode, ElementAlign, ElementSide, OverlayAnchor, OverlayHandle, OverlayOps,
    ViewportPlacement, ViewportRect,
};
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

/// Per-overlay runtime state. Stored in `WebBackend::overlay_instances`,
/// keyed by the `data-overlay-id` attribute stamped on the portal
/// root. Holds the JS closures so they stay alive while the overlay
/// is mounted; dropping the instance fires `Closure::Drop` which
/// destroys the wasm-bindgen handle and frees its JS-side allocation.
pub(crate) struct OverlayInstance {
    /// The outer `<div>` parented to `<body>`. Removing this from
    /// `<body>` on `release` tears down the entire overlay subtree.
    portal_root: web_sys::Element,
    /// Click handler attached to the backdrop. Held so JS doesn't
    /// drop it while the overlay is alive. Only populated when
    /// `BackdropMode::Dismiss` was requested.
    #[allow(dead_code)]
    backdrop_click_handler: Option<Closure<dyn FnMut(web_sys::Event)>>,
    /// Escape-key handler attached to `document`. Removed in
    /// `release_overlay`. Only populated when the overlay has an
    /// `on_dismiss` callback.
    #[allow(dead_code)]
    escape_handler: Option<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
}

/// All live overlay instances, keyed by `data-overlay-id`.
pub(crate) type OverlayInstances = HashMap<u32, OverlayInstance>;

pub(crate) fn create(
    b: &mut WebBackend,
    anchor: OverlayAnchor,
    backdrop: BackdropMode,
    on_dismiss: Option<Rc<dyn Fn()>>,
    _trap_focus: bool,
) -> Node {
    inject_overlay_css(b);

    let id = b.next_overlay_id;
    b.next_overlay_id += 1;

    // ---- Portal root: viewport-pinned, parented to <body> ----
    let portal_root = b
        .doc
        .create_element("div")
        .expect("create_element div failed");
    let _ = portal_root.set_attribute("data-overlay-root", "1");
    let _ = portal_root.set_attribute("data-overlay-id", &id.to_string());
    // `pointer-events: none` here means clicks outside the content
    // container fall through to the page underneath. The backdrop +
    // content children re-enable pointer events for themselves.
    let _ = portal_root.set_attribute(
        "style",
        "position: fixed; inset: 0; pointer-events: none; z-index: 1000;",
    );

    // ---- Backdrop ----
    let backdrop_el = b
        .doc
        .create_element("div")
        .expect("create_element div failed");
    let backdrop_style = match backdrop {
        BackdropMode::Dismiss | BackdropMode::Opaque => {
            // Full-viewport scrim that captures pointer events.
            "position: absolute; inset: 0; pointer-events: auto; \
             background: rgba(0, 0, 0, 0.45);"
        }
        BackdropMode::None => {
            // No scrim — hide the element entirely. Keep it in the
            // DOM as a slot so style-application code (which targets
            // the backdrop element) doesn't have to special-case
            // None.
            "display: none;"
        }
    };
    let _ = backdrop_el.set_attribute("style", backdrop_style);
    let _ = backdrop_el.set_attribute("data-overlay-backdrop", "1");
    portal_root
        .append_child(&backdrop_el)
        .expect("append backdrop");

    // Wire backdrop click → on_dismiss when in Dismiss mode.
    let backdrop_click_handler = if matches!(backdrop, BackdropMode::Dismiss) {
        on_dismiss.clone().map(|dismiss| {
            let closure = Closure::wrap(Box::new(move |_ev: web_sys::Event| {
                (dismiss)();
            }) as Box<dyn FnMut(web_sys::Event)>);
            let _ = backdrop_el.add_event_listener_with_callback(
                "click",
                closure.as_ref().unchecked_ref(),
            );
            closure
        })
    } else {
        None
    };

    // ---- Content container ----
    let content = b
        .doc
        .create_element("div")
        .expect("create_element div failed");
    // Position the content according to the anchor. Element anchors
    // need measurement, which we do once on mount; the framework
    // doesn't yet drive re-positioning on scroll/resize.
    let content_style = position_styles_for_anchor(&anchor);
    let _ = content.set_attribute("style", &content_style);
    let _ = content.set_attribute("data-overlay-content", "1");
    portal_root.append_child(&content).expect("append content");

    // ---- Escape-key handler on document ----
    let escape_handler = on_dismiss.clone().map(|dismiss| {
        let closure = Closure::wrap(Box::new(move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" {
                (dismiss)();
            }
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);
        let _ = b.doc.add_event_listener_with_callback(
            "keydown",
            closure.as_ref().unchecked_ref(),
        );
        closure
    });

    // ---- Attach portal root to <body> ----
    if let Some(body) = b.doc.body() {
        let _ = body.append_child(&portal_root);
    }

    // ---- Stash the instance for cleanup ----
    b.overlay_instances.insert(
        id,
        OverlayInstance {
            portal_root: portal_root.clone(),
            backdrop_click_handler,
            escape_handler,
        },
    );

    // Return the CONTENT container as the framework's Node. The
    // walker will call insert(parent=content, child=...) for each
    // user child, populating the content with the overlay's body.
    // We also stamp the content with the same `data-overlay-root`
    // attribute the `view::insert` hijack looks for — but on the
    // content this is unwanted (children should be inserted INTO
    // it). Instead, we use `data-overlay-id` for cleanup lookup
    // and rely on the surrounding scope's `insert(parent, this)`
    // call landing on a node that's been stamped with the SKIP
    // attribute. So we add the SKIP attribute now:
    let _ = content.set_attribute("data-overlay-skip-insert", "1");
    content.unchecked_into::<Node>()
}

/// Apply a resolved-style rules block to the overlay's backdrop
/// element. Located by walking up from the content node we returned
/// in `create` (which is the framework's tracked node) to the portal
/// root, then finding the backdrop child.
pub(crate) fn apply_backdrop_style(
    b: &mut WebBackend,
    node: &Node,
    rules: &Rc<framework_core::StyleRules>,
) {
    // The "node" the framework hands us is the CONTENT container.
    // Walk to its parent (the portal root) and find the backdrop
    // child by its data attribute.
    let content: &web_sys::Element = match node.dyn_ref::<web_sys::Element>() {
        Some(el) => el,
        None => return,
    };
    let portal_root = match content.parent_element() {
        Some(p) => p,
        None => return,
    };
    let backdrop = match portal_root
        .query_selector("[data-overlay-backdrop]")
        .ok()
        .flatten()
    {
        Some(b) => b,
        None => return,
    };
    // Reuse the existing rules-to-css emitter so backdrop styling
    // honors the full property set (background, opacity,
    // transitions, etc.).
    let css = crate::style::rules_to_css(rules);
    let _ = backdrop.set_attribute("style", &format!("position: absolute; inset: 0; pointer-events: auto; {}", css));
    // Re-apply pointer events / inset since the user's style block
    // may have wiped them.
    let _ = b; // silence unused param when no backend state touched
}

/// Tear down an overlay. Removes the portal from `<body>`, drops the
/// closures (Escape + backdrop-click), and removes the instance
/// entry. Called by `release_overlay` when the surrounding scope
/// drops.
pub(crate) fn release(b: &mut WebBackend, node: &Node) {
    // The node we received is the CONTENT container; walk up to the
    // portal root.
    let content: &web_sys::Element = match node.dyn_ref::<web_sys::Element>() {
        Some(el) => el,
        None => return,
    };
    let portal_root = match content.parent_element() {
        Some(p) => p,
        None => return,
    };
    let id_str = portal_root
        .get_attribute("data-overlay-id")
        .unwrap_or_default();
    let id: u32 = id_str.parse().unwrap_or(u32::MAX);

    // Detach from <body>. Browsers automatically remove the
    // element's event listeners as part of GC once nothing else
    // holds a reference, but our wasm-bindgen Closure handles are
    // kept alive by the OverlayInstance entry — dropping that
    // entry below is what actually frees them.
    if let Some(body) = b.doc.body() {
        let _ = body.remove_child(portal_root.unchecked_ref());
    }

    // Drop the instance — fires Closure::Drop on the held handlers,
    // freeing wasm-bindgen state JS-side.
    if id != u32::MAX {
        b.overlay_instances.remove(&id);
    }
}

pub(crate) fn make_handle(node: &Node) -> OverlayHandle {
    let el: web_sys::HtmlElement = node
        .clone()
        .dyn_into()
        .expect("overlay node is not an HtmlElement");
    OverlayHandle::new(Rc::new(el), &WebOverlayOps)
}

struct WebOverlayOps;
impl OverlayOps for WebOverlayOps {}

// ---------------------------------------------------------------------------
// Positioning
// ---------------------------------------------------------------------------

/// Inline CSS that positions the content container for the given
/// anchor. For element anchors we attempt a one-shot measurement of
/// the target's viewport rect — if the target hasn't been mounted
/// yet (its `Ref` is still empty), we fall back to centering as a
/// safe default.
fn position_styles_for_anchor(anchor: &OverlayAnchor) -> String {
    let base = "position: absolute; pointer-events: auto;";
    match anchor {
        OverlayAnchor::Viewport(p) => {
            let placement = match p {
                ViewportPlacement::Center => {
                    "top: 50%; left: 50%; transform: translate(-50%, -50%);"
                }
                ViewportPlacement::Top => "top: 0; left: 0; right: 0;",
                ViewportPlacement::Bottom => "bottom: 0; left: 0; right: 0;",
                ViewportPlacement::Left => "top: 0; bottom: 0; left: 0;",
                ViewportPlacement::Right => "top: 0; bottom: 0; right: 0;",
                ViewportPlacement::FullScreen => "inset: 0;",
            };
            format!("{} {}", base, placement)
        }
        OverlayAnchor::Element(e) => {
            // Measure target now. If the target isn't filled yet
            // (Ref hasn't been bound at mount time), fall back to
            // centering — better than positioning at (0, 0).
            let Some(rect) = e.target.rect() else {
                return format!(
                    "{} top: 50%; left: 50%; transform: translate(-50%, -50%);",
                    base
                );
            };
            element_anchor_styles(base, rect, e.side, e.align, e.offset)
        }
    }
}

fn element_anchor_styles(
    base: &str,
    rect: ViewportRect,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
) -> String {
    // Decide the position relative to the target's rect.
    let (top, left) = match side {
        ElementSide::Below => (rect.y + rect.height + offset, anchor_horizontal(rect, align)),
        ElementSide::Above => (rect.y - offset, anchor_horizontal(rect, align)),
        ElementSide::Start => (anchor_vertical(rect, align), rect.x - offset),
        ElementSide::End => (anchor_vertical(rect, align), rect.x + rect.width + offset),
    };
    // For `Above` and `Start` we want the overlay to grow back
    // toward the anchor — use a translate to flip the origin.
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
    format!("{} top: {}px; left: {}px; {}", base, top, left, transform)
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

// ---------------------------------------------------------------------------
// CSS injection
// ---------------------------------------------------------------------------

/// Idempotent — first overlay mount stamps the flag so future calls
/// short-circuit. No global rules are needed today; every overlay
/// sets its position inline. Reserved for future use (e.g. focus-trap
/// styling, scrollbar suppression while a modal is open).
fn inject_overlay_css(b: &mut WebBackend) {
    if b.overlay_css_injected {
        return;
    }
    b.overlay_css_injected = true;
}
