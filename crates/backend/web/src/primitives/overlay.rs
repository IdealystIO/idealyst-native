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
    AnchorTarget, AnchoredOverlayHandle, AnchoredOverlayOps, BackdropMode, ElementAlign,
    ElementSide, OverlayHandle, OverlayOps, ViewportPlacement, ViewportRect,
};

/// Internal discriminator that lets the shared `create` helper
/// handle both viewport-anchored ([`Backend::create_overlay`]) and
/// element-anchored ([`Backend::create_anchored_overlay`]) overlays
/// without two near-duplicate copies. Public-facing types are split;
/// the web implementation just routes through here.
///
/// [`Backend::create_overlay`]: framework_core::Backend::create_overlay
/// [`Backend::create_anchored_overlay`]: framework_core::Backend::create_anchored_overlay
#[derive(Clone)]
pub(crate) enum OverlaySpec {
    Viewport(ViewportPlacement),
    Element {
        target: AnchorTarget,
        side: ElementSide,
        align: ElementAlign,
        offset: f32,
    },
}
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
    /// Scroll + resize handlers that re-measure the anchor target
    /// and rewrite the content's inline `top`/`left` so the overlay
    /// keeps tracking the trigger as the page scrolls or the
    /// viewport resizes. Only populated for `OverlayAnchor::Element`
    /// overlays — viewport-anchored overlays pin via `inset` /
    /// `margin: auto` already.
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
    /// `create_overlay` *before* it inserts the overlay's children,
    /// so the content element has no size when we install the
    /// initial inline `top`/`left`. We schedule a one-shot
    /// `requestAnimationFrame` here that fires once the children
    /// have mounted and a paint has measured them, at which point
    /// the shared `reposition` closure replaces the initial
    /// placement with a measured-and-clamped version.
    ///
    /// Held on the instance so a fast teardown (overlay closed
    /// before the frame fires) drops the rAF and the wasm-bindgen
    /// `Closure` inside it. Without that the rAF would still fire
    /// against a freed Rust state.
    #[allow(dead_code)]
    initial_measure_task: Option<framework_core::ScheduledTask>,
}

/// All live overlay instances, keyed by `data-overlay-id`.
pub(crate) type OverlayInstances = HashMap<u32, OverlayInstance>;

/// Public entry point for viewport-anchored overlays — wraps the
/// shared `create` helper with `OverlaySpec::Viewport`.
pub(crate) fn create_viewport(
    b: &mut WebBackend,
    placement: ViewportPlacement,
    backdrop: BackdropMode,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
) -> Node {
    create(b, OverlaySpec::Viewport(placement), backdrop, on_dismiss, trap_focus)
}

/// Public entry point for element-anchored overlays.
pub(crate) fn create_anchored(
    b: &mut WebBackend,
    target: AnchorTarget,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    backdrop: BackdropMode,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
) -> Node {
    create(
        b,
        OverlaySpec::Element { target, side, align, offset },
        backdrop,
        on_dismiss,
        trap_focus,
    )
}

