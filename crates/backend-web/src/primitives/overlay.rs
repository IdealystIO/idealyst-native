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
use std::cell::RefCell;
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
    /// Document-level mousedown handler that fires `on_dismiss`
    /// when the click target is OUTSIDE the overlay's content
    /// element. Only populated for `BackdropMode::None` overlays
    /// (popovers, dropdowns, tooltips) whose host wants
    /// click-outside-to-close behavior. Modal/Drawer use a real
    /// scrim (handled by `backdrop_click_handler`) so this stays
    /// `None` for them.
    ///
    /// Wrapped in `Rc<RefCell<Option<…>>>` because installation is
    /// deferred — a `requestAnimationFrame` callback writes into
    /// this slot one frame after mount. Sharing the slot between
    /// the install task and the instance lets `release_overlay`
    /// retrieve the closure (if it was installed) and call
    /// `removeEventListener` so the document doesn't keep firing
    /// callbacks against freed Rust state.
    document_click_handler: Rc<RefCell<Option<Closure<dyn FnMut(web_sys::Event)>>>>,
    /// Pending install task for `document_click_handler`. Holding
    /// it lets a quick teardown cancel the install before it runs
    /// — otherwise the wasm-bindgen `Closure` inside the task
    /// would be dropped while the browser still had it queued.
    /// Dropped together with the rest of the instance.
    #[allow(dead_code)]
    deferred_install: Option<framework_core::ScheduledTask>,
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
    // are measured once at mount; on scroll the overlay stays at its
    // initial viewport coordinates (same trade-off as MUI/Quasar —
    // chasing the trigger via scroll events feels floaty because JS
    // scroll events fire after the compositor has already painted).
    // We compensate by ensuring the initial placement always fits
    // inside the viewport (see `position_styles_for_anchor`).
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

    // ---- Click-outside dismissal for BackdropMode::None ----
    //
    // Modal/Drawer get click-outside dismissal via their scrim
    // element (already wired above). Popovers / dropdowns /
    // tooltips use `BackdropMode::None` — no scrim — but the host
    // usually wants clicking elsewhere on the page to close the
    // overlay. So when there's no scrim AND an on_dismiss, we
    // install a document-level `mousedown` listener that fires
    // dismiss when the click target isn't inside our content
    // element.
    //
    // Two subtleties:
    //
    // 1. The install is deferred to the next animation frame.
    //    Otherwise the *same click* that originally opened the
    //    overlay (still in flight through the browser's click
    //    dispatch after the host's on_click flipped the signal)
    //    would be caught as an outside-click and re-close the
    //    overlay immediately.
    //
    // 2. The closure handle has to be reachable from `release` so
    //    we can `removeEventListener` — otherwise the listener
    //    keeps firing for the rest of the session, referencing a
    //    dismiss callback that's been freed. We share the closure
    //    via an `Rc<RefCell<Option<Closure>>>` slot: the install
    //    task writes the closure in when it runs; release reads it
    //    out and detaches the listener.
    let document_click_handler: Rc<RefCell<Option<Closure<dyn FnMut(web_sys::Event)>>>> =
        Rc::new(RefCell::new(None));
    let deferred_install: Option<framework_core::ScheduledTask> =
        if matches!(backdrop, BackdropMode::None) {
            on_dismiss.clone().map(|dismiss| {
                let doc = b.doc.clone();
                let content_for_listener = content.clone();
                let slot_for_install = document_click_handler.clone();
                framework_core::after_animation_frame(move || {
                    let closure: Closure<dyn FnMut(web_sys::Event)> =
                        Closure::wrap(Box::new(move |ev: web_sys::Event| {
                            let target_node: Option<web_sys::Node> = ev
                                .target()
                                .and_then(|t| t.dyn_into::<web_sys::Node>().ok());
                            let Some(target_node) = target_node else {
                                return;
                            };
                            if !content_for_listener.contains(Some(&target_node)) {
                                (dismiss)();
                            }
                        }) as Box<dyn FnMut(web_sys::Event)>);
                    let _ = doc.add_event_listener_with_callback(
                        "mousedown",
                        closure.as_ref().unchecked_ref(),
                    );
                    *slot_for_install.borrow_mut() = Some(closure);
                })
            })
        } else {
            None
        };

    // ---- Page scroll-lock ----
    //
    // While any overlay is open we freeze the page so the trigger
    // can't scroll out from under an Element-anchored overlay
    // (popover, dropdown), and so the background doesn't move
    // distractingly behind a modal/drawer. Refcounted, so stacked
    // overlays release the lock at the right moment.
    acquire_scroll_lock(b);

    // ---- Stash the instance for cleanup ----
    b.overlay_instances.insert(
        id,
        OverlayInstance {
            portal_root: portal_root.clone(),
            backdrop_click_handler,
            escape_handler,
            document_click_handler,
            deferred_install,
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

    // The document-level click-outside listener (popover case)
    // doesn't go away just because the overlay's content was
    // detached from the DOM. We have to explicitly
    // removeEventListener with the same closure that was added;
    // otherwise the browser keeps invoking it for the rest of the
    // page session, and after the instance drops below the
    // closure's captured `dismiss` Rc points at freed framework
    // state.
    if id != u32::MAX {
        if let Some(inst) = b.overlay_instances.get(&id) {
            if let Some(closure) = inst.document_click_handler.borrow().as_ref() {
                let _ = b.doc.remove_event_listener_with_callback(
                    "mousedown",
                    closure.as_ref().unchecked_ref(),
                );
            }
        }
    }

    // Drop the instance — fires Closure::Drop on the held handlers,
    // freeing wasm-bindgen state JS-side.
    if id != u32::MAX {
        b.overlay_instances.remove(&id);
    }

    // Drop our scroll-lock refcount. On the final release this
    // restores `body.overflow` to whatever the app had set
    // pre-overlay.
    release_scroll_lock(b);
}

/// Increment the scroll-lock refcount. On the `0 → 1` transition,
/// save the current `document.body` `overflow` value and force it
/// to `hidden`. Subsequent increments are no-ops at the DOM level
/// (one set is enough).
fn acquire_scroll_lock(b: &mut WebBackend) {
    b.scroll_lock_count = b.scroll_lock_count.saturating_add(1);
    if b.scroll_lock_count != 1 {
        return;
    }
    let Some(body) = b.doc.body() else { return };
    let html: web_sys::HtmlElement = match body.dyn_into() {
        Ok(el) => el,
        Err(_) => return,
    };
    // Save whatever the app had set (likely empty / unset) so we can
    // restore it precisely on release. Using `getPropertyValue` on
    // the inline style — *not* `getComputedStyle` — because we want
    // to round-trip only the inline declaration, not bake the
    // computed value (e.g. "visible") back in as inline.
    let saved = html.style().get_property_value("overflow").unwrap_or_default();
    b.saved_body_overflow = Some(saved);
    let _ = html.style().set_property("overflow", "hidden");
}

/// Decrement the scroll-lock refcount. On the `1 → 0` transition,
/// restore the saved `body.overflow` value. Other transitions leave
/// the lock in place.
fn release_scroll_lock(b: &mut WebBackend) {
    if b.scroll_lock_count == 0 {
        return;
    }
    b.scroll_lock_count -= 1;
    if b.scroll_lock_count != 0 {
        return;
    }
    let Some(body) = b.doc.body() else { return };
    let html: web_sys::HtmlElement = match body.dyn_into() {
        Ok(el) => el,
        Err(_) => return,
    };
    let saved = b.saved_body_overflow.take().unwrap_or_default();
    if saved.is_empty() {
        let _ = html.style().remove_property("overflow");
    } else {
        let _ = html.style().set_property("overflow", &saved);
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
    // Important: positioning must NOT use the `transform` CSS
    // property. Presence animations write to `transform` for their
    // own translate/scale, and CSS doesn't compose two inline
    // `transform` writes — the later one overwrites the earlier.
    // So Center uses the `inset: 0; margin: auto` centering trick
    // (modern CSS, works on every shipping browser) which leaves
    // `transform` free for presence.
    let base = "position: absolute; pointer-events: auto;";
    match anchor {
        OverlayAnchor::Viewport(p) => {
            let placement = match p {
                // Centering without transform: `inset: 0` pins the
                // box to all four edges; `margin: auto` distributes
                // remaining space equally on each axis; `width:
                // max-content` lets the box size to its content
                // rather than expanding to the viewport.
                ViewportPlacement::Center => {
                    "inset: 0; margin: auto; width: max-content; height: max-content;"
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
            // viewport-centered (same transform-free centering as
            // above).
            let Some(rect) = e.target.rect() else {
                return format!(
                    "{} inset: 0; margin: auto; width: max-content; height: max-content;",
                    base
                );
            };
            // Flip the requested side if the trigger is too close to
            // the corresponding viewport edge to fit the overlay. We
            // don't know the overlay's actual size yet (its children
            // haven't been inserted), so we use a conservative
            // 200px estimate — enough for a typical select menu /
            // tooltip / dropdown to fit in the remaining space test.
            let viewport = viewport_size();
            let side = flip_side_to_fit(e.side, rect, viewport, e.offset);
            element_anchor_styles(base, rect, side, e.align, e.offset, viewport)
        }
    }
}

/// Pick the actual side the overlay should anchor on. If the
/// requested side doesn't have room for an estimated overlay size,
/// flip to the opposite side — *unless* the opposite is even
/// tighter (then we keep the original and let the overlay overflow,
/// matching what most popover libs do).
///
/// Estimate is intentionally rough: we don't know the overlay's
/// rendered size until after children are inserted, so we use a
/// 200px slot and compare relative space on each side. This makes
/// the right call for the common case (trigger near the bottom of
/// the viewport → flip to Above) without measuring.
fn flip_side_to_fit(
    side: ElementSide,
    rect: ViewportRect,
    viewport: (f32, f32),
    offset: f32,
) -> ElementSide {
    const ESTIMATED_OVERLAY_SIZE: f32 = 200.0;
    let (vw, vh) = viewport;
    match side {
        ElementSide::Below => {
            let space_below = vh - (rect.y + rect.height + offset);
            let space_above = rect.y - offset;
            if space_below < ESTIMATED_OVERLAY_SIZE && space_above > space_below {
                ElementSide::Above
            } else {
                ElementSide::Below
            }
        }
        ElementSide::Above => {
            let space_above = rect.y - offset;
            let space_below = vh - (rect.y + rect.height + offset);
            if space_above < ESTIMATED_OVERLAY_SIZE && space_below > space_above {
                ElementSide::Below
            } else {
                ElementSide::Above
            }
        }
        ElementSide::Start => {
            let space_start = rect.x - offset;
            let space_end = vw - (rect.x + rect.width + offset);
            if space_start < ESTIMATED_OVERLAY_SIZE && space_end > space_start {
                ElementSide::End
            } else {
                ElementSide::Start
            }
        }
        ElementSide::End => {
            let space_end = vw - (rect.x + rect.width + offset);
            let space_start = rect.x - offset;
            if space_end < ESTIMATED_OVERLAY_SIZE && space_start > space_end {
                ElementSide::Start
            } else {
                ElementSide::End
            }
        }
    }
}

/// Viewport size in CSS pixels. Used by the flip + clamp logic.
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

/// Compute `(top, left)` for an element anchor before any
/// viewport-fit clamping.
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

fn element_anchor_styles(
    base: &str,
    rect: ViewportRect,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    viewport: (f32, f32),
) -> String {
    // Decide the position relative to the target's rect.
    let (top, left) = anchor_origin_position(rect, side, align, offset);
    // Clamp `left` horizontally so the overlay stays inside the
    // viewport when the trigger is near a vertical edge. We don't
    // know the actual overlay width yet (its children haven't been
    // inserted), but we know the *anchor point* — for `Center`
    // alignment the overlay grows symmetrically from `left`, so we
    // bias toward keeping `left` at least one half-width inside each
    // edge using an estimate. For `Start` / `End` alignment the
    // overlay grows in one direction so just clamping `left` to
    // `[edge_gap, viewport - edge_gap]` is enough — if the overlay
    // is wider than the viewport everything overflows anyway.
    let (vw, vh) = viewport;
    const EDGE_GAP: f32 = 8.0;
    let left = match side {
        ElementSide::Below | ElementSide::Above => {
            left.clamp(EDGE_GAP, (vw - EDGE_GAP).max(EDGE_GAP))
        }
        _ => left,
    };
    let top = match side {
        ElementSide::Start | ElementSide::End => {
            top.clamp(EDGE_GAP, (vh - EDGE_GAP).max(EDGE_GAP))
        }
        _ => top,
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
