//! `Primitive::Overlay` — Android `Dialog` (viewport-anchored) or
//! `PopupWindow` (element-anchored).
//!
//! # Two flavors, one Node shape
//!
//! Both code paths return a `LinearLayout` content holder as the
//! framework `Node`. The walker calls `insert_children` on it to
//! populate; `view::insert` checks `is_overlay_node` and skips when
//! the walker later tries to splice the holder into its surrounding
//! parent view (the Dialog window / PopupWindow already owns its
//! parenting).
//!
//! ## Viewport-anchored: `Dialog`
//!
//! `OverlayAnchor::Viewport(Center | Top | Bottom | Left | Right |
//! FullScreen)`. Wraps the holder in an Android `Dialog`. Window
//! gravity + size are derived from the `ViewportPlacement`:
//!
//! - Center        → centered, wrap content
//! - Top / Bottom  → full-width sheet at edge
//! - Left / Right  → full-height drawer at edge
//! - FullScreen    → fills the viewport
//!
//! `BackdropMode::Dismiss` wires `Dialog.setOnCancelListener` for
//! tap-outside + back-button. `Opaque` disables cancellation. `None`
//! clears the platform scrim (FLAG_DIM_BEHIND) and lets pointer
//! events pass through.
//!
//! ## Element-anchored: `PopupWindow`
//!
//! `OverlayAnchor::Element(ElementAnchor { target, side, align,
//! offset })`. Anchored to the trigger's screen rect (resolved via
//! `target.rect()` — see [`super::button::AndroidButtonOps::rect`]).
//! Backed by an Android `PopupWindow` which floats above the
//! activity without its own scrim. Tap-outside dismissal is wired
//! when `BackdropMode::Dismiss` is set (`outsideTouchable = true` +
//! `setBackgroundDrawable(transparent)` — Android refuses to
//! deliver outside-touch dismissals without a non-null background).
//!
//! `BackdropMode::Opaque` and `Dismiss` both render the same
//! visually (no scrim either way — PopupWindow doesn't render one);
//! they differ only in dismissal behavior.

use crate::imp::callbacks::{leak, OverlayDismissCallback};
use crate::imp::helpers::view_screen_rect;
use crate::imp::{with_env, AndroidBackend};
use framework_core::primitives::overlay::{
    AnchorTarget, BackdropMode, ElementAlign, ElementSide, ViewportPlacement, ViewportRect,
};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::jlong;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Per-overlay backend state. Discriminates between the two host
/// types so `release_overlay` knows which dismissal API to call.
pub(crate) enum OverlayHost {
    Dialog(GlobalRef),
    Popup(GlobalRef),
}

pub(crate) struct OverlayInstance {
    /// The Android host object (Dialog or PopupWindow). Held as a
    /// `GlobalRef` so the JVM doesn't GC it while shown.
    pub(crate) host: OverlayHost,
    /// Raw pointer to the leaked `OverlayDismissCallback`. Used by
    /// `release_overlay` to blank the inner closure before tearing
    /// down the host (otherwise the host's dismiss listener would
    /// re-fire the user closure during framework-driven teardown).
    pub(crate) dismiss_cb_ptr: jlong,
}

/// All live overlays, keyed by the content-holder node's raw pointer
/// (same scheme `anim_state` uses for animation state).
pub(crate) type OverlayInstances = HashMap<usize, OverlayInstance>;

// ---------------------------------------------------------------------------
// Public entry points — viewport-anchored vs element-anchored. The
// framework's `Backend::create_overlay` / `create_anchored_overlay`
// route here.
// ---------------------------------------------------------------------------

pub(crate) fn create_viewport(
    b: &mut AndroidBackend,
    placement: ViewportPlacement,
    backdrop: BackdropMode,
    on_dismiss: Option<Rc<dyn Fn()>>,
) -> GlobalRef {
    create_dialog_overlay(b, placement, backdrop, on_dismiss)
}

pub(crate) fn create_anchored(
    b: &mut AndroidBackend,
    target: AnchorTarget,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    backdrop: BackdropMode,
    on_dismiss: Option<Rc<dyn Fn()>>,
) -> GlobalRef {
    create_popup_overlay(b, target, side, align, offset, backdrop, on_dismiss)
}

