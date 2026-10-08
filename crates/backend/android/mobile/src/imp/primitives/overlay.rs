//! `Element::Portal` — a view overlay added to the Activity root, for both
//! viewport-anchored and element-anchored portals.
//!
//! # Two flavors, one Node shape
//!
//! Both code paths return a content holder as the framework `Node`.
//! The walker calls `insert_children` on it to populate; `view::insert`
//! checks `is_portal_node` and skips when the walker later tries to
//! splice the holder into its surrounding parent view (the overlay
//! container already owns its parenting).
//!
//! ## Viewport-anchored: a "dumb" view overlay
//!
//! `PortalTarget::Viewport(Center | Top | Bottom | Left | Right |
//! FullScreen)`. The portal is a plain `FrameLayout` overlay added on
//! top of the app content in the SAME window — `root.addView(overlay)`,
//! where `root` is the Activity-provided container the app tree already
//! appends into. A later child of a `FrameLayout` paints above earlier
//! ones, so the overlay sits over the app. This mirrors web (a DOM node
//! high in the tree) and iOS (a subview on the window).
//!
//! The overlay always fills the viewport (`MATCH_PARENT` ×
//! `MATCH_PARENT`) and is registered as a Taffy ROOT sized to the
//! viewport, laid out in the NORMAL `run_layout_pass`. There is no
//! per-placement window gravity or `WRAP_CONTENT` sizing — the
//! *content* positions itself (the idea-ui `Modal` centers its card via
//! a flex-center wrapper + an absolutely-positioned backdrop child).
//! Composition (`AnimatedValue` on the content) owns all enter/exit
//! motion; the overlay view itself just appears, so there is no window
//! slide/fade and no deferred-show flicker.
//!
//! The runtime-core composition layers a backdrop primitive INSIDE the
//! portal (it becomes the first child of the content holder); the
//! backend configures no scrim. Tap-outside dismissal is a
//! composition-level concern (the backdrop child's `on_click`) — and
//! because everything is one view tree, the outside tap naturally hits
//! the backdrop child beneath the card.
//!
//! ### Touch modality (one view tree, no window flags)
//!
//! Modality emerges from the *content*, not from window flags
//! ([[project_android_nonmodal_overlay_passthrough]]):
//!   - **Modal** (`trap_focus = true`, e.g. `Modal`): a full-bleed
//!     pressable backdrop child consumes touches, so the app beneath is
//!     blocked. The overlay is made focusable so the back key routes to
//!     it (see below).
//!   - **Non-modal** (`trap_focus = false`, e.g. `ToastHost`): the
//!     overlay `FrameLayout` is left non-clickable and non-focusable. A
//!     plain `FrameLayout` whose children don't consume a touch returns
//!     `false` from `dispatchTouchEvent`, so the touch falls through to
//!     the sibling app content beneath it in `root`. No `NOT_TOUCHABLE`
//!     window flag is needed (there is no separate window) — and the
//!     "hamburger dead" regression can't recur, because a view overlay
//!     never steals touches it doesn't have an interactive child for.
//!
//! ### Back button
//!
//! A `Dialog` gave hardware/gesture back dismissal for free via
//! `setOnCancelListener`. A view overlay has no window to route back
//! into, so for MODAL overlays we make the overlay focusable-in-touch-
//! mode, request focus, and attach a `RustOverlayKeyListener`
//! (`View.OnKeyListener`) that fires the user's `on_dismiss` on
//! KEYCODE_BACK ACTION_UP and consumes the event. Non-modal overlays
//! attach no key listener and back falls through to the app/navigator.
//!
//! ## Element-anchored: the same overlay, content placed by `AnchoredPlacer`
//!
//! `PortalTarget::Anchor { target, side, align, offset }` (popovers, menus,
//! tooltips). The same full-bleed overlay (so modality, back key and
//! touch pass-through behave exactly as above), whose Taffy root uses
//! `anchored_portal_policy::anchored_container_rules` so the content child
//! — the LAST child inserted, after any backdrop — sizes to its content.
//! Its top-left is then resolved by one long-lived
//! `anchored_portal_policy::AnchorTracker` (the shared `AnchoredPlacer`:
//! flip only when the content stops fitting, keep the settled side, clamp
//! with `ANCHOR_EDGE_GAP`):
//!
//! - in every layout pass ([`place_anchored_contents`]), with the content
//!   size Taffy just computed — so the first visible frame is already
//!   placed, and a content resize (a menu filter dropping rows) re-places
//!   on the pass it triggers;
//! - every frame while open ([`track_anchor`], a `raf_loop`), so the
//!   popover follows an anchor that moves without a layout pass of ours
//!   (scrolling). Mirrors iOS's per-vsync `CADisplayLink` tracker.
//!
//! The anchor rect comes from the handle's `rect()` in viewport dp (root-
//! relative — `view_rect::view_viewport_rect`), the same space as the
//! overlay's frames. This replaced a `PopupWindow` placed once with an
//! unmeasured (0×0) content size; see `crate::anchored_portal_policy`.

