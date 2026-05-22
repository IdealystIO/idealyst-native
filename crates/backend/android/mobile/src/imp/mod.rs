//! Android-specific implementation of [`crate::AndroidBackend`].
//!
//! This module is the parent of every per-primitive file and shared
//! helper. The `Backend` impl at the bottom delegates each method to
//! the matching submodule.

mod animation;
mod callbacks;
mod font;
mod jni_exports;
mod primitives;
pub(crate) mod scheduler;
mod style;
// `view_screen_rect` lives here because it depends on this crate's
// `with_env` / `JAVA_VM` state (owned by `JNI_OnLoad`, which is a
// per-cdylib singleton). The rest of the JNI helpers — and the
// render loop driver — live in `backend-android-core` and are
// imported directly by their callers.
pub(crate) mod view_rect;

use framework_core::{Backend, ButtonHandle, StyleRules};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::{jint, jlong, JNI_VERSION_1_6};
use jni::{JNIEnv, JavaVM};
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::OnceLock;

use callbacks::StateCallback;

/// Cached `JavaVM`. Filled by `JNI_OnLoad` when the .so is dlopen'd
/// by the Android runtime. Every JNI call inside the backend goes
/// through this to attach the current thread.
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
pub(super) fn with_env<R>(f: impl FnOnce(&mut JNIEnv) -> R) -> R {
    let vm = JAVA_VM.get().expect("JNI_OnLoad has not been called");
    let mut env = vm
        .attach_current_thread_permanently()
        .expect("attach_current_thread_permanently");
    f(&mut env)
}

/// Per-node animation state. Keyed by the raw `*JObject` pointer
/// extracted from each node's `GlobalRef` — the JVM keeps the
/// underlying object alive as long as we hold the `GlobalRef`, so the
/// pointer is stable for the node's lifetime.
///
/// We track:
/// - the *last applied* value for each animatable property, so
///   `apply_style` can detect "this property actually changed"
///   before launching an animator;
/// - the *running animator* per property, so a value change mid-
///   animation cancels the current animator and starts fresh without
///   leaking JVM objects;
/// - the persistent `GradientDrawable` used for background + border +
///   radii, so corner/stroke animation can mutate one drawable
///   instead of rebuilding it every frame.
#[derive(Default)]
pub(crate) struct NodeAnim {
    // Last-applied snapshots (Android pixel-space values).
    pub(crate) last_bg: Option<i32>,           // packed ARGB
    pub(crate) last_text_color: Option<i32>,   // packed ARGB
    pub(crate) last_caret_color: Option<i32>,  // packed ARGB — short-circuits redundant setTextCursorDrawable
    pub(crate) last_alpha: Option<f32>,
    pub(crate) last_padding: [Option<i32>; 4], // L, T, R, B
    pub(crate) last_radii: [Option<f32>; 4],   // tl, tr, br, bl (px)
    pub(crate) last_stroke_w: Option<i32>,
    pub(crate) last_stroke_color: Option<i32>,

    // Running animator handles, one per animatable bucket. Each is a
    // JVM `Animator` we cancel + restart on value change.
    pub(crate) anim_bg: Option<GlobalRef>,
    pub(crate) anim_text_color: Option<GlobalRef>,
    pub(crate) anim_alpha: Option<GlobalRef>,
    pub(crate) anim_padding: [Option<GlobalRef>; 4],
    pub(crate) anim_radii: [Option<GlobalRef>; 4],
    /// Single animator drives both stroke width and color (one
    /// `setStroke` call interpolates both at once via the Kotlin
    /// helper); no separate color slot needed.
    pub(crate) anim_stroke_w: Option<GlobalRef>,

    // Persistent drawable for backgrounds that have border/radius.
    // Held so corner/stroke animators can mutate one drawable
    // instead of `setBackground`-ing a fresh one every tick.
    pub(crate) drawable: Option<GlobalRef>,

    /// Per-stop sRGB colors for the node's `background_gradient`.
    /// Stashed by `apply_gradient_to_drawable` so the per-frame
    /// `set_animated_color(GradientStopColor)` path can mutate one
    /// entry, repack the ARGB `int[]`, and call `setColors` on the
    /// stored drawable without re-allocating. Empty when the node
    /// has no gradient.
    pub(crate) gradient_stops: Vec<[f32; 4]>,
    /// Per-stop offsets (0.0..=1.0) parallel to `gradient_stops`.
    /// Required for the API-29+ `setColors(int[], float[])` path
    /// that honors non-uniform offsets; ignored on the legacy
    /// path. Stashed alongside `gradient_stops` at apply time so
    /// the per-frame writer doesn't need to walk the original
    /// `Gradient.stops` again.
    pub(crate) gradient_offsets: Vec<f32>,

    /// Static `transform: translate(N%, …)` requests, stashed at
    /// apply-style time and resolved against the view's actual
    /// pixel dimensions in the layout pass. CSS-spec translate-% is
    /// BOX-relative, so we can't compute the px shift until Taffy
    /// produces a frame. `None` on an axis means "no percent
    /// translate requested" (a `Length::Px` translate was already
    /// applied directly at style time).
    pub(crate) transform_translate_pct_x: Option<f32>,
    pub(crate) transform_translate_pct_y: Option<f32>,

    /// Radial gradient extent + radius factor, stashed when a
    /// `GradientKind::Radial` is applied. `GradientDrawable.setGradientRadius`
    /// takes pixels, but at apply-style time the view hasn't been
    /// measured yet — `getMeasuredWidth/Height` both return 0, and
    /// the apply path falls back to a fixed default. The layout
    /// pass calls `sync_radial_gradient_radius` with the just-laid-
    /// out frame to recompute the radius and write the real value.
    pub(crate) gradient_radial_extent: Option<framework_core::RadialExtent>,
    pub(crate) gradient_radial_radius_factor: Option<f32>,

    /// Raw pointer to the leaked `Box<StateCallback>` held by the
    /// JVM-side `RustStateListener`. Blanked (inner closure cleared)
    /// — not freed — when the node is unstyled; see the `StateCallback`
    /// doc for why. Zero means none allocated yet.
    pub(crate) state_callback_ptr: jlong,
}

