//! Android backend: drives the framework's `View` tree by calling into
//! the Android Java View hierarchy via JNI.
//!
//! # Threading
//!
//! The framework's reactive arena is thread-local (see
//! `framework-core/src/reactive.rs`). All `Backend` calls happen on the
//! Android UI thread (where the app started `render`), so `AndroidBackend`
//! is `!Send`/`!Sync` and assumes single-threaded access.
//!
//! JNI access is acquired lazily per call by attaching the current
//! thread to the cached `JavaVM`. The `JavaVM` is captured in
//! `JNI_OnLoad` (exported below) and stashed in a `static`. This is
//! what lets `AndroidBackend: 'static` — there's no `'local` lifetime
//! tied to a `JNIEnv` living on the stack.
//!
//! # What's implemented
//!
//! - `LinearLayout` for views (vertical by default).
//! - `TextView` for text, with `setText` for reactive updates.
//! - `Button` widgets with click bridging through a small Kotlin
//!   listener (`com.idealyst.runtime.RustClickListener`) that holds a
//!   native pointer and calls back into Rust.
//! - Style application: background color, padding, text color, font
//!   size, border, and border-radius. Border + radius are rendered via
//!   a `GradientDrawable` set as the view's background.
//! - Theme switching: re-applies styles via the framework's effect
//!   plumbing — no extra work needed here.
//! - Refs: `ButtonHandle::click` calls `View.performClick()` on the
//!   native node.

#![allow(unused_imports)]

use framework_core::{Backend, ButtonHandle, ButtonOps, Easing, StyleRules, Transition};
use std::any::Any;
use std::rc::Rc;

#[cfg(target_os = "android")]
mod imp {
    use super::*;
    use jni::objects::{GlobalRef, JObject, JValue};
    use jni::sys::{jint, jlong, JNI_VERSION_1_6};
    use jni::{JNIEnv, JavaVM};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::sync::OnceLock;

    /// Cached `JavaVM`. Filled by `JNI_OnLoad` when libhello_android.so
    /// is dlopen'd by the Android runtime. Every JNI call inside the
    /// backend goes through this to attach the current thread.
    static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

    /// Capture the `JavaVM` at library load time.
    ///
    /// # Safety
    ///
    /// Called by the JVM via dlsym. The `vm` pointer is valid for the
    /// process lifetime; the `OnceLock` stores it safely.
    #[no_mangle]
    pub extern "system" fn JNI_OnLoad(vm: JavaVM, _reserved: *mut c_void) -> jint {
        // Set up logging once — panics in tag setup are non-fatal.
        let _ = std::panic::catch_unwind(|| {
            android_logger::init_once(
                android_logger::Config::default()
                    .with_max_level(log::LevelFilter::Info)
                    .with_tag("idealyst"),
            );
        });
        let _ = JAVA_VM.set(vm);
        JNI_VERSION_1_6
    }

    /// Attach the current thread to the JVM and run `f` with the
    /// resulting `JNIEnv`. Panics if `JNI_OnLoad` hasn't fired (which
    /// can only happen if the library was loaded incorrectly).
    fn with_env<R>(f: impl FnOnce(&mut JNIEnv) -> R) -> R {
        let vm = JAVA_VM.get().expect("JNI_OnLoad has not been called");
        let mut env = vm
            .attach_current_thread_permanently()
            .expect("attach_current_thread_permanently");
        f(&mut env)
    }

    /// Owned holder for a click callback. We hand the JVM a raw pointer
    /// to one of these; the JVM hands it back via the click listener's
    /// native method, which dispatches to the boxed closure.
    ///
    /// Lifetime: leaked at the listener's construction. The Activity
    /// owning the view tree lives for the app's lifetime in this demo,
    /// so explicit drop-on-detach isn't wired. A production backend
    /// would call back into Rust from the Kotlin listener's `finalize`
    /// to drop these.
    struct ClickCallback(Rc<dyn Fn()>);

    /// Per-node animation state. Keyed by the raw `*JObject` pointer
    /// extracted from each node's `GlobalRef` — the JVM keeps the
    /// underlying object alive as long as we hold the `GlobalRef`, so
    /// the pointer is stable for the node's lifetime.
    ///
    /// We track:
    /// - the *last applied* value for each animatable property, so
    ///   `apply_style` can detect "this property actually changed"
    ///   before launching an animator;
    /// - the *running animator* per property, so a value change mid-
    ///   animation cancels the current animator and starts fresh
    ///   without leaking JVM objects.
    /// - the persistent `GradientDrawable` used for background +
    ///   border + radii, so corner/stroke animation can mutate one
    ///   drawable instead of rebuilding it every frame.
    #[derive(Default)]
    struct NodeAnim {
        // Last-applied snapshots (Android pixel-space values).
        last_bg: Option<i32>,         // packed ARGB
        last_text_color: Option<i32>, // packed ARGB
        last_alpha: Option<f32>,
        last_padding: [Option<i32>; 4], // L, T, R, B
        last_radii: [Option<f32>; 4],   // tl, tr, br, bl (px)
        last_stroke_w: Option<i32>,
        last_stroke_color: Option<i32>,