use crate::imp::callbacks::{leak, OverlayDismissCallback};
use crate::imp::{with_env, AndroidBackend};
use crate::anchored_portal_policy::{anchored_container_rules, AnchorTracker};
use runtime_shared::primitives::portal::{
    AnchorTarget, ElementAlign, ElementSide, PortalTarget, ViewportPlacement,
};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Per-portal backend state. Discriminates between the two portal kinds
/// so `release_portal` / the layout pass know which bookkeeping applies.
/// Both render into the same kind of host: a full-bleed `FrameLayout`
/// overlay in the Activity `root`.
pub(crate) enum PortalHost {
    /// Viewport-anchored: the content positions itself inside the overlay
    /// with flex. `release_portal` `removeView`s it from `root` and drops
    /// its Taffy node.
    ViewOverlay(GlobalRef),
    /// Element-anchored: the content child is placed against the anchor by
    /// [`AnchorState`]'s placer on every layout pass and every frame.
    Anchored {
        overlay: GlobalRef,
        state: Rc<AnchorState>,
        /// Per-frame anchor tracker; dropping it cancels the loop.
        _tracker: runtime_shared::RafLoop,
    },
}

pub(crate) struct PortalInstance {
    /// The overlay view (+ anchored placement state). Held as a
    /// `GlobalRef` so the JVM doesn't GC it while shown.
    pub(crate) host: PortalHost,
    /// Raw pointer to the leaked `OverlayDismissCallback`. Used by
    /// `release_portal` to blank the inner closure before tearing
    /// down the host (otherwise the host's dismiss listener would
    /// re-fire the user closure during framework-driven teardown).
    pub(crate) dismiss_cb_ptr: jlong,
}

/// All live portals, keyed by the content-holder node's raw pointer
/// (same scheme `anim_state` uses for animation state).
pub(crate) type PortalInstances = HashMap<usize, PortalInstance>;

// ---------------------------------------------------------------------------
// Public entry point — dispatches on PortalTarget.
// ---------------------------------------------------------------------------

pub(crate) fn create(
    b: &mut AndroidBackend,
    target: PortalTarget,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
) -> GlobalRef {
    match target {
        PortalTarget::Viewport(placement) => {
            create_overlay_portal(b, placement, on_dismiss, trap_focus)
        }
        PortalTarget::Anchor {
            target,
            side,
            align,
            offset,
        } => create_anchored_portal(b, target, side, align, offset, on_dismiss, trap_focus),
        // Named slots: no backend mounting infrastructure yet.
        // Fall back to a viewport-centered overlay so authors don't
        // see a hard crash — same posture as the iOS skin's Named
        // fallback.
        PortalTarget::Named(_) => {
            create_overlay_portal(b, ViewportPlacement::Center, on_dismiss, trap_focus)
        }
    }
}

