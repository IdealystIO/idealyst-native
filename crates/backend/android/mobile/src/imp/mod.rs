//! Android-specific implementation of [`crate::AndroidBackend`].
//!
//! This module is the parent of every per-primitive file and shared
//! helper. The `Backend` impl at the bottom delegates each method to
//! the matching submodule.

mod a11y;
mod animation;
/// Cooperative main-thread async executor. Gated on `async-driver` since it
/// needs `runtime_core::driver` (which the feature brings in). Mirrors the
/// Apple backend's `async_executor` module gating.
#[cfg(feature = "async-driver")]
pub(crate) mod async_executor;
pub(crate) mod callbacks;
mod font;
mod jni_exports;
pub(crate) mod keyboard;
mod primitives;
pub(crate) mod scheduler;
mod screenshot;
pub(crate) mod sticky;
mod style;
// `view_screen_rect` lives here because it depends on this crate's
// `with_env` / `JAVA_VM` state (owned by `JNI_OnLoad`, which is a
// per-cdylib singleton). The rest of the JNI helpers — and the
// render loop driver — live in `backend-android-core` and are
// imported directly by their callers.
pub(crate) mod view_rect;

use crate::phase_timer;
use runtime_core::primitives::navigator::NavigatorOps;
use runtime_core::{Backend, ButtonHandle, StyleRules};

/// No-op `NavigatorOps` returned by `make_navigator_handle` when no
/// SDK handler is stored for the requested node. Keeps the fallback
/// handle inert without panicking on misuse.
struct NoopNavOps;
impl NavigatorOps for NoopNavOps {}
static NOOP_NAV_OPS: NoopNavOps = NoopNavOps;
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

/// The app's `ClassLoader`, captured off the Android `Context` on the main
/// thread in [`init_ndk_context`]. Needed because a bare `JNIEnv::find_class`
/// on a thread attached via `AttachCurrentThread` resolves against the JVM's
/// SYSTEM classloader, which only sees platform classes — never the app's
/// `io.idealyst.*` runtime classes. The async executor's `TaskWaker` fires on
/// arbitrary background threads (a future completing on a worker), so the
/// `RustAsyncPoll` it constructs there hit a `ClassNotFoundException` and
/// aborted the process. Resolving app classes through this loader's
/// `loadClass` (see [`find_app_class`]) fixes it.
static APP_CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Resolve a class by its JNI binary name (slash-separated, e.g.
/// `"io/idealyst/runtime/RustAsyncPoll"`), going through the captured app
/// [`APP_CLASS_LOADER`] when available so the lookup works from ANY thread —
/// not just the main thread / JVM-originated threads where `find_class`'s
/// system-classloader resolution happens to see app classes.
///
/// Falls back to `env.find_class` if the loader hasn't been captured yet
/// (pre-mount, or platform classes which the system loader can resolve
/// anywhere). Use this for any `io.idealyst.*` class that might be resolved
/// off a background thread; plain `find_class` is fine for `android.*` /
/// `java.*` platform classes.
pub(crate) fn find_app_class<'a>(
    env: &mut JNIEnv<'a>,
    binary_name: &str,
) -> jni::errors::Result<jni::objects::JClass<'a>> {
    let Some(loader) = APP_CLASS_LOADER.get() else {
        return env.find_class(binary_name);
    };
    // `ClassLoader.loadClass` wants the dotted binary name, not the
    // slash-separated JNI form.
    let dotted = binary_name.replace('/', ".");
    let jname = env.new_string(dotted)?;
    let class = env
        .call_method(
            loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&jname)],
        )?
        .l()?;
    Ok(jni::objects::JClass::from(class))
}

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
        // Route `runtime_core::log` / `log_info!` through the same `log`
        // facade so author-level logs reach logcat instead of vanishing
        // into Android's discarded native stderr (the `StderrLogger`
        // fallback). Idempotent.
        crate::logger::install_logger();
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

/// Publish the JVM + Android `Context` to the ecosystem-standard
/// [`ndk_context`] global, once per process. SDKs such as `camera` obtain the
/// `JavaVM` / `Context` via `ndk_context::android_context()`; since the
/// idealyst host is a Kotlin `MainActivity` + JNI bridge (not a
/// `NativeActivity`), nothing populates that global unless we do here.
///
/// The `Context` is published through a *separate, leaked* `GlobalRef` so its
/// pointer stays valid for the whole process — `ndk_context` holds it
/// indefinitely, and it must outlive any `AndroidBackend` (which releases its
/// own ref on detach). Guarded by a `Once` so re-attach (hot reload) doesn't
/// re-initialize.
fn init_ndk_context(context: &GlobalRef) {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let Some(vm) = JAVA_VM.get() else { return };
        // Capture the app's ClassLoader off the Context while we're on the
        // main thread. `find_app_class` uses it to resolve `io.idealyst.*`
        // classes from background threads (e.g. the async executor's waker),
        // where a bare `find_class` would hit the system loader and fail. See
        // [`APP_CLASS_LOADER`].
        let loader = with_env(|env| {
            let cl = env
                .call_method(
                    context.as_obj(),
                    "getClassLoader",
                    "()Ljava/lang/ClassLoader;",
                    &[],
                )
                .ok()?
                .l()
                .ok()?;
            env.new_global_ref(&cl).ok()
        });
        if let Some(loader) = loader {
            let _ = APP_CLASS_LOADER.set(loader);
        }
        let vm_ptr = vm.get_java_vm_pointer() as *mut std::ffi::c_void;
        // A dedicated, process-lifetime ref to the Context.
        let ctx_ref = with_env(|env| {
            env.new_global_ref(context.as_obj())
                .expect("ndk_context: new_global_ref(context)")
        });
        let ctx_ptr = ctx_ref.as_obj().as_raw() as *mut std::ffi::c_void;
        std::mem::forget(ctx_ref);
        // SAFETY: `vm_ptr` is the process-lifetime JavaVM captured in
        // `JNI_OnLoad`; `ctx_ptr` is a leaked global ref to the Android
        // Context, valid for the process lifetime. Called once.
        unsafe { ndk_context::initialize_android_context(vm_ptr, ctx_ptr) };
    });
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

    /// Persistent `RustBorderDrawable` set as the view's foreground
    /// when any of `border_{top,right,bottom,left}_width` is non-
    /// zero. The framework's per-side border API doesn't map onto
    /// Android's `GradientDrawable.setStroke` (uniform-only), so we
    /// paint per-side via this custom Drawable. Stashed across
    /// apply-style calls so repeated style applies mutate the same
    /// instance instead of churning the GC. `None` until the first
    /// border is requested.
    pub(crate) border_drawable: Option<GlobalRef>,
    /// Last-applied per-side border state (px widths, packed ARGB
    /// colors). Used to short-circuit the JNI call when nothing
    /// changed. Stored as an option array so a `0`-width side with a
    /// stale color doesn't trip an unnecessary update.
    pub(crate) last_border_widths: [Option<i32>; 4],
    pub(crate) last_border_colors: [Option<i32>; 4],
    /// Running ValueAnimator driving per-side border interpolation.
    /// Cancelled + restarted on each transitionable border change.
    pub(crate) anim_border: Option<GlobalRef>,

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
    pub(crate) gradient_radial_extent: Option<runtime_core::RadialExtent>,
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
    pub(crate) layout: runtime_layout::LayoutTree,
    /// View pointer → (`GlobalRef`, Taffy node). Indexed by the same
    /// raw `JObject*` pointer scheme as `anim_state`. Iterated in the
    /// layout pass to apply computed frames.
    pub(crate) view_to_layout:
        HashMap<usize, (GlobalRef, runtime_layout::LayoutNode)>,
    /// Registry of third-party `Element::External` handlers,
    /// populated by `register_external::<T>(...)` calls from
    /// per-platform leaf crates (e.g. `webview-android::register`).
    /// `create_external` looks the handler up by payload TypeId;
    /// unregistered kinds fall through to a "not supported" placeholder
    /// TextView.
    pub(crate) external_handlers:
        runtime_core::ExternalRegistry<AndroidBackend>,
    /// Registry of `Element::Navigator` handler factories.
    /// SDK leaf crates install factories keyed by their presentation
    /// TypeId via `register_navigator`.
    pub(crate) navigator_handlers:
        runtime_core::NavigatorRegistry<AndroidBackend>,
    /// Per-navigator-instance SDK handler. Keyed by the node's
    /// `node_key_of` (JObject raw pointer). `Backend::create_navigator`
    /// stores the handler here after `init` so the unified
    /// `navigator_attach_initial` / `release_navigator` /
    /// `make_navigator_handle` / `apply_navigator_slot_style` trait
    /// methods can route through the handler's kind-specific logic
    /// instead of branching on a kind discriminant + calling per-kind
    /// inherent helpers directly.
    pub(crate) nav_handler_instances: HashMap<
        usize,
        std::rc::Rc<
            std::cell::RefCell<Box<dyn runtime_core::NavigatorHandler<AndroidBackend>>>,
        >,
    >,
    /// Per-`Typeface` registry of custom fonts. Filled by
    /// [`Backend::register_asset`] for `AssetTag::Font`
    /// (bytes → Android `Typeface.createFromFile`) and
    /// [`Backend::register_typeface`] (records the (weight, style) →
    /// Typeface map per family). Consulted by the style applier to
    /// drive `TextView.setTypeface`.
    pub(crate) font_registry: font::FontRegistry,
    /// `Position::Sticky` bookkeeping. Keyed by the enclosing
    /// `ScrollView`/`HorizontalScrollView`'s JObject pointer; the
    /// entry holds a Kotlin `RustStickyScrollListener` that
    /// dispatches per-scroll-event recompute back into
    /// [`sticky::on_scroll_event`]. See [`sticky`] for the rationale
    /// (side registry over ScrollView subclass).
    pub(crate) sticky_registry: sticky::StickyRegistry,
    /// User-supplied `on_scroll` callbacks for `Element::ScrollView`.
    /// Keyed by the scroll view's JObject pointer. Lives parallel to
    /// `sticky_registry` so both subsystems can ride the single
    /// `setOnScrollChangeListener` slot Android allows per view \u{2014}
    /// the JNI dispatch fans out to both registries on every scroll
    /// event.
    pub(crate) scroll_observers: std::collections::HashMap<usize, Rc<dyn Fn(f32, f32)>>,
    /// Centralized "Kotlin `RustStickyScrollListener` attached to
    /// this scroll view" map, refcounted across the sticky subsystem
    /// and `on_scroll`. Both call into [`sticky::ensure_scroll_listener`]
    /// to install once; the listener is detached only when both
    /// subsystems release.
    pub(crate) scroll_listeners: std::collections::HashMap<usize, jni::objects::GlobalRef>,
    /// Sticky views whose `apply_style` ran BEFORE their first
    /// `insert`, so the parent walk couldn't yet find an enclosing
    /// scroll view. The walker calls `apply_style` (via
    /// `attach_style`) inside the per-primitive `build`, then the
    /// parent's `insert_children` does `backend.insert(...)`
    /// afterwards — so at apply-style time the child is still a
    /// detached floating view. We stash `(view_ptr, threshold)`
    /// here and complete the registration in `insert` once the
    /// view is actually in a parent chain. Mirrors iOS's
    /// `pending_sticky`.
    pub(crate) pending_sticky: HashMap<usize, f32>,
    /// Portal overlays awaiting their first laid-out frame. Each is the
    /// overlay `FrameLayout`'s `GlobalRef`, set `View.INVISIBLE` at
    /// `create_overlay_portal` time and flipped back to `View.VISIBLE`
    /// at the END of the next `run_layout_pass`. Prevents the bug where
    /// a portal overlay paints one unlaid-out frame at the 0,0 origin on
    /// mount (children not yet centered) — the overlay literally cannot
    /// paint until it's been laid out, so the modal is centered from its
    /// first VISIBLE frame regardless of AV/Choreographer timing.
    pub(crate) pending_reveal: Vec<jni::objects::GlobalRef>,
    /// Content-view pointers of "detached window roots" — views that
    /// live in their OWN `WindowManager` window (the `screen_recorder`
    /// private layer's PixelCopy-excluded overlay) rather than in the
    /// Activity `root`. Two consult sites:
    ///
    /// - `view::insert` SKIPS the `parent.addView` reparent for these,
    ///   exactly like a portal content holder. The External walker's
    ///   `insert(parent, external_node)` would otherwise try to splice
    ///   the window-root into the main (captured) tree — Android would
    ///   throw `IllegalStateException("child already has a parent")`
    ///   because it's already added via `WindowManager.addView`, and
    ///   even if it didn't, reparenting into the captured window would
    ///   defeat exclusion.
    /// - `run_layout_pass` SKIPS these roots when building the frame
    ///   batch: the top-level view of a `WindowManager` window is
    ///   positioned by its `WindowManager.LayoutParams` (full-screen),
    ///   not by a `MarginLayoutParams` — wrapping it would break the
    ///   window. Its CHILDREN still get Taffy frames normally.
    ///
    /// The entry holds the `WindowManager.LayoutParams` `GlobalRef`
    /// used at `addView` time; `removeView` in
    /// `release_private_layer_window` tears the window down.
    pub(crate) detached_window_roots:
        HashMap<usize, jni::objects::GlobalRef>,
    /// Leaked `KeyDownCallback` pointer for the app-level key handler installed
    /// by `set_app_key_handler` (a `RustGlobalKeyListener` on the root view holds
    /// the same value and trampolines into `nativeGlobalKey`). `Some` while a
    /// handler is installed; freed + detached when replaced or cleared.
    pub(crate) app_key_ptr: Option<jlong>,
}

