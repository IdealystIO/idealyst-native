//! `Element::Portal` — view overlay reparented into the Activity root
//! (viewport-anchored) or `PopupWindow` (element-anchored).
//!
//! # Two flavors, one Node shape
//!
//! Both code paths return a content holder as the framework `Node`.
//! The walker calls `insert_children` on it to populate; `view::insert`
//! checks `is_portal_node` and skips when the walker later tries to
//! splice the holder into its surrounding parent view (the overlay
//! container / PopupWindow already owns its parenting).
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
//! ## Element-anchored: `PopupWindow`
//!
//! `PortalTarget::Anchor { target, side, align, offset }`. Anchored
//! to the trigger's screen rect (resolved via `target.rect()`).
//! Backed by an Android `PopupWindow`. The popup is left
//! non-focusable + non-outside-touchable: any backdrop the host
//! supplies inside the portal's content tree is responsible for
//! catching the outside tap. Back-button dismissal in this flow is
//! best-effort; without `focusable=true` the popup doesn't receive
//! the press, but enabling focus traps the IME and breaks input on
//! the surrounding screen. We accept the trade-off — popovers are
//! transient and dismissed by their own pressable backdrop or by a
//! reactive open-state change.

use crate::imp::callbacks::{leak, OverlayDismissCallback};
use crate::imp::{with_env, AndroidBackend};
use runtime_core::primitives::portal::{
    AnchorTarget, ElementAlign, ElementSide, PortalTarget, ViewportPlacement, ViewportRect,
};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::jlong;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Per-portal backend state. Discriminates between the two host
/// types so `release_portal` knows which teardown path to take.
pub(crate) enum PortalHost {
    /// Viewport-anchored: a `FrameLayout` overlay added on top of the
    /// app content inside the Activity `root`. `release_portal`
    /// `removeView`s it from `root` and drops its Taffy node.
    ViewOverlay(GlobalRef),
    /// Element-anchored: an `android.widget.PopupWindow`. Torn down via
    /// `PopupWindow.dismiss()`.
    Popup(GlobalRef),
}