pub struct AndroidBackend {
    /// Application/Activity context — used as the first argument to
    /// every `View(Context)` constructor.
    pub(crate) context: GlobalRef,
    /// Root container provided by the Activity. `finish` is a no-op
    /// because we don't own the root; we just append into it.
    pub(crate) root: GlobalRef,
    /// Per-node animation state, keyed by raw `JObject*` pointer.
    /// Entries created lazily on first `apply_style`; removed on
    /// `on_node_unstyled` via the framework's lifecycle hook.
    pub(crate) anim_state: HashMap<usize, NodeAnim>,
    /// Per-navigator state. Keyed by the navigator container's raw
    /// `JObject*` pointer (the same scheme `anim_state` uses).
    /// Entries inserted on `create_navigator`, removed in
    /// `release_navigator`.
    pub(crate) navigator_instances: primitives::navigator::NavigatorInstances,
    /// Per-tab/drawer-navigator state. Keyed by the navigator
    /// container's raw `JObject*` pointer (same scheme
    /// `navigator_instances` uses). Tab + drawer navigators on
    /// Android are plain FrameLayout + View-swap; they don't use
    /// FragmentManager, so they get their own instance table to keep
    /// the stack navigator's machinery uncluttered.
    pub(crate) tab_drawer_instances: primitives::tab_drawer::TabDrawerInstances,
    /// ScrollView outer→inner mapping. Keyed by the outer
    /// (framework-visible) ScrollView's raw `JObject*` pointer; value
    /// is a `GlobalRef` to its inner LinearLayout, where child
    /// inserts actually land. Populated by `scroll_view::create`,
    /// cleared in `on_node_unstyled` (most ScrollViews are styled;
    /// for unstyled instances the entry persists for the backend's
    /// lifetime — small and bounded).
    pub(crate) scroll_view_inner: HashMap<usize, GlobalRef>,
    /// Per-portal state. Keyed by the dialog's content-holder
    /// node's raw `JObject*` pointer. Populated by `overlay::create`,
    /// removed by `release_portal`. `view::insert` looks here to
    /// detect that a portal's content holder shouldn't be spliced
    /// into the surrounding parent view — the dialog window owns
    /// its parenting.
    pub(crate) portal_instances: primitives::overlay::PortalInstances,
    /// Taffy layout tree. Mirrors the iOS backend: every backend-
    /// created view registers a Taffy node, every `insert` adds the
    /// child to the parent's Taffy node, every `apply_style` mirrors
    /// the resolved style into Taffy. `finish` (and any later
    /// `apply_style` on a mounted view) runs `compute(root, vw, vh)`
    /// and writes per-child `FrameLayout.LayoutParams { leftMargin,
    /// topMargin, width, height }` so absolute-positioned and
    /// flex-laid-out children both land where Taffy says they should.
    pub(crate) layout: native_layout::LayoutTree,
    /// View pointer → (`GlobalRef`, Taffy node). Indexed by the same
    /// raw `JObject*` pointer scheme as `anim_state`. Iterated in the
    /// layout pass to apply computed frames.
    pub(crate) view_to_layout:
        HashMap<usize, (GlobalRef, native_layout::LayoutNode)>,
    /// Registry of third-party `Primitive::External` handlers,
    /// populated by `register_external::<T>(...)` calls from
    /// per-platform leaf crates (e.g. `webview-android::register`).
    /// `create_external` looks the handler up by payload TypeId;
    /// unregistered kinds fall through to a "not supported" placeholder
    /// TextView.
    pub(crate) external_handlers:
        framework_core::ExternalRegistry<AndroidBackend>,
    /// Per-`Typeface` registry of custom fonts. Filled by
    /// [`Backend::register_asset`] for `AssetTag::Font`
    /// (bytes → Android `Typeface.createFromFile`) and
    /// [`Backend::register_typeface`] (records the (weight, style) →
    /// Typeface map per family). Consulted by the style applier to
    /// drive `TextView.setTypeface`.
    pub(crate) font_registry: font::FontRegistry,
}

/// Read the device's `density` (screen-pixels-per-dp) from the
/// host view's resources. `1.0` on the unlikely happy-path where
/// the call fails (preserves the dp-as-pixel fallback in the rest
/// of the style path).
fn density_of(env: &mut JNIEnv, view: &JObject) -> Option<f32> {
    let resources = env
        .call_method(view, "getResources", "()Landroid/content/res/Resources;", &[])
        .and_then(|v| v.l())
        .ok()?;
    let metrics = env
        .call_method(
            &resources,
            "getDisplayMetrics",
            "()Landroid/util/DisplayMetrics;",
            &[],
        )
        .and_then(|v| v.l())
        .ok()?;
    let density: f32 = env
        .get_field(&metrics, "density", "F")
        .and_then(|v| v.f())
        .ok()?;
    Some(density)
}

/// Apply a Taffy-computed `Frame` to the view's `LayoutParams`. The
/// view is expected to be a child of a `FrameLayout`-shaped parent —
/// `FrameLayout.LayoutParams` (which extends `MarginLayoutParams`)
/// reads `leftMargin`/`topMargin` for the child's top-left and
/// `width`/`height` for its size. dp-space values are converted to
/// device pixels via the host's display density.
fn apply_frame_to_layout_params(
    env: &mut JNIEnv,
    view: &GlobalRef,
    frame: native_layout::Frame,
) {
    let view_obj = view.as_obj();
    let density = density_of(env, &view_obj).unwrap_or(1.0);
    let left_px = (frame.x * density).round() as i32;
    let top_px = (frame.y * density).round() as i32;
    let w_px = (frame.width * density).round() as i32;
    let h_px = (frame.height * density).round() as i32;

    // Read the current LayoutParams. If the view isn't attached
    // yet there may be no LP — fall back to fresh
    // `FrameLayout.LayoutParams(w, h)`.
    let lp_obj = env
        .call_method(
            &view_obj,
            "getLayoutParams",
            "()Landroid/view/ViewGroup$LayoutParams;",
            &[],
        )
        .ok()
        .and_then(|v| v.l().ok());
    let lp = match lp_obj {
        Some(o) if !o.is_null() => {
            // Already a LayoutParams of *some* shape. We need it to
            // be `MarginLayoutParams` (or subclass — `FrameLayout`'s
            // own LP class extends MarginLayoutParams) so we can
            // write margins. If it isn't, wrap it.
            let mlp_class = env
                .find_class("android/view/ViewGroup$MarginLayoutParams")
                .unwrap();
            let is_mlp = env.is_instance_of(&o, &mlp_class).unwrap_or(false);
            if is_mlp {
                o
            } else {
                env.new_object(
                    &mlp_class,
                    "(II)V",
                    &[JValue::Int(w_px), JValue::Int(h_px)],
                )
                .unwrap()
            }
        }
        _ => {
            let mlp_class = env
                .find_class("android/view/ViewGroup$MarginLayoutParams")
                .unwrap();
            env.new_object(
                &mlp_class,
                "(II)V",
                &[JValue::Int(w_px), JValue::Int(h_px)],
            )
            .unwrap()
        }
    };
    let _ = env.set_field(&lp, "width", "I", JValue::Int(w_px));
    let _ = env.set_field(&lp, "height", "I", JValue::Int(h_px));
    let _ = env.set_field(&lp, "leftMargin", "I", JValue::Int(left_px));
    let _ = env.set_field(&lp, "topMargin", "I", JValue::Int(top_px));
    // Zero out trailing margins — they're authored via the same
    // taffy-computed frame and writing 0 keeps stale values from a
    // prior layout pass from leaking through.
    let _ = env.set_field(&lp, "rightMargin", "I", JValue::Int(0));
    let _ = env.set_field(&lp, "bottomMargin", "I", JValue::Int(0));
    let _ = env.call_method(
        &view_obj,
        "setLayoutParams",
        "(Landroid/view/ViewGroup$LayoutParams;)V",
        &[JValue::Object(&lp)],
    );
}