/// Read the device's `density` (screen-pixels-per-dp) from the
/// host view's resources. `1.0` on the unlikely happy-path where
/// the call fails (preserves the dp-as-pixel fallback in the rest
/// of the style path).
pub(crate) fn density_of(env: &mut JNIEnv, view: &JObject) -> Option<f32> {
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

thread_local! {
    /// Cached `android.widget.TextView` class. Used by `is_text_view`
    /// (apply_style's padding + text-styling branches), which fires
    /// once per styled view per apply pass — easily the busiest
    /// classloader probe in the backend.
    static TEXTVIEW_CLASS: std::cell::RefCell<Option<GlobalRef>> =
        std::cell::RefCell::new(None);

    /// Cached `android.widget.EditText` class. Used by `is_edit_text` to
    /// distinguish editable inputs (which keep their native default text
    /// color) from display text views (which default to the theme color).
    static EDITTEXT_CLASS: std::cell::RefCell<Option<GlobalRef>> =
        std::cell::RefCell::new(None);

    /// Cached `android.view.View` class. Used as the element type when
    /// building the `View[]` handed to the batch frame applier.
    static VIEW_CLASS: std::cell::RefCell<Option<GlobalRef>> =
        std::cell::RefCell::new(None);

    /// Cached `io.idealyst.runtime.RustLayoutApply` class — the JVM-side
    /// batch frame applier. `find_class` is a slow classloader probe; the
    /// layout pass calls its static `applyFrames` every pass (60 Hz under
    /// animation), so resolve the class once and reuse.
    static LAYOUT_APPLY_CLASS: std::cell::RefCell<Option<GlobalRef>> =
        std::cell::RefCell::new(None);

    /// Display density (dp → px scaling), constant for the lifetime of
    /// the Activity. Cached after the first successful read; reused on
    /// every dp-to-px conversion in the per-view layout path. The 5
    /// JNI calls behind a fresh density read add up fast on big trees.
    static CACHED_DENSITY: std::cell::Cell<Option<f32>> =
        std::cell::Cell::new(None);

    /// Last viewport size (dp) mirrored into the reactive
    /// `runtime_core::viewport_size()` signal. `viewport_size()` is
    /// called every layout pass to feed Taffy; we only want to *notify*
    /// reactive subscribers when the size actually changes, and we must
    /// do it OUTSIDE the layout pass (see `viewport_size` for why), so
    /// this dedups repeated same-size samples and coalesces the deferred
    /// mirror to one microtask per real change.
    static LAST_MIRRORED_VIEWPORT: std::cell::Cell<Option<(f32, f32)>> =
        std::cell::Cell::new(None);
}

/// Get the display density, computing on the first call and reusing
/// the cached value afterwards. Density is per-display and we don't
/// support display changes mid-mount, so the cache lifetime equals
/// the Activity lifetime — see `CACHED_DENSITY` for the thread-local
/// hand-off. Falls back to 1.0 (caller-side default) on JNI failure;
/// the layout writes are best-effort.
fn cached_density(env: &mut JNIEnv, view: &JObject) -> f32 {
    if let Some(d) = CACHED_DENSITY.with(|c| c.get()) {
        return d;
    }
    let d = density_of(env, view).unwrap_or(1.0);
    CACHED_DENSITY.with(|c| c.set(Some(d)));
    d
}

/// Install a generic `View.measure(...)`-based Taffy measure_fn on
/// the given view's layout node. Used by `create_external` so author-
/// supplied native views (HorizontalScrollView, CardView, …) report a
/// sensible preferred size to the flex layout instead of collapsing
/// to 0×0.
///
/// The measure function:
///   - Maps Taffy's `AvailableSpace` to Android `MeasureSpec` modes:
///     `Definite(w)` → `AT_MOST | w_px`, `MaxContent` → UNSPECIFIED
///     (TextView's natural max width), `MinContent` → `EXACTLY 0`.
///   - Calls `view.measure(width_spec, height_spec)`.
///   - Reads `getMeasuredWidth/Height` and converts back to dp.
///
/// Container views (FrameLayout, the HorizontalScrollView used by
/// RustCodeBlock) measure their children internally during this call,
/// so the reported size accounts for the whole subtree.
fn install_external_measure_fn(b: &mut AndroidBackend, view: &GlobalRef) {
    let layout = b.layout_for_view(view);
    let view_for_measure = view.clone();
    b.layout.set_measure_fn(
        layout,
        std::rc::Rc::new(move |known_dimensions, available_space| {
            measure_external_view(&view_for_measure, known_dimensions, available_space)
        }),
    );
}

/// Implementation of [`install_external_measure_fn`]'s measure
/// callback. Split out for clarity; the closure body is just a
/// shim that hands its args here.
fn measure_external_view(
    view: &GlobalRef,
    known_dimensions: runtime_layout::Size<Option<f32>>,
    available_space: runtime_layout::Size<runtime_layout::AvailableSpace>,
) -> runtime_layout::Size<f32> {
    use runtime_layout::AvailableSpace;
    with_env(|env| {
        let view_obj = view.as_obj();
        let density = cached_density(env, &view_obj);
        // MeasureSpec mode bits (Android API constants):
        const UNSPECIFIED: i32 = 0; // 0 << 30
        const EXACTLY: i32 = 0x4000_0000; // 1 << 30
        const AT_MOST: i32 = -0x8000_0000; // 2 << 30, i32 high bit
        let make_spec = |known: Option<f32>, avail: AvailableSpace| -> i32 {
            // Only honor a positive known dimension as EXACTLY — a
            // known=Some(0.0) is Taffy's MinContent intrinsic-size
            // probe, NOT "you must be 0 tall", and Android's
            // MeasureSpec.EXACTLY clamps the view to literally 0
            // pixels (the codeblock vanishes). For zero / min-content
            // / non-finite cases, fall through to UNSPECIFIED so the
            // view returns its preferred size — same convention the
            // text primitive's measure_fn uses.
            if let Some(dp) = known {
                if dp > 0.0 {
                    let px = (dp * density).round() as i32;
                    return EXACTLY | (px & 0x3fff_ffff);
                }
            }
            match avail {
                AvailableSpace::Definite(dp) if dp > 0.0 => {
                    let px = (dp * density).round() as i32;
                    AT_MOST | (px & 0x3fff_ffff)
                }
                _ => UNSPECIFIED,
            }
        };
        let w_spec = make_spec(known_dimensions.width, available_space.width);
        let h_spec = make_spec(known_dimensions.height, available_space.height);
        let _ = env.call_method(
            &view_obj,
            "measure",
            "(II)V",
            &[JValue::Int(w_spec), JValue::Int(h_spec)],
        );
        let w_px = env
            .call_method(&view_obj, "getMeasuredWidth", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0);
        let h_px = env
            .call_method(&view_obj, "getMeasuredHeight", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0);
        runtime_layout::Size {
            width: w_px as f32 / density,
            height: h_px as f32 / density,
        }
    })
}

/// `true` when `view` is a `TextView` (or any subclass like
/// `EditText`, `IdealystLabel`). Used by `apply_rules` to gate the
/// `setPadding` / text-styling branches without hitting `find_class`
/// per call.
pub(crate) fn is_text_view(env: &mut JNIEnv, view: &JObject) -> bool {
    TEXTVIEW_CLASS.with(|slot| {
        let mut borrow = slot.borrow_mut();
        if borrow.is_none() {
            if let Ok(class) = env.find_class("android/widget/TextView") {
                if let Ok(g) = env.new_global_ref(&class) {
                    *borrow = Some(g);
                }
            }
        }
        let Some(g) = borrow.as_ref() else {
            return false;
        };
        let class_obj = g.as_obj();
        let class: &jni::objects::JClass = unsafe {
            std::mem::transmute::<&JObject, &jni::objects::JClass>(class_obj)
        };
        env.is_instance_of(view, class).unwrap_or(false)
    })
}

/// `true` when `view` is an `EditText` (or subclass). Used by
/// `apply_rules` to keep editable inputs on their native default text
/// color — only non-editable display text (`text()`, button titles) gets
/// the theme-color default for an unstyled `color`. `EditText` extends
/// `TextView`, so `is_text_view` alone can't tell them apart.
pub(crate) fn is_edit_text(env: &mut JNIEnv, view: &JObject) -> bool {
    EDITTEXT_CLASS.with(|slot| {
        let mut borrow = slot.borrow_mut();
        if borrow.is_none() {
            if let Ok(class) = env.find_class("android/widget/EditText") {
                if let Ok(g) = env.new_global_ref(&class) {
                    *borrow = Some(g);
                }
            }
        }
        let Some(g) = borrow.as_ref() else {
            return false;
        };
        let class_obj = g.as_obj();
        let class: &jni::objects::JClass = unsafe {
            std::mem::transmute::<&JObject, &jni::objects::JClass>(class_obj)
        };
        env.is_instance_of(view, class).unwrap_or(false)
    })
}

/// Get the `android.view.View` class as a pinned `GlobalRef`, looked up
/// once (mirrors the other lazily-cached class refs). Used as the
/// element type when allocating the `View[]` handed to
/// [`apply_frames_batch`].
fn cached_view_class(env: &mut JNIEnv) -> Option<GlobalRef> {
    VIEW_CLASS.with(|slot| {
        let mut borrow = slot.borrow_mut();
        if borrow.is_none() {
            let class = env.find_class("android/view/View").ok()?;
            *borrow = Some(env.new_global_ref(&class).ok()?);
        }
        borrow.clone()
    })
}

/// Get the `io.idealyst.runtime.RustLayoutApply` class as a pinned
/// `GlobalRef`, looked up once. `find_class` is a slow classloader probe
/// and the layout pass calls the class's static `applyFrames` every
/// pass, so cache the class to skip the probe each time.
fn cached_layout_apply_class(env: &mut JNIEnv) -> Option<GlobalRef> {
    LAYOUT_APPLY_CLASS.with(|slot| {
        let mut borrow = slot.borrow_mut();
        if borrow.is_none() {
            let class = env
                .find_class("io/idealyst/runtime/RustLayoutApply")
                .ok()?;
            *borrow = Some(env.new_global_ref(&class).ok()?);
        }
        borrow.clone()
    })
}

/// Apply every Taffy-computed frame in ONE Rust→JVM call.
///
/// `views[i]` pairs with the px quad `packed[i*4 .. i*4+4]` =
/// `[leftMargin, topMargin, width, height]`. We hand the parallel arrays
/// to `RustLayoutApply.applyFrames`, which performs all the
/// `MarginLayoutParams` writes JVM-side (cheap virtual calls, no JNI
/// transition). This replaces the old per-view `apply_frame_to_layout_params`
/// that paid ~9 JNI crossings per view — for a 200-view tree, ~1800
/// crossings a pass collapse to the array marshalling plus one call.
///
/// The drawer-panel skip and the "wrap non-`MarginLayoutParams`" fallback
/// moved into the Kotlin loop verbatim; the caller pre-filters zero-size
/// views and detached window roots. Best-effort: a JNI failure here drops
/// the whole pass's frame write, which the next pass retries.
///
/// NOTE: building the `View[]` still costs one `set_object_array_element`
/// per view. A future refinement could maintain a persistent JVM-side
/// `View` registry updated at insert/remove time so a pass passes only
/// the int quads — getting total crossings per pass down to ~2.
fn apply_frames_batch(env: &mut JNIEnv, views: &[&GlobalRef], packed: &[i32]) {
    debug_assert_eq!(views.len() * 4, packed.len());
    if views.is_empty() {
        return;
    }
    let Some(view_class) = cached_view_class(env) else {
        return;
    };
    let Some(apply_class) = cached_layout_apply_class(env) else {
        return;
    };
    let view_arr = match env.new_object_array(views.len() as i32, &view_class, JObject::null()) {
        Ok(a) => a,
        Err(_) => return,
    };
    for (i, v) in views.iter().enumerate() {
        if env
            .set_object_array_element(&view_arr, i as i32, v.as_obj())
            .is_err()
        {
            return;
        }
    }
    // One bulk copy for all frame data (`set_int_array_region` is a
    // single region copy, not per-element).
    let int_arr = match env.new_int_array(packed.len() as i32) {
        Ok(a) => a,
        Err(_) => return,
    };
    if env.set_int_array_region(&int_arr, 0, packed).is_err() {
        return;
    }
    let _ = env.call_static_method(
        &apply_class,
        "applyFrames",
        "([Landroid/view/View;[I)V",
        &[JValue::Object(&view_arr), JValue::Object(&int_arr)],
    );
}

/// Build `Intent(ACTION_VIEW, Uri.parse(url))` and hand it to
/// `context.startActivity(...)`, opening `url` in the system handler
/// (browser, mail app, dialer). Split out so the `url_opener` closure
/// can use `?` and report a single Result.
fn start_view_intent(
    env: &mut JNIEnv,
    context: &GlobalRef,
    url: &str,
) -> jni::errors::Result<()> {
    // Uri.parse(url)
    let j_url = env.new_string(url)?;
    let uri = env
        .call_static_method(
            "android/net/Uri",
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&j_url)],
        )?
        .l()?;

    // new Intent(Intent.ACTION_VIEW, uri)
    let action = env.new_string("android.intent.action.VIEW")?;
    let intent_class = env.find_class("android/content/Intent")?;
    let intent = env.new_object(
        &intent_class,
        "(Ljava/lang/String;Landroid/net/Uri;)V",
        &[JValue::Object(&action), JValue::Object(&uri)],
    )?;

    // FLAG_ACTIVITY_NEW_TASK — required when `context` isn't an
    // Activity (e.g. the Application context), or startActivity throws.
    const FLAG_ACTIVITY_NEW_TASK: jint = 0x1000_0000;
    env.call_method(
        &intent,
        "addFlags",
        "(I)Landroid/content/Intent;",
        &[JValue::Int(FLAG_ACTIVITY_NEW_TASK)],
    )?;

    // context.startActivity(intent)
    env.call_method(
        context,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[JValue::Object(&intent)],
    )?;
    Ok(())
}

/// An inventory-collected external registrar. An SDK's Android module
/// `inventory::submit!`s one of these (carrying a `fn(&mut AndroidBackend)`);
/// [`AndroidBackend::new`] drains them so the SDK self-registers its
/// `Element::External` handler without the app naming the concrete backend.
/// See [[project_inventory_self_registration]].
pub struct AndroidExternalRegistrar(pub fn(&mut AndroidBackend));
inventory::collect!(AndroidExternalRegistrar);

/// Navigator analogue of [`AndroidExternalRegistrar`]; a navigator SDK's Android
/// module submits one so the app needn't call `<nav>::register` per platform.
/// See [[project_inventory_self_registration]].
pub struct AndroidNavigatorRegistrar(pub fn(&mut AndroidBackend));
inventory::collect!(AndroidNavigatorRegistrar);

impl AndroidBackend {
    /// Install every SDK-submitted external + navigator handler. Native
    /// (non-wasm) so inventory's link-time ctors populate the slices before
    /// construction.
    fn drain_self_registrars(&mut self) {
        for r in inventory::iter::<AndroidExternalRegistrar> {
            (r.0)(self);
        }
        for r in inventory::iter::<AndroidNavigatorRegistrar> {
            (r.0)(self);
        }
    }

    /// Construct a backend rooted at the provided Android `Context`
    /// and a parent `ViewGroup` to mount under.
    pub fn new(context: GlobalRef, root: GlobalRef) -> Self {
        // Publish the JVM + Android Context to the ecosystem-standard
        // `ndk_context` global so SDKs that obtain the VM/Context that way
        // (e.g. `camera`'s `ndk_context::android_context()`) work. We don't use
        // NativeActivity, so nothing else populates it. Without this, the first
        // SDK call panics "android context was not initialized" → abort.
        init_ndk_context(&context);
        let mut backend = Self {
            context,
            root,
            anim_state: HashMap::new(),
            scroll_view_inner: HashMap::new(),
            portal_instances: HashMap::new(),
            layout: runtime_layout::LayoutTree::new(),
            view_to_layout: HashMap::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
            navigator_handlers: runtime_core::NavigatorRegistry::new(),
            nav_handler_instances: HashMap::new(),
            font_registry: font::FontRegistry::new(),
            sticky_registry: HashMap::new(),
            scroll_observers: HashMap::new(),
            scroll_listeners: HashMap::new(),
            pending_sticky: HashMap::new(),
            pending_reveal: Vec::new(),
            detached_window_roots: HashMap::new(),
            app_key_ptr: None,
        };
        backend.drain_self_registrars();
        backend.install_viewport_resize_listener();
        backend
    }

