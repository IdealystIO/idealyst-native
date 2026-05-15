//! Android-specific implementation of [`crate::AndroidBackend`].
//!
//! This module is the parent of every per-primitive file and shared
//! helper. The `Backend` impl at the bottom delegates each method to
//! the matching submodule.

mod animation;
mod callbacks;
mod helpers;
mod jni_exports;
mod primitives;
mod style;

use framework_core::{Backend, ButtonHandle, StyleRules};
use jni::objects::{GlobalRef, JValue};
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
pub(crate) fn with_env<R>(f: impl FnOnce(&mut JNIEnv) -> R) -> R {
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
    pub(crate) last_bg: Option<i32>,         // packed ARGB
    pub(crate) last_text_color: Option<i32>, // packed ARGB
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
    /// ScrollView outer→inner mapping. Keyed by the outer
    /// (framework-visible) ScrollView's raw `JObject*` pointer; value
    /// is a `GlobalRef` to its inner LinearLayout, where child
    /// inserts actually land. Populated by `scroll_view::create`,
    /// cleared in `on_node_unstyled` (most ScrollViews are styled;
    /// for unstyled instances the entry persists for the backend's
    /// lifetime — small and bounded).
    pub(crate) scroll_view_inner: HashMap<usize, GlobalRef>,
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
            scroll_view_inner: HashMap::new(),
        }
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
// Backend trait impl. Each method delegates to the matching primitive
// module (or to one of the style/helpers helpers). Keep this thin —
// anything substantial belongs in the primitive's file.
// ---------------------------------------------------------------------------

impl Backend for AndroidBackend {
    type Node = GlobalRef;

    fn create_view(&mut self) -> Self::Node {
        primitives::view::create(self)
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
        primitives::text::create(self, content)
    }

    fn create_button(&mut self, label: &str, on_click: Rc<dyn Fn()>) -> Self::Node {
        primitives::button::create(self, label, on_click)
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        primitives::view::insert(self, parent, child)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        primitives::text::update_text(node, content)
    }

    fn create_image(&mut self, src: &str, alt: Option<&str>) -> Self::Node {
        primitives::image::create(self, src, alt)
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
    ) -> Self::Node {
        primitives::text_input::create(self, initial_value, placeholder, on_change)
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        primitives::text_input::update_value(node, value)
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

    fn create_web_view(&mut self, url: &str) -> Self::Node {
        primitives::web_view::create(self, url)
    }

    fn update_web_view_url(&mut self, node: &Self::Node, url: &str) {
        primitives::web_view::update_url(node, url)
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

    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
        primitives::button::make_handle(node)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        primitives::view::clear_children(self, node)
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let key = Self::node_key(node);
        // Lazy-create per-node state on first apply.
        let state = self.anim_state.entry(key).or_default();
        with_env(|env| {
            style::apply_rules(env, node, state, style);
        });
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