// ---------------------------------------------------------------------------
// View-overlay path (viewport-anchored)
// ---------------------------------------------------------------------------

fn create_overlay_portal(
    b: &mut AndroidBackend,
    placement: ViewportPlacement,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
) -> GlobalRef {
    // `placement` no longer drives window gravity/size — the overlay
    // always fills the viewport and the content positions itself (the
    // idea-ui `Modal` centers via a flex-center wrapper; a Top/Bottom
    // sheet aligns itself with flex). Every viewport placement renders
    // identically at the backend layer: a full-bleed overlay laid out in
    // viewport space. Kept in the signature for API parity and in case a
    // future placement needs a backend-side hint.
    let _ = placement;

    let dismiss_cb_ptr = leak(OverlayDismissCallback {
        inner: RefCell::new(on_dismiss.clone()),
    });

    let overlay = make_overlay_view(b, trap_focus, on_dismiss.is_some(), dismiss_cb_ptr);

    // Queue the overlay for reveal once the next `run_layout_pass` lays
    // out its content. See the INVISIBLE set in `make_overlay_view` for the bug this
    // prevents (one unlaid-out frame at the 0,0 origin on mount).
    b.pending_reveal.push(overlay.clone());

    let key = AndroidBackend::node_key_of(&overlay);
    b.portal_instances.insert(
        key,
        PortalInstance {
            host: PortalHost::ViewOverlay(overlay.clone()),
            dismiss_cb_ptr,
        },
    );

    // Register the overlay as a Taffy ROOT sized to the viewport. It's a
    // detached sub-root (not a child of the app tree's Taffy root) so it
    // lays out in viewport space — its children then position themselves
    // (the Modal's flex-center wrapper, the toast host's bottom align).
    // `layout_for_view` creates a fresh node with both axes `Auto`, which
    // `run_layout_pass` force-fills to the viewport because the node is a
    // root. No `set_root_axes_wrap` — that band-aid existed only to let a
    // WRAP_CONTENT Dialog window's gravity center the card; with a
    // full-bleed overlay, centering is pure flex inside the content.
    b.layout_for_view(&overlay);

    // The overlay's subtree is inserted by the walker AFTER this returns;
    // `Backend::insert` kicks a coalesced layout pass when it sees an
    // insert into a portal content holder (it checks `portal_instances`),
    // so the overlay's Taffy root gets `compute()`d once its children
    // exist. No deferred `show()` is needed — the overlay is already in
    // the view tree and simply paints on the next frame.

    overlay
}