    /// Attach a one-time `OnLayoutChangeListener` to the host root so ANY
    /// size change — orientation flip, split-screen, AND the soft keyboard
    /// (IME) opening/closing — schedules a layout pass. The layout pass
    /// re-reads `getHeight()` and re-mirrors the reactive viewport (see
    /// [`Self::viewport_size`]).
    ///
    /// Without this hook the viewport only re-mirrors when something *else*
    /// happens to drive a layout pass. The keyboard OPEN is covered
    /// incidentally (focus + IME animation keep frames running), but the
    /// CLOSE is a quiet moment with no frames in flight, so the restored
    /// height is never re-read and the layout stays stuck shrunk. The
    /// listener makes open and close symmetric.
    ///
    /// Best-effort: if the staged Kotlin runtime lacks the class (e.g. the
    /// CLI wasn't reinstalled after this feature landed), `find_class`
    /// throws — we clear the pending JNI exception and no-op so boot never
    /// breaks, mirroring [`keyboard::set_app_key_handler`]. The view holds a
    /// strong ref to the listener, so there's nothing to retain or free on
    /// the Rust side.
    fn install_viewport_resize_listener(&self) {
        let root = self.root.clone();
        with_env(|env| {
            let class = match env.find_class("io/idealyst/runtime/RustViewportResizeListener") {
                Ok(c) => c,
                Err(_) => {
                    let _ = env.exception_clear();
                    return;
                }
            };
            let listener = match env.new_object(&class, "()V", &[]) {
                Ok(o) => o,
                Err(_) => {
                    let _ = env.exception_clear();
                    return;
                }
            };
            let _ = env.call_method(
                &root,
                "addOnLayoutChangeListener",
                "(Landroid/view/View$OnLayoutChangeListener;)V",
                &[JValue::Object(&listener)],
            );
            // Clear any exception a call left pending so it can't surface on
            // an unrelated later JNI call.
            let _ = env.exception_clear();
        });
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

    /// Register a navigator-kind handler factory. Mirrors `register_external`
    /// but for `Element::Navigator`. SDK leaf crates
    /// (`stack_navigator::register`, etc.) call this once at app bootstrap.
    pub fn register_navigator<P, F>(&mut self, factory: F)
    where
        P: 'static,
        F: Fn() -> Box<dyn runtime_core::NavigatorHandler<AndroidBackend>> + 'static,
    {
        self.navigator_handlers.register::<P, _>(factory);
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

    /// Create a capture-excluded overlay surface for the
    /// `screen_recorder` private layer and return its content view's
    /// `GlobalRef`. The `screen_recorder` SDK calls this from its
    /// `PrivateLayer` external handler; the walker then parents the
    /// layer's children into the returned content view and tries to
    /// `insert` it into the surrounding tree — which `view::insert`
    /// skips because we register the content view as a detached window
    /// root.
    ///
    /// ## Why a separate `WindowManager` window excludes it from capture
    ///
    /// MediaProjection capture on this SDK uses `PixelCopy` against the
    /// **app's main window** (`activity.getWindow()` decor view). A view
    /// added through `WindowManager.addView` lives in its OWN window,
    /// outside that decor view, so PixelCopy never copies it — the
    /// overlay is visible to the user but absent from the recording.
    /// The orchestrator verifies this on the emulator.
    ///
    /// ## Touch passthrough
    ///
    /// The window's `LayoutParams` carry
    /// `FLAG_NOT_FOCUSABLE | FLAG_NOT_TOUCH_MODAL | FLAG_LAYOUT_NO_LIMITS`
    /// with a `TRANSLUCENT` pixel format. `NOT_TOUCH_MODAL` lets touches
    /// outside the window's own (full-screen) bounds reach the app
    /// beneath — but a full-screen window with `NOT_TOUCH_MODAL` alone
    /// would still swallow touches INSIDE its bounds. So the content
    /// view is a `FrameLayout` that is left non-clickable: a child that
    /// doesn't consume a touch returns `false` from
    /// `dispatchTouchEvent`, and combined with `NOT_FOCUSABLE` the touch
    /// falls through to the app window. Touches that DO land on an
    /// interactive private-layer child are consumed by that child.
    /// (`project_android_nonmodal_overlay_passthrough`: a non-modal
    /// overlay must not steal touches it has no interactive child for,
    /// or the app's hamburger/buttons go dead.)
    ///
    /// ## Layout
    ///
    /// The content view is registered in `view_to_layout`, making it a
    /// Taffy ROOT sized to the viewport by `run_layout_pass`. The
    /// window itself is full-screen (`MATCH_PARENT`), so the content
    /// view fills it; the layer's controls position inside via normal
    /// flex/absolute style. The root view itself is excluded from the
    /// frame batch in `run_layout_pass` (it's positioned by the window's
    /// `WindowManager.LayoutParams`, not margin params).
    pub fn create_private_layer_window(&mut self) -> GlobalRef {
        // Content holder: a FrameLayout (like the portal overlay) so children
        // are placed by Taffy frames on FrameLayout.LayoutParams.
        let content = with_env(|env| {
            // `RustOverlayPassthrough` is a `FrameLayout` subclass: the overlay
            // is a full-screen window (so screen capture can exclude it), but a
            // full-screen window would swallow ALL app touches. This view
            // forwards any touch that misses an interactive child to the MAIN
            // activity window's decor view — both windows are in the same
            // process, so `dispatchTouchEvent` reaches the app's real view tree
            // (the drawable canvas behind). The overlay's own controls (a
            // toolbar) still get their taps. This is the Android analog of
            // iOS's `OverlayPassthroughView` hit-test passthrough.
            let cls = env
                .find_class("io/idealyst/runtime/RustOverlayPassthrough")
                .expect("RustOverlayPassthrough class missing — bundle the kotlin runtime");
            let content = env
                .new_object(
                    &cls,
                    "(Landroid/content/Context;)V",
                    &[JValue::Object(&self.context.as_obj())],
                )
                .unwrap();
            // Point it at the main window's decor view to forward through to.
            if let Some(decor) = env
                .call_method(
                    &self.context.as_obj(),
                    "getWindow",
                    "()Landroid/view/Window;",
                    &[],
                )
                .ok()
                .and_then(|v| v.l().ok())
                .filter(|w| !w.is_null())
                .and_then(|w| {
                    env.call_method(&w, "getDecorView", "()Landroid/view/View;", &[])
                        .ok()
                        .and_then(|v| v.l().ok())
                })
            {
                let _ = env.call_method(
                    &content,
                    "setBehind",
                    "(Landroid/view/View;)V",
                    &[JValue::Object(&decor)],
                );
            }
            env.new_global_ref(content).unwrap()
        });

        // Register as a Taffy root + detached window root BEFORE attaching the
        // OS window. Children insert + lay out against this root immediately;
        // only the `WindowManager.addView` is deferred — see
        // `try_attach_private_layer_window` for why (the activity window token
        // is null during the initial mount). The map value is the content
        // GlobalRef so `release_private_layer_window` can `removeView` it.
        self.layout_for_view(&content);
        let key = Self::node_key_of(&content);
        self.detached_window_roots.insert(key, content.clone());

        self.try_attach_private_layer_window(&content, 0);

        content
    }

    /// Attach the private-layer content view to its own `WindowManager`
    /// window. A `TYPE_APPLICATION_PANEL` window needs the host activity's
    /// window token, which is **null until the activity's window is attached
    /// to the WindowManager** — a frame or two after `onCreate`. Since this
    /// handler runs during the initial mount (inside `attach` → `mount`),
    /// `addView` would throw `BadTokenException` and the overlay would never
    /// show (the bug this fixes). So if the token isn't live yet — or
    /// `addView` throws — we re-post on the main looper with a short backoff
    /// and try again, up to a bounded number of attempts.
    fn try_attach_private_layer_window(&mut self, content: &GlobalRef, retry: u32) {
        const MAX_RETRIES: u32 = 30;

        let attached = with_env(|env| -> bool {
            // WindowManager from the Activity context (Context.WINDOW_SERVICE).
            let svc_name = env.new_string("window").unwrap();
            let wm = match env
                .call_method(
                    &self.context.as_obj(),
                    "getSystemService",
                    "(Ljava/lang/String;)Ljava/lang/Object;",
                    &[JValue::Object(&svc_name)],
                )
                .ok()
                .and_then(|v| v.l().ok())
            {
                Some(wm) if !wm.is_null() => wm,
                _ => return false,
            };

            // The activity's window token via getWindow().getDecorView()
            // .getWindowToken(). Null until the window is attached — bail and
            // retry. A non-null token is the signal the window is live.
            let token = env
                .call_method(
                    &self.context.as_obj(),
                    "getWindow",
                    "()Landroid/view/Window;",
                    &[],
                )
                .ok()
                .and_then(|v| v.l().ok())
                .filter(|w| !w.is_null())
                .and_then(|window| {
                    env.call_method(&window, "getDecorView", "()Landroid/view/View;", &[])
                        .ok()
                        .and_then(|v| v.l().ok())
                })
                .filter(|d| !d.is_null())
                .and_then(|decor| {
                    env.call_method(&decor, "getWindowToken", "()Landroid/os/IBinder;", &[])
                        .ok()
                        .and_then(|v| v.l().ok())
                });
            let token = match token {
                Some(t) if !t.is_null() => t,
                _ => return false, // window not attached yet — retry
            };

            // WindowManager.LayoutParams — full-screen, passthrough,
            // translucent. TYPE_APPLICATION_PANEL (1000) is a child window of
            // the app's own window (uses the activity token, so no
            // SYSTEM_ALERT_WINDOW permission).
            const MATCH_PARENT: i32 = -1;
            const TYPE_APPLICATION_PANEL: i32 = 1000;
            const FLAG_NOT_FOCUSABLE: i32 = 0x0000_0008;
            // The window is TOUCHABLE (no FLAG_NOT_TOUCHABLE): its content view
            // is a `RustOverlayPassthrough` that handles taps on the overlay's
            // controls and FORWARDS every other touch to the main window's
            // decor view (same process), so the app behind stays interactive.
            // A full-screen NOT_TOUCHABLE window would make the overlay's own
            // controls dead; FLAG_NOT_TOUCH_MODAL alone can't help a
            // full-screen window (it only passes through touches OUTSIDE the
            // bounds). Forwarding is the Android analog of iOS's hit-test
            // passthrough.
            const FLAG_NOT_TOUCH_MODAL: i32 = 0x0000_0020;
            const FLAG_LAYOUT_NO_LIMITS: i32 = 0x0000_0200;
            const TRANSLUCENT: i32 = -3;
            let flags = FLAG_NOT_FOCUSABLE | FLAG_NOT_TOUCH_MODAL | FLAG_LAYOUT_NO_LIMITS;

            let lp_class = env
                .find_class("android/view/WindowManager$LayoutParams")
                .unwrap();
            // (int w, int h, int type, int flags, int format)
            let lp = env
                .new_object(
                    &lp_class,
                    "(IIIII)V",
                    &[
                        JValue::Int(MATCH_PARENT),
                        JValue::Int(MATCH_PARENT),
                        JValue::Int(TYPE_APPLICATION_PANEL),
                        JValue::Int(flags),
                        JValue::Int(TRANSLUCENT),
                    ],
                )
                .unwrap();
            let _ = env.set_field(&lp, "token", "Landroid/os/IBinder;", JValue::Object(&token));

            // addView(content, lp) — creates the separate window.
            let _ = env.call_method(
                &wm,
                "addView",
                "(Landroid/view/View;Landroid/view/ViewGroup$LayoutParams;)V",
                &[JValue::Object(content.as_obj()), JValue::Object(&lp)],
            );
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
                return false; // BadToken (token went stale mid-attach) — retry
            }
            true
        });

        if attached {
            // Window is up — compute the detached root against the viewport so
            // the layer's children position (its frame is skipped in the pass,
            // but its children get frames).
            crate::imp::scheduler::schedule_layout_pass_retry(0);
            return;
        }

        if retry >= MAX_RETRIES {
            log::error!(
                "[backend-android] private layer: window token never became valid \
                 after {MAX_RETRIES} retries — overlay not shown"
            );
            return;
        }

        // Re-try on the main looper after one frame. The closure can't capture
        // `&mut self`, so it re-reaches the backend via the global self-handle.
        let content = content.clone();
        crate::imp::scheduler::post_runnable(
            16,
            Box::new(move || {
                if let Some(rc) = crate::imp::backend_self_weak().and_then(|w| w.upgrade()) {
                    if let Ok(mut b) = rc.try_borrow_mut() {
                        b.try_attach_private_layer_window(&content, retry + 1);
                    }
                }
            }),
        );
    }

    /// Tear down the private-layer overlay window created by
    /// [`Self::create_private_layer_window`] — `WindowManager.removeView`
    /// + drop the Taffy node / view-table entry (mirrors the portal
    /// `release` path).
    pub fn release_private_layer_window(&mut self, node: &GlobalRef) {
        let key = Self::node_key_of(node);
        if self.detached_window_roots.remove(&key).is_none() {
            return;
        }
        with_env(|env| {
            let svc_name = env.new_string("window").unwrap();
            if let Ok(wm) = env
                .call_method(
                    &self.context.as_obj(),
                    "getSystemService",
                    "(Ljava/lang/String;)Ljava/lang/Object;",
                    &[JValue::Object(&svc_name)],
                )
                .and_then(|v| v.l())
            {
                let _ = env.call_method(
                    &wm,
                    "removeView",
                    "(Landroid/view/View;)V",
                    &[JValue::Object(&node.as_obj())],
                );
                // removeView throws if the view was never added (token
                // failure above); swallow so teardown stays best-effort.
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_clear();
                }
            }
        });
        let layout_node = self.layout_for_view(node);
        self.layout.remove_node(layout_node);
        self.view_to_layout.remove(&key);
    }

    /// Get or create a Taffy layout node for the given view. Called
    /// from every `create_*` so each backend-created view has a
    /// corresponding node in the layout tree.
    pub(crate) fn layout_for_view(
        &mut self,
        view: &GlobalRef,
    ) -> runtime_layout::LayoutNode {
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
        let (w, h) = with_env(|env| {
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
        });
        // Mirror into the framework's reactive viewport signal so
        // `viewport_size()` subscribers (breakpoint hooks, responsive
        // containers, a page-level drawer menu button) re-fire on size
        // changes. Skip pushing when both dims are zero — pre-layout
        // reads shouldn't overwrite a previously-valid value.
        //
        // CRITICAL: this method is called from `run_layout_pass` to feed
        // Taffy, so we are INSIDE a layout pass here. Calling
        // `set_viewport_size` synchronously would notify reactive
        // subscribers re-entrantly mid-layout — an effect that rebuilds
        // (and thus schedules another layout / borrows backend state the
        // pass already holds) panics, and the panic-during-notify aborts
        // the process (observed as a startup SIGABRT once any screen read
        // `viewport_size()`). So defer the mirror to a microtask: the
        // notify then runs AFTER the pass completes, the same way iOS
        // mirrors the viewport from a UIKit resize callback rather than
        // inside layout (see `callbacks.rs`). Dedup by last-mirrored size
        // so a resize schedules exactly one microtask and the steady
        // state (unchanged size every pass) schedules none.
        if w > 0.0 && h > 0.0 {
            let changed = LAST_MIRRORED_VIEWPORT.with(|c| c.get()) != Some((w, h));
            if changed {
                LAST_MIRRORED_VIEWPORT.with(|c| c.set(Some((w, h))));
                runtime_core::schedule_microtask(move || {
                    runtime_core::set_viewport_size(runtime_core::ViewportSize {
                        width: w,
                        height: h,
                    });
                });
            }
        }
        (w, h)
    }