impl AndroidBackend {
    /// Construct a backend rooted at the provided Android `Context`
    /// and a parent `ViewGroup` to mount under.
    pub fn new(context: GlobalRef, root: GlobalRef) -> Self {
        Self {
            context,
            root,
            anim_state: HashMap::new(),
            navigator_instances: HashMap::new(),
            tab_drawer_instances: HashMap::new(),
            scroll_view_inner: HashMap::new(),
            portal_instances: HashMap::new(),
            layout: native_layout::LayoutTree::new(),
            view_to_layout: HashMap::new(),
            external_handlers: framework_core::ExternalRegistry::new(),
            font_registry: font::FontRegistry::new(),
        }
    }

    /// Register a handler for the third-party external primitive whose
    /// payload type is `T`. Called by per-platform leaf crates (e.g.
    /// `webview_android::register`) during app bootstrap. The handler
    /// receives the typed payload + a mutable borrow of the backend
    /// and produces the `GlobalRef` to the Android `View` to mount.
    pub fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&std::rc::Rc<T>, &mut AndroidBackend) -> GlobalRef + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }

    /// `true` if a handler for payload type `T` has been registered.
    /// Useful for opt-in graceful degradation in user code (render a
    /// static fallback if the SDK isn't available on Android).
    pub fn has_external<T: 'static>(&self) -> bool {
        self.external_handlers.has::<T>()
    }

    /// SDK extension entry point: run a closure with a JNI env and the
    /// backend's Activity/Application context. Third-party
    /// `register_external` handlers use this to construct Android
    /// `View`s (every `View(Context)` constructor takes the context as
    /// its first argument).
    ///
    /// The context is reference-stable for the backend's lifetime — it
    /// matches the `Context` passed to `AndroidBackend::new`. Returning
    /// a `GlobalRef` from the closure is the usual pattern (the SDK
    /// stashes it as its node).
    pub fn with_jni<R>(&self, f: impl FnOnce(&mut jni::JNIEnv, &GlobalRef) -> R) -> R {
        with_env(|env| f(env, &self.context))
    }

    /// Get or create a Taffy layout node for the given view. Called
    /// from every `create_*` so each backend-created view has a
    /// corresponding node in the layout tree.
    pub(crate) fn layout_for_view(
        &mut self,
        view: &GlobalRef,
    ) -> native_layout::LayoutNode {
        let key = Self::node_key(view);
        if let Some((_, node)) = self.view_to_layout.get(&key) {
            return *node;
        }
        let node = self.layout.new_node();
        self.view_to_layout.insert(key, (view.clone(), node));
        node
    }

    /// `true` once the host has a non-zero size — the layout pass
    /// can produce meaningful frames. Used by the retry loop in
    /// `scheduler::schedule_layout_pass_retry`.
    pub(crate) fn viewport_is_ready(&self) -> bool {
        let (vw, vh) = self.viewport_size();
        vw > 0.0 && vh > 0.0
    }

    /// Read the viewport (host_root) size in device-independent
    /// pixels. Taffy works in dp so the layout pass needs the host
    /// size in the same units the rest of the style path uses.
    fn viewport_size(&self) -> (f32, f32) {
        with_env(|env| {
            let host = self.root.as_obj();
            let (w_px, h_px) = (
                env.call_method(host, "getWidth", "()I", &[])
                    .and_then(|v| v.i())
                    .unwrap_or(0),
                env.call_method(host, "getHeight", "()I", &[])
                    .and_then(|v| v.i())
                    .unwrap_or(0),
            );
            if w_px <= 0 || h_px <= 0 {
                return (0.0, 0.0);
            }
            // `getResources().getDisplayMetrics().density` converts
            // device pixels back to dp so Taffy reasons in the same
            // unit the StyleRules use.
            let density = density_of(env, host).unwrap_or(1.0);
            (w_px as f32 / density, h_px as f32 / density)
        })
    }

    /// Run the layout pass: for every Taffy root (the framework's
    /// app root plus any disconnected sub-roots), compute, then
    /// iterate every registered view and write its frame onto the
    /// view's `FrameLayout.LayoutParams`.
    pub(crate) fn run_layout_pass(&mut self) {
        let (vw, vh) = self.viewport_size();
        if vw <= 0.0 || vh <= 0.0 {
            log::info!("[layout] ABORT: viewport is zero ({}, {})", vw, vh);
            return;
        }
        log::info!(
            "[layout] run_layout_pass viewport=({:.1}, {:.1}) registered_views={}",
            vw,
            vh,
            self.view_to_layout.len()
        );
        let roots: Vec<native_layout::LayoutNode> = self
            .view_to_layout
            .values()
            .map(|(_, n)| *n)
            .filter(|n| self.layout.is_root(*n))
            .collect();
        for root_node in &roots {
            self.layout.compute(*root_node, vw, vh);
        }
        // Snapshot the entries up front so the mutable JNI calls
        // below don't conflict with the borrow on `self.view_to_layout`.
        let frames: Vec<(GlobalRef, native_layout::Frame)> = self
            .view_to_layout
            .values()
            .map(|(view, n)| (view.clone(), self.layout.frame_of(*n)))
            .collect();
        with_env(|env| {
            for (view, frame) in &frames {
                if frame.width <= 0.0 && frame.height <= 0.0 {
                    continue;
                }
                apply_frame_to_layout_params(env, view, *frame);
                let key = Self::node_key(view);
                if let Some(state) = self.anim_state.get(&key) {
                    // Resolve any percent-valued `transform: translate`
                    // requests now that the box has real pixel
                    // dimensions. CSS spec: translate-% is box-relative,
                    // so the shift needs the box's own width / height —
                    // not knowable at apply-style time when bounds are
                    // still zero.
                    style::sync_transform_translate_percent(
                        env,
                        view.as_obj(),
                        state,
                        frame.width,
                        frame.height,
                    );
                    // Recompute the radial gradient's px radius now that
                    // the view has a real size. The apply-style path
                    // ran `getMeasuredWidth/Height` before the view was
                    // measured (both returned 0) and wrote a placeholder
                    // 100dp radius — that's the "small sun" smell on
                    // any view sized via a percent / aspect_ratio.
                    let density = density_of(env, &view.as_obj()).unwrap_or(1.0);
                    style::sync_radial_gradient_radius(
                        env,
                        state,
                        frame.width,
                        frame.height,
                        density,
                    );
                }
            }
        });
    }

    /// Stable key for the node's animation state. The pointer comes
    /// from the `JObject` the `GlobalRef` wraps; the JVM guarantees
    /// it's stable for as long as we hold the global ref.
    fn node_key(node: &GlobalRef) -> usize {
        node.as_obj().as_raw() as usize
    }

    /// Public sibling of `node_key`. Used by per-primitive modules
    /// (navigator) that need to key off the same JObject pointer
    /// stability the animation state relies on.
    pub(crate) fn node_key_of(node: &GlobalRef) -> usize {
        node.as_obj().as_raw() as usize
    }
}