/// Build the full-bleed overlay `FrameLayout` both portal flavors render
/// into, add it on top of the app content in the Activity `root` (same
/// window), and leave it `INVISIBLE` until its first layout pass (the
/// caller queues it in `pending_reveal`). See the module docs for the
/// touch-modality / back-key posture `trap_focus` selects.
fn make_overlay_view(
    b: &AndroidBackend,
    trap_focus: bool,
    has_dismiss: bool,
    dismiss_cb_ptr: jlong,
) -> GlobalRef {
    with_env(|env| {
        // The overlay container IS the content holder: a FrameLayout the
        // walker inserts portal children into directly. FrameLayout (vs
        // LinearLayout) because the backend drives all child placement
        // through Taffy frames written onto FrameLayout.LayoutParams —
        // same shape as every other `view::create`d container.
        let fl_class = env.find_class("android/widget/FrameLayout").unwrap();
        let overlay = env
            .new_object(
                &fl_class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();

        // MATCH_PARENT × MATCH_PARENT so the overlay fills the Activity
        // root regardless of the root's own layout. Use FrameLayout.
        // LayoutParams (root is a FrameLayout) so the child is laid out
        // full-bleed by the parent — its own children are then placed by
        // Taffy frames in viewport space.
        const MATCH_PARENT: i32 = -1;
        let lp_class = env
            .find_class("android/widget/FrameLayout$LayoutParams")
            .unwrap();
        let lp = env
            .new_object(
                &lp_class,
                "(II)V",
                &[JValue::Int(MATCH_PARENT), JValue::Int(MATCH_PARENT)],
            )
            .unwrap();
        let _ = env.call_method(
            &overlay,
            "setLayoutParams",
            "(Landroid/view/ViewGroup$LayoutParams;)V",
            &[JValue::Object(&lp)],
        );

        // Touch modality is content-driven (see module docs). For a
        // MODAL overlay we additionally:
        //   - make the overlay focusable-in-touch-mode + requestFocus so
        //     it can receive the hardware back key;
        //   - attach a RustOverlayKeyListener that routes KEYCODE_BACK to
        //     the user's on_dismiss (replacing Dialog.setOnCancelListener).
        // A NON-MODAL overlay stays non-focusable + non-clickable; a
        // FrameLayout with no consuming child returns false from
        // dispatchTouchEvent and the touch falls through to the app
        // content beneath it in `root` — so a toast host can't make the
        // app untappable (the "hamburger dead" hazard). Back also falls
        // through to the app/navigator for non-modal overlays, which is
        // the desired behavior (a toast shouldn't swallow back).
        if trap_focus {
            let _ = env.call_method(&overlay, "setFocusable", "(Z)V", &[JValue::Bool(1)]);
            let _ = env.call_method(
                &overlay,
                "setFocusableInTouchMode",
                "(Z)V",
                &[JValue::Bool(1)],
            );
            let _ = env.call_method(&overlay, "requestFocus", "()Z", &[]);

            if has_dismiss {
                let listener_class = env
                    .find_class("io/idealyst/runtime/RustOverlayKeyListener")
                    .unwrap();
                let listener = env
                    .new_object(&listener_class, "(J)V", &[JValue::Long(dismiss_cb_ptr)])
                    .unwrap();
                let _ = env.call_method(
                    &overlay,
                    "setOnKeyListener",
                    "(Landroid/view/View$OnKeyListener;)V",
                    &[JValue::Object(&listener)],
                );
            }
        }

        // Add the overlay on top of the app content in the SAME window.
        // FrameLayout paints children in add order, so this later child
        // paints above the app tree's root (added first via `finish`).
        let _ = env.call_method(
            &b.root.as_obj(),
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&overlay)],
        );

        // Hide the overlay until its first layout pass positions its
        // content (revealed at the END of `run_layout_pass` via
        // `pending_reveal`). The overlay stays INVISIBLE until its first
        // layout pass positions its content, else it paints one unlaid-
        // out frame at the 0,0 origin on mount (children not yet
        // centered) — a visible snap when the modal opens. View.INVISIBLE
        // = 4 (NOT GONE = 8, which would drop it from the layout pass
        // entirely; INVISIBLE keeps it laid out but unpainted).
        const INVISIBLE: i32 = 4;
        let _ = env.call_method(
            &overlay,
            "setVisibility",
            "(I)V",
            &[JValue::Int(INVISIBLE)],
        );

        env.new_global_ref(overlay).unwrap()
    })
}

// ---------------------------------------------------------------------------
// Anchored path (element-anchored): same overlay, content re-placed
// ---------------------------------------------------------------------------

/// Live placement state of an anchored portal. Shared between the
/// `PortalInstance` (read by the layout pass) and the per-frame tracker.
pub(crate) struct AnchorState {
    target: AnchorTarget,
    /// The ONE `AnchoredPlacer` for this portal's lifetime (sticky side).
    tracker: RefCell<AnchorTracker>,
    /// The content child the placement moves: the LATEST child inserted
    /// into the overlay. Composition inserts `[backdrop, content]` with the
    /// content last, so re-pointing on every insert lands on the content —
    /// the same rule as iOS `portal_policy::anchored_insert_action`.
    content: RefCell<Option<GlobalRef>>,
    /// Content size (dp) from the last layout pass — the per-frame tracker
    /// re-places with it without re-running Taffy.
    content_size: Cell<(f32, f32)>,
    /// Viewport (overlay) size in dp from the last layout pass.
    viewport: Cell<(f32, f32)>,
}