    /// Public wrapper around [`Self::run_layout_pass`]. Used by the
    /// runtime-server shell — in runtime-server mode the backend lives by-value inside an
    /// `RuntimeServerClient`, so `install_global_self` is never called and the
    /// `schedule_layout_pass_retry` path bails on the missing
    /// `ANDROID_BACKEND_SELF.upgrade()`. The shell calls this
    /// synchronously after each `apply_batch` instead. Mirrors the
    /// iOS shell's `backend_mut().run_layout()` shape.
    pub fn run_layout(&mut self) {
        self.run_layout_pass();
    }

    /// Read system safe-area insets (status bar, navigation bar,
    /// display cutout) from the host root's `WindowInsets`. Returns
    /// values in dp so they match Taffy's coordinate space.
    ///
    /// On API 30+ uses `WindowInsets.getInsets(systemBars | displayCutout)`
    /// which reports the unconsumed insets regardless of
    /// `fitsSystemWindows`. The pre-30 deprecated
    /// `getSystemWindowInset*` accessors return zero when the
    /// activity isn't in edge-to-edge mode (system "consumed" them);
    /// the new path always returns the real values.
    fn platform_safe_area_insets(&self) -> runtime_core::EdgeInsets {
        let host = self.root.as_obj();
        let mut final_insets = runtime_core::EdgeInsets::ZERO;
        let result = with_env(|env| -> Option<runtime_core::EdgeInsets> {
            let density = density_of(env, &host).unwrap_or(1.0);
            let insets_obj = env
                .call_method(
                    &host,
                    "getRootWindowInsets",
                    "()Landroid/view/WindowInsets;",
                    &[],
                )
                .ok()
                .and_then(|v| v.l().ok())?;
            if insets_obj.is_null() {
                return None;
            }
            // Prefer `WindowInsets.getInsets(int typeMask)` (API 30+)
            // which honors edge-to-edge / non-edge-to-edge alike. The
            // mask is `Type.systemBars() | Type.displayCutout()` —
            // `systemBars` is 0x1|0x2|0x4 = 7 (statusBars|navigationBars|captionBar)
            // and `displayCutout` is 0x80 = 128. So mask = 135.
            // `android.view.WindowInsets$Type.systemBars()` returns 7;
            // we hardcode the bits to avoid the static-method lookup
            // round-trip. These constants are stable in the AOSP
            // source since they were added in API 30.
            let type_mask: i32 = 7 | 128;
            // First try the API-30+ `getInsets(int)` returning Insets.
            let insets_struct = env
                .call_method(
                    &insets_obj,
                    "getInsets",
                    "(I)Landroid/graphics/Insets;",
                    &[jni::objects::JValue::Int(type_mask)],
                )
                .ok()
                .and_then(|v| v.l().ok());
            let (top_px, right_px, bottom_px, left_px) = match insets_struct {
                Some(ref s) if !s.is_null() => {
                    // Insets is a final class with public int fields
                    // (top, left, bottom, right).
                    let mut read_field = |name: &str| -> i32 {
                        env.get_field(s, name, "I").and_then(|v| v.i()).unwrap_or(0)
                    };
                    (
                        read_field("top"),
                        read_field("right"),
                        read_field("bottom"),
                        read_field("left"),
                    )
                }
                _ => {
                    // Fallback for pre-API-30: deprecated getSystemWindowInset*.
                    let mut read = |name: &str| -> i32 {
                        env.call_method(&insets_obj, name, "()I", &[])
                            .and_then(|v| v.i())
                            .unwrap_or(0)
                    };
                    (
                        read("getSystemWindowInsetTop"),
                        read("getSystemWindowInsetRight"),
                        read("getSystemWindowInsetBottom"),
                        read("getSystemWindowInsetLeft"),
                    )
                }
            };
            // Some activity configurations report all-zero insets
            // (system "consumed" the bars and resized the activity
            // view above them). On Android's emulator with default
            // gesture nav, that means the activity's view ends right
            // at the gesture bar's top edge — but the gesture
            // indicator pill still renders OVER the bottom of the
            // activity's content. Children at the activity's bottom
            // (sidebar toggle in our docs example) end up half-hidden
            // by the pill. Fall back to Android's standard system
            // resource lookups so authors get reasonable bottom
            // breathing room regardless of edge-to-edge state.
            let (mut top, mut right, mut bottom, mut left) = (
                top_px as f32 / density,
                right_px as f32 / density,
                bottom_px as f32 / density,
                left_px as f32 / density,
            );
            if top == 0.0 && bottom == 0.0 && left == 0.0 && right == 0.0 {
                if let Some((sb, nb)) = read_system_bar_dimens(env, &host) {
                    log::info!(
                        "[safe-area] fallback dimens status_bar={}dp nav_bar={}dp",
                        sb, nb
                    );
                    top = sb;
                    bottom = nb;
                }
                // Last-resort fallback: even when `getIdentifier`
                // returns 0 (some OEM ROMs strip the platform dimen),
                // the gesture/nav bar still overlays the bottom of the
                // activity. A conservative 24dp keeps the toggle row /
                // last-item area out from under the indicator pill.
                if bottom == 0.0 {
                    bottom = 24.0;
                }
            }
            Some(runtime_core::EdgeInsets { top, right, bottom, left })
        });
        final_insets = result.unwrap_or(runtime_core::EdgeInsets::ZERO);
        // Even after the `Insets`/deprecated/`Resources` fallbacks, if
        // we still see 0 it means `getRootWindowInsets` returned null
        // (host hasn't been attached to a window yet, e.g. very early
        // in mount). Apply a conservative bottom inset so the
        // sidebar's toggle row isn't permanently hidden behind the
        // gesture pill. The next inset-changed cycle will replace
        // this with real measurements.
        if final_insets.top == 0.0
            && final_insets.bottom == 0.0
            && final_insets.left == 0.0
            && final_insets.right == 0.0
        {
            final_insets.top = 24.0;
            final_insets.bottom = 24.0;
        }
        // INTERIM (non-edge-to-edge): this Android activity lays its
        // content out BELOW the status bar already — the window insets
        // are effectively consumed at the top. So a `.safe_area(TOP)`
        // surface (e.g. the drawer sidebar) must NOT re-add the
        // status-bar inset, or it double-insets (~2× the bar height; the
        // sidebar brand sat far too low). Zero the top so `.safe_area(TOP)`
        // is a no-op here and the system's own layout inset is the single
        // source of top spacing. The bottom is intentionally KEPT: the
        // gesture / navigation pill overlays content even when not
        // edge-to-edge, so a bottom inset is genuinely needed.
        //
        // When the app adopts edge-to-edge (deferred to a dedicated
        // effort), this must instead report the view's real *unconsumed*
        // top inset — matching iOS's per-view `safeAreaInsets`, which is
        // already correct because iOS surfaces are full-screen under the
        // notch. iOS is a separate backend and unaffected by this.
        final_insets.top = 0.0;
        final_insets
    }