// ---------------------------------------------------------------------------
// Typed-handle ops impls. These ZSTs sit behind `make_view_handle` /
// `make_text_handle`'s `&'static dyn` slots so author-level code can
// hold a `Ref<ViewHandle>` and reach the underlying `GlobalRef` via
// `as_any().downcast_ref::<GlobalRef>()`. They expose no methods
// today; if a primitive grows operations (e.g. `ViewOps::rect`), the
// impls below are the place to wire them.
// ---------------------------------------------------------------------------

pub(crate) struct AndroidViewOps;
impl framework_core::ViewOps for AndroidViewOps {
    /// Node's rect in its parent's coordinate system, in dp. Mirrors
    /// `IosViewOps::frame` so author-level code reading
    /// `Ref<ViewHandle>::frame()` gets equivalent behavior on both
    /// native platforms. The welcome example's planet-orbit driver
    /// depends on this — without an override the trait default returns
    /// `None` and the orbit falls back to a hard-coded portrait
    /// viewport even after rotation.
    ///
    /// `View.getX/getY` return device pixels (float); `getWidth/
    /// getHeight` return device pixels (int). Divide by display
    /// density to land in the same dp units Taffy / `StyleRules`
    /// reason in. Returns `None` when the view hasn't been measured
    /// yet (width/height == 0).
    fn frame(
        &self,
        node: &dyn std::any::Any,
    ) -> Option<framework_core::primitives::portal::ViewportRect> {
        let view = node.downcast_ref::<GlobalRef>()?;
        with_env(|env| {
            let obj = view.as_obj();
            let w_px = env
                .call_method(&obj, "getWidth", "()I", &[])
                .and_then(|v| v.i())
                .unwrap_or(0);
            let h_px = env
                .call_method(&obj, "getHeight", "()I", &[])
                .and_then(|v| v.i())
                .unwrap_or(0);
            if w_px <= 0 || h_px <= 0 {
                return None;
            }
            let x_px = env
                .call_method(&obj, "getX", "()F", &[])
                .and_then(|v| v.f())
                .unwrap_or(0.0);
            let y_px = env
                .call_method(&obj, "getY", "()F", &[])
                .and_then(|v| v.f())
                .unwrap_or(0.0);
            let density = density_of(env, &obj).unwrap_or(1.0);
            Some(framework_core::primitives::portal::ViewportRect {
                x: x_px / density,
                y: y_px / density,
                width: w_px as f32 / density,
                height: h_px as f32 / density,
            })
        })
    }
}
pub(crate) static ANDROID_VIEW_OPS: AndroidViewOps = AndroidViewOps;

pub(crate) struct AndroidTextOps;
impl framework_core::TextOps for AndroidTextOps {}
pub(crate) static ANDROID_TEXT_OPS: AndroidTextOps = AndroidTextOps;

// ---------------------------------------------------------------------------
// Global self-handle. Mirrors `IOS_BACKEND_SELF` — host code installs
// a `Weak<RefCell<AndroidBackend>>` once at `attach` so the
// cross-platform animation system's per-frame subscribers can reach
// the backend without the welcome example having to thread the
// `Rc<RefCell<AndroidBackend>>` through every closure.
// ---------------------------------------------------------------------------

thread_local! {
    static ANDROID_BACKEND_SELF: std::cell::RefCell<Option<std::rc::Weak<std::cell::RefCell<AndroidBackend>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install the backend's self-reference. Called once by the host
/// wrapper after wrapping the backend in `Rc<RefCell<>>`. Without it,
/// `set_animated_f32` / `set_animated_color` quietly no-op.
pub fn install_global_self(weak: std::rc::Weak<std::cell::RefCell<AndroidBackend>>) {
    ANDROID_BACKEND_SELF.with(|s| {
        *s.borrow_mut() = Some(weak);
    });
}

/// Push a scalar animation property update to `node` on the installed
/// global backend. Same shape as `backend_ios::set_animated_f32`.
/// No-ops cleanly if no backend is installed, the install has been
/// dropped, or the backend is currently borrowed (the in-flight call
/// will see the new AV value on its next frame).
pub fn set_animated_f32(
    node: &GlobalRef,
    prop: framework_core::animation::AnimProp,
    value: f32,
) {
    let weak = ANDROID_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        use framework_core::Backend;
        b.set_animated_f32(node, prop, value);
    };
}