fn create_anchored_portal(
    b: &mut AndroidBackend,
    target: AnchorTarget,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
) -> GlobalRef {
    let dismiss_cb_ptr = leak(OverlayDismissCallback {
        inner: RefCell::new(on_dismiss.clone()),
    });
    // The same in-window overlay the viewport portal uses — NOT a
    // `PopupWindow` (a second window placed once, unmeasured; see
    // `crate::anchored_portal_policy` for the bugs that caused). A
    // non-`trap_focus` popover leaves the overlay non-clickable, so taps
    // outside its content fall through to the app (iOS's passthrough
    // container); a backdrop child, when the composition asks for one,
    // catches them instead.
    let overlay = make_overlay_view(b, trap_focus, on_dismiss.is_some(), dismiss_cb_ptr);
    b.pending_reveal.push(overlay.clone());

    let state = Rc::new(AnchorState {
        target,
        tracker: RefCell::new(AnchorTracker::new(side, align, offset)),
        content: RefCell::new(None),
        content_size: Cell::new((0.0, 0.0)),
        viewport: Cell::new((0.0, 0.0)),
    });

    // Re-place every frame while open: the anchor can move without any
    // layout pass of ours (a scroll under the popover, an animation). The
    // layout pass covers content resizes and the first placement; this
    // covers anchor motion. Mirrors iOS's per-vsync `CADisplayLink`
    // tracker. Dropped (cancelled) with the `PortalInstance`.
    let per_frame = state.clone();
    let tracker = runtime_shared::raf_loop(move || track_anchor(&per_frame));

    let key = AndroidBackend::node_key_of(&overlay);
    b.portal_instances.insert(
        key,
        PortalInstance {
            host: PortalHost::Anchored { overlay: overlay.clone(), state, _tracker: tracker },
            dismiss_cb_ptr,
        },
    );

    // Taffy ROOT sized to the viewport (like the viewport overlay), styled
    // so the content child sizes to its content instead of stretching —
    // the placer needs the content's real size.
    let node = b.layout_for_view(&overlay);
    b.layout
        .set_style(node, &anchored_container_rules(&runtime_shared::StyleRules::default()));
    overlay
}

/// One frame of the anchor tracker: re-resolve the content's top-left and,
/// if it moved, write just that view's frame (one `applyFrames` call).
fn track_anchor(state: &AnchorState) {
    let Some(content) = state.content.borrow().clone() else { return };
    let (w, h) = state.content_size.get();
    let moved = state
        .tracker
        .borrow_mut()
        .replace_if_moved(state.target.rect(), (w, h), state.viewport.get());
    let Some((x, y)) = moved else { return };
    with_env(|env| {
        let d = crate::imp::cached_density(env, &content.as_obj());
        let px = |v: f32| (v * d).round() as i32;
        crate::imp::apply_frames_batch(env, &[&content], &[px(x), px(y), px(w), px(h)]);
    });
}

/// `insert(parent, child)` hook: when `parent` is an anchored portal's
/// overlay, `child` becomes the content the placement moves.
pub(crate) fn note_inserted_child(b: &AndroidBackend, parent: &GlobalRef, child: &GlobalRef) {
    let key = AndroidBackend::node_key_of(parent);
    if let Some(PortalInstance { host: PortalHost::Anchored { state, .. }, .. }) =
        b.portal_instances.get(&key)
    {
        *state.content.borrow_mut() = Some(child.clone());
    }
}