    /// Run the layout pass: for every Taffy root (the framework's
    /// app root plus any disconnected sub-roots), compute, then
    /// iterate every registered view and write its frame onto the
    /// view's `FrameLayout.LayoutParams`.
    pub(crate) fn run_layout_pass(&mut self) {
        let (vw, vh) = self.viewport_size();
        if vw <= 0.0 || vh <= 0.0 {
            return;
        }
        let _t_total = phase_timer::PhaseTimer::start("layout_pass_total");
        let roots: Vec<runtime_layout::LayoutNode> = self
            .view_to_layout
            .values()
            .map(|(_, n)| *n)
            .filter(|n| self.layout.is_root(*n))
            .collect();
        {
            let _t = phase_timer::PhaseTimer::start("layout_taffy_compute");
            for root_node in &roots {
                self.layout.compute(*root_node, vw, vh);
            }
        }
        // Snapshot the entries up front so the mutable JNI calls
        // below don't conflict with the borrow on `self.view_to_layout`.
        let frames: Vec<(GlobalRef, runtime_layout::Frame)> = {
            let _t = phase_timer::PhaseTimer::start("layout_snapshot_frames");
            self.view_to_layout
                .values()
                .map(|(view, n)| (view.clone(), self.layout.frame_of(*n)))
                .collect()
        };
        with_env(|env| {
            // Build the batch: a parallel `View[]` + packed px quads,
            // applying the same filters the old per-view path used (skip
            // zero-size views and detached window roots). The drawer-panel
            // skip + non-`MarginLayoutParams` wrap moved into the Kotlin
            // `applyFrames` loop. Density is a per-display constant — read
            // once off the first view and reuse for every conversion.
            let _t_apply = phase_timer::PhaseTimer::start("apply_frames_batch");
            let density = frames
                .first()
                .map(|(v, _)| cached_density(env, &v.as_obj()))
                .unwrap_or(1.0);
            let mut batch_views: Vec<&GlobalRef> = Vec::with_capacity(frames.len());
            let mut packed: Vec<i32> = Vec::with_capacity(frames.len() * 4);
            for (view, frame) in &frames {
                if frame.width <= 0.0 && frame.height <= 0.0 {
                    continue;
                }
                // Detached window root (screen_recorder private layer):
                // its top-level view is positioned by its
                // `WindowManager.LayoutParams` (full-screen), NOT a
                // `MarginLayoutParams` — applying a Taffy frame here
                // would wrap it in margin params and break the window.
                // Its CHILDREN are ordinary views that DO flow through
                // the batch, so the layer's controls position correctly.
                if self.detached_window_roots.contains_key(&Self::node_key(view)) {
                    continue;
                }
                batch_views.push(view);
                packed.push((frame.x * density).round() as i32);
                packed.push((frame.y * density).round() as i32);
                packed.push((frame.width * density).round() as i32);
                packed.push((frame.height * density).round() as i32);
            }
            // One Rust→JVM call applies every frame (vs ~9 crossings/view).
            apply_frames_batch(env, &batch_views, &packed);
            drop(_t_apply);
            // Per-view animation/gradient sync — only the minority of
            // views with active anim state. Kept per-view: it reads
            // `anim_state` and does percent/radius resolution that doesn't
            // batch cleanly. Independent of the `LayoutParams` write above
            // (both read the frame dims passed directly, not the applied
            // LP), so running it after the batch is equivalent to the old
            // interleaved order.
            for (view, frame) in &frames {
                if frame.width <= 0.0 && frame.height <= 0.0 {
                    continue;
                }
                if self.detached_window_roots.contains_key(&Self::node_key(view)) {
                    continue;
                }
                let key = Self::node_key(view);
                // Feed any `on_layout` subscribers (a `.container()` view's
                // inline-size signal) with this view's resolved size in dp.
                // The callbacks change-guard, so re-firing at an unchanged
                // width is a no-op — which keeps the container-query
                // restyle→relayout loop convergent.
                fire_layout_for_view(key, frame.width, frame.height);
                if let Some(state) = self.anim_state.get(&key) {
                    {
                        let _t = phase_timer::PhaseTimer::start("transform_pct");
                        // Resolve any percent-valued `transform: translate`
                        // requests now that the box has real pixel
                        // dimensions.
                        style::sync_transform_translate_percent(
                            env,
                            view.as_obj(),
                            state,
                            frame.width,
                            frame.height,
                        );
                    }
                    let _t = phase_timer::PhaseTimer::start("radial_gradient_resize");
                    // Recompute the radial gradient's px radius now that
                    // the view has a real size.
                    style::sync_radial_gradient_radius(
                        env,
                        state,
                        frame.width,
                        frame.height,
                        density,
                    );
                }
            }
            // Refresh `layout_y` for every Position::Sticky child
            // now that Taffy has re-laid out the tree. Without
            // this, a tree rebuild (route switch, branch swap)
            // leaves stale layout-y values and the sticky child
            // pins to the wrong place — most visibly when the
            // user scrolls a freshly-mounted screen for the first
            // time. Cheap walk; the registry is tiny by
            // construction. Mirrors iOS's
            // `sticky::refresh_layout_positions` call in
            // `run_layout_pass_global`.
            {
                let _t = phase_timer::PhaseTimer::start("sticky_refresh");
                sticky::refresh_layout_positions(
                    env,
                    &mut self.sticky_registry,
                    &self.layout,
                    &self.view_to_layout,
                );
            }
        });
        // Reveal any portal overlays that were created INVISIBLE and
        // are now laid out. `create_overlay_portal` sets a fresh overlay
        // to `View.INVISIBLE` and queues it here; the frames above have
        // just positioned its content, so flip it to `View.VISIBLE`.
        // The overlay stays INVISIBLE until its first layout pass
        // positions its content, else it paints one unlaid-out frame at
        // the 0,0 origin on mount (children not yet centered) — a
        // visible snap. `setVisibility` on an overlay that was since
        // detached from `root` is a harmless no-op, so no guard is
        // needed beyond not panicking. INVISIBLE (not GONE) keeps the
        // overlay in the layout pass; only its painting is suppressed.
        if !self.pending_reveal.is_empty() {
            let to_reveal = std::mem::take(&mut self.pending_reveal);
            with_env(|env| {
                const VISIBLE: i32 = 0; // View.VISIBLE
                for overlay in &to_reveal {
                    let _ = env.call_method(
                        overlay,
                        "setVisibility",
                        "(I)V",
                        &[JValue::Int(VISIBLE)],
                    );
                }
            });
        }
        drop(_t_total);
        // Drain the per-phase counters accumulated during this pass
        // and the apply-style work that preceded it. Logging here
        // (rather than at app exit) gives a per-pass histogram so
        // we can spot regressions immediately — and clears the
        // table so the next pass starts fresh.
        phase_timer::drain_and_log_phase_counters();
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

thread_local! {
    /// Per-view `on_layout` callbacks, keyed by the JObject pointer (the
    /// same `node_key` the animation state + `view_to_layout` use). Fired
    /// from `run_layout_pass` after each view's frame is applied — the
    /// Android analog of the web `ResizeObserver`, which is how a
    /// `.container()` view's inline-size signal gets fed. The UI thread is
    /// single-threaded, so a thread-local registry is safe.
    static LAYOUT_SUBS: std::cell::RefCell<Vec<(usize, Rc<dyn Fn(f32, f32)>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Fire every `on_layout` callback registered for `view_key` with the
/// view's resolved inline-size (`w`) and block-size (`h`) in dp. The
/// callbacks change-guard, so re-firing at an unchanged size is a no-op.
pub(crate) fn fire_layout_for_view(view_key: usize, w: f32, h: f32) {
    let cbs: Vec<Rc<dyn Fn(f32, f32)>> = LAYOUT_SUBS.with(|m| {
        m.borrow()
            .iter()
            .filter(|(k, _)| *k == view_key)
            .map(|(_, c)| c.clone())
            .collect()
    });
    for c in cbs {
        c(w, h);
    }
}

pub(crate) struct AndroidViewOps;
impl runtime_core::ViewOps for AndroidViewOps {
    fn subscribe_layout(
        &self,
        node: &dyn std::any::Any,
        callback: Box<dyn Fn(f32, f32)>,
    ) -> runtime_core::LayoutSubscription {
        let Some(view) = node.downcast_ref::<GlobalRef>() else {
            return runtime_core::LayoutSubscription::noop();
        };
        // Same key derivation as `node_key` / `view_to_layout`.
        let key = view.as_obj().as_raw() as usize;
        let cb: Rc<dyn Fn(f32, f32)> = Rc::from(callback);
        let cb_id = Rc::as_ptr(&cb) as *const () as usize;
        LAYOUT_SUBS.with(|m| m.borrow_mut().push((key, cb)));
        runtime_core::LayoutSubscription::new(move || {
            LAYOUT_SUBS.with(|m| {
                m.borrow_mut()
                    .retain(|(k, c)| !(*k == key && Rc::as_ptr(c) as *const () as usize == cb_id))
            });
        })
    }

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
    ) -> Option<runtime_core::primitives::portal::ViewportRect> {
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
            Some(runtime_core::primitives::portal::ViewportRect {
                x: x_px / density,
                y: y_px / density,
                width: w_px as f32 / density,
                height: h_px as f32 / density,
            })
        })
    }

    /// Route `AnimatedValue::bind` writes through the existing
    /// `backend_android_mobile::set_animated_f32` free function so
    /// the framework's animation-binding helper doesn't have to
    /// know about `GlobalRef`. Mirrors `IosViewOps::set_animated_f32`.
    fn set_animated_f32(
        &self,
        node: &dyn std::any::Any,
        prop: runtime_core::animation::AnimProp,
        value: f32,
    ) {
        if let Some(n) = node.downcast_ref::<GlobalRef>() {
            crate::set_animated_f32(n, prop, value);
        }
    }

    /// Color-family analog of [`Self::set_animated_f32`].
    fn set_animated_color(
        &self,
        node: &dyn std::any::Any,
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        if let Some(n) = node.downcast_ref::<GlobalRef>() {
            crate::set_animated_color(n, prop, value);
        }
    }
}
pub(crate) static ANDROID_VIEW_OPS: AndroidViewOps = AndroidViewOps;

pub(crate) struct AndroidTextOps;
impl runtime_core::TextOps for AndroidTextOps {
    /// Route text-color animations through the backend's
    /// `set_animated_color` — Android's `ForegroundColor` branch
    /// dispatches to `TextView.setTextColor`, which is what makes
    /// the welcome headline's dark→light transition visible on
    /// label nodes.
    fn set_animated_color(
        &self,
        node: &dyn std::any::Any,
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        if let Some(n) = node.downcast_ref::<GlobalRef>() {
            crate::set_animated_color(n, prop, value);
        }
    }
}
pub(crate) static ANDROID_TEXT_OPS: AndroidTextOps = AndroidTextOps;

// ---------------------------------------------------------------------------
// Global self-handle. Mirrors `IOS_BACKEND_SELF` — host code installs
// a `Weak<RefCell<AndroidBackend>>` once at `attach` so the
// cross-platform animation system's per-frame subscribers can reach
// the backend without the welcome example having to thread the
// `Rc<RefCell<AndroidBackend>>` through every closure.
// ---------------------------------------------------------------------------

thread_local! {
    pub(crate) static ANDROID_BACKEND_SELF: std::cell::RefCell<Option<std::rc::Weak<std::cell::RefCell<AndroidBackend>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Read Android's well-known `status_bar_height` / `navigation_bar_height`
/// dimens from the platform's internal resources. Used as a fallback
/// when `WindowInsets` reports all zeros (some activities consume
/// insets at the system level but the gesture/nav bar still renders
/// over content). Returns `(status_bar_dp, navigation_bar_dp)`.
fn read_system_bar_dimens(
    env: &mut JNIEnv,
    host: &JObject,
) -> Option<(f32, f32)> {
    let density = density_of(env, host).unwrap_or(1.0);
    let context = env
        .call_method(host, "getContext", "()Landroid/content/Context;", &[])
        .ok()
        .and_then(|v| v.l().ok())?;
    if context.is_null() {
        return None;
    }
    let resources = env
        .call_method(&context, "getResources", "()Landroid/content/res/Resources;", &[])
        .ok()
        .and_then(|v| v.l().ok())?;
    let read_dimen = |env: &mut JNIEnv, name: &str| -> f32 {
        let id_name = match env.new_string(name) {
            Ok(s) => s,
            Err(_) => return 0.0,
        };
        let android_str = match env.new_string("android") {
            Ok(s) => s,
            Err(_) => return 0.0,
        };
        let dimen_str = match env.new_string("dimen") {
            Ok(s) => s,
            Err(_) => return 0.0,
        };
        let res_id: i32 = env
            .call_method(
                &resources,
                "getIdentifier",
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I",
                &[
                    jni::objects::JValue::Object(&id_name),
                    jni::objects::JValue::Object(&dimen_str),
                    jni::objects::JValue::Object(&android_str),
                ],
            )
            .and_then(|v| v.i())
            .unwrap_or(0);
        if res_id == 0 {
            return 0.0;
        }
        let px: i32 = env
            .call_method(
                &resources,
                "getDimensionPixelSize",
                "(I)I",
                &[jni::objects::JValue::Int(res_id)],
            )
            .and_then(|v| v.i())
            .unwrap_or(0);
        px as f32 / density
    };
    Some((
        read_dimen(env, "status_bar_height"),
        read_dimen(env, "navigation_bar_height"),
    ))
}

/// Walk the subtree rooted at `view`, checking each view's pointer
/// against `pending_sticky`. Any pending entry whose view can now
/// resolve a ScrollView ancestor (i.e. the just-inserted subtree is
/// now wired into one) gets promoted into the live registry via
/// [`sticky::register`]. The view keys to remove from
/// `pending_sticky` are collected in `to_remove` so the caller can
/// drop them after the walk (avoids borrowing `pending_sticky`
/// mutably across the recursion).
///
/// Subtree walk (not just the root view): a `Element::View`
/// containing a `View { position: Sticky }` child will see the
/// outer View as `child_view` in `insert`, with the sticky child
/// nested inside. Both flagged in `pending_sticky` until this walk
/// promotes them. Mirrors iOS's `promote_pending_sticky_recursive`.
fn promote_pending_sticky_recursive(
    env: &mut JNIEnv,
    view: &GlobalRef,
    pending: &mut HashMap<usize, f32>,
    registry: &mut sticky::StickyRegistry,
    scroll_listeners: &mut HashMap<usize, GlobalRef>,
    scroll_observers: &HashMap<usize, Rc<dyn Fn(f32, f32)>>,
    to_remove: &mut Vec<usize>,
) {
    let key = view.as_obj().as_raw() as usize;
    if let Some(&threshold) = pending.get(&key) {
        if sticky::register(env, registry, scroll_listeners, view, threshold, scroll_observers) {
            to_remove.push(key);
        }
        // If register returned false, the view STILL has no scroll
        // ancestor — leave it in `pending` so a future re-parent
        // could pick it up.
    }
    // `getChildCount`/`getChildAt` live on ViewGroup, NOT on every
    // View — TextView/ImageView/etc. extend View directly. iOS gets
    // away without a runtime check here because `UIView.subviews` is
    // defined on the base UIView class and a UILabel just returns an
    // empty array. On Android, calling getChildCount on a TextView
    // raises `NoSuchMethodError`. The Rust call site's `unwrap_or(0)`
    // masks the error from our side, but the JNI **pending exception**
    // stays live on `env` and the next ART-side bookkeeping check
    // aborts the process ("No pending exception expected"). Gate the
    // call on `instanceof ViewGroup` so leaf views short-circuit before
    // touching the missing method.
    if !is_view_group(env, view.as_obj()) {
        return;
    }
    let child_count = env
        .call_method(view.as_obj(), "getChildCount", "()I", &[])
        .and_then(|v| v.i())
        .unwrap_or(0);
    for i in 0..child_count {
        let Ok(child_obj) = env
            .call_method(
                view.as_obj(),
                "getChildAt",
                "(I)Landroid/view/View;",
                &[JValue::Int(i)],
            )
            .and_then(|v| v.l())
        else {
            continue;
        };
        if child_obj.is_null() {
            continue;
        }
        // Need a `GlobalRef` to recurse (sticky::register takes
        // `&GlobalRef`). Wrap the local — short-lived; dropped at
        // end of the recursive call.
        let Ok(child_global) = env.new_global_ref(&child_obj) else {
            continue;
        };
        promote_pending_sticky_recursive(
            env,
            &child_global,
            pending,
            registry,
            scroll_listeners,
            scroll_observers,
            to_remove,
        );
    }
}

/// `instanceof android.view.ViewGroup`. Used by the sticky walkers
/// before calling `getChildCount`/`getChildAt`, which only exist on
/// ViewGroup — invoking them on a leaf View (TextView, ImageView,
/// etc.) raises `NoSuchMethodError` and crashes the runtime via the
/// JNI pending-exception path. Returns `false` (don't recurse) on
/// class-lookup failure to stay safe.
fn is_view_group(env: &mut JNIEnv, view: &JObject) -> bool {
    let Ok(class) = env.find_class("android/view/ViewGroup") else {
        return false;
    };
    env.is_instance_of(view, &class).unwrap_or(false)
}

/// Recursive cleanup helper used by `clear_children`. For each
/// view in the subtree being removed: deregister it as a sticky
/// child (if any), drop its `pending_sticky` entry (if any), and
/// if it IS a scroll view, deregister it as a scroll-host so its
/// descendants' sticky bookkeeping is cleaned up too. Mirrors iOS's
/// `walk_and_deregister`.
fn walk_and_deregister_sticky(
    env: &mut JNIEnv,
    view: &JObject,
    registry: &mut sticky::StickyRegistry,
    scroll_listeners: &mut HashMap<usize, GlobalRef>,
    scroll_observers: &mut HashMap<usize, Rc<dyn Fn(f32, f32)>>,
    pending: &mut HashMap<usize, f32>,
    sv_class: Option<&jni::objects::JClass>,
    hsv_class: Option<&jni::objects::JClass>,
) {
    // Wrap into a temporary GlobalRef so we can reuse the
    // GlobalRef-based deregister helpers. Dropping the ref at end
    // of scope releases the temporary handle; the underlying
    // Java view is still parented and reachable elsewhere.
    let Ok(global) = env.new_global_ref(view) else {
        return;
    };
    sticky::deregister(env, registry, scroll_listeners, &global, scroll_observers);
    let key = global.as_obj().as_raw() as usize;
    pending.remove(&key);

    // If this view itself is a scroll view, deregister the whole
    // scroll-host entry so descendants under it are cleaned up.
    let is_scroll = if let (Some(sv), Some(hsv)) = (sv_class, hsv_class) {
        env.is_instance_of(view, sv).unwrap_or(false)
            || env.is_instance_of(view, hsv).unwrap_or(false)
    } else {
        false
    };
    if is_scroll {
        // Drop any user-supplied on_scroll callback for this scroll
        // view BEFORE we ask `deregister_scroll_view` to release the
        // shared listener \u{2014} otherwise the listener slot stays
        // pinned and the JVM-side listener leaks.
        scroll_observers.remove(&key);
        sticky::deregister_scroll_view(
            env,
            registry,
            scroll_listeners,
            &global,
            scroll_observers,
        );
    }

    // Recurse into children. Same ViewGroup-guard rationale as
    // `promote_pending_sticky_recursive` — getChildCount on a leaf
    // View raises a JNI pending exception that crashes the next
    // ART check.
    if !is_view_group(env, view) {
        return;
    }
    let child_count = env
        .call_method(view, "getChildCount", "()I", &[])
        .and_then(|v| v.i())
        .unwrap_or(0);
    for i in 0..child_count {
        let Ok(child_obj) = env
            .call_method(view, "getChildAt", "(I)Landroid/view/View;", &[JValue::Int(i)])
            .and_then(|v| v.l())
        else {
            continue;
        };
        if child_obj.is_null() {
            continue;
        }
        walk_and_deregister_sticky(
            env,
            &child_obj,
            registry,
            scroll_listeners,
            scroll_observers,
            pending,
            sv_class,
            hsv_class,
        );
    }
}

/// Install the backend's self-reference. Called once by the host
/// wrapper after wrapping the backend in `Rc<RefCell<>>`. Without it,
/// `set_animated_f32` / `set_animated_color` quietly no-op.
pub fn install_global_self(weak: std::rc::Weak<std::cell::RefCell<AndroidBackend>>) {
    ANDROID_BACKEND_SELF.with(|s| {
        *s.borrow_mut() = Some(weak);
    });
}

/// Read the installed backend self-handle without consuming it.
/// Returns `None` if `install_global_self` hasn't fired (e.g. in
/// runtime-server-client mode where the backend is moved by value).
/// Crates outside this one use this to reach the backend from JNI
/// trampolines and SDK helper code without each having to wire up
/// its own thread-local.
pub fn backend_self_weak() -> Option<std::rc::Weak<std::cell::RefCell<AndroidBackend>>> {
    ANDROID_BACKEND_SELF.with(|s| s.borrow().clone())
}

/// Push a scalar animation property update to `node` on the installed
/// global backend. Same shape as `backend_ios::set_animated_f32`.
/// No-ops cleanly if no backend is installed, the install has been
/// dropped, or the backend is currently borrowed (the in-flight call
/// will see the new AV value on its next frame).
pub fn set_animated_f32(
    node: &GlobalRef,
    prop: runtime_core::animation::AnimProp,
    value: f32,
) {
    let weak = ANDROID_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        use runtime_core::Backend;
        b.set_animated_f32(node, prop, value);
    };
}

/// Color-family counterpart of [`set_animated_f32`]. Routes through
/// the global backend's `set_animated_color`.
pub fn set_animated_color(
    node: &GlobalRef,
    prop: runtime_core::animation::AnimProp,
    value: [f32; 4],
) {
    let weak = ANDROID_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        use runtime_core::Backend;
        b.set_animated_color(node, prop, value);
    };
}

// ---------------------------------------------------------------------------
/// Generic external-registration entry (mirrors `impl RegisterExternal for
/// MacosBackend`): lets `register<B: RegisterExternal>(b)` — e.g.
/// `canvas_vello::register` — target Android without naming the concrete
/// backend. Forwards to the same `external_handlers` registry as the inherent
/// [`AndroidBackend::register_external`], so an explicit call overrides an
/// inventory-registered handler for the same payload (last-registration-wins).
impl runtime_core::RegisterExternal for AndroidBackend {
    fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&std::rc::Rc<T>, &mut AndroidBackend) -> GlobalRef + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }
}

// Backend trait impl. Each method delegates to the matching primitive
// module (or to one of the style/helpers helpers). Keep this thin —
// anything substantial belongs in the primitive's file.
// ---------------------------------------------------------------------------

impl Backend for AndroidBackend {
    type Node = GlobalRef;

    /// Navigator abstraction calls this after every command (see the trait doc).
    fn schedule_layout_pass() {
        crate::schedule_layout_pass();
    }

    fn platform(&self) -> runtime_core::Platform {
        runtime_core::Platform::Android
    }

    fn supports_screenshot(&self) -> bool {
        // Capability, not current state: Android can always draw a view
        // hierarchy to a Bitmap. A capture before the root is laid out
        // returns an error rather than failing this gate.
        true
    }

    fn capture_screenshot(
        &self,
        done: Box<dyn FnOnce(Result<runtime_core::Screenshot, String>)>,
    ) {
        done(screenshot::capture(&self.root));
    }