/// Color-family counterpart of [`set_animated_f32`]. Routes through
/// the global backend's `set_animated_color`.
pub fn set_animated_color(
    node: &GlobalRef,
    prop: framework_core::animation::AnimProp,
    value: [f32; 4],
) {
    let weak = ANDROID_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        use framework_core::Backend;
        b.set_animated_color(node, prop, value);
    };
}

// ---------------------------------------------------------------------------
// Backend trait impl. Each method delegates to the matching primitive
// module (or to one of the style/helpers helpers). Keep this thin —
// anything substantial belongs in the primitive's file.
// ---------------------------------------------------------------------------

impl Backend for AndroidBackend {
    type Node = GlobalRef;

    fn color_scheme(&self) -> framework_core::ColorScheme {
        // context.getResources().getConfiguration().uiMode & UI_MODE_NIGHT_MASK
        // UI_MODE_NIGHT_UNDEFINED = 0x00, UI_MODE_NIGHT_NO = 0x10,
        // UI_MODE_NIGHT_YES = 0x20
        with_env(|env| {
            let resources = env
                .call_method(&self.context, "getResources", "()Landroid/content/res/Resources;", &[])
                .and_then(|r| r.l());
            let config = resources.and_then(|res| {
                env.call_method(&res, "getConfiguration", "()Landroid/content/res/Configuration;", &[])
                    .and_then(|c| c.l())
            });
            let ui_mode = config.and_then(|cfg| {
                env.get_field(&cfg, "uiMode", "I").and_then(|v| v.i())
            });
            match ui_mode {
                Ok(mode) => match mode & 0x30 {
                    0x10 => framework_core::ColorScheme::Light,
                    0x20 => framework_core::ColorScheme::Dark,
                    _ => framework_core::ColorScheme::Auto,
                },
                Err(_) => framework_core::ColorScheme::Auto,
            }
        })
    }

    fn create_view(&mut self) -> Self::Node {
        primitives::view::create(self)
    }

    fn create_link(
        &mut self,
        config: framework_core::primitives::link::LinkConfig,
    ) -> Self::Node {
        primitives::link::create(self, config.on_activate)
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
        primitives::text::create(self, content)
    }

    fn create_button(&mut self, label: &str, on_click: &framework_core::Action, _leading_icon: Option<&framework_core::IconData>, _trailing_icon: Option<&framework_core::IconData>) -> Self::Node {
        // TODO: render icons as compound drawables on the button
        primitives::button::create(self, label, on_click.fire.clone())
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        primitives::view::insert(self, parent, child)
    }

    fn install_touch_handler(
        &mut self,
        node: &Self::Node,
        handler: framework_core::TouchHandler,
    ) {
        primitives::touch::install(self, node, handler)
    }

    fn claim_touch(
        &mut self,
        node: &Self::Node,
        _touch_id: framework_core::TouchId,
    ) {
        // The Kotlin `RustTouchListener` already calls
        // `requestDisallowInterceptTouchEvent` inline when a touch
        // returns `claim: true`; the Backend trait method exists for
        // symmetry with the framework's abstract claim protocol and
        // any future code path that wants to claim outside a
        // `MotionEvent` dispatch.
        primitives::touch::claim(self, node)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        primitives::text::update_text(node, content)
    }

    fn create_image(&mut self, src: &str, alt: Option<&str>) -> Self::Node {
        primitives::image::create(self, src, alt)
    }

    fn create_icon(
        &mut self,
        data: &framework_core::primitives::icon::IconData,
        color: Option<&framework_core::Color>,
    ) -> Self::Node {
        primitives::icon::create(self, data, color)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &framework_core::Color) {
        primitives::icon::update_color(node, color)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        primitives::icon::update_stroke(node, progress)
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: framework_core::Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        primitives::icon::animate_stroke(node, from, to, duration_ms, easing, infinite, autoreverses)
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<framework_core::primitives::key::KeyDownHandler>,
    ) -> Self::Node {
        primitives::text_input::create(self, initial_value, placeholder, on_change, on_key_down)
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        primitives::text_input::update_value(node, value)
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<framework_core::primitives::key::KeyDownHandler>,
    ) -> Self::Node {
        primitives::text_input::create_multiline(self, initial_value, placeholder, on_change, on_key_down)
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        primitives::text_input::update_value(node, value)
    }

    fn make_text_input_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::text_input::TextInputHandle {
        primitives::text_input::make_text_input_handle(node)
    }

    fn make_text_area_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::text_area::TextAreaHandle {
        primitives::text_input::make_text_area_handle(node)
    }