/// Layout-pass hook: for every anchored portal, place its content child
/// against the anchor with the content's freshly computed size, overriding
/// the in-flow `(x, y)` Taffy gave it. Called with the pass's frame
/// snapshot before it is applied, so the content lands placed on the same
/// pass that sized it (no frame at the overlay origin first).
pub(crate) fn place_anchored_contents(
    b: &AndroidBackend,
    viewport: (f32, f32),
    frames: &mut [(GlobalRef, runtime_layout::Frame)],
) {
    for instance in b.portal_instances.values() {
        let PortalHost::Anchored { state, .. } = &instance.host else { continue };
        let Some(content) = state.content.borrow().clone() else { continue };
        let ckey = AndroidBackend::node_key_of(&content);
        let Some((_, frame)) = frames
            .iter_mut()
            .find(|(v, _)| AndroidBackend::node_key_of(v) == ckey)
        else {
            continue;
        };
        state.content_size.set((frame.width, frame.height));
        state.viewport.set(viewport);
        if let Some((x, y)) = state.tracker.borrow_mut().place(
            state.target.rect(),
            (frame.width, frame.height),
            viewport,
        ) {
            frame.x = x;
            frame.y = y;
        }
    }
}

/// `apply_style` hook: an anchored portal's overlay keeps the content-
/// sizing container rules under whatever style the portal itself carries.
pub(crate) fn overlay_layout_style(
    b: &AndroidBackend,
    node: &GlobalRef,
    style: &runtime_shared::StyleRules,
) -> Option<runtime_shared::StyleRules> {
    match b.portal_instances.get(&AndroidBackend::node_key_of(node)) {
        Some(PortalInstance { host: PortalHost::Anchored { .. }, .. }) => {
            Some(anchored_container_rules(style))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// release — common path for both portal kinds
// ---------------------------------------------------------------------------

pub(crate) fn release(b: &mut AndroidBackend, node: &GlobalRef) {
    let key = AndroidBackend::node_key_of(node);
    let Some(instance) = b.portal_instances.remove(&key) else {
        return;
    };

    // Step 1: blank the user closure so any in-flight dismiss event
    // (a back key already queued for the overlay) becomes a no-op for
    // user code. Without this the framework-driven teardown would
    // re-fire on_dismiss, flipping the open-state signal that's
    // already off, which is harmless but noisy.
    unsafe {
        if instance.dismiss_cb_ptr != 0 {
            let cb = &*(instance.dismiss_cb_ptr as *const OverlayDismissCallback);
            *cb.inner.borrow_mut() = None;
        }
    }

    // Step 2: tear down the host — both kinds are an overlay in `root`:
    // remove it (so it stops painting + receiving input) and drop its
    // Taffy node + view-table entry so the next layout pass doesn't lay
    // out a detached subtree. Dropping an anchored instance also drops its
    // `RafLoop`, cancelling the per-frame tracker.
    let overlay = match &instance.host {
        PortalHost::ViewOverlay(overlay) | PortalHost::Anchored { overlay, .. } => overlay.clone(),
    };
    with_env(|env| {
        let _ = env.call_method(
            &b.root.as_obj(),
            "removeView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&overlay.as_obj())],
        );
    });
    let layout_node = b.layout_for_view(&overlay);
    b.layout.remove_node(layout_node);
    b.view_to_layout.remove(&AndroidBackend::node_key_of(&overlay));
    drop(instance.host);

    // Step 3: deliberately leak `instance.dismiss_cb_ptr` — Android
    // can dispatch a queued dismiss event after we've returned (a
    // back-key already in flight), and the
    // trampoline would dereference a freed pointer. Same posture as
    // `StateCallback`: leak rather than risk UAF.
}

// ---------------------------------------------------------------------------
// view::insert support
// ---------------------------------------------------------------------------

/// True if `node` is a registered portal's content holder. Used by
/// `view::insert` to skip the `addView` call — portal content holders
/// are already parented (the view overlay was added to the Activity
/// `root`), and the walker's
/// parent-side insert would throw
/// `IllegalStateException("specified child already has a parent")`.
pub(crate) fn is_portal_node(b: &AndroidBackend, node: &GlobalRef) -> bool {
    let key = AndroidBackend::node_key_of(node);
    b.portal_instances.contains_key(&key)
}