pub(crate) fn create(
    b: &mut WebBackend,
    anchor: OverlaySpec,
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

    // ---- Scroll / resize reposition for Element anchors ----
    //
    // The element-anchored overlay measures the trigger once at
    // mount. Without these handlers the inline `top`/`left` would
    // become stale as the page scrolls. We re-measure on every
    // scroll + resize and rewrite *just* `top`/`left` (not the
    // whole inline `style`, which would clobber presence-driven
    // `transform`/`opacity` writes).
    //
    // `capture: true` on scroll is the standard trick: scroll
    // events from nested scroll containers don't bubble (spec
    // disables bubbling for `scroll`), so a `window` listener with
    // `useCapture=true` is the only way to catch ancestor scrolls
    // without attaching one listener per scroll container.
    //
    // Viewport anchors don't need re-measurement — `inset`/`margin:
    // auto` already pin them to the viewport regardless of scroll.
    let (reposition_scroll_handler, reposition_resize_handler, initial_measure_task) =
        if let OverlaySpec::Element { target, side: el_side, align: el_align, offset: el_offset } =
            &anchor
        {
            let window = web_sys::window().expect("window");
            let content_html: web_sys::HtmlElement = content.clone().unchecked_into();
            let target_for_reposition = target.clone();
            let side_for_reposition = *el_side;
            let align_for_reposition = *el_align;
            let offset_for_reposition = *el_offset;
            // Measure-based reposition: read the overlay's *rendered*
            // rect via `getBoundingClientRect`, pick the side with
            // enough room for it, compute the visual top-left
            // directly (no transform-translate trick — we know the
            // size, so we just shift the box manually), and clamp the
            // top-left so the whole rendered rect stays inside the
            // viewport.
            //
            // No hardcoded estimates: every measurement comes from
            // the browser. The first tick (post-mount rAF) provides
            // the initial fit; subsequent scroll/resize ticks
            // re-evaluate using whatever the overlay measures *now*
            // (its size can change if the content reflows).
            let reposition: Rc<dyn Fn()> = Rc::new(move || {
                let Some(trigger) = target_for_reposition.rect() else {
                    return;
                };
                let viewport = viewport_size();
                let overlay_size = measure_overlay_size(&content_html);
                let side = pick_side(
                    side_for_reposition,
                    trigger,
                    overlay_size,
                    viewport,
                    offset_for_reposition,
                );
                let (top, left) = measured_position(
                    trigger,
                    side,
                    align_for_reposition,
                    offset_for_reposition,
                    overlay_size,
                );
                let (top, left) = clamp_measured(top, left, overlay_size, viewport);
                let style = content_html.style();
                let _ = style.remove_property("transform");
                let _ = style.set_property("top", &format!("{}px", top));
                let _ = style.set_property("left", &format!("{}px", left));
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
            // children yet, so the content element has no measurable
            // size right now. The next animation frame runs after
            // the walker's `insert_children` call completes and the
            // browser has laid the children out, so we can measure
            // and re-place from a real `getBoundingClientRect`.
            let reposition_initial = reposition.clone();
            let initial_measure_task = framework_core::after_animation_frame(move || {
                (reposition_initial)();
            });

            (
                Some(scroll_closure),
                Some(resize_closure),
                Some(initial_measure_task),
            )
        } else {
            (None, None, None)
        };

    // ---- Stash the instance for cleanup ----
    b.overlay_instances.insert(
        id,
        OverlayInstance {
            portal_root: portal_root.clone(),
            backdrop_click_handler,
            escape_handler,
            document_click_handler,
            deferred_install,
            reposition_scroll_handler,
            reposition_resize_handler,
            initial_measure_task,
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

    // Every document- or window-level listener installed during
    // `create` has to be explicitly removed here. Dropping the
    // OverlayInstance below frees the wasm-bindgen `Closure` handle,
    // which destroys the JS-side wrapper — but the EventTarget (the
    // `document` / `window`) still has the wrapper registered as a
    // listener. The next matching event will invoke a freed closure
    // and throw "closure invoked recursively or after being dropped"
    // — once per leaked listener, per event. Removing here keeps the
    // listener count at zero across overlay lifetimes.
    //
    // Element-level listeners (the backdrop's click handler) don't
    // need this dance: the element itself is removed from the DOM
    // when we detach the portal root, and the browser drops any
    // pending listeners on detached subtrees.
    if id != u32::MAX {
        if let Some(inst) = b.overlay_instances.get(&id) {
            if let Some(closure) = inst.escape_handler.as_ref() {
                let _ = b.doc.remove_event_listener_with_callback(
                    "keydown",
                    closure.as_ref().unchecked_ref(),
                );
            }
            if let Some(closure) = inst.document_click_handler.borrow().as_ref() {
                let _ = b.doc.remove_event_listener_with_callback(
                    "mousedown",
                    closure.as_ref().unchecked_ref(),
                );
            }
            if let Some(window) = web_sys::window() {
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
        }
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

pub(crate) fn make_anchored_handle(node: &Node) -> AnchoredOverlayHandle {
    let el: web_sys::HtmlElement = node
        .clone()
        .dyn_into()
        .expect("overlay node is not an HtmlElement");
    AnchoredOverlayHandle::new(Rc::new(el), &WebAnchoredOverlayOps)
}

struct WebOverlayOps;
impl OverlayOps for WebOverlayOps {}

struct WebAnchoredOverlayOps;
impl AnchoredOverlayOps for WebAnchoredOverlayOps {}

// ---------------------------------------------------------------------------
// Positioning
// ---------------------------------------------------------------------------

/// Inline CSS that positions the content container for the given
/// anchor. For element anchors we attempt a one-shot measurement of
/// the target's viewport rect — if the target hasn't been mounted
/// yet (its `Ref` is still empty), we fall back to centering as a
/// safe default.
fn position_styles_for_anchor(anchor: &OverlaySpec) -> String {
    // Important: positioning must NOT use the `transform` CSS
    // property. Presence animations write to `transform` for their
    // own translate/scale, and CSS doesn't compose two inline
    // `transform` writes — the later one overwrites the earlier.
    // So Center uses the `inset: 0; margin: auto` centering trick
    // (modern CSS, works on every shipping browser) which leaves
    // `transform` free for presence.
    let base = "position: absolute; pointer-events: auto;";
    match anchor {
        OverlaySpec::Viewport(p) => {
            let placement = match p {
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
        OverlaySpec::Element { target, side, align, offset } => {
            // Initial mount only — `requestAnimationFrame` will
            // remeasure and re-place after children have laid out.
            let Some(rect) = target.rect() else {
                return format!(
                    "{} inset: 0; margin: auto; width: max-content; height: max-content;",
                    base
                );
            };
            element_anchor_styles(base, rect, *side, *align, *offset)
        }
    }
}

/// Pick the side the overlay should anchor on, given the rendered
/// overlay size. If the requested side doesn't fit the actual
/// overlay, flip to the opposite side — *unless* the opposite is
/// even tighter (then keep the original and let it overflow,
/// matching what most popover libs do).
///
/// Caller must supply a measured `overlay_size`. The version of this
/// function that ran before the overlay had been measured used a
/// hardcoded estimate; that's now gone — the only caller is the
/// post-mount rAF / scroll / resize path, which reads the rendered
/// rect via `getBoundingClientRect`.
fn pick_side(
    requested: ElementSide,
    trigger: ViewportRect,
    overlay_size: (f32, f32),
    viewport: (f32, f32),
    offset: f32,
) -> ElementSide {
    let (ow, oh) = overlay_size;
    let (vw, vh) = viewport;
    let needed = match requested {
        ElementSide::Above | ElementSide::Below => oh + offset,
        ElementSide::Start | ElementSide::End => ow + offset,
    };
    let (have, opposite_have, opposite) = match requested {
        ElementSide::Below => (vh - (trigger.y + trigger.height), trigger.y, ElementSide::Above),
        ElementSide::Above => (trigger.y, vh - (trigger.y + trigger.height), ElementSide::Below),
        ElementSide::Start => (trigger.x, vw - (trigger.x + trigger.width), ElementSide::End),
        ElementSide::End => (vw - (trigger.x + trigger.width), trigger.x, ElementSide::Start),
    };
    if have < needed && opposite_have > have {
        opposite
    } else {
        requested
    }
}

/// Read the overlay content's rendered `(width, height)` from the
/// DOM. Returns the bounding client rect's size; if the element has
/// no layout yet (e.g. called before insertion), returns `(0, 0)`
/// and the caller's clamp will keep the top-left at the gutter.
fn measure_overlay_size(content: &web_sys::HtmlElement) -> (f32, f32) {
    let rect = content.get_bounding_client_rect();
    (rect.width() as f32, rect.height() as f32)
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

/// Compute the unmeasured `(top, left)` *anchor point* relative to
/// the trigger. The final visual top-left of the overlay is then
/// derived from this via a CSS `transform: translate(...)` based on
/// the requested alignment — see `element_anchor_styles`. Used only
/// on initial mount, before the overlay has been measured.
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

/// Compute the overlay's *visual top-left* directly from the
/// trigger rect, side, align, and the measured overlay size. No
/// transform translate needed — we shift the box ourselves using
/// the known dimensions.
///
/// This frees the `transform` CSS property for presence animations,
/// which compose translate/scale of their own.
fn measured_position(
    rect: ViewportRect,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    overlay_size: (f32, f32),
) -> (f32, f32) {
    let (ow, oh) = overlay_size;
    let (top, left) = match side {
        ElementSide::Below => {
            let top = rect.y + rect.height + offset;
            let left = match align {
                ElementAlign::Start => rect.x,
                ElementAlign::Center => rect.x + rect.width / 2.0 - ow / 2.0,
                ElementAlign::End => rect.x + rect.width - ow,
            };
            (top, left)
        }
        ElementSide::Above => {
            let top = rect.y - offset - oh;
            let left = match align {
                ElementAlign::Start => rect.x,
                ElementAlign::Center => rect.x + rect.width / 2.0 - ow / 2.0,
                ElementAlign::End => rect.x + rect.width - ow,
            };
            (top, left)
        }
        ElementSide::Start => {
            let left = rect.x - offset - ow;
            let top = match align {
                ElementAlign::Start => rect.y,
                ElementAlign::Center => rect.y + rect.height / 2.0 - oh / 2.0,
                ElementAlign::End => rect.y + rect.height - oh,
            };
            (top, left)
        }
        ElementSide::End => {
            let left = rect.x + rect.width + offset;
            let top = match align {
                ElementAlign::Start => rect.y,
                ElementAlign::Center => rect.y + rect.height / 2.0 - oh / 2.0,
                ElementAlign::End => rect.y + rect.height - oh,
            };
            (top, left)
        }
    };
    (top, left)
}

/// Clamp the overlay's *visual* top-left so its full measured rect
/// stays inside the viewport with an 8px gutter on every side.
///
/// `top` / `left` are the visual top-left of the overlay box (post
/// transform-origin flip), and `overlay_size` is the measured
/// `(width, height)` from `getBoundingClientRect`. Used by the
/// scroll/resize/post-mount reposition path.
fn clamp_measured(
    top: f32,
    left: f32,
    overlay_size: (f32, f32),
    viewport: (f32, f32),
) -> (f32, f32) {
    const EDGE_GAP: f32 = 8.0;
    let (ow, oh) = overlay_size;
    let (vw, vh) = viewport;
    // For each axis, build `[min, max]` for the *top-left* such that
    // the full box fits. If the overlay is bigger than the available
    // gutter-to-gutter span, prefer aligning to the leading edge
    // (top/left) — the trailing edge will overflow, matching
    // floating-UI defaults.
    let max_left = (vw - EDGE_GAP - ow).max(EDGE_GAP);
    let max_top = (vh - EDGE_GAP - oh).max(EDGE_GAP);
    let left = left.clamp(EDGE_GAP, max_left);
    let top = top.clamp(EDGE_GAP, max_top);
    (top, left)
}

fn element_anchor_styles(
    base: &str,
    rect: ViewportRect,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
) -> String {
    let (top, left) = anchor_origin_position(rect, side, align, offset);
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