    fn create_toggle(&mut self, initial_value: bool, on_change: Rc<dyn Fn(bool)>) -> Self::Node {
        primitives::toggle::create(self, initial_value, on_change)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        primitives::toggle::update_value(node, value)
    }

    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
        primitives::scroll_view::create(self, horizontal)
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    ) -> Self::Node {
        primitives::slider::create(self, initial_value, min, max, step, on_change)
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        primitives::slider::update_value(node, value)
    }

    fn create_video(
        &mut self,
        src: &str,
        autoplay: bool,
        controls: bool,
        loop_playback: bool,
    ) -> Self::Node {
        primitives::video::create(self, src, autoplay, controls, loop_playback)
    }

    fn update_video_src(&mut self, node: &Self::Node, src: &str) {
        primitives::video::update_src(node, src)
    }

    fn create_virtualizer(
        &mut self,
        callbacks: framework_core::VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
    ) -> Self::Node {
        primitives::virtualizer::create(self, callbacks, overscan, horizontal)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        primitives::virtualizer::data_changed(node)
    }

    fn create_activity_indicator(
        &mut self,
        size: framework_core::primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&framework_core::Color>,
    ) -> Self::Node {
        primitives::activity_indicator::create(self, size, color)
    }

    fn make_video_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::video::VideoHandle {
        primitives::video::make_handle(node)
    }

    fn create_navigator(
        &mut self,
        callbacks: framework_core::NavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::NavigatorControl>,
    ) -> Self::Node {
        primitives::navigator::create(self, callbacks, control)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        _options: framework_core::ScreenOptions,
    ) {
        primitives::navigator::attach_initial(self, navigator, screen, scope_id)
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        primitives::navigator::release(self, node)
    }

    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::NavigatorHandle {
        primitives::navigator::make_handle(self, node)
    }

    // Tab + drawer navigators on Android — plain FrameLayout +
    // View-swap, no FragmentManager involvement. The author's
    // .layout() closure draws the chrome (tab bar / drawer
    // sidebar); the framework swaps the active screen on Select.
    fn create_tab_navigator(
        &mut self,
        callbacks: framework_core::TabNavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::NavigatorControl>,
    ) -> Self::Node {
        primitives::tab_drawer::create_tab(self, callbacks, control)
    }

    fn tab_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::ScreenOptions,
    ) {
        primitives::tab_drawer::attach_initial(self, navigator, screen, scope_id, options)
    }

    fn release_tab_navigator(&mut self, node: &Self::Node) {
        primitives::tab_drawer::release(self, node)
    }

    fn make_tab_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::TabsHandle {
        primitives::tab_drawer::make_tab_handle(self, node)
    }

    fn create_drawer_navigator(
        &mut self,
        callbacks: framework_core::DrawerNavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::NavigatorControl>,
    ) -> Self::Node {
        primitives::tab_drawer::create_drawer(self, callbacks, control)
    }

    fn drawer_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::ScreenOptions,
    ) {
        primitives::tab_drawer::attach_initial(self, navigator, screen, scope_id, options)
    }

    fn drawer_navigator_attach_sidebar(
        &mut self,
        navigator: &Self::Node,
        sidebar: Self::Node,
    ) {
        primitives::tab_drawer::attach_sidebar(self, navigator, sidebar)
    }

    fn apply_navigator_header_style(
        &mut self,
        navigator: &Self::Node,
        style: &std::rc::Rc<framework_core::StyleRules>,
    ) {
        primitives::tab_drawer::apply_header_style(self, navigator, style)
    }

    fn apply_navigator_title_style(
        &mut self,
        navigator: &Self::Node,
        style: &std::rc::Rc<framework_core::StyleRules>,
    ) {
        primitives::tab_drawer::apply_title_style(self, navigator, style)
    }

    fn apply_navigator_button_style(
        &mut self,
        navigator: &Self::Node,
        style: &std::rc::Rc<framework_core::StyleRules>,
    ) {
        primitives::tab_drawer::apply_button_style(self, navigator, style)
    }

    fn apply_navigator_body_style(
        &mut self,
        navigator: &Self::Node,
        style: &std::rc::Rc<framework_core::StyleRules>,
    ) {
        primitives::tab_drawer::apply_body_style(self, navigator, style)
    }

    fn release_drawer_navigator(&mut self, node: &Self::Node) {
        primitives::tab_drawer::release(self, node)
    }

    fn make_drawer_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::DrawerHandle {
        primitives::tab_drawer::make_drawer_handle(self, node)
    }

    fn create_graphics(
        &mut self,
        on_ready: framework_core::primitives::graphics::OnReady,
        on_resize: framework_core::primitives::graphics::OnResize,
        on_lost: framework_core::primitives::graphics::OnLost,
    ) -> Self::Node {
        primitives::graphics::create(self, on_ready, on_resize, on_lost)
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        primitives::graphics::release(self, node)
    }

    fn make_graphics_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::graphics::GraphicsHandle {
        primitives::graphics::make_handle(node)
    }

    fn create_portal(
        &mut self,
        target: framework_core::primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
    ) -> Self::Node {
        primitives::overlay::create(self, target, on_dismiss, trap_focus)
    }

    fn release_portal(&mut self, node: &Self::Node) {
        primitives::overlay::release(self, node)
    }

    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
    ) -> Self::Node {
        // Look up the handler; clone the Rc so we can drop the registry
        // borrow before calling the handler (which itself needs
        // `&mut self`).
        if let Some(handler) = self.external_handlers.get(type_id) {
            return handler(payload, self);
        }
        // No handler registered → render a placeholder TextView so the
        // dev/user sees that an SDK binding is missing on Android
        // rather than a silent hole. `has_external::<T>()` is the
        // supported way to render custom degradation in user space.
        external_placeholder_view(self, type_name)
    }

    fn release_external(&mut self, _node: &Self::Node) {
        // No per-external bookkeeping today. Future SDK leaves that
        // keep instance state (e.g. cached callback pointers, GL
        // contexts) would clean up here, keyed by `node_key` like
        // animations/navigators do.
    }

    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
        primitives::button::make_handle(node)
    }

    /// Override the framework default so the typed handle carries the
    /// underlying `GlobalRef`. Author-level animation drivers downcast
    /// `view_handle.as_any()` to `GlobalRef` and reach the backend
    /// through `set_animated_f32` / `set_animated_color`; without this
    /// override the handle stores `Rc<()>` and the downcast fails.
    fn make_view_handle(&self, node: &Self::Node) -> framework_core::ViewHandle {
        framework_core::ViewHandle::new(Rc::new(node.clone()), &ANDROID_VIEW_OPS)
    }

    /// See [`Self::make_view_handle`]. Same plumbing for `TextHandle`
    /// so the welcome example's per-frame `setTextColor` write can
    /// reach a `TextView` (rather than `setTintColor`-equivalent on a
    /// generic wrapper) and animate `color` end-to-end.
    fn make_text_handle(&self, node: &Self::Node) -> framework_core::TextHandle {
        framework_core::TextHandle::new(Rc::new(node.clone()), &ANDROID_TEXT_OPS)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        primitives::view::clear_children(self, node)
    }

    fn register_asset(
        &mut self,
        id: framework_core::AssetId,
        kind: framework_core::AssetTag,
        source: &framework_core::AssetSource,
    ) {
        // Only the font branch needs JNI today; images on Android go
        // through `create_image(src)` directly. Future image / video
        // caches would chain here the same way the iOS backend does.
        if kind != framework_core::AssetTag::Font {
            return;
        }
        let context = self.context.clone();
        let registry = &mut self.font_registry;
        with_env(|env| {
            registry.register_asset(env, &context, id, kind, source);
        });
    }

    fn unregister_asset(
        &mut self,
        id: framework_core::AssetId,
        kind: framework_core::AssetTag,
    ) {
        self.font_registry.unregister_asset(id, kind);
    }

    fn register_typeface(
        &mut self,
        id: framework_core::assets::TypefaceId,
        family_name: &str,
        faces: &[framework_core::assets::TypefaceFace],
        fallback: framework_core::assets::SystemFallback,
    ) {
        self.font_registry
            .register_typeface(id, family_name, faces, fallback);
    }

    fn unregister_typeface(&mut self, id: framework_core::assets::TypefaceId) {
        self.font_registry.unregister_typeface(id);
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let key = Self::node_key(node);
        // Lazy-create per-node state on first apply.
        let state = self.anim_state.entry(key).or_default();
        let font_registry = &self.font_registry;
        with_env(|env| {
            style::apply_rules(env, node, state, style, font_registry);
        });
        // Mirror the style into Taffy so flex direction, gaps,
        // `position: absolute`, percent widths, inset top/right/
        // bottom/left etc. all participate in the layout pass.
        // Native sizing on the view's `LayoutParams` (set inside
        // `apply_rules`) is preserved — the layout pass below
        // overwrites width/height/margins with the Taffy-computed
        // frame, which itself reads the style's width/height/
        // padding/etc., so the final frame matches author intent.
        let layout_node = self.layout_for_view(node);
        self.layout.set_style(layout_node, style);
    }

    fn set_animated_f32(
        &mut self,
        node: &Self::Node,
        prop: framework_core::animation::AnimProp,
        value: f32,
    ) {
        // Android View has separate native properties for each
        // transform component (translationX/Y, scaleX/Y, rotation)
        // plus alpha — no composition needed. Each AnimProp maps
        // directly to one setter via JNI.
        use framework_core::animation::AnimProp as P;
        let (method, sig) = match prop {
            P::Opacity => ("setAlpha", "(F)V"),
            P::TranslateX => ("setTranslationX", "(F)V"),
            P::TranslateY => ("setTranslationY", "(F)V"),
            P::Scale | P::ScaleX => ("setScaleX", "(F)V"),
            P::ScaleY => ("setScaleY", "(F)V"),
            P::RotateZ => ("setRotation", "(F)V"),
            // Wrong family; silently ignored.
            P::BackgroundColor | P::ForegroundColor | P::GradientStopColor(_) => {
                return
            }
        };
        with_env(|env| {
            // `setTranslationX/Y` on Android takes DEVICE PIXELS,
            // but framework animation values come in dp (same unit
            // as Taffy frames). Convert via the view's density so
            // a translate of "100 dp" actually moves the view 100
            // dp on-screen regardless of display density. Mirrors
            // the dp→px conversion `sync_transform_translate_percent`
            // already does for static percent translates.
            let out_value = if matches!(prop, P::TranslateX | P::TranslateY) {
                backend_android_core::helpers::dp_to_px(env, node.as_obj(), value)
                    as f32
            } else {
                value
            };
            let _ = env.call_method(
                node.as_obj(),
                method,
                sig,
                &[jni::objects::JValue::Float(out_value)],
            );
            // `Scale` is uniform — also write Y.
            if matches!(prop, P::Scale) {
                let _ = env.call_method(
                    node.as_obj(),
                    "setScaleY",
                    "(F)V",
                    &[jni::objects::JValue::Float(value)],
                );
            }
        });
    }

    fn set_animated_color(
        &mut self,
        node: &Self::Node,
        prop: framework_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        use framework_core::animation::AnimProp as P;
        // Pack sRGB[r,g,b,a] (0..1 floats) into Android ARGB
        // (0xAARRGGBB) — the int Android's setBackgroundColor takes.
        let r = (value[0].clamp(0.0, 1.0) * 255.0).round() as u32;
        let g = (value[1].clamp(0.0, 1.0) * 255.0).round() as u32;
        let b = (value[2].clamp(0.0, 1.0) * 255.0).round() as u32;
        let a = (value[3].clamp(0.0, 1.0) * 255.0).round() as u32;
        let argb = ((a & 0xff) << 24) | ((r & 0xff) << 16) | ((g & 0xff) << 8) | (b & 0xff);
        let argb_i32 = argb as i32;

        match prop {
            P::BackgroundColor => {
                with_env(|env| {
                    let _ = env.call_method(
                        node.as_obj(),
                        "setBackgroundColor",
                        "(I)V",
                        &[jni::objects::JValue::Int(argb_i32)],
                    );
                });
            }
            P::ForegroundColor => {
                // Android's "foreground color" is widget-specific —
                // `TextView.setTextColor`, `ImageView.setImageTintList`,
                // etc. There is no universal View setter (`setForeground`
                // exists on API 23+ but takes a Drawable, not a color).
                // For now we attempt `setTextColor(int)` which TextView
                // and its subclasses (Button, EditText) accept; on
                // other Views the call throws which we silently swallow.
                with_env(|env| {
                    let _ = env.call_method(
                        node.as_obj(),
                        "setTextColor",
                        "(I)V",
                        &[jni::objects::JValue::Int(argb_i32)],
                    );
                });
            }
            P::GradientStopColor(idx) => {
                // Per-frame stop update on the node's
                // `GradientDrawable`. Reads/writes only this node's
                // animation state — `apply_gradient_to_drawable`
                // stashed the resolved stop colors when the style
                // was first applied.
                let key = Self::node_key(node);
                let Some(state) = self.anim_state.get_mut(&key) else {
                    return;
                };
                with_env(|env| {
                    style::set_animated_gradient_stop(env, state, idx as usize, value);
                });
            }
            P::Opacity
            | P::TranslateX
            | P::TranslateY
            | P::Scale
            | P::ScaleX
            | P::ScaleY
            | P::RotateZ => {}
        }
    }

    fn frame(&self, node: &Self::Node) -> Option<framework_core::primitives::portal::ViewportRect> {
        // Parent-relative rect in dp — matches iOS's `Backend::frame`
        // impl. Framework portal / anchoring code consults this; the
        // ViewHandle-side analog used by author code lives on
        // `AndroidViewOps::frame` (same body, different trait).
        <AndroidViewOps as framework_core::ViewOps>::frame(
            &ANDROID_VIEW_OPS,
            node as &dyn std::any::Any,
        )
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        // If this node is a ScrollView outer, drop our held inner
        // GlobalRef so the JVM can GC the inner LinearLayout once
        // the outer is released.
        primitives::scroll_view::forget_inner(self, node);
        // Free per-node animator state + the leaked state-callback
        // box when the node detaches. Drops the held `GlobalRef`s,
        // which lets the JVM GC the animator/listener objects.
        if let Some(entry) = self.anim_state.remove(&Self::node_key(node)) {
            if entry.state_callback_ptr != 0 {
                // Blank the inner closure instead of freeing the box.
                // See the type doc on `StateCallback` — a late
                // touch/focus dispatch could otherwise read a freed
                // pointer. With the inner cleared, the dispatch is a
                // harmless no-op.
                //
                // SAFETY: the pointer was produced by Box::into_raw
                // on a `Box<StateCallback>` in `attach_states`, so
                // the pointer remains valid for the program's
                // lifetime (we never free it).
                unsafe {
                    let cb = &*(entry.state_callback_ptr as *const StateCallback);
                    cb.inner.borrow_mut().take();
                }
            }
        }
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        with_env(|env| {
            let _ = env.call_method(
                node.as_obj(),
                "setEnabled",
                "(Z)V",
                &[JValue::Bool(if disabled { 0 } else { 1 })],
            );
        });
    }

    fn attach_states(
        &mut self,
        node: &Self::Node,
        setter: Rc<dyn Fn(framework_core::StateBits, bool)>,
    ) {
        // Box the setter behind a stable raw pointer the JVM can hand
        // back via JNI on event firings, mirroring the
        // RustClickListener pattern. The Kotlin side holds the
        // pointer in a small wrapper class (RustStateListener) whose
        // listener methods call back into `nativeStateEvent` with the
        // bit (PRESSED/FOCUSED) and on/off boolean.
        //
        // Note: Android has no `hovered` for touch devices. We wire
        // only PRESSED + FOCUSED — HOVERED bit never flips on mobile,
        // which is the intended cross-platform no-op.
        let boxed: Box<StateCallback> = Box::new(StateCallback {
            inner: std::cell::RefCell::new(Some(setter)),
        });
        let ptr = Box::into_raw(boxed) as jlong;

        with_env(|env| {
            let listener_class = match env.find_class("io/idealyst/runtime/RustStateListener") {
                Ok(c) => c,
                Err(_) => return,
            };
            let Ok(listener) = env.new_object(&listener_class, "(J)V", &[JValue::Long(ptr)]) else {
                return;
            };
            // Touch listener — drives PRESSED.
            let _ = env.call_method(
                node.as_obj(),
                "setOnTouchListener",
                "(Landroid/view/View$OnTouchListener;)V",
                &[JValue::Object(&listener)],
            );
            // Focus listener — drives FOCUSED.
            let _ = env.call_method(
                node.as_obj(),
                "setOnFocusChangeListener",
                "(Landroid/view/View$OnFocusChangeListener;)V",
                &[JValue::Object(&listener)],
            );
        });

        // Stash the pointer in the per-node state so we can blank it
        // on unstyle. The animation cache already keys by node;
        // reuse it.
        let key = Self::node_key(node);
        let entry = self.anim_state.entry(key).or_default();
        entry.state_callback_ptr = ptr;
    }

    fn finish(&mut self, root: Self::Node) {
        // Idempotent: in AAS mode, each reconnect / re-snapshot from
        // the dev-server replays the full command stream, which
        // includes the `Finish` that drives this method. The
        // `WireBackend` (in `dev-client`) is idempotent for tree
        // commands, so `root` here is the SAME native `UIView`-/
        // `View`-equivalent as the previous snapshot. Calling
        // `addView` on a child whose `getParent()` is non-null
        // throws `IllegalStateException`, which used to surface as
        // a JNI panic and (before the panic hook + exception clear
        // in the AAS shell) crashed the process outright.
        //
        // The fix: check the current parent. If it's already
        // `self.root`, we're done. If it's some OTHER ViewGroup
        // (shouldn't normally happen, but defensively handled),
        // detach first. Then addView, and if even that throws,
        // log + clear — never let a JNI exception escape.
        with_env(|env| {
            let host = self.root.as_obj();
            let child_node = root.as_obj();
            let child: &jni::objects::JObject = &child_node;

            let current_parent: Option<jni::objects::JObject> = env
                .call_method(child, "getParent", "()Landroid/view/ViewParent;", &[])
                .ok()
                .and_then(|v| v.l().ok());

            if let Some(parent) = current_parent {
                if !parent.is_null() {
                    if env.is_same_object(&parent, host).unwrap_or(false) {
                        // Already attached to our host_root — no-op.
                        return;
                    }
                    // Attached to some other parent. Detach so the
                    // subsequent addView won't throw. Best-effort —
                    // some ViewParent implementations don't expose
                    // `removeView(View)`; we swallow errors and
                    // clear any exception so the addView still has
                    // a clean slate.
                    let _ = env.call_method(
                        &parent,
                        "removeView",
                        "(Landroid/view/View;)V",
                        &[JValue::Object(child)],
                    );
                    if env.exception_check().unwrap_or(false) {
                        let _ = env.exception_describe();
                        let _ = env.exception_clear();
                    }
                }
            }

            if let Err(e) = env.call_method(
                host,
                "addView",
                "(Landroid/view/View;)V",
                &[JValue::Object(child)],
            ) {
                log::error!("[backend-android] finish(): addView failed: {e:?}");
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                }
            }
        });
        // The host hasn't been measured yet at `finish` time —
        // `getWidth()/getHeight()` both read back as 0. Posting via
        // the main Looper *alone* doesn't help: Handler.post just
        // schedules the runnable for the next looper turn, which is
        // typically before Android's layout pass for the host. The
        // layout machinery on the framework root + host hierarchy
        // runs on the next vsync frame.
        //
        // Schedule the layout pass with a tiny delay so it lands
        // AFTER Android's first layout cycle. If the host still
        // measures 0 we retry once more — covers the
        // resume-after-paused case where the activity is re-attached
        // and the very first frame is still 0×0.
        crate::imp::scheduler::schedule_layout_pass_retry(0);
    }
}

/// Build a placeholder TextView for an unregistered external primitive
/// — visible in dev so missing SDK bindings on Android are obvious.
/// User-space `has_external::<T>()` discovery is the supported way to
/// render custom degradation instead of relying on this fallback.
fn external_placeholder_view(b: &mut AndroidBackend, type_name: &'static str) -> GlobalRef {
    use backend_android_core::helpers::{apply_default_layout_params, set_text};
    with_env(|env| {
        let class = env.find_class("android/widget/TextView").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        set_text(
            env,
            &local,
            &format!("External \"{type_name}\" not supported on Android"),
        );
        // Red text on the system default background, matching the web
        // placeholder's intent (visible, clearly an error, not a
        // production-quality rendering).
        let _ = env.call_method(
            &local,
            "setTextColor",
            "(I)V",
            // 0xFF C0392B — same hex the web placeholder uses.
            &[JValue::Int(0xFFC0392Bu32 as i32)],
        );
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}