// ---------------------------------------------------------------------------
// Dialog path (viewport-anchored)
// ---------------------------------------------------------------------------

fn create_dialog_overlay(
    b: &mut AndroidBackend,
    placement: ViewportPlacement,
    backdrop: BackdropMode,
    on_dismiss: Option<Rc<dyn Fn()>>,
) -> GlobalRef {
    let dismiss_cb_ptr = leak(OverlayDismissCallback {
        inner: RefCell::new(on_dismiss.clone()),
    });

    let (dialog, content_holder) = with_env(|env| {
        // ---- Dialog instance ----
        let dialog_class = env.find_class("android/app/Dialog").unwrap();
        let dialog = env
            .new_object(
                &dialog_class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        // FEATURE_NO_TITLE = 1.
        let _ = env.call_method(&dialog, "requestWindowFeature", "(I)Z", &[JValue::Int(1)]);

        // ---- Content holder ----
        let content = make_content_holder(env, &b.context);

        env.call_method(
            &dialog,
            "setContentView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&content)],
        )
        .unwrap();

        // ---- Cancellation behavior ----
        //   Dismiss  → cancelable + cancel-on-touch-outside
        //   Opaque   → neither
        //   None     → neither (no scrim → no tap-outside; back-button
        //              left to platform default of not closing)
        let (cancelable, cancel_on_touch) = match backdrop {
            BackdropMode::Dismiss => (true, true),
            BackdropMode::Opaque => (false, false),
            BackdropMode::None => (false, false),
        };
        let _ = env.call_method(
            &dialog,
            "setCancelable",
            "(Z)V",
            &[JValue::Bool(cancelable as u8)],
        );
        let _ = env.call_method(
            &dialog,
            "setCanceledOnTouchOutside",
            "(Z)V",
            &[JValue::Bool(cancel_on_touch as u8)],
        );

        if cancelable && on_dismiss.is_some() {
            let listener_class = env
                .find_class("io/idealyst/runtime/RustOverlayDismissListener")
                .unwrap();
            let listener = env
                .new_object(&listener_class, "(J)V", &[JValue::Long(dismiss_cb_ptr)])
                .unwrap();
            let _ = env.call_method(
                &dialog,
                "setOnCancelListener",
                "(Landroid/content/DialogInterface$OnCancelListener;)V",
                &[JValue::Object(&listener)],
            );
        }

        // ---- Window gravity + size ----
        let window = env
            .call_method(&dialog, "getWindow", "()Landroid/view/Window;", &[])
            .unwrap()
            .l()
            .unwrap();

        // Gravity: CENTER=17, TOP=48, BOTTOM=80,
        // START=8388611, END=8388613.
        let gravity: i32 = match placement {
            ViewportPlacement::Center | ViewportPlacement::FullScreen => 17,
            ViewportPlacement::Top => 48,
            ViewportPlacement::Bottom => 80,
            ViewportPlacement::Left => 8388611,
            ViewportPlacement::Right => 8388613,
        };
        let _ = env.call_method(&window, "setGravity", "(I)V", &[JValue::Int(gravity)]);

        const MATCH_PARENT: i32 = -1;
        const WRAP_CONTENT: i32 = -2;
        let (w, h) = match placement {
            ViewportPlacement::Top | ViewportPlacement::Bottom => (MATCH_PARENT, WRAP_CONTENT),
            ViewportPlacement::Left | ViewportPlacement::Right => (WRAP_CONTENT, MATCH_PARENT),
            ViewportPlacement::FullScreen => (MATCH_PARENT, MATCH_PARENT),
            ViewportPlacement::Center => (WRAP_CONTENT, WRAP_CONTENT),
        };
        let _ = env.call_method(
            &window,
            "setLayout",
            "(II)V",
            &[JValue::Int(w), JValue::Int(h)],
        );

        if matches!(backdrop, BackdropMode::None) {
            // clearFlags(FLAG_DIM_BEHIND = 2).
            let _ = env.call_method(&window, "clearFlags", "(I)V", &[JValue::Int(2)]);
            set_transparent_window_background(env, &window);
        }

        let _ = env.call_method(&dialog, "show", "()V", &[]);

        (
            env.new_global_ref(dialog).unwrap(),
            env.new_global_ref(content).unwrap(),
        )
    });

    let key = AndroidBackend::node_key_of(&content_holder);
    b.overlay_instances.insert(
        key,
        OverlayInstance {
            host: OverlayHost::Dialog(dialog),
            dismiss_cb_ptr,
        },
    );

    content_holder
}

// ---------------------------------------------------------------------------
// PopupWindow path (element-anchored)
// ---------------------------------------------------------------------------

fn create_popup_overlay(
    b: &mut AndroidBackend,
    target: AnchorTarget,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    backdrop: BackdropMode,
    on_dismiss: Option<Rc<dyn Fn()>>,
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

        // ---- Dismiss configuration ----
        // For tap-outside dismissal Android REQUIRES the PopupWindow
        // to have a non-null background drawable. Without it,
        // outside-touch events don't make it to the popup at all
        // (the event-dispatch shortcut path returns false). We use a
        // transparent ColorDrawable so the visual remains scrim-less.
        let dismiss_on_touch = matches!(backdrop, BackdropMode::Dismiss);
        if dismiss_on_touch {
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
            let _ = env.call_method(
                &popup,
                "setOutsideTouchable",
                "(Z)V",
                &[JValue::Bool(1)],
            );
            // Focusable=true makes the popup receive the back-button
            // press (popup.dismiss is the platform default response).
            // Required for back-button dismissal; the on_dismiss
            // listener picks up both paths.
            let _ = env.call_method(&popup, "setFocusable", "(Z)V", &[JValue::Bool(1)]);
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
    b.overlay_instances.insert(
        key,
        OverlayInstance {
            host: OverlayHost::Popup(popup),
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

/// Build the LinearLayout that hosts the overlay's children.
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

/// Set a fully-transparent ColorDrawable as the dialog window's
/// background. Used together with `clearFlags(FLAG_DIM_BEHIND)` to
/// achieve `BackdropMode::None` (no scrim, pointer events pass
/// through outside the content area).
fn set_transparent_window_background(env: &mut jni::JNIEnv, window: &JObject) {
    let color_drawable_class = env
        .find_class("android/graphics/drawable/ColorDrawable")
        .unwrap();
    let drawable = env
        .new_object(&color_drawable_class, "(I)V", &[JValue::Int(0)])
        .unwrap();
    let _ = env.call_method(
        window,
        "setBackgroundDrawable",
        "(Landroid/graphics/drawable/Drawable;)V",
        &[JValue::Object(&drawable)],
    );
}

// ---------------------------------------------------------------------------
// release — common path for both Dialog and PopupWindow
// ---------------------------------------------------------------------------

pub(crate) fn release(b: &mut AndroidBackend, node: &GlobalRef) {
    let key = AndroidBackend::node_key_of(node);
    let Some(instance) = b.overlay_instances.remove(&key) else {
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

    // Step 2: dismiss the host. The Dialog path's setOnCancelListener
    // only fires for user-initiated cancels (so dismiss() doesn't
    // re-fire), but PopupWindow's OnDismissListener fires for ALL
    // dismissals — step 1's blanking is what keeps that benign.
    with_env(|env| match &instance.host {
        OverlayHost::Dialog(d) => {
            let _ = env.call_method(d, "dismiss", "()V", &[]);
        }
        OverlayHost::Popup(p) => {
            let _ = env.call_method(p, "dismiss", "()V", &[]);
        }
    });

    // Step 3: deliberately leak `instance.dismiss_cb_ptr` — Android
    // can dispatch a queued dismiss event after we've returned from
    // dismiss(), and the trampoline would dereference a freed
    // pointer. Same posture as `StateCallback`: leak rather than
    // risk UAF.
}

// ---------------------------------------------------------------------------
// view::insert support
// ---------------------------------------------------------------------------

/// True if `node` is a registered overlay's content holder. Used by
/// `view::insert` to skip the `addView` call — overlay content
/// holders are already parented to the dialog window / popup, and
/// the walker's parent-side insert would throw
/// `IllegalStateException("specified child already has a parent")`.
pub(crate) fn is_overlay_node(b: &AndroidBackend, node: &GlobalRef) -> bool {
    let key = AndroidBackend::node_key_of(node);
    b.overlay_instances.contains_key(&key)
}