pub(crate) struct PortalInstance {
    /// The Android host object (overlay View or PopupWindow). Held as a
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
        } => create_popup_portal(b, target, side, align, offset, on_dismiss, trap_focus),
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

    let overlay = with_env(|env| {
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

            if on_dismiss.is_some() {
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

        env.new_global_ref(overlay).unwrap()
    });

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

// ---------------------------------------------------------------------------
// PopupWindow path (element-anchored)
// ---------------------------------------------------------------------------

fn create_popup_portal(
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

    // Resolve the trigger rect now. The target's primitive has
    // already mounted (the user clicked it to open this popover),
    // so `.rect()` returns real coords. If for some reason it
    // doesn't (target ref hasn't been filled), fall back to the
    // zero rect which positions at top-left of the screen — visible
    // and obvious, but not crashy.
    let trigger_rect = target.rect().unwrap_or_default();
    let (x_dp, y_dp) = compute_popup_position(&trigger_rect, side, align, offset);

    let (popup, content_holder) = with_env(|env| {
        let content = make_content_holder(env, &b.context);

        // ---- PopupWindow ----
        // Three-arg constructor: (View contentView, int width, int height).
        // WRAP_CONTENT for both — the content's stylesheet drives size.
        const WRAP_CONTENT: i32 = -2;
        let popup_class = env.find_class("android/widget/PopupWindow").unwrap();
        let popup = env
            .new_object(
                &popup_class,
                "(Landroid/view/View;II)V",
                &[
                    JValue::Object(&content),
                    JValue::Int(WRAP_CONTENT),
                    JValue::Int(WRAP_CONTENT),
                ],
            )
            .unwrap();

        // Backdrop is composition-level — the host supplies a
        // fullscreen pressable child if it wants tap-outside
        // dismissal. PopupWindow itself stays scrim-less.
        //
        // Focus posture:
        //   - trap_focus=false (default): non-focusable popup. Surface
        //     under the popup stays interactive; back-button does NOT
        //     dismiss (Android quirk: popup must be focusable to
        //     receive the press). Reactive open-state flips handle the
        //     usual close paths.
        //   - trap_focus=true: focusable popup. Steals input focus +
        //     receives back-button. Required for keyboard-driven UI.
        if trap_focus {
            let _ = env.call_method(&popup, "setFocusable", "(Z)V", &[JValue::Bool(1)]);
            // Non-null background drawable is required for tap-outside
            // dispatch — needed when the popup is focusable, otherwise
            // back-button dismissal works but the popup never receives
            // its own dismiss event. Transparent so we don't add a
            // visible scrim.
            let color_drawable_class = env
                .find_class("android/graphics/drawable/ColorDrawable")
                .unwrap();
            let drawable = env
                .new_object(&color_drawable_class, "(I)V", &[JValue::Int(0)])
                .unwrap();
            let _ = env.call_method(
                &popup,
                "setBackgroundDrawable",
                "(Landroid/graphics/drawable/Drawable;)V",
                &[JValue::Object(&drawable)],
            );
        }

        // ---- Dismiss listener ----
        if on_dismiss.is_some() {
            let listener_class = env
                .find_class("io/idealyst/runtime/RustPopupDismissListener")
                .unwrap();
            let listener = env
                .new_object(&listener_class, "(J)V", &[JValue::Long(dismiss_cb_ptr)])
                .unwrap();
            let _ = env.call_method(
                &popup,
                "setOnDismissListener",
                "(Landroid/widget/PopupWindow$OnDismissListener;)V",
                &[JValue::Object(&listener)],
            );
        }

        // ---- Show ----
        // showAtLocation needs an anchor View for window-token resolution
        // (PopupWindow attaches to the same window). The backend's root
        // view works for any popup anchored anywhere on screen.
        // Gravity.NO_GRAVITY = 0 means "x and y are absolute screen coords."
        let _ = env.call_method(
            &popup,
            "showAtLocation",
            "(Landroid/view/View;III)V",
            &[
                JValue::Object(&b.root.as_obj()),
                JValue::Int(0), // NO_GRAVITY
                JValue::Int(x_dp),
                JValue::Int(y_dp),
            ],
        );

        (
            env.new_global_ref(popup).unwrap(),
            env.new_global_ref(content).unwrap(),
        )
    });

    let key = AndroidBackend::node_key_of(&content_holder);
    b.portal_instances.insert(
        key,
        PortalInstance {
            host: PortalHost::Popup(popup),
            dismiss_cb_ptr,
        },
    );

    content_holder
}

/// Compute the popup's top-left position in screen pixels from the
/// trigger's screen rect + the desired side/align/offset.
///
/// This is the unmeasured anchor path — we don't yet know the
/// popup's rendered size (it hasn't been laid out), so `End`-align
/// and `Center`-align with `Below`/`Above` will be slightly off
/// until first layout. Web does a post-mount measure + re-position
/// to refine; this implementation skips that pass for now. In
/// practice for typical popover sizes the initial placement is
/// already close enough. A follow-up could call `popup.getWidth() /
/// getHeight()` after `showAtLocation` and re-`update(x, y, ...)`.
fn compute_popup_position(
    trigger: &ViewportRect,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
) -> (i32, i32) {
    // Without a measured popup size we treat ow/oh as 0; the
    // alignment math collapses to "align to the trigger's
    // start/center/end edge."
    let (ow, oh) = (0.0_f32, 0.0_f32);
    let (top, left) = match side {
        ElementSide::Below => {
            let top = trigger.y + trigger.height + offset;
            let left = align_horizontal(trigger, align, ow);
            (top, left)
        }
        ElementSide::Above => {
            // Without a known popup height we can't subtract oh from
            // the trigger top; the popup will overlap the trigger
            // until the post-mount measure pass exists. Conservative
            // fallback: place just above the trigger top.
            let top = trigger.y - offset - oh;
            let left = align_horizontal(trigger, align, ow);
            (top, left)
        }
        ElementSide::Start => {
            let top = align_vertical(trigger, align, oh);
            let left = trigger.x - offset - ow;
            (top, left)
        }
        ElementSide::End => {
            let top = align_vertical(trigger, align, oh);
            let left = trigger.x + trigger.width + offset;
            (top, left)
        }
    };
    (left.round() as i32, top.round() as i32)
}

fn align_horizontal(trigger: &ViewportRect, align: ElementAlign, ow: f32) -> f32 {
    match align {
        ElementAlign::Start => trigger.x,
        ElementAlign::Center => trigger.x + trigger.width / 2.0 - ow / 2.0,
        ElementAlign::End => trigger.x + trigger.width - ow,
    }
}

fn align_vertical(trigger: &ViewportRect, align: ElementAlign, oh: f32) -> f32 {
    match align {
        ElementAlign::Start => trigger.y,
        ElementAlign::Center => trigger.y + trigger.height / 2.0 - oh / 2.0,
        ElementAlign::End => trigger.y + trigger.height - oh,
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Build the LinearLayout that hosts the portal's children.
/// VERTICAL orientation matches the framework's default flex-column.
fn make_content_holder<'l>(env: &mut jni::JNIEnv<'l>, ctx: &GlobalRef) -> JObject<'l> {
    let ll_class = env.find_class("android/widget/LinearLayout").unwrap();
    let content = env
        .new_object(
            &ll_class,
            "(Landroid/content/Context;)V",
            &[JValue::Object(&ctx.as_obj())],
        )
        .unwrap();
    // setOrientation(LinearLayout.VERTICAL = 1).
    let _ = env.call_method(&content, "setOrientation", "(I)V", &[JValue::Int(1)]);
    content
}

// ---------------------------------------------------------------------------
// release — common path for both view overlay and PopupWindow
// ---------------------------------------------------------------------------

pub(crate) fn release(b: &mut AndroidBackend, node: &GlobalRef) {
    let key = AndroidBackend::node_key_of(node);
    let Some(instance) = b.portal_instances.remove(&key) else {
        return;
    };

    // Step 1: blank the user closure so any in-flight dismiss event
    // — including the one Android dispatches synchronously when we
    // call dismiss() below on PopupWindow — becomes a no-op for
    // user code. Without this the framework-driven teardown would
    // re-fire on_dismiss, flipping the open-state signal that's
    // already off, which is harmless but noisy.
    unsafe {
        if instance.dismiss_cb_ptr != 0 {
            let cb = &*(instance.dismiss_cb_ptr as *const OverlayDismissCallback);
            *cb.inner.borrow_mut() = None;
        }
    }

    // Step 2: tear down the host.
    //   - ViewOverlay: remove the overlay from `root` (so it stops
    //     painting + receiving input) and drop its Taffy node so the
    //     next layout pass doesn't try to lay out a detached subtree.
    //   - Popup: PopupWindow.dismiss(). Its OnDismissListener fires for
    //     ALL dismissals — step 1's blanking is what keeps that benign.
    with_env(|env| match &instance.host {
        PortalHost::ViewOverlay(overlay) => {
            let _ = env.call_method(
                &b.root.as_obj(),
                "removeView",
                "(Landroid/view/View;)V",
                &[JValue::Object(&overlay.as_obj())],
            );
        }
        PortalHost::Popup(p) => {
            let _ = env.call_method(p, "dismiss", "()V", &[]);
        }
    });

    // For a view overlay, also drop its Taffy node + view-table entry.
    // `node_key_of(node)` is the overlay's own key (the content holder IS
    // the overlay), so look the layout node up before it's gone.
    if let PortalHost::ViewOverlay(overlay) = &instance.host {
        let layout_node = b.layout_for_view(overlay);
        b.layout.remove_node(layout_node);
        b.view_to_layout.remove(&AndroidBackend::node_key_of(overlay));
    }

    // Step 3: deliberately leak `instance.dismiss_cb_ptr` — Android
    // can dispatch a queued dismiss event after we've returned (a
    // back-key already in flight, a popup dismiss event), and the
    // trampoline would dereference a freed pointer. Same posture as
    // `StateCallback`: leak rather than risk UAF.
}

// ---------------------------------------------------------------------------
// view::insert support
// ---------------------------------------------------------------------------

/// True if `node` is a registered portal's content holder. Used by
/// `view::insert` to skip the `addView` call — portal content holders
/// are already parented (the view overlay was added to the Activity
/// `root`; the popup owns its own content view), and the walker's
/// parent-side insert would throw
/// `IllegalStateException("specified child already has a parent")`.
pub(crate) fn is_portal_node(b: &AndroidBackend, node: &GlobalRef) -> bool {
    let key = AndroidBackend::node_key_of(node);
    b.portal_instances.contains_key(&key)
}