    fn url_opener(&self) -> Option<std::rc::Rc<dyn Fn(&str)>> {
        // Clone the Context's GlobalRef into the closure — the JVM
        // keeps the object alive as long as the ref lives, and the
        // closure outlives this borrow of `self`.
        let context = self.context.clone();
        Some(std::rc::Rc::new(move |url: &str| {
            with_env(|env| {
                if let Err(e) = start_view_intent(env, &context, url) {
                    // A thrown Java exception (e.g. ActivityNotFound)
                    // stays pending and would poison the next JNI call
                    // — clear it before returning.
                    let _ = env.exception_clear();
                    runtime_core::log(
                        runtime_core::LogLevel::Warn,
                        &format!("open_url: ACTION_VIEW intent failed: {e:?}"),
                    );
                }
            });
        }))
    }

    fn fullscreen_setter(&self) -> Option<std::rc::Rc<dyn Fn(bool)>> {
        // Same capture pattern as `url_opener`: the Activity Context's
        // GlobalRef rides into the closure (JVM keeps it alive). The
        // closure calls the static Kotlin helper `RustSystemUi`, which
        // owns the per-API-level WindowInsetsController dance.
        let context = self.context.clone();
        Some(std::rc::Rc::new(move |enabled: bool| {
            with_env(|env| {
                // `RustSystemUi` is additive runtime — an embedded Kotlin
                // runtime older than this native lib won't have it. Clear
                // the pending exception and soft-fail so full-screen is
                // silently absent rather than poisoning later JNI calls.
                let class = match env.find_class("io/idealyst/runtime/RustSystemUi") {
                    Ok(c) => c,
                    Err(_) => {
                        let _ = env.exception_clear();
                        runtime_core::log(
                            runtime_core::LogLevel::Warn,
                            "set_fullscreen: RustSystemUi runtime class unavailable — \
                             full-screen skipped; rebuild the idealyst CLI to ship it.",
                        );
                        return;
                    }
                };
                if let Err(e) = env.call_static_method(
                    class,
                    "setFullscreen",
                    "(Landroid/content/Context;Z)V",
                    &[
                        JValue::Object(&context.as_obj()),
                        JValue::Bool(enabled as jni::sys::jboolean),
                    ],
                ) {
                    let _ = env.exception_clear();
                    runtime_core::log(
                        runtime_core::LogLevel::Warn,
                        &format!("set_fullscreen: RustSystemUi.setFullscreen failed: {e:?}"),
                    );
                }
            });
        }))
    }