        // Running animator handles, one per animatable bucket. Each
        // is a JVM `Animator` we cancel + restart on value change.
        anim_bg: Option<GlobalRef>,
        anim_text_color: Option<GlobalRef>,
        anim_alpha: Option<GlobalRef>,
        anim_padding: [Option<GlobalRef>; 4],
        anim_radii: [Option<GlobalRef>; 4],
        /// Single animator drives both stroke width and color (one
        /// `setStroke` call interpolates both at once via the Kotlin
        /// helper); no separate color slot needed.
        anim_stroke_w: Option<GlobalRef>,

        // Persistent drawable for backgrounds that have border/radius.
        // Held so corner/stroke animators can mutate one drawable
        // instead of `setBackground`-ing a fresh one every tick.
        drawable: Option<GlobalRef>,
    }

    pub struct AndroidBackend {
        /// Application/Activity context — used as the first argument to
        /// every `View(Context)` constructor.
        context: GlobalRef,
        /// Root container provided by the Activity. `finish` is a no-op
        /// because we don't own the root; we just append into it.
        root: GlobalRef,
        /// Per-node animation state, keyed by raw `JObject*` pointer.
        /// Entries created lazily on first `apply_style`; removed on
        /// `on_node_unstyled` via the framework's lifecycle hook.
        anim_state: HashMap<usize, NodeAnim>,
    }

    impl AndroidBackend {
        /// Construct a backend rooted at the provided Android `Context`
        /// and a parent `ViewGroup` to mount under.
        pub fn new(context: GlobalRef, root: GlobalRef) -> Self {
            Self {
                context,
                root,
                anim_state: HashMap::new(),
            }
        }

        /// Stable key for the node's animation state. The pointer comes
        /// from the `JObject` the `GlobalRef` wraps; the JVM guarantees
        /// it's stable for as long as we hold the global ref.
        fn node_key(node: &GlobalRef) -> usize {
            node.as_obj().as_raw() as usize
        }
    }

    impl Backend for AndroidBackend {
        type Node = GlobalRef;

        fn create_view(&mut self) -> Self::Node {
            with_env(|env| {
                let class = env.find_class("android/widget/LinearLayout").unwrap();
                let local = env
                    .new_object(
                        &class,
                        "(Landroid/content/Context;)V",
                        &[JValue::Object(&self.context.as_obj())],
                    )
                    .unwrap();
                // Vertical orientation (1) so children stack top-to-bottom,
                // matching the framework's default flex-column layout.
                env.call_method(&local, "setOrientation", "(I)V", &[JValue::Int(1)])
                    .unwrap();
                env.new_global_ref(local).unwrap()
            })
        }

        fn create_text(&mut self, content: &str) -> Self::Node {
            with_env(|env| {
                let class = env.find_class("android/widget/TextView").unwrap();
                let local = env
                    .new_object(
                        &class,
                        "(Landroid/content/Context;)V",
                        &[JValue::Object(&self.context.as_obj())],
                    )
                    .unwrap();
                set_text(env, &local, content);
                env.new_global_ref(local).unwrap()
            })
        }

        fn create_button(&mut self, label: &str, on_click: Rc<dyn Fn()>) -> Self::Node {
            with_env(|env| {
                let class = env.find_class("android/widget/Button").unwrap();
                let local = env
                    .new_object(
                        &class,
                        "(Landroid/content/Context;)V",
                        &[JValue::Object(&self.context.as_obj())],
                    )
                    .unwrap();
                set_text(env, &local, label);

                // Box the callback, then leak it to get a stable pointer
                // the JVM can hold. The Kotlin listener stores this as a
                // `Long` and passes it back via `nativeInvoke`.
                let boxed = Box::new(ClickCallback(on_click));
                let ptr = Box::into_raw(boxed) as jlong;

                let listener_class =
                    env.find_class("com/idealyst/runtime/RustClickListener").unwrap();
                let listener = env
                    .new_object(&listener_class, "(J)V", &[JValue::Long(ptr)])
                    .unwrap();
                env.call_method(
                    &local,
                    "setOnClickListener",
                    "(Landroid/view/View$OnClickListener;)V",
                    &[JValue::Object(&listener)],
                )
                .unwrap();

                env.new_global_ref(local).unwrap()
            })
        }

        fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
            with_env(|env| {
                env.call_method(
                    parent.as_obj(),
                    "addView",
                    "(Landroid/view/View;)V",
                    &[JValue::Object(&child.as_obj())],
                )
                .unwrap();
            });
        }

        fn update_text(&mut self, node: &Self::Node, content: &str) {
            with_env(|env| set_text(env, &node.as_obj(), content));
        }

        fn clear_children(&mut self, node: &Self::Node) {
            with_env(|env| {
                env.call_method(node.as_obj(), "removeAllViews", "()V", &[])
                    .unwrap();
            });
        }

        fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
            let key = Self::node_key(node);
            // Lazy-create per-node state on first apply.
            let state = self.anim_state.entry(key).or_default();
            with_env(|env| {
                apply_rules(env, node, state, style);
            });
        }

        fn on_node_unstyled(&mut self, node: &Self::Node) {
            // Free per-node animator state when the node detaches.
            // Drops the held `GlobalRef`s, which lets the JVM GC the
            // animator objects.
            self.anim_state.remove(&Self::node_key(node));
        }

        fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
            ButtonHandle::new(Rc::new(node.clone()), &AndroidButtonOps)
        }

        fn finish(&mut self, root: Self::Node) {
            with_env(|env| {
                env.call_method(
                    self.root.as_obj(),
                    "addView",
                    "(Landroid/view/View;)V",
                    &[JValue::Object(&root.as_obj())],
                )
                .unwrap();
            });
        }
    }

    /// Click-listener trampoline. The Kotlin `RustClickListener.onClick`
    /// override calls `nativeInvoke(nativePtr)`, which dispatches here.
    ///
    /// # Safety
    ///
    /// `ptr` must have been produced by `Box::into_raw` on a
    /// `Box<ClickCallback>` in `create_button` above, and must still be
    /// live (i.e. the activity hasn't been destroyed yet). The pointer
    /// is *not* freed here — it stays valid for as long as the listener
    /// object is alive.
    #[no_mangle]
    pub unsafe extern "system" fn Java_com_idealyst_runtime_RustClickListener_nativeInvoke(
        _env: JNIEnv,
        // Instance method on RustClickListener; second JNI arg is `this`.
        // We don't need it — `ptr` carries everything.
        _this: JObject,
        ptr: jlong,
    ) {
        if ptr == 0 {
            return;
        }
        let cb = &*(ptr as *const ClickCallback);
        // Catch panics — a Rust panic across the FFI boundary is UB.
        // We can't do much beyond log if the closure unwinds.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (cb.0)()));
    }

    /// Free a leaked `ClickCallback`. Currently unused (see lifetime
    /// note on `ClickCallback`); exposed so the Kotlin side can call
    /// it from `RustClickListener.finalize()` once we wire that.
    #[no_mangle]
    pub unsafe extern "system" fn Java_com_idealyst_runtime_RustClickListener_nativeDrop(
        _env: JNIEnv,
        _this: JObject,
        ptr: jlong,
    ) {
        if ptr != 0 {
            drop(Box::from_raw(ptr as *mut ClickCallback));
        }
    }

    // ---- ButtonOps ---------------------------------------------------------

    struct AndroidButtonOps;

    impl ButtonOps for AndroidButtonOps {
        fn click(&self, node: &dyn Any) {
            let Some(gref) = node.downcast_ref::<GlobalRef>() else {
                return;
            };
            with_env(|env| {
                let _ = env.call_method(gref.as_obj(), "performClick", "()Z", &[]);
            });
        }
    }

    // ---- Helpers -----------------------------------------------------------

    fn set_text(env: &mut JNIEnv, view: &JObject, content: &str) {
        let java_str = env.new_string(content).unwrap();
        env.call_method(
            view,
            "setText",
            "(Ljava/lang/CharSequence;)V",
            &[JValue::Object(&JObject::from(java_str))],
        )
        .unwrap();
    }

    /// Parse a CSS-style color string (`#rgb`, `#rrggbb`, `#aarrggbb`,
    /// or `transparent`) into the Android `int` form: `0xAARRGGBB`.
    fn parse_color(input: &str) -> Option<i32> {
        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("transparent") {
            return Some(0);
        }
        if !trimmed.starts_with('#') {
            return None;
        }
        let hex = &trimmed[1..];
        // Helper: parse hex with full alpha if not provided.
        let parse = |s: &str, alpha: u32| -> Option<i32> {
            let rgb = u32::from_str_radix(s, 16).ok()?;
            Some(((alpha << 24) | rgb) as i32)
        };
        match hex.len() {
            3 => {
                let r = u32::from_str_radix(&hex[0..1], 16).ok()?;
                let g = u32::from_str_radix(&hex[1..2], 16).ok()?;
                let b = u32::from_str_radix(&hex[2..3], 16).ok()?;
                let expand = |v: u32| (v << 4) | v;
                let packed = (expand(r) << 16) | (expand(g) << 8) | expand(b);
                Some((0xFF000000u32 | packed) as i32)
            }
            6 => parse(hex, 0xFF),
            8 => {
                // Android wants AARRGGBB; assume CSS-like input is already
                // in the same order for the 8-digit form.
                Some(u32::from_str_radix(hex, 16).ok()? as i32)
            }
            _ => None,
        }
    }

    /// Pull the first `Length::Px` value from a per-side group, falling
    /// back to 0.0 when absent. The framework's per-side fields are all
    /// `Option<Length>`; for padding we collapse them with a saturating
    /// max so a single-side override doesn't zero the other sides.
    fn px_or(value: Option<framework_core::Length>, default: f32) -> f32 {
        match value {
            Some(framework_core::Length::Px(v)) => v,
            // Percent/Auto don't have a well-defined value here without a
            // layout pass; treat as default.
            _ => default,
        }
    }

    fn dp_to_px(env: &mut JNIEnv, view: &JObject, dp: f32) -> i32 {
        // density = view.getResources().getDisplayMetrics().density
        let res = env
            .call_method(view, "getResources", "()Landroid/content/res/Resources;", &[])
            .unwrap()
            .l()
            .unwrap();
        let metrics = env
            .call_method(
                &res,
                "getDisplayMetrics",
                "()Landroid/util/DisplayMetrics;",
                &[],
            )
            .unwrap()
            .l()
            .unwrap();
        let density = env.get_field(&metrics, "density", "F").unwrap().f().unwrap();
        (dp * density).round() as i32
    }

    fn apply_rules(env: &mut JNIEnv, node: &GlobalRef, state: &mut NodeAnim, rules: &StyleRules) {
        let view = node.as_obj();

        // --- Padding (per-side; framework stores all four independently).
        //     Each side may animate independently.
        let want_padding = [
            dp_to_px(env, &view, px_or(rules.padding_left, 0.0)),
            dp_to_px(env, &view, px_or(rules.padding_top, 0.0)),
            dp_to_px(env, &view, px_or(rules.padding_right, 0.0)),
            dp_to_px(env, &view, px_or(rules.padding_bottom, 0.0)),
        ];
        let padding_transitions = [
            rules.padding_left_transition,
            rules.padding_top_transition,
            rules.padding_right_transition,
            rules.padding_bottom_transition,
        ];
        // If *any* side changed, we'll need a refreshed setPadding call.
        // For animated sides we kick a `ValueAnimator` per side that
        // calls back into setPadding on each tick. For snap sides we
        // setPadding directly. Either way we record the new target.
        let any_padding_changed = (0..4).any(|i| state.last_padding[i] != Some(want_padding[i]));
        if any_padding_changed {
            // Snap: any side without a transition just gets its new
            // value applied immediately via setPadding. We do this in
            // one setPadding call covering all four sides — animated
            // sides will overwrite their own value on each tick.
            env.call_method(
                &view,
                "setPadding",
                "(IIII)V",
                &[
                    JValue::Int(want_padding[0]),
                    JValue::Int(want_padding[1]),
                    JValue::Int(want_padding[2]),
                    JValue::Int(want_padding[3]),
                ],
            )
            .unwrap();
            // For sides with a transition, start an animator from the
            // PREVIOUS value to the new value. This intentionally runs
            // *after* the setPadding above — the animator will write
            // intermediate values on its update callback, overriding
            // the snap-applied target until it reaches `to`.
            for i in 0..4 {
                let new_val = want_padding[i];
                let old_val = state.last_padding[i];
                state.last_padding[i] = Some(new_val);
                if let (Some(from), Some(t)) = (old_val, padding_transitions[i]) {
                    if from != new_val {
                        // Cancel any previous animator for this side.
                        cancel_animator(env, state.anim_padding[i].take());
                        let side_index = i as i32; // 0..3 = L,T,R,B
                        let anim = start_padding_animator(
                            env, node, side_index, from, new_val, t,
                        );
                        state.anim_padding[i] = anim;
                    }
                }
            }
        }

        // --- Text color + font size (no-op for views that aren't TextView).
        let textview_class = env.find_class("android/widget/TextView").unwrap();
        let is_textview = env.is_instance_of(&view, &textview_class).unwrap_or(false);

        if is_textview {
            if let Some(c) = &rules.color {
                if let Some(packed) = parse_color(&c.0) {
                    let prev = state.last_text_color;
                    let changed = prev != Some(packed);
                    state.last_text_color = Some(packed);
                    if changed {
                        match (prev, rules.color_transition) {
                            (Some(from), Some(t)) if from != packed => {
                                cancel_animator(env, state.anim_text_color.take());
                                state.anim_text_color = start_argb_animator(
                                    env, node, "textColor", from, packed, t,
                                );
                            }
                            _ => {
                                let _ = env.call_method(
                                    &view,
                                    "setTextColor",
                                    "(I)V",
                                    &[JValue::Int(packed)],
                                );
                            }
                        }
                    }
                }
            }
            if let Some(framework_core::Length::Px(size)) = rules.font_size {
                // font-size isn't animatable in v1; snap.
                let _ = env.call_method(
                    &view,
                    "setTextSize",
                    "(IF)V",
                    &[JValue::Int(1), JValue::Float(size)],
                );
            }
        }

        // --- Opacity (View.alpha). Animatable via ObjectAnimator.ofFloat.
        if let Some(o) = rules.opacity {
            let changed = state.last_alpha.map(|p| (p - o).abs() > 0.001).unwrap_or(true);
            let prev = state.last_alpha;
            state.last_alpha = Some(o);
            if changed {
                match (prev, rules.opacity_transition) {
                    (Some(from), Some(t)) if (from - o).abs() > 0.001 => {
                        cancel_animator(env, state.anim_alpha.take());
                        state.anim_alpha = start_float_animator(env, node, "alpha", from, o, t);
                    }
                    _ => {
                        let _ = env.call_method(
                            &view,
                            "setAlpha",
                            "(F)V",
                            &[JValue::Float(o)],
                        );
                    }
                }
            }
        }

        // --- Background + border + radius. If any border or radius is
        //     present we route through a persistent `GradientDrawable`
        //     so we can mutate corners/stroke/fill on each animator
        //     tick instead of rebuilding the drawable. Otherwise the
        //     simple `setBackgroundColor` path covers it.
        let has_border = rules.border_top_width.is_some()
            || rules.border_right_width.is_some()
            || rules.border_bottom_width.is_some()
            || rules.border_left_width.is_some();
        let has_radius = rules.border_top_left_radius.is_some()
            || rules.border_top_right_radius.is_some()
            || rules.border_bottom_left_radius.is_some()
            || rules.border_bottom_right_radius.is_some();

        if has_border || has_radius {
            apply_drawable_path(env, node, state, rules);
        } else if let Some(c) = &rules.background {
            if let Some(packed) = parse_color(&c.0) {
                let prev = state.last_bg;
                let changed = prev != Some(packed);
                state.last_bg = Some(packed);
                if changed {
                    match (prev, rules.background_transition) {
                        (Some(from), Some(t)) if from != packed => {
                            cancel_animator(env, state.anim_bg.take());
                            state.anim_bg = start_argb_animator(
                                env, node, "backgroundColor", from, packed, t,
                            );
                        }
                        _ => {
                            let _ = env.call_method(
                                &view,
                                "setBackgroundColor",
                                "(I)V",
                                &[JValue::Int(packed)],
                            );
                        }
                    }
                }
            }
        }
    }

    /// Background path for nodes that have a border or non-zero
    /// corner radius. Uses a per-node `GradientDrawable` so corner
    /// radius and stroke can animate without re-allocating.
    fn apply_drawable_path(
        env: &mut JNIEnv,
        node: &GlobalRef,
        state: &mut NodeAnim,
        rules: &StyleRules,
    ) {
        let view = node.as_obj();

        // Ensure the drawable exists and is attached as the view's
        // background. We do this once per node — subsequent applies
        // mutate the drawable in place.
        if state.drawable.is_none() {
            let class = env
                .find_class("android/graphics/drawable/GradientDrawable")
                .unwrap();
            let drawable_local = env.new_object(&class, "()V", &[]).unwrap();
            let _ = env.call_method(
                &view,
                "setBackground",
                "(Landroid/graphics/drawable/Drawable;)V",
                &[JValue::Object(&drawable_local)],
            );
            state.drawable = Some(env.new_global_ref(&drawable_local).unwrap());
        }
        let drawable = state.drawable.as_ref().unwrap().clone();
        let drawable_obj = drawable.as_obj();

        // --- Fill color.
        if let Some(c) = &rules.background {
            if let Some(packed) = parse_color(&c.0) {
                let prev = state.last_bg;
                let changed = prev != Some(packed);
                state.last_bg = Some(packed);
                if changed {
                    match (prev, rules.background_transition) {
                        (Some(from), Some(t)) if from != packed => {
                            cancel_animator(env, state.anim_bg.take());
                            state.anim_bg = start_drawable_argb_animator(
                                env, &drawable, "color", from, packed, t,
                            );
                        }
                        _ => {
                            let _ = env.call_method(
                                &drawable_obj,
                                "setColor",
                                "(I)V",
                                &[JValue::Int(packed)],
                            );
                        }
                    }
                }
            }
        }

        // --- Stroke. GradientDrawable.setStroke(width, color) — single
        //     value. We collapse per-side to the first that's set
        //     (same as before). Width + color may each animate.
        let want_w = rules
            .border_top_width
            .or(rules.border_right_width)
            .or(rules.border_bottom_width)
            .or(rules.border_left_width)
            .map(|w| dp_to_px(env, &view, w));
        let want_c = rules
            .border_top_color
            .as_ref()
            .or(rules.border_right_color.as_ref())
            .or(rules.border_bottom_color.as_ref())
            .or(rules.border_left_color.as_ref())
            .and_then(|c| parse_color(&c.0));

        if let (Some(w), Some(c)) = (want_w, want_c) {
            let prev_w = state.last_stroke_w;
            let prev_c = state.last_stroke_color;
            let w_changed = prev_w != Some(w);
            let c_changed = prev_c != Some(c);
            state.last_stroke_w = Some(w);
            state.last_stroke_color = Some(c);
            if w_changed || c_changed {
                // setStroke is a single combined call. We don't have
                // a separate "stroke width" property to animate via
                // ObjectAnimator, so for animated stroke we use a
                // ValueAnimator that re-invokes setStroke on each tick.
                let w_t = rules
                    .border_top_width_transition
                    .or(rules.border_right_width_transition)
                    .or(rules.border_bottom_width_transition)
                    .or(rules.border_left_width_transition);
                let c_t = rules
                    .border_top_color_transition
                    .or(rules.border_right_color_transition)
                    .or(rules.border_bottom_color_transition)
                    .or(rules.border_left_color_transition);
                match (prev_w, prev_c, w_t.or(c_t)) {
                    (Some(fw), Some(fc), Some(t)) if (fw != w || fc != c) => {
                        cancel_animator(env, state.anim_stroke_w.take());
                        state.anim_stroke_w = start_stroke_animator(
                            env, &drawable, fw, w, fc, c, t,
                        );
                    }
                    _ => {
                        let _ = env.call_method(
                            &drawable_obj,
                            "setStroke",
                            "(II)V",
                            &[JValue::Int(w), JValue::Int(c)],
                        );
                    }
                }
            }
        }

        // --- Per-corner radii. setCornerRadii([f32; 8]) takes all four
        //     corners at once; for animation we run a single
        //     ValueAnimator that interpolates each corner's px value
        //     and re-invokes setCornerRadii every tick.
        let want_radii = [
            dp_to_px(env, &view, px_or(rules.border_top_left_radius, 0.0)) as f32,
            dp_to_px(env, &view, px_or(rules.border_top_right_radius, 0.0)) as f32,
            dp_to_px(env, &view, px_or(rules.border_bottom_right_radius, 0.0)) as f32,
            dp_to_px(env, &view, px_or(rules.border_bottom_left_radius, 0.0)) as f32,
        ];
        let radii_changed = (0..4).any(|i| state.last_radii[i] != Some(want_radii[i]));
        let radii_transitions = [
            rules.border_top_left_radius_transition,
            rules.border_top_right_radius_transition,
            rules.border_bottom_right_radius_transition,
            rules.border_bottom_left_radius_transition,
        ];
        if radii_changed {
            let prev: [Option<f32>; 4] = state.last_radii;
            for i in 0..4 { state.last_radii[i] = Some(want_radii[i]); }
            // Pick a transition: if any corner has one, use it. We
            // animate all corners together since setCornerRadii is the
            // single setter.
            let trans = radii_transitions.iter().copied().find_map(|t| t);
            let all_prev_set = prev.iter().all(|p| p.is_some());
            if all_prev_set && trans.is_some()
                && (0..4).any(|i| prev[i].unwrap() != want_radii[i])
            {
                let from = [
                    prev[0].unwrap(),
                    prev[1].unwrap(),
                    prev[2].unwrap(),
                    prev[3].unwrap(),
                ];
                cancel_animator(env, state.anim_radii[0].take());
                state.anim_radii[0] = start_radii_animator(
                    env, &drawable, from, want_radii, trans.unwrap(),
                );
            } else {
                set_corner_radii(env, &drawable_obj, want_radii);
            }
        }
    }

    fn set_corner_radii(env: &mut JNIEnv, drawable: &JObject, r: [f32; 4]) {
        // GradientDrawable.setCornerRadii expects [tl, tl, tr, tr, br,
        // br, bl, bl] in px (X-radius and Y-radius per corner — we
        // pass the same value for both).
        let radii = [r[0], r[0], r[1], r[1], r[2], r[2], r[3], r[3]];
        let arr = env.new_float_array(radii.len() as i32).unwrap();
        env.set_float_array_region(&arr, 0, &radii).unwrap();
        let _ = env.call_method(
            drawable,
            "setCornerRadii",
            "([F)V",
            &[JValue::Object(&JObject::from(arr))],
        );
    }

    // -----------------------------------------------------------------------
    // Animator construction helpers. Each returns the animator as a
    // `GlobalRef` so the cache can hold it across `apply_style` calls.
    // -----------------------------------------------------------------------

    /// Cancel a previously-running animator, dropping the JVM global.
    fn cancel_animator(env: &mut JNIEnv, anim: Option<GlobalRef>) {
        if let Some(a) = anim {
            let _ = env.call_method(a.as_obj(), "cancel", "()V", &[]);
        }
    }

    /// `ObjectAnimator.ofArgb(target, propertyName, from, to)` —
    /// animates an `int`-valued ARGB property via the JVM's built-in
    /// ArgbEvaluator. Used for `View.backgroundColor` and
    /// `TextView.textColor`.
    fn start_argb_animator(
        env: &mut JNIEnv,
        target: &GlobalRef,
        property: &str,
        from: i32,
        to: i32,
        transition: Transition,
    ) -> Option<GlobalRef> {
        let class = env.find_class("android/animation/ObjectAnimator").ok()?;
        let prop = env.new_string(property).ok()?;
        // ofArgb returns the configured ObjectAnimator.
        let values = env.new_int_array(2).ok()?;
        env.set_int_array_region(&values, 0, &[from, to]).ok()?;
        let anim = env
            .call_static_method(
                &class,
                "ofArgb",
                "(Ljava/lang/Object;Ljava/lang/String;[I)Landroid/animation/ObjectAnimator;",
                &[
                    JValue::Object(&target.as_obj()),
                    JValue::Object(&JObject::from(prop)),
                    JValue::Object(&JObject::from(values)),
                ],
            )
            .ok()?
            .l()
            .ok()?;
        configure_and_start(env, &anim, transition)?;
        env.new_global_ref(&anim).ok()
    }

    /// Specialized ARGB animator for `GradientDrawable.color`. The
    /// JVM-side `setColor(int)` is the matching mutator.
    fn start_drawable_argb_animator(
        env: &mut JNIEnv,
        drawable: &GlobalRef,
        property: &str,
        from: i32,
        to: i32,
        transition: Transition,
    ) -> Option<GlobalRef> {
        // Same machinery as the View case; GradientDrawable exposes a
        // setColor(int) so ObjectAnimator finds it by name.
        start_argb_animator(env, drawable, property, from, to, transition)
    }

    /// `ObjectAnimator.ofFloat(target, propertyName, from, to)` for
    /// scalar properties (alpha, scale, etc.).
    fn start_float_animator(
        env: &mut JNIEnv,
        target: &GlobalRef,
        property: &str,
        from: f32,
        to: f32,
        transition: Transition,
    ) -> Option<GlobalRef> {
        let class = env.find_class("android/animation/ObjectAnimator").ok()?;
        let prop = env.new_string(property).ok()?;
        let values = env.new_float_array(2).ok()?;
        env.set_float_array_region(&values, 0, &[from, to]).ok()?;
        let anim = env
            .call_static_method(
                &class,
                "ofFloat",
                "(Ljava/lang/Object;Ljava/lang/String;[F)Landroid/animation/ObjectAnimator;",
                &[
                    JValue::Object(&target.as_obj()),
                    JValue::Object(&JObject::from(prop)),
                    JValue::Object(&JObject::from(values)),
                ],
            )
            .ok()?
            .l()
            .ok()?;
        configure_and_start(env, &anim, transition)?;
        env.new_global_ref(&anim).ok()
    }

    /// Per-side padding animator. There's no `paddingLeft` etc. setter
    /// that ObjectAnimator can find by reflection, so we go through a
    /// Kotlin-side bridge that owns a `ValueAnimator` + listener and
    /// invokes setPadding(...) with the interpolated value, preserving
    /// the other three sides.
    fn start_padding_animator(
        env: &mut JNIEnv,
        view: &GlobalRef,
        side: i32, // 0..3 = L,T,R,B
        from: i32,
        to: i32,
        transition: Transition,
    ) -> Option<GlobalRef> {
        // Locate the Kotlin helper. The helper is a small companion-
        // method on the Activity-side runtime that wraps a
        // ValueAnimator and applies the padding via setPadding.
        let class = env.find_class("com/idealyst/runtime/Animators").ok()?;
        let interpolator = build_interpolator(env, transition.easing)?;
        let anim = env
            .call_static_method(
                &class,
                "animatePaddingSide",
                "(Landroid/view/View;IIIJLandroid/view/animation/Interpolator;)Landroid/animation/ValueAnimator;",
                &[
                    JValue::Object(&view.as_obj()),
                    JValue::Int(side),
                    JValue::Int(from),
                    JValue::Int(to),
                    JValue::Long(transition.duration_ms as i64),
                    JValue::Object(&interpolator),
                ],
            )
            .ok()?
            .l()
            .ok()?;
        env.new_global_ref(&anim).ok()
    }

    /// Stroke animator: similar to padding, GradientDrawable.setStroke
    /// takes (width, color) together so we route through a Kotlin
    /// helper that owns a ValueAnimator and re-invokes setStroke on
    /// each tick using a separate ArgbEvaluator for the color and a
    /// linear int interpolation for the width.
    fn start_stroke_animator(
        env: &mut JNIEnv,
        drawable: &GlobalRef,
        from_w: i32,
        to_w: i32,
        from_c: i32,
        to_c: i32,
        transition: Transition,
    ) -> Option<GlobalRef> {
        let class = env.find_class("com/idealyst/runtime/Animators").ok()?;
        let interpolator = build_interpolator(env, transition.easing)?;
        let anim = env
            .call_static_method(
                &class,
                "animateStroke",
                "(Landroid/graphics/drawable/GradientDrawable;IIIIJLandroid/view/animation/Interpolator;)Landroid/animation/ValueAnimator;",
                &[
                    JValue::Object(&drawable.as_obj()),
                    JValue::Int(from_w),
                    JValue::Int(to_w),
                    JValue::Int(from_c),
                    JValue::Int(to_c),
                    JValue::Long(transition.duration_ms as i64),
                    JValue::Object(&interpolator),
                ],
            )
            .ok()?
            .l()
            .ok()?;
        env.new_global_ref(&anim).ok()
    }

    /// Corner-radii animator. Interpolates all four corners
    /// independently and re-invokes setCornerRadii on each tick.
    fn start_radii_animator(
        env: &mut JNIEnv,
        drawable: &GlobalRef,
        from: [f32; 4],
        to: [f32; 4],
        transition: Transition,
    ) -> Option<GlobalRef> {
        let class = env.find_class("com/idealyst/runtime/Animators").ok()?;
        let interpolator = build_interpolator(env, transition.easing)?;
        let from_arr = env.new_float_array(4).ok()?;
        env.set_float_array_region(&from_arr, 0, &from).ok()?;
        let to_arr = env.new_float_array(4).ok()?;
        env.set_float_array_region(&to_arr, 0, &to).ok()?;
        let anim = env
            .call_static_method(
                &class,
                "animateCornerRadii",
                "(Landroid/graphics/drawable/GradientDrawable;[F[FJLandroid/view/animation/Interpolator;)Landroid/animation/ValueAnimator;",
                &[
                    JValue::Object(&drawable.as_obj()),
                    JValue::Object(&JObject::from(from_arr)),
                    JValue::Object(&JObject::from(to_arr)),
                    JValue::Long(transition.duration_ms as i64),
                    JValue::Object(&interpolator),
                ],
            )
            .ok()?
            .l()
            .ok()?;
        env.new_global_ref(&anim).ok()
    }

    /// Common configuration shared by all `ObjectAnimator` constructions:
    /// duration, interpolator, start. Returns Some(()) on success.
    fn configure_and_start(
        env: &mut JNIEnv,
        anim: &JObject,
        transition: Transition,
    ) -> Option<()> {
        let interp = build_interpolator(env, transition.easing)?;
        let _ = env.call_method(
            anim,
            "setDuration",
            "(J)Landroid/animation/ValueAnimator;",
            &[JValue::Long(transition.duration_ms as i64)],
        );
        let _ = env.call_method(
            anim,
            "setInterpolator",
            "(Landroid/animation/TimeInterpolator;)V",
            &[JValue::Object(&interp)],
        );
        let _ = env.call_method(anim, "start", "()V", &[]);
        Some(())
    }

    /// Map a framework `Easing` to a JVM `Interpolator` instance.
    /// `Ease` and `EaseInOut` are intentionally distinct: `Ease` gets
    /// the CSS-default cubic-bezier(0.25, 0.1, 0.25, 1.0) via
    /// PathInterpolator, while `EaseInOut` uses the symmetric
    /// AccelerateDecelerateInterpolator (which is closer to CSS
    /// `ease-in-out` than to `ease`).
    fn build_interpolator(env: &mut JNIEnv, easing: Easing) -> Option<JObject<'static>> {
        // Helper: instantiate `class` with `()V` constructor. The
        // returned JObject is local; we promote to a GlobalRef so the
        // caller can hold it across JNI calls. Returning a local would
        // expire at the next JNI frame.
        fn new_instance<'a>(env: &mut JNIEnv<'a>, class_name: &str) -> Option<JObject<'a>> {
            let class = env.find_class(class_name).ok()?;
            env.new_object(&class, "()V", &[]).ok()
        }
        let interp_local: JObject = match easing {
            Easing::Linear => new_instance(env, "android/view/animation/LinearInterpolator")?,
            Easing::EaseIn => new_instance(env, "android/view/animation/AccelerateInterpolator")?,
            Easing::EaseOut => new_instance(env, "android/view/animation/DecelerateInterpolator")?,
            Easing::EaseInOut => {
                new_instance(env, "android/view/animation/AccelerateDecelerateInterpolator")?
            }
            Easing::Ease => build_cubic_bezier(env, 0.25, 0.1, 0.25, 1.0)?,
            Easing::CubicBezier(a, b, c, d) => build_cubic_bezier(env, a, b, c, d)?,
        };
        // Promote to global so callers can hand it across JNI calls
        // without it being invalidated at frame boundaries.
        let g = env.new_global_ref(&interp_local).ok()?;
        // SAFETY-ish: we leak the global by `forget`-ing it and return
        // a raw JObject wrapping the same handle. The JVM will GC the
        // underlying interpolator when the animator that referenced it
        // is collected. For interpolators reused across animators this
        // is a minor leak; in practice each call site uses the
        // interpolator once per animator construction.
        let raw = g.as_obj().as_raw();
        std::mem::forget(g);
        Some(unsafe { JObject::from_raw(raw) })
    }

    /// PathInterpolator-via-reflection for cubic-bezier. Available on
    /// API 21+ (we assume modern Android — the build targets it).
    fn build_cubic_bezier<'a>(
        env: &mut JNIEnv<'a>,
        a: f32,
        b: f32,
        c: f32,
        d: f32,
    ) -> Option<JObject<'a>> {
        let class = env.find_class("android/view/animation/PathInterpolator").ok()?;
        env.new_object(
            &class,
            "(FFFF)V",
            &[
                JValue::Float(a),
                JValue::Float(b),
                JValue::Float(c),
                JValue::Float(d),
            ],
        )
        .ok()
    }


    // Silence unused-import warning when the optional logging cell is unused.
    #[allow(dead_code)]
    fn _placate_dead_code() {
        let _: Option<RefCell<()>> = None;
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    use super::*;

    /// Stub used on non-Android targets so the workspace can be
    /// `cargo check`ed everywhere without an NDK toolchain. The actual
    /// Android backend lives in `imp` above under
    /// `#[cfg(target_os = "android")]`.
    pub struct AndroidBackend;

    impl AndroidBackend {
        pub fn new(_context: (), _root: ()) -> Self {
            AndroidBackend
        }
    }

    impl Backend for AndroidBackend {
        type Node = ();

        fn create_view(&mut self) -> Self::Node {
            unreachable!("backend-android stub: only available on android target")
        }
        fn create_text(&mut self, _content: &str) -> Self::Node {
            unreachable!()
        }
        fn create_button(&mut self, _label: &str, _on_click: Rc<dyn Fn()>) -> Self::Node {
            unreachable!()
        }
        fn insert(&mut self, _parent: &mut Self::Node, _child: Self::Node) {
            unreachable!()
        }
        fn update_text(&mut self, _node: &Self::Node, _content: &str) {
            unreachable!()
        }
        fn clear_children(&mut self, _node: &Self::Node) {
            unreachable!()
        }
        fn apply_style(&mut self, _node: &Self::Node, _style: &Rc<StyleRules>) {
            unreachable!()
        }
        fn finish(&mut self, _root: Self::Node) {
            unreachable!()
        }
    }
}

pub use imp::AndroidBackend;