    fn color_scheme(&self) -> runtime_core::ColorScheme {
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
                    0x10 => runtime_core::ColorScheme::Light,
                    0x20 => runtime_core::ColorScheme::Dark,
                    _ => runtime_core::ColorScheme::Auto,
                },
                Err(_) => runtime_core::ColorScheme::Auto,
            }
        })
    }

    fn create_view(&mut self, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        let node = primitives::view::create(self);
        a11y::apply(&node, a11y, None);
        node
    }

    /// Opt into ANCHORLESS reactive regions. The framework then splices a
    /// `when`/`switch`/`for` region's children DIRECTLY into the real
    /// parent (via `insert_at` / `remove_child`) instead of nesting them
    /// under a `create_reactive_anchor` wrapper FrameLayout.
    ///
    /// This is the native analog of web's `display: contents` anchor and
    /// the root fix for the "`when`-mounted box never appears on Android"
    /// bug: a wrapper FrameLayout AUTO-sizes to its in-flow children, so a
    /// branch whose only content is `position: Absolute` (the whiteboard's
    /// bottom-right camera box) collapsed the wrapper to 0×0 and the
    /// absolute child — though Taffy gave it a correct frame — never
    /// painted (a 0×0 ViewGroup doesn't lay out a larger child). Splicing
    /// the branch into the real parent gives in-flow content normal flow
    /// AND absolute content the real parent as its containing block, both
    /// matching web with no per-case wrapper hack. It also upgrades
    /// reactive `for` to keyed reconciliation (per-row state survives).
    fn supports_child_splice(&self) -> bool {
        true
    }

    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        primitives::view::remove_child(self, parent, child);
        crate::imp::scheduler::schedule_layout_pass_retry(0);
    }

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        primitives::view::insert_at(self, parent, child, index);
        // Same dynamic-mount layout-pass policy as `insert`: a region
        // splicing rows into an already-attached parent mounts after the
        // initial `finish()` pass, so it must kick its own (coalesced)
        // pass or the new views render at default 0×0.
        let is_portal_parent = self.portal_instances.contains_key(&Self::node_key_of(parent));
        let parent_attached = with_env(|env| {
            env.call_method(parent.as_obj(), "isAttachedToWindow", "()Z", &[])
                .and_then(|v| v.z())
                .unwrap_or(false)
        });
        if crate::layout_policy::insert_needs_layout_pass(is_portal_parent, parent_attached) {
            crate::imp::scheduler::schedule_layout_pass_retry(0);
        }
    }

    /// Override the trait default (which silently falls back to
    /// `create_view`, dropping `on_click`) so Pressable's tap handler
    /// actually fires on Android. Same `FrameLayout + setClickable +
    /// setOnClickListener` shape `create_link` uses — the only
    /// difference between a Link and a Pressable at this layer is the
    /// trigger semantics (nav dispatch vs author closure), and
    /// `link::create` already takes the bare `Rc<dyn Fn()>` we have
    /// here. The default fall-through was the root cause of the
    /// `Switch` rewrite's track-tap-doesn't-toggle bug — and any
    /// other author Pressable on Android.
    fn create_pressable(
        &mut self,
        on_click: std::rc::Rc<dyn Fn()>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::link::create(self, on_click);
        a11y::apply(&node, a11y, None);
        node
    }

    fn create_link(
        &mut self,
        config: runtime_core::primitives::link::LinkConfig,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let route = config.route;
        let url = config.url.clone();
        let external = config.external;
        let node = primitives::link::create(self, config.on_activate);
        // Mirror iOS: default Link label = the route (in-app) or the
        // URL (external), if no author label was given. `a11y::apply`
        // clears the label when `props.label.is_none()`; we re-set it
        // afterwards so reactive prop changes that explicitly clear
        // the label fall back rather than leaving the link unlabelled.
        // Author overrides still win.
        let resolved_label = a11y.label.clone().unwrap_or_else(|| {
            if external {
                url.clone()
            } else {
                route.to_string()
            }
        });
        let effective_a11y = runtime_core::accessibility::AccessibilityProps {
            label: Some(resolved_label),
            ..a11y.clone()
        };
        a11y::apply(
            &node,
            &effective_a11y,
            Some(runtime_core::accessibility::Role::Link),
        );
        node
    }

    fn create_text(&mut self, content: &str, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        let node = primitives::text::create(self, content);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::Text));
        node
    }

    fn create_button(&mut self, label: &str, on_click: &runtime_core::Action, _leading_icon: Option<&runtime_core::IconData>, _trailing_icon: Option<&runtime_core::IconData>, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        // TODO: render icons as compound drawables on the button
        let node = primitives::button::create(self, label, on_click.fire.clone());
        // A bare `android.widget.Button` is a 0×0 Taffy leaf without a
        // measure_fn — as a flex child with no explicit height it collapses
        // and vanishes (the "Start camera button is missing" bug). Hook its
        // intrinsic `measure()` like Switch/SeekBar do.
        primitives::measure::install_intrinsic_measure(self, &node);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::Button));
        node
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let child_for_sticky = child.clone();
        primitives::view::insert(self, parent, child);
        // Retry pending sticky registrations now that this subtree
        // is wired into the parent chain. The walker fires
        // `apply_style` before `insert`, so any `Position::Sticky`
        // child created in this build cycle deferred its
        // registration to `pending_sticky`. Walk the just-inserted
        // subtree's view tree (with the child as root) and promote
        // each pending entry that can now resolve a scroll
        // ancestor. Entries that still can't — genuinely no
        // scroll-view ancestor — stay in the pending map until the
        // view is removed. Mirrors iOS's `promote_pending_sticky_recursive`.
        let mut to_remove = Vec::new();
        with_env(|env| {
            promote_pending_sticky_recursive(
                env,
                &child_for_sticky,
                &mut self.pending_sticky,
                &mut self.sticky_registry,
                &mut self.scroll_listeners,
                &self.scroll_observers,
                &mut to_remove,
            );
        });
        for k in to_remove {
            self.pending_sticky.remove(&k);
        }

        // A dynamically-mounted subtree needs a layout pass that the initial
        // build's `finish()` hook can't provide, because it mounts AFTER
        // `finish()` already ran. This covers three cases:
        //
        //   1. Portals (their open signal flips) — parent is a portal holder.
        //   2. ANY reactive control-flow child — `when` toggling true, a
        //      `switch`/`match` branch swapping, an `Each`/list inserting a
        //      row, a `presence` entering — mounting into a parent that's
        //      already live in the window hierarchy.
        //
        // Without a pass here the new views keep their default LayoutParams
        // (0×0 at the origin), so the subtree is invisible — exactly the
        // "the `when`-mounted camera widget never appears" bug.
        //
        // Discriminate mid-build vs. post-mount the same way iOS does
        // (`parent.window != nil` — see project_ios_insert_layout_discriminator):
        // if the parent is already attached to the window, this is a
        // post-`finish()` dynamic insert → kick a pass. If it's a floating
        // parent still being built, defer to the upcoming `finish()` pass;
        // scheduling here would compute against a partial tree and cache wrong
        // sizes. The coalescing flag (LAYOUT_PASS_QUEUED) collapses a burst of
        // sibling inserts in one runloop turn into a single pass, so this is
        // cheap even when a list mounts many rows at once.
        let is_portal_parent = self.portal_instances.contains_key(&Self::node_key_of(parent));
        // `isAttachedToWindow()` is true exactly when the parent is live in the
        // window's view hierarchy — i.e. the initial build's `finish()` pass
        // already ran and this is a later dynamic mount. A floating parent
        // still being built returns false (defer to `finish()`).
        let parent_attached = with_env(|env| {
            env.call_method(parent.as_obj(), "isAttachedToWindow", "()Z", &[])
                .and_then(|v| v.z())
                .unwrap_or(false)
        });
        if crate::layout_policy::insert_needs_layout_pass(is_portal_parent, parent_attached) {
            crate::imp::scheduler::schedule_layout_pass_retry(0);
        }
    }

    fn install_touch_handler(
        &mut self,
        node: &Self::Node,
        handler: runtime_core::TouchHandler,
    ) {
        primitives::touch::install(self, node, handler)
    }

    fn claim_touch(
        &mut self,
        node: &Self::Node,
        _touch_id: runtime_core::TouchId,
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
        primitives::text::update_text(node, content);
        // `TextView.setText` repaints the glyphs itself, but Android
        // never tells Taffy the intrinsic content size changed — the
        // TextView's measure_fn (installed in `primitives::text::create`)
        // only re-runs when its Taffy node is marked dirty. Without this,
        // the cached measure from the previous content keeps winning:
        // a reactive `text(|| ...)` that grows (e.g. "" → "1") stays
        // clipped to its old frame — often 0×0, hence the "the number
        // never renders on select" symptom — and one that shrinks leaves
        // stale blank space. Mark the node dirty so it re-measures on the
        // next coalesced layout pass. Mirrors iOS `update_text` and the
        // sibling `update_text_area_value`.
        let layout = self.layout_for_view(node);
        self.layout.mark_dirty(layout);
        crate::imp::scheduler::schedule_layout_pass_retry(0);
    }

    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        let node = primitives::image::create(self, src, alt);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::Image));
        node
    }

    fn create_icon(
        &mut self,
        data: &runtime_core::primitives::icon::IconData,
        color: Option<&runtime_core::Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::icon::create(self, data, color);
        // Give the icon a Taffy intrinsic size. The ImageView's drawable
        // has a 24dp intrinsic size and `FIT_CENTER` scaling, but Taffy
        // overwrites the create-time LayoutParams during apply_frames, so
        // without a measure_fn the icon collapsed to 0 width in a flex
        // row (letting the sibling label overlap it) and stretched on the
        // cross axis. The generic `View.measure`-based measure_fn reports
        // the drawable's intrinsic 24dp (and an explicit style
        // width/height still wins); FIT_CENTER then scales the glyph to
        // whatever box Taffy assigns. Mirrors iOS `install_icon_measure`.
        install_external_measure_fn(self, &node);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::Image));
        node
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &runtime_core::Color) {
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
        easing: runtime_core::Easing,
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
        on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        secure: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // TODO(secure): wire `secure` to the EditText input type
        // (`PasswordTransformationMethod` / `TYPE_TEXT_VARIATION_PASSWORD`).
        // Stubbed for now so the backend matches the updated Backend trait —
        // part of the in-flight text-input rework, not this video change.
        let _ = secure;
        let node = primitives::text_input::create(self, initial_value, placeholder, on_change, on_key_down);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::TextField));
        node
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        primitives::text_input::update_value(node, value)
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        wrap: bool,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::text_input::create_multiline(self, initial_value, placeholder, wrap, on_change, on_key_down);
        // Intrinsic content sizing in wrap mode (only): give the EditText
        // a `View.measure`-based measure_fn so Taffy sizes it to the
        // height the wrapped text needs. With no pinned height it grows
        // to fit; a `max_height` clamps it (the EditText scrolls past);
        // a pinned `height` wins. The code-editor shape (`wrap == false`)
        // is a fixed-height horizontal scroller, so it gets no measure_fn
        // — mirrors iOS and web.
        if wrap {
            install_external_measure_fn(self, &node);
        }
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::TextArea));
        node
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        primitives::text_input::update_value(node, value);
        // The text changed, so the EditText's measured height changed,
        // but Android doesn't invalidate Taffy. Mark the node dirty so a
        // wrap-mode textarea's measure_fn re-runs on the next (coalesced)
        // layout pass; a code-mode textarea has no measure_fn, so this
        // just reproduces its style-given size. Mirrors iOS.
        let layout = self.layout_for_view(node);
        self.layout.mark_dirty(layout);
        crate::imp::scheduler::schedule_layout_pass_retry(0);
    }

    fn make_text_input_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::text_input::TextInputHandle {
        primitives::text_input::make_text_input_handle(node)
    }

    fn make_text_area_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::text_area::TextAreaHandle {
        primitives::text_input::make_text_area_handle(node)
    }

    fn create_toggle(&mut self, initial_value: bool, on_change: Rc<dyn Fn(bool)>, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        // `primitives::toggle::create` now installs an intrinsic-size
        // `measure_fn`, so it needs `&mut self` to reach Taffy. Without
        // the measure_fn the Switch was a 0×0 leaf in flex layout, the
        // surrounding column gave it no height, and the widget got
        // clipped behind the next sibling — visible as a missing
        // dark-mode toggle in the docs sidebar.
        let node = primitives::toggle::create(self, initial_value, on_change);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::Switch));
        node
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        primitives::toggle::update_value(node, value)
    }

    fn set_app_key_handler(
        &mut self,
        handler: Option<runtime_core::primitives::key::KeyDownHandler>,
    ) {
        keyboard::set_app_key_handler(self, handler);
    }

    fn set_app_background(&mut self, color: &runtime_core::Tokenized<runtime_core::Color>) {
        // Paint the activity window's DECOR view — NOT a content view — so the
        // app background fills the ENTIRE screen, behind any safe-area-inset
        // content and behind hidden system bars / the display cutout. Without
        // this, a full-screen (immersive) screen shows the decor view's default
        // black in the now-uncovered status/nav-bar strips and the cutout. The
        // framework calls this on theme install + swap (see
        // `style::set_app_background`), so the window tracks the theme bg.
        let Some(packed) = backend_android_core::helpers::parse_color(&color.value().0) else {
            return;
        };
        with_env(|env| {
            let decor = env
                .call_method(
                    &self.context.as_obj(),
                    "getWindow",
                    "()Landroid/view/Window;",
                    &[],
                )
                .ok()
                .and_then(|v| v.l().ok())
                .filter(|w| !w.is_null())
                .and_then(|w| {
                    env.call_method(&w, "getDecorView", "()Landroid/view/View;", &[])
                        .ok()
                        .and_then(|v| v.l().ok())
                });
            if let Some(decor) = decor {
                let _ = env.call_method(
                    &decor,
                    "setBackgroundColor",
                    "(I)V",
                    &[JValue::Int(packed)],
                );
            }
        });
    }

    fn apply_safe_area_padding(
        &mut self,
        node: &Self::Node,
        sides: runtime_core::SafeAreaSides,
    ) {
        let insets = self.platform_safe_area_insets();
        log::info!(
            "[safe-area] apply_safe_area_padding sides={:?} insets=(t={},r={},b={},l={})",
            sides, insets.top, insets.right, insets.bottom, insets.left
        );
        let top = if sides.contains(runtime_core::SafeAreaSides::TOP) {
            insets.top
        } else {
            0.0
        };
        let right = if sides.contains(runtime_core::SafeAreaSides::RIGHT) {
            insets.right
        } else {
            0.0
        };
        let bottom = if sides.contains(runtime_core::SafeAreaSides::BOTTOM) {
            insets.bottom
        } else {
            0.0
        };
        let left = if sides.contains(runtime_core::SafeAreaSides::LEFT) {
            insets.left
        } else {
            0.0
        };
        let layout_node = self.layout_for_view(node);
        self.layout
            .set_safe_area_extra(layout_node, top, right, bottom, left);
        crate::imp::scheduler::schedule_layout_pass_retry(0);
    }

    fn apply_scroll_view_safe_area_inset(
        &mut self,
        node: &Self::Node,
        sides: runtime_core::SafeAreaSides,
    ) {
        // For a ScrollView we apply the safe-area inset via Android's
        // native `setPadding(...)` + `setClipToPadding(false)` —
        // matches the documented behavior in `Backend::apply_scroll_view_safe_area_inset`
        // ("scroll surface bleeds edge-to-edge while content origin
        // is inset"). Going through `setPadding` rather than Taffy
        // padding here is intentional: the inner FrameLayout that
        // holds the children is `MATCH_PARENT`-sized to the parent's
        // *content area*, so when the ScrollView's content area
        // shrinks the inner shrinks with it and the last child (the
        // sidebar's theme toggle in the docs example) gets pushed up
        // out from under the gesture pill. The Taffy `set_safe_area_extra`
        // path used by `apply_safe_area_padding` reaches the outer's
        // padding fields but doesn't fall through to the inner's
        // `MATCH_PARENT` measurement, so the inner stays full-height
        // and the toggle still ends up clipped.
        let insets = self.platform_safe_area_insets();
        let top = if sides.contains(runtime_core::SafeAreaSides::TOP) { insets.top } else { 0.0 };
        let right = if sides.contains(runtime_core::SafeAreaSides::RIGHT) { insets.right } else { 0.0 };
        let bottom = if sides.contains(runtime_core::SafeAreaSides::BOTTOM) { insets.bottom } else { 0.0 };
        let left = if sides.contains(runtime_core::SafeAreaSides::LEFT) { insets.left } else { 0.0 };
        with_env(|env| {
            let view_obj = node.as_obj();
            let density = density_of(env, &view_obj).unwrap_or(1.0);
            let _ = env.call_method(
                &view_obj,
                "setPadding",
                "(IIII)V",
                &[
                    jni::objects::JValue::Int((left * density).round() as i32),
                    jni::objects::JValue::Int((top * density).round() as i32),
                    jni::objects::JValue::Int((right * density).round() as i32),
                    jni::objects::JValue::Int((bottom * density).round() as i32),
                ],
            );
            // Children that scroll past the padded edge should still
            // render — matches iOS `UIScrollView`'s behavior with
            // `contentInsetAdjustmentBehavior = .always`. Without
            // this the scroll thumb and overscroll hint clip at the
            // padding boundary.
            let _ = env.call_method(
                &view_obj,
                "setClipToPadding",
                "(Z)V",
                &[jni::objects::JValue::Bool(0)],
            );
        });
        crate::imp::scheduler::schedule_layout_pass_retry(0);
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::scroll_view::create(self, horizontal);

        // Wire `on_scroll` via the shared Kotlin listener. The
        // `setOnScrollChangeListener` slot is also used by
        // `Position::Sticky` children; both subsystems install via
        // [`sticky::ensure_scroll_listener`] which is idempotent.
        // Scroll positions reported here are converted from device
        // pixels (Android's native unit on `View.getScrollY()`) to
        // dp via the scroll view's display density, so author code
        // sees the same coordinate space across every backend.
        if let Some(cb) = on_scroll {
            let scroll_key = Self::node_key(&node);
            // Density read up front \u{2014} reading per-event would
            // hit JNI on every scroll tick.
            let density = with_env(|env| density_of(env, &node.as_obj()).unwrap_or(1.0));
            let density = if density <= 0.0 { 1.0 } else { density };
            let wrapped: Rc<dyn Fn(f32, f32)> = Rc::new(move |x_px, y_px| {
                cb(x_px / density, y_px / density);
            });
            self.scroll_observers.insert(scroll_key, wrapped);
            let node_clone = node.clone();
            with_env(|env| {
                sticky::ensure_scroll_listener(
                    env,
                    &mut self.scroll_listeners,
                    &node_clone,
                    scroll_key,
                );
            });
        }

        // ScrollView has no first-class role — Android handles scroll
        // chrome itself. apply() still writes author-set label / hint
        // / identifier when present.
        a11y::apply(&node, a11y, None);
        node
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::slider::create(self, initial_value, min, max, step, on_change);
        // Same 0×0-leaf hazard as Button: a bare `SeekBar` needs its intrinsic
        // `measure()` hooked into Taffy or it collapses to zero height.
        primitives::measure::install_intrinsic_measure(self, &node);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::Slider));
        node
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        primitives::slider::update_value(node, value)
    }

    fn create_virtualizer(
        &mut self,
        callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::virtualizer::create(self, callbacks, overscan, horizontal);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::List));
        node
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        primitives::virtualizer::data_changed(node)
    }

    fn create_activity_indicator(
        &mut self,
        size: runtime_core::primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&runtime_core::Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::activity_indicator::create(self, size, color);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::Spinner));
        node
    }


    // ------------------------------------------------------------------
    // Navigator — unified path for SDK-supplied navigator kinds.
    //
    // `create_navigator` resolves the SDK-registered factory, runs
    // `init`, and stashes the returned handler on
    // `nav_handler_instances`. Subsequent dispatch
    // (`attach_initial` / `release` / `make_handle` /
    // `apply_slot_style`) looks the handler up by node key and
    // forwards through it; the handler in turn drives the
    // backend's existing per-kind inherent helpers
    // (`stack_navigator_attach_initial`, `apply_navigator_header_style`,
    // …) as appropriate.
    // ------------------------------------------------------------------

    fn create_navigator(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        presentation: Rc<dyn std::any::Any>,
        host: runtime_core::NavigatorHost<Self::Node>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let factory = self
            .navigator_handlers
            .get(type_id)
            .unwrap_or_else(|| {
                panic!(
                    "AndroidBackend::create_navigator: navigator kind '{}' \
                     is not registered. Did the app forget to call \
                     `<navigator-sdk>::register(&mut backend)` during bootstrap?",
                    type_name
                )
            });
        let mut handler = factory();
        let node = handler.init(self, host, presentation);
        // Stash the handler keyed by the container's node key so
        // subsequent dispatch routes through the SDK handler instead
        // of through a kind switch. The handler internally retains
        // its container `GlobalRef` so its post-init methods can call
        // back into the backend's legacy per-kind helpers.
        self.nav_handler_instances.insert(
            AndroidBackend::node_key_of(&node),
            std::rc::Rc::new(std::cell::RefCell::new(handler)),
        );
        node
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn std::any::Any>,
    ) {
        let handler = self
            .nav_handler_instances
            .get(&AndroidBackend::node_key_of(navigator))
            .cloned();
        let Some(handler) = handler else { return };
        handler.borrow_mut().attach_initial(self, screen, scope_id, options);
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        let key = AndroidBackend::node_key_of(node);
        let handler = self.nav_handler_instances.remove(&key);
        let Some(handler) = handler else { return };
        handler.borrow_mut().release(self);
    }

    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::NavigatorHandle {
        let handler = self
            .nav_handler_instances
            .get(&AndroidBackend::node_key_of(node))
            .cloned();
        match handler {
            Some(h) => h.borrow().make_handle(),
            None => runtime_core::NavigatorHandle::new(Rc::new(()), &NOOP_NAV_OPS),
        }
    }

    fn apply_navigator_slot_style(
        &mut self,
        navigator: &Self::Node,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let handler = self
            .nav_handler_instances
            .get(&AndroidBackend::node_key_of(navigator))
            .cloned();
        let Some(handler) = handler else { return };
        handler.borrow_mut().apply_slot_style(self, slot, style);
    }

    fn create_graphics(
        &mut self,
        on_ready: runtime_core::primitives::graphics::OnReady,
        on_resize: runtime_core::primitives::graphics::OnResize,
        on_lost: runtime_core::primitives::graphics::OnLost,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::graphics::create(self, on_ready, on_resize, on_lost);
        // Graphics surfaces are GPU-rendered content with no inherent
        // a11y role; authors opt in via props.role / props.label.
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        primitives::graphics::release(self, node)
    }

    fn make_graphics_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::graphics::GraphicsHandle {
        primitives::graphics::make_handle(node)
    }

    fn create_portal(
        &mut self,
        target: runtime_core::primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::overlay::create(self, target, on_dismiss, trap_focus);
        // Portal container is transparent — author sets role
        // explicitly (Dialog / AlertDialog / Drawer / Popover) via
        // props.role; we don't infer one here.
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_portal(&mut self, node: &Self::Node) {
        primitives::overlay::release(self, node)
    }

    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Look up the handler; clone the Rc so we can drop the registry
        // borrow before calling the handler (which itself needs
        // `&mut self`).
        let node = if let Some(handler) = self.external_handlers.get(type_id) {
            handler(payload, self)
        } else {
            // No handler registered → render a placeholder TextView so
            // the dev/user sees that an SDK binding is missing on
            // Android rather than a silent hole.
            // `has_external::<T>()` is the supported way to render
            // custom degradation in user space.
            external_placeholder_view(self, type_name)
        };
        // External primitives carry no inherent role — third-party
        // SDK authors set the right one via props.role.
        a11y::apply(&node, a11y, None);
        // Install a Taffy measure_fn that asks the underlying Android
        // View for its preferred size via `View.measure(spec, spec)`.
        // Without this, every External view collapses to 0×0 in the
        // flex layout — the framework's text/button primitives install
        // measure_fns themselves, but External handlers go through a
        // generic path that doesn't know the concrete widget type. A
        // generic `View.measure` works because every Android View
        // implements it; container views (FrameLayout, scroll views)
        // measure their children internally and report the right size.
        install_external_measure_fn(self, &node);
        node
    }

    fn release_external(&mut self, node: &Self::Node) {
        // Detached window root (screen_recorder private layer):
        // `WindowManager.removeView` its overlay window so it stops
        // compositing when the layer unmounts.
        // `release_private_layer_window` returns early for any node that
        // isn't a registered detached root, so this is a cheap no-op for
        // every other external. Future SDK leaves that keep instance
        // state (cached callback pointers, GL contexts) would also clean
        // up here, keyed by `node_key` like animations/navigators do.
        if self
            .detached_window_roots
            .contains_key(&Self::node_key_of(node))
        {
            self.release_private_layer_window(node);
        }
    }

    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
        primitives::button::make_handle(node)
    }

    /// Override the framework default so the typed handle carries the
    /// underlying `GlobalRef`. Author-level animation drivers downcast
    /// `view_handle.as_any()` to `GlobalRef` and reach the backend
    /// through `set_animated_f32` / `set_animated_color`; without this
    /// override the handle stores `Rc<()>` and the downcast fails.
    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        runtime_core::ViewHandle::new(Rc::new(node.clone()), &ANDROID_VIEW_OPS)
    }

    /// See [`Self::make_view_handle`]. Same plumbing for `TextHandle`
    /// so the welcome example's per-frame `setTextColor` write can
    /// reach a `TextView` (rather than `setTintColor`-equivalent on a
    /// generic wrapper) and animate `color` end-to-end.
    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        runtime_core::TextHandle::new(Rc::new(node.clone()), &ANDROID_TEXT_OPS)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        // Drop any sticky bookkeeping for the entire subtree we're
        // about to remove BEFORE the native `removeAllViews` call.
        // Walk recursively so a sticky child nested inside an
        // intermediate View also deregisters (otherwise its
        // registry entry survives the unmount and the scroll
        // listener keeps trying to apply translations to a
        // detached view). If any descendant IS a scroll view,
        // deregister it as a scroll-host so its descendants'
        // sticky bookkeeping is cleaned up too. Mirrors iOS's
        // `walk_and_deregister`.
        with_env(|env| {
            let sv_class = env.find_class("android/widget/ScrollView").ok();
            let hsv_class = env.find_class("android/widget/HorizontalScrollView").ok();
            walk_and_deregister_sticky(
                env,
                &node.as_obj(),
                &mut self.sticky_registry,
                &mut self.scroll_listeners,
                &mut self.scroll_observers,
                &mut self.pending_sticky,
                sv_class.as_ref(),
                hsv_class.as_ref(),
            );
        });
        primitives::view::clear_children(self, node)
    }

    fn register_asset(
        &mut self,
        id: runtime_core::AssetId,
        kind: runtime_core::AssetTag,
        source: &runtime_core::AssetSource,
    ) {
        // Only the font branch needs JNI today; images on Android go
        // through `create_image(src)` directly. Future image / video
        // caches would chain here the same way the iOS backend does.
        if kind != runtime_core::AssetTag::Font {
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
        id: runtime_core::AssetId,
        kind: runtime_core::AssetTag,
    ) {
        self.font_registry.unregister_asset(id, kind);
    }

    fn register_typeface(
        &mut self,
        id: runtime_core::assets::TypefaceId,
        family_name: &str,
        faces: &[runtime_core::assets::TypefaceFace],
        fallback: runtime_core::assets::SystemFallback,
    ) {
        self.font_registry
            .register_typeface(id, family_name, faces, fallback);
    }

    fn unregister_typeface(&mut self, id: runtime_core::assets::TypefaceId) {
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
        // Strip padding from Text leaves: padding on a Text node has
        // no children to shift and the renderer (TextView) doesn't
        // honor it natively in a way that's portable. Authors wrap a
        // Text in a styled View when they want spacing around it.
        // Mirror logic in iOS backend for IosNode::Label.
        let is_text_view = with_env(|env| {
            env.find_class("android/widget/TextView")
                .ok()
                .and_then(|c| env.is_instance_of(&node.as_obj(), &c).ok())
                .unwrap_or(false)
        });
        if is_text_view {
            let mut text_style: runtime_core::StyleRules = (**style).clone();
            text_style.padding_left = None;
            text_style.padding_right = None;
            text_style.padding_top = None;
            text_style.padding_bottom = None;
            self.layout.set_style(layout_node, &text_style);
        } else {
            self.layout.set_style(layout_node, style);
        }

        // Position::Sticky → register against the enclosing
        // ScrollView so the per-scroll-event sticky listener pins
        // this view when scrolled past the threshold. Any other
        // Position value (or `None`) must first deregister so a
        // previous Sticky → Relative transition cleans up its
        // registry entry + clears the carried translationY. See
        // `sticky.rs`.
        //
        // The walker fires `apply_style` (via `attach_style`)
        // BEFORE the parent's `insert(parent, child)` call. At that
        // moment the child is still a floating View with no parent
        // chain, so `sticky::register`'s `getParent` walk can't
        // find the scroll ancestor yet. We try anyway (succeeds
        // for re-applies on already-mounted views — stylesheet
        // variant flips, theme changes) and fall back to recording
        // in `pending_sticky` for the first-mount case. `insert`
        // consults `pending_sticky` after attaching the subtree
        // and promotes any entries it can now resolve.
        match style.position {
            Some(runtime_core::Position::Sticky) => {
                let threshold_top = style
                    .top
                    .as_ref()
                    .map(|t| match t.resolve() {
                        runtime_core::Length::Px(v) => v,
                        // Percent / Auto for sticky's pin offset
                        // isn't meaningful — same rationale as
                        // iOS's `_ => 0.0` fallthrough.
                        _ => 0.0,
                    })
                    .unwrap_or(0.0);
                let registered = with_env(|env| {
                    sticky::register(
                        env,
                        &mut self.sticky_registry,
                        &mut self.scroll_listeners,
                        node,
                        threshold_top,
                        &self.scroll_observers,
                    )
                });
                if !registered {
                    // No enclosing scroll view *yet*. Could be a
                    // first-mount (insert hasn't run) or genuinely
                    // not in a scroll view. Record either way;
                    // `insert` retries and `clear_children` /
                    // `on_node_unstyled` clear the entry.
                    self.pending_sticky.insert(key, threshold_top);
                }
            }
            _ => {
                with_env(|env| {
                    sticky::deregister(
                        env,
                        &mut self.sticky_registry,
                        &mut self.scroll_listeners,
                        node,
                        &self.scroll_observers,
                    );
                });
                self.pending_sticky.remove(&key);
            }
        }

        // A reactive style change can alter layout-affecting properties
        // (width / height / position / inset / flex). We mirrored them into
        // Taffy via `set_style` above, but that only updates the cached style —
        // the frame isn't recomputed until a layout pass runs. For an
        // already-attached view the initial build's `finish()` pass is long
        // gone, so without kicking a pass the new geometry never reaches the
        // native `LayoutParams`: e.g. a reactive box that grows from 0×0 to its
        // real size when a signal flips stays invisible. Kick a coalesced pass.
        //
        // Gate on window-attachment exactly like `insert`
        // (`layout_policy::insert_needs_layout_pass`): a mid-build `apply_style`
        // on a floating view defers to the upcoming `finish()` pass; scheduling
        // then would compute against a partial tree. The coalescing flag
        // (LAYOUT_PASS_QUEUED) collapses the per-node `apply_style` storm of a
        // theme switch into a single pass.
        let attached = with_env(|env| {
            env.call_method(node.as_obj(), "isAttachedToWindow", "()Z", &[])
                .and_then(|v| v.z())
                .unwrap_or(false)
        });
        if attached {
            crate::imp::scheduler::schedule_layout_pass_retry(0);
        }
    }

    fn set_animated_f32(
        &mut self,
        node: &Self::Node,
        prop: runtime_core::animation::AnimProp,
        value: f32,
    ) {
        // Android View has separate native properties for each
        // transform component (translationX/Y, scaleX/Y, rotation)
        // plus alpha — no composition needed. Each AnimProp maps
        // directly to one setter via JNI.
        use runtime_core::animation::AnimProp as P;
        let (method, sig) = match prop {
            P::Opacity => ("setAlpha", "(F)V"),
            P::TranslateX => ("setTranslationX", "(F)V"),
            P::TranslateY => ("setTranslationY", "(F)V"),
            P::Scale | P::ScaleX => ("setScaleX", "(F)V"),
            P::ScaleY => ("setScaleY", "(F)V"),
            P::RotateZ => ("setRotation", "(F)V"),
            // `setTranslationZ` lifts the view above its siblings in
            // the parent's draw order — same role as `style.zIndex`
            // on web / `layer.zPosition` on iOS. Takes device pixels;
            // the dp-to-px conversion below handles the unit.
            P::ZIndex => ("setTranslationZ", "(F)V"),
            // `MaxHeight` would need the LayoutParams height path +
            // a ValueAnimator to drive layout-affecting properties
            // smoothly. For now, snap-stub (no-op) — matches the
            // iOS/gpu MaxHeight TODO. Authors hitting this on Android
            // get a non-animated open/close until the native-animator
            // plumbing lands.
            P::MaxHeight => return,
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
            let out_value = if matches!(prop, P::TranslateX | P::TranslateY | P::ZIndex) {
                // Translates land in device px (so the visual offset
                // matches what `style.transform: translate(<dp>px)`
                // would have produced on web). `setTranslationZ`
                // takes device pixels too — same conversion keeps
                // the relative ordering scale-stable across
                // densities.
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
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        use runtime_core::animation::AnimProp as P;
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
            | P::RotateZ
            | P::ZIndex
            | P::MaxHeight => {}
        }
    }

    /// `Backend::apply_presence` for Android.
    ///
    /// Maps a [`PresenceState`] (opacity + 2D translate + uniform
    /// scale) onto the node's `View`. Missing fields resolve to the
    /// resting identity (opacity 1.0, translate 0, scale 1.0) — the
    /// presence walker only ever supplies the enter/exit/rest states,
    /// so writing every property each call keeps the view consistent
    /// (a partial state leaves the unwritten props at identity, never
    /// at a stale value).
    ///
    /// Translates arrive in dp (the same unit as Taffy frames);
    /// Android's `setTranslationX/Y` and the ViewPropertyAnimator's
    /// `translationX/Y` take DEVICE PIXELS, so we convert via the
    /// view's density — see project memory
    /// `project_android_setTranslation_device_px`. Opacity and scale
    /// are unitless.
    ///
    /// `transition`:
    /// - `Some((ms, easing))` → drive a `ViewPropertyAnimator`
    ///   (`view.animate()…start()`) so Android interpolates from the
    ///   current value over `ms` with the mapped `Interpolator`. The
    ///   easing mapping reuses [`animation::build_interpolator`] so
    ///   presence and `AnimatedValue` agree on every curve.
    /// - `None` → call the per-property setters directly (instant
    ///   snap), matching the pre-paint enter setup and web's snap path.
    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: runtime_core::PresenceState,
        transition: Option<(u32, runtime_core::Easing)>,
    ) {
        let alpha = state.opacity.unwrap_or(1.0);
        let tx_dp = state.translate_x.unwrap_or(0.0);
        let ty_dp = state.translate_y.unwrap_or(0.0);
        let scale = state.scale.unwrap_or(1.0);

        with_env(|env| {
            let obj = node.as_obj();
            // dp → device px for the two translate axes (memory:
            // project_android_setTranslation_device_px).
            let tx_px = backend_android_core::helpers::dp_to_px(env, obj, tx_dp) as f32;
            let ty_px = backend_android_core::helpers::dp_to_px(env, obj, ty_dp) as f32;

            match transition {
                None => {
                    let mut f = |m: &str, v: f32| {
                        let _ = env.call_method(obj, m, "(F)V", &[JValue::Float(v)]);
                    };
                    f("setAlpha", alpha);
                    f("setTranslationX", tx_px);
                    f("setTranslationY", ty_px);
                    f("setScaleX", scale);
                    f("setScaleY", scale);
                }
                Some((duration_ms, easing)) => {
                    // ViewPropertyAnimator: `view.animate()` returns it,
                    // each property setter returns `this` so we chain,
                    // then `setDuration`/`setInterpolator`/`start`.
                    let animator = match env
                        .call_method(obj, "animate", "()Landroid/view/ViewPropertyAnimator;", &[])
                    {
                        Ok(v) => match v.l() {
                            Ok(o) => o,
                            Err(_) => return,
                        },
                        Err(_) => return,
                    };
                    // Each of these returns the same ViewPropertyAnimator;
                    // ignore the returned handle and keep using `animator`.
                    let vpa_sig = "(F)Landroid/view/ViewPropertyAnimator;";
                    let _ = env.call_method(&animator, "alpha", vpa_sig, &[JValue::Float(alpha)]);
                    let _ = env.call_method(
                        &animator,
                        "translationX",
                        vpa_sig,
                        &[JValue::Float(tx_px)],
                    );
                    let _ = env.call_method(
                        &animator,
                        "translationY",
                        vpa_sig,
                        &[JValue::Float(ty_px)],
                    );
                    let _ =
                        env.call_method(&animator, "scaleX", vpa_sig, &[JValue::Float(scale)]);
                    let _ =
                        env.call_method(&animator, "scaleY", vpa_sig, &[JValue::Float(scale)]);
                    let _ = env.call_method(
                        &animator,
                        "setDuration",
                        "(J)Landroid/view/ViewPropertyAnimator;",
                        &[JValue::Long(duration_ms as i64)],
                    );
                    if let Some(interp) = animation::build_interpolator(env, easing) {
                        // NB: `ViewPropertyAnimator.setInterpolator` returns
                        // `ViewPropertyAnimator` (not void, unlike
                        // `ValueAnimator.setInterpolator`). Using `()V`
                        // here is a NoSuchMethodError → JNI abort.
                        let _ = env.call_method(
                            &animator,
                            "setInterpolator",
                            "(Landroid/animation/TimeInterpolator;)Landroid/view/ViewPropertyAnimator;",
                            &[JValue::Object(&interp)],
                        );
                    }
                    let _ = env.call_method(&animator, "start", "()V", &[]);
                }
            }
        });
    }

    fn frame(&self, node: &Self::Node) -> Option<runtime_core::primitives::portal::ViewportRect> {
        // Parent-relative rect in dp — matches iOS's `Backend::frame`
        // impl. Framework portal / anchoring code consults this; the
        // ViewHandle-side analog used by author code lives on
        // `AndroidViewOps::frame` (same body, different trait).
        <AndroidViewOps as runtime_core::ViewOps>::frame(
            &ANDROID_VIEW_OPS,
            node as &dyn std::any::Any,
        )
    }

    fn device_frame(
        &self,
        node: &Self::Node,
    ) -> Option<runtime_core::primitives::portal::ViewportRect> {
        // Physical screen-pixel rect for OS-level input injection
        // (`adb shell input tap`). `view_screen_rect` reuses the same
        // `getLocationOnScreen` path overlay anchoring already trusts —
        // origin = display top-left (status bar included), units = real
        // device pixels, which is *exactly* adb's tap coordinate space.
        // No dp→px conversion on the host: the density is applied here,
        // on-device, where it's authoritative.
        let rect = view_rect::view_screen_rect(node);
        // `view_screen_rect` returns a zero rect for not-yet-laid-out
        // views; report that as "no frame" so the driver doesn't tap
        // (0,0) blindly.
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return None;
        }
        Some(rect)
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        // Drop any sticky bookkeeping for this node. Covers both
        // "I'm a sticky child being detached" (deregister from
        // whatever scroll view owns me) and "I'm a scroll view
        // being detached" (deregister my whole entry).
        let node_key = Self::node_key(node);
        // Drop any user-supplied `on_scroll` callback for this node
        // BEFORE asking the sticky deregister path to release the
        // shared listener \u{2014} otherwise the listener slot stays
        // pinned by the on_scroll registry.
        self.scroll_observers.remove(&node_key);
        with_env(|env| {
            sticky::deregister(
                env,
                &mut self.sticky_registry,
                &mut self.scroll_listeners,
                node,
                &self.scroll_observers,
            );
            sticky::deregister_scroll_view(
                env,
                &mut self.sticky_registry,
                &mut self.scroll_listeners,
                node,
                &self.scroll_observers,
            );
        });
        self.pending_sticky.remove(&node_key);
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
        setter: Rc<dyn Fn(runtime_core::StateBits, bool)>,
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

    // =================================================================
    // Accessibility
    // =================================================================
    //
    // `dump_accessibility_tree` is intentionally left at its default
    // (returns `None`). TalkBack walks each `View`'s
    // `contentDescription` / `setAccessibilityLiveRegion` /
    // `AccessibilityNodeInfo` directly — there's no parallel
    // semantics tree to dump.

    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y_props: &runtime_core::accessibility::AccessibilityProps,
        inferred_role: Option<runtime_core::accessibility::Role>,
    ) {
        a11y::apply(node, a11y_props, inferred_role);
    }

    fn announce_for_accessibility(
        &mut self,
        msg: &str,
        priority: runtime_core::accessibility::LiveRegionPriority,
    ) {
        // Routed through the backend's host root view —
        // `announceForAccessibility` exists on `View`, and the host
        // root is the most reliable target (always attached, always
        // visible to TalkBack). Polite vs Assertive both map to the
        // same call on Android; the priority is observed for
        // cross-backend parity but not first-class in the platform
        // API.
        let root = self.root.clone();
        with_env(|env| {
            a11y::announce(env, &root.as_obj(), msg, priority);
        });
    }

    fn finish(&mut self, root: Self::Node) {
        // Idempotent: in runtime-server mode, each reconnect / re-snapshot from
        // the dev-server replays the full command stream, which
        // includes the `Finish` that drives this method. The
        // `WireBackend` (in `dev-client`) is idempotent for tree
        // commands, so `root` here is the SAME native `UIView`-/
        // `View`-equivalent as the previous snapshot. Calling
        // `addView` on a child whose `getParent()` is non-null
        // throws `IllegalStateException`, which used to surface as
        // a JNI panic and (before the panic hook + exception clear
        // in the runtime-server shell) crashed the process outright.
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

    /// Backend-trait entry point the runtime-server shell uses to drive layout
    /// when the deferred `schedule_layout_pass_retry` path's
    /// `ANDROID_BACKEND_SELF.upgrade()` returns `None` (runtime-server mode
    /// owns the backend by-value inside `RuntimeServerClient`, so the global
    /// self-ref is never installed). Delegates to the existing
    /// public [`AndroidBackend::run_layout`] wrapper around
    /// `run_layout_pass`.
    fn run_layout(&mut self) {
        AndroidBackend::run_layout(self);
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

// Legacy nav helpers removed — every kind-specific navigator
// implementation lives in `android-navigator-helpers` as of the
// substrate refactor. The Backend trait's `create_navigator` /
// `navigator_attach_initial` / `release_navigator` /
// `make_navigator_handle` / `apply_navigator_slot_style` methods route
// through SDK handlers (see `nav_handler_instances`) which in turn
// call into `android-navigator-helpers`.
