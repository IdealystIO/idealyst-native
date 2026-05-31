//! TabNavigator + DrawerNavigator on Android.
//!
//! These navigator kinds don't use `RustNavigator` / `FragmentManager`
//! — they're simpler. Each one builds a small native subtree described
//! below; the *body* region holds exactly one child View (the
//! currently-active screen) which swaps on Select.
//!
//! # Per-kind shapes
//!
//! - **TabNavigator**: navigator node is just a body `FrameLayout`.
//!   Tab chrome (a tab bar) is the author's responsibility via
//!   `.layout(...)`.
//!
//! - **DrawerNavigator**: navigator node is a
//!   `RustExactFrameLayout` (a wrapper that measures its child with
//!   EXACTLY mode — required by `DrawerLayout.onMeasure`) containing
//!   a `RustDrawerLayout`. The DrawerLayout's two children are:
//!     1. A body `LinearLayout` (vertical) that holds a per-screen
//!        Toolbar + the active screen's native node.
//!     2. The author's sidebar subtree (attached separately via
//!        [`attach_sidebar`]).
//!
//! # Why no FragmentManager
//!
//! Nesting `RustNavigator` would put the drawer's screens on the
//! *same* activity-level back stack as the root stack navigator —
//! tapping a drawer item would push a fragment that the system Back
//! button would later pop, conflicting with stack-navigator
//! semantics. View-swap avoids the conflict entirely: drawer/tab
//! selections never touch the back stack.

use crate::{
    node_key, AndroidDrawerCallbacks, AndroidNavCallbacks, AndroidScreenOptions, AndroidTabCallbacks,
    DrawerCmd, DrawerSide, MountPolicy,
};
use backend_android_core::helpers::apply_default_layout_params;
use backend_android::{with_jni_env, AndroidBackend, HeaderButtonCallback};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::jlong;
use runtime_core::primitives::navigator::{
    MountResult, NavCommand, NavigatorControl, NavigatorHandle, NavigatorOps,
};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

// =============================================================================
// Per-instance state
// =============================================================================

/// Leaked-box payload the `RustDrawerLayout`'s JNI callbacks
/// (`nativeOnDrawerOpened` / `nativeOnDrawerClosed`) dereference to
/// flip the framework's `is_open` signal + fire the `open_changed`
/// callback. The pointer is freed in [`release`] when the navigator's
/// enclosing scope drops.
struct DrawerListenerBox {
    is_open: runtime_core::Signal<bool>,
    open_changed: Rc<dyn Fn(bool)>,
}

/// Per-kind metadata stashed on the [`TabDrawerInstance`]. Distinguishes
/// tab navigators (just a body FrameLayout, no drawer state) from
/// drawer navigators (RustDrawerLayout with attached listener box).
enum DrawerKind {
    /// Tab navigator — no drawer commands, no listener pointer.
    Tab,
    /// Drawer navigator. `drawer_view` is the actual `RustDrawerLayout`
    /// instance — drawer commands and `attach_sidebar` target this
    /// rather than the wrapper `outer`. `listener_ptr` is freed in
    /// [`release`].
    Drawer {
        drawer_view: GlobalRef,
        listener_ptr: jlong,
        #[allow(dead_code)]
        swipe_to_open: bool,
        /// Author-configured drawer width in dp (from
        /// `.drawer_width(N)` on the SDK builder). Forwarded to
        /// `RustDrawerLayout.attachDrawer(view, widthDp)` so the
        /// drawer panel takes exactly this width instead of
        /// DrawerLayout's `screen - 56dp` fallback. `0.0` means
        /// "use WRAP_CONTENT" (legacy behavior).
        drawer_width: f32,
    },
}

/// A screen tracked across switches. Mirrors iOS's `MountedScreen`:
/// for persistent policies the view + toolbar stay attached to `body`
/// and visibility flips via `setVisibility(GONE)`; for `LazyDisposing`
/// the entry is dropped from the map on blur and the view +
/// scope are released. `effective_policy` is the per-screen
/// override (read from `AndroidScreenOptions::mount_policy`) or the
/// navigator-global fallback.
pub(crate) struct MountedScreen {
    pub(crate) view: GlobalRef,
    pub(crate) toolbar: Option<GlobalRef>,
    pub(crate) scope_id: u64,
    pub(crate) options: AndroidScreenOptions,
    pub(crate) effective_policy: MountPolicy,
}

/// Per-instance state for a tab or drawer navigator. The `body` is a
/// `FrameLayout`/`LinearLayout` that holds one or more cached screen
/// subtrees (one per LazyPersistent/EagerPersistent route ever
/// visited; one for the active LazyDisposing route). `outer` is the
/// framework-visible navigator node — for tabs this is the same as
/// `body`; for drawer this is the
/// `RustExactFrameLayout`/`RustDrawerLayout` chain.
pub(crate) struct TabDrawerInstance {
    /// The framework-visible navigator container.
    #[allow(dead_code)]
    outer: GlobalRef,
    /// The FrameLayout/LinearLayout that holds the active screen
    /// (and cached persistent screens hidden via `View.GONE`).
    body: GlobalRef,
    /// Activity context for building per-screen Toolbars on swap.
    context: GlobalRef,
    /// Cached screens keyed by route name. Disposing screens stay in
    /// the cache only while they are the active screen; they are
    /// removed (view + scope released) on the next blur.
    mounted: HashMap<&'static str, MountedScreen>,
    /// The currently-active route. Used by [`swap_body`] to look up
    /// the leaving screen's cached `effective_policy` so it knows
    /// whether to hide (Persistent) or dispose (LazyDisposing).
    current_route: Option<&'static str>,
    /// Navigator-global default policy. Per-screen
    /// `AndroidScreenOptions::mount_policy` overrides this when set.
    nav_global_policy: MountPolicy,
    /// `nav_state.active_route` signal, snapshot of the navigator's
    /// active route name. Used by `attach_initial` to key the first
    /// mounted screen in the cache (before `swap_body` has run).
    active_route_sig: runtime_core::Signal<&'static str>,
    /// Release the previous scope on swap.
    release_screen: Rc<dyn Fn(u64)>,
    /// Mount the next screen on `Select`.
    mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<GlobalRef>>,
    /// Per-kind metadata. Drawer carries the leaked listener pointer.
    kind: DrawerKind,
}

pub(crate) struct TabDrawerEntry {
    pub(crate) instance: Rc<RefCell<TabDrawerInstance>>,
    pub(crate) control: Rc<NavigatorControl>,
}

thread_local! {
    pub(crate) static TAB_DRAWER_INSTANCES: RefCell<HashMap<usize, TabDrawerEntry>> =
        RefCell::new(HashMap::new());
}

// =============================================================================
// Tab navigator
// =============================================================================

/// Create a tab navigator. Returns a `FrameLayout` GlobalRef (the body
/// — tabs don't have a sidebar at this layer).
pub(crate) fn create_tab(
    backend: &mut AndroidBackend,
    callbacks: AndroidTabCallbacks,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    let mount_policy = callbacks.mount_policy;
    let AndroidTabCallbacks { navigator, .. } = callbacks;
    let body = make_frame_layout(backend);
    install_instance(
        backend,
        navigator,
        control,
        body.clone(),
        body,
        DrawerKind::Tab,
        mount_policy,
    )
}

// =============================================================================
// Drawer navigator
// =============================================================================

/// Create a drawer navigator backed by an `androidx.drawerlayout.
/// widget.DrawerLayout` (via our `RustDrawerLayout` subclass).
pub(crate) fn create_drawer(
    backend: &mut AndroidBackend,
    callbacks: AndroidDrawerCallbacks,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    let mount_policy = callbacks.mount_policy;
    let drawer_width = callbacks.drawer_width;
    let AndroidDrawerCallbacks {
        navigator,
        is_open,
        open_changed,
        side,
        swipe_to_open,
        ..
    } = callbacks;

    // Leak a listener box so the RustDrawerLayout's JNI callbacks can
    // find the `is_open` signal + `open_changed` callback. Freed in
    // [`release`].
    let listener_box = Box::new(DrawerListenerBox { is_open, open_changed });
    let listener_ptr = Box::into_raw(listener_box) as jlong;

    // Build the RustDrawerLayout (constructor takes Context + listener
    // pointer).
    let drawer_layout_ref = backend.with_jni(|env, context| {
        let class = match env.find_class("io/idealyst/runtime/RustDrawerLayout") {
            Ok(c) => c,
            Err(e) => {
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                }
                log::error!(
                    "RustDrawerLayout class not found — make sure the consuming app's \
                     build.gradle.kts includes `androidx.drawerlayout:drawerlayout:1.2.0` \
                     in its dependencies. Underlying error: {:?}",
                    e
                );
                panic!("RustDrawerLayout class missing from APK");
            }
        };
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;J)V",
                &[
                    JValue::Object(&context.as_obj()),
                    JValue::Long(listener_ptr),
                ],
            )
            .expect("new RustDrawerLayout failed");
        // DrawerLayout requires its parent to measure it with
        // MeasureSpec.EXACTLY. `apply_default_layout_params`
        // (MATCH_PARENT x WRAP_CONTENT) gives WRAP_CONTENT which
        // measures as AT_MOST and trips DrawerLayout's `onMeasure`
        // assertion. Force MATCH_PARENT on both axes so the parent
        // always measures us exactly.
        let lp_class = env
            .find_class("android/view/ViewGroup$LayoutParams")
            .expect("ViewGroup$LayoutParams class");
        let lp = env
            .new_object(
                &lp_class,
                "(II)V",
                &[JValue::Int(-1), JValue::Int(-1)],
            )
            .expect("new ViewGroup.LayoutParams");
        let _ = env.call_method(
            &local,
            "setLayoutParams",
            "(Landroid/view/ViewGroup$LayoutParams;)V",
            &[JValue::Object(&lp)],
        );

        // Set drawer gravity before attaching the drawer view —
        // `RustDrawerLayout.setDrawerGravity` stashes it for the
        // upcoming `attachDrawer` call. Gravity.START = 0x00800003,
        // Gravity.END = 0x00800005.
        let gravity = match side {
            DrawerSide::Start => 0x00800003i32,
            DrawerSide::End => 0x00800005i32,
        };
        let _ = env.call_method(&local, "setDrawerGravity", "(I)V", &[JValue::Int(gravity)]);

        // Initial swipe-to-open lock mode.
        let _ = env.call_method(
            &local,
            "setSwipeEnabled",
            "(Z)V",
            &[JValue::Bool(if swipe_to_open { 1 } else { 0 })],
        );

        env.new_global_ref(local).expect("global_ref RustDrawerLayout")
    });

    // Body LinearLayout (the active-screen container). Vertical so a
    // per-screen Toolbar (built from `AndroidScreenOptions` in
    // `attach_initial`) stacks above the screen.
    let body = make_body_linear(backend);
    with_jni_env(|env| {
        let _ = env.call_method(
            drawer_layout_ref.as_obj(),
            "attachContent",
            "(Landroid/view/View;)V",
            &[JValue::Object(&body.as_obj())],
        );
    });

    // Wrap the DrawerLayout in a `RustExactFrameLayout`. Required by
    // `DrawerLayout.onMeasure` — see comment above.
    let outer = backend.with_jni(|env, context| {
        let class = env
            .find_class("io/idealyst/runtime/RustExactFrameLayout")
            .expect("RustExactFrameLayout class not found");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&context.as_obj())],
            )
            .expect("new RustExactFrameLayout failed");
        apply_default_layout_params(env, &local);
        let _ = env.call_method(
            &local,
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&drawer_layout_ref.as_obj())],
        );
        env.new_global_ref(local).expect("global_ref RustExactFrameLayout")
    });

    install_instance(
        backend,
        navigator,
        control,
        outer,
        body,
        DrawerKind::Drawer {
            drawer_view: drawer_layout_ref,
            listener_ptr,
            swipe_to_open,
            drawer_width,
        },
        mount_policy,
    )
}

fn make_frame_layout(backend: &AndroidBackend) -> GlobalRef {
    backend.with_jni(|env, context| {
        let class = env
            .find_class("android/widget/FrameLayout")
            .expect("FrameLayout class not found");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&context.as_obj())],
            )
            .expect("new FrameLayout failed");
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).expect("global_ref FrameLayout")
    })
}

/// Build a vertical `LinearLayout` for the drawer body. Children are
/// [Toolbar, screen] stacked top-to-bottom. The Toolbar is added by
/// [`attach_initial`] from `AndroidScreenOptions`; the screen view is
/// the framework-built navigator content. Using a vertical LinearLayout
/// Wrap a screen view in an `android.widget.ScrollView`. The drawer
/// body is a vertical `LinearLayout` whose children (Toolbar +
/// Screen) get sized to their measured height — a screen taller than
/// the available viewport gets clipped because LinearLayout itself
/// doesn't scroll. Wrapping the screen in a ScrollView gives it
/// vertical scroll affordance, matching iOS's body_scroll +
/// web's drawer body div.
///
/// The wrapper is what we hand back to the caller; cache it in
/// `MountedScreen.view` so swap_body's `removeView(m.view)` removes
/// the wrapper (and its inner screen by hierarchy). Visibility flips
/// on the wrapper hide the inner content too.
fn wrap_in_scroll_view(env: &mut jni::JNIEnv, context: &GlobalRef, screen: &GlobalRef) -> Option<GlobalRef> {
    let class = env.find_class("android/widget/ScrollView").ok()?;
    let scroll = env
        .new_object(
            &class,
            "(Landroid/content/Context;)V",
            &[JValue::Object(&context.as_obj())],
        )
        .ok()?;
    // Default LayoutParams (`WRAP_CONTENT, WRAP_CONTENT`) on the
    // ScrollView would size it to its child — no scrolling. The
    // parent LinearLayout uses `layout_weight=1` semantics via
    // `setLayoutParams(LinearLayout.LayoutParams(MATCH_PARENT, 0,
    // weight=1))` to make it fill the leftover vertical space below
    // the Toolbar. ScrollView's child stays at its Taffy-assigned
    // height; ScrollView is whatever the LinearLayout gives it.
    if let Ok(lp_class) = env.find_class("android/widget/LinearLayout$LayoutParams") {
        if let Ok(lp) = env.new_object(
            &lp_class,
            "(IIF)V",
            &[
                JValue::Int(-1), // MATCH_PARENT width
                JValue::Int(0),  // 0 height — weight takes over
                JValue::Float(1.0),
            ],
        ) {
            let _ = env.call_method(
                &scroll,
                "setLayoutParams",
                "(Landroid/view/ViewGroup$LayoutParams;)V",
                &[JValue::Object(&lp)],
            );
        }
    }
    // Add the screen as the (only) child. The screen's own
    // LayoutParams (set by apply_frame) determine its scrollable
    // height inside the ScrollView's content area.
    if env
        .call_method(
            &scroll,
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&screen.as_obj())],
        )
        .is_err()
    {
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        return None;
    }
    env.new_global_ref(&scroll).ok()
}

/// (not a FrameLayout) lets the Toolbar reserve its measured height
/// at the top without overlapping the screen.
fn make_body_linear(backend: &AndroidBackend) -> GlobalRef {
    backend.with_jni(|env, context| {
        let class = env
            .find_class("android/widget/LinearLayout")
            .expect("LinearLayout class not found");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&context.as_obj())],
            )
            .expect("new LinearLayout failed");
        // setOrientation(LinearLayout.VERTICAL=1).
        let _ = env.call_method(&local, "setOrientation", "(I)V", &[JValue::Int(1)]);
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).expect("global_ref LinearLayout")
    })
}

/// Install the per-instance state in the thread-local registry, wire
/// up the dispatcher, and return the framework-visible outer node.
fn install_instance(
    backend: &AndroidBackend,
    callbacks: AndroidNavCallbacks,
    control: Rc<NavigatorControl>,
    outer: GlobalRef,
    body: GlobalRef,
    kind: DrawerKind,
    nav_global_policy: MountPolicy,
) -> GlobalRef {
    let is_drawer = matches!(kind, DrawerKind::Drawer { .. });
    let context = backend.with_jni(|_env, ctx| ctx.clone());
    let active_route_sig = callbacks.nav_state.active_route;
    let instance = Rc::new(RefCell::new(TabDrawerInstance {
        outer: outer.clone(),
        body,
        context,
        mounted: HashMap::new(),
        current_route: None,
        nav_global_policy,
        active_route_sig,
        release_screen: callbacks.release_screen.clone(),
        mount_screen: callbacks.mount_screen.clone(),
        kind,
    }));

    // Tabs and drawers select; they don't push. Author-side `Link`
    // primitives default to `NavCommand::Push`, which the dispatcher
    // below panics on — so rewrite the default activation to `Select`
    // here. Stacks don't install this and keep the Push default.
    let select_activator: Rc<
        dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand,
    > = Rc::new(|name, url, params| NavCommand::Select {
        name,
        url,
        params,
        state: None,
    });
    control.install_link_activator(select_activator);

    let dispatcher_instance = instance.clone();
    control.install(Box::new(move |cmd| {
        match cmd {
            NavCommand::Select { name, params, url: _, state: _ } => {
                swap_body(&dispatcher_instance, name, params);
                // Auto-close the drawer after selecting an item.
                if matches!(dispatcher_instance.borrow().kind, DrawerKind::Drawer { .. }) {
                    drawer_jni_call(&dispatcher_instance, "closeDrawerProgrammatic");
                }
            }
            NavCommand::Reset { name, params, url: _, state: _ } => {
                swap_body(&dispatcher_instance, name, params);
                if matches!(dispatcher_instance.borrow().kind, DrawerKind::Drawer { .. }) {
                    drawer_jni_call(&dispatcher_instance, "closeDrawerProgrammatic");
                }
            }
            NavCommand::Custom(payload) => {
                if !matches!(dispatcher_instance.borrow().kind, DrawerKind::Drawer { .. }) {
                    // Tabs don't understand Custom payloads today; ignore
                    // foreign types instead of panicking so future SDK
                    // additions are forward-compatible.
                    return;
                }
                if let Ok(cmd) = payload.downcast::<DrawerCmd>() {
                    match *cmd {
                        DrawerCmd::Open => {
                            drawer_jni_call(&dispatcher_instance, "openDrawerProgrammatic");
                        }
                        DrawerCmd::Close => {
                            drawer_jni_call(&dispatcher_instance, "closeDrawerProgrammatic");
                        }
                        DrawerCmd::Toggle => {
                            drawer_jni_call(&dispatcher_instance, "toggleDrawer");
                        }
                    }
                }
            }
            NavCommand::Push { .. } | NavCommand::Pop | NavCommand::Replace { .. } => {
                let kind = if is_drawer { "DrawerNavigator" } else { "TabNavigator" };
                panic!(
                    "{} received an unsupported NavCommand — \
                     tabs/drawer accept Select (+ Reset for go-home, \
                     and drawer accepts Custom(DrawerCmd)). \
                     Push/Pop/Replace belong on a stack navigator.",
                    kind
                );
            }
        }
    }));

    let key = node_key(&outer);
    TAB_DRAWER_INSTANCES.with(|m| {
        m.borrow_mut().insert(key, TabDrawerEntry { instance, control });
    });

    outer
}

/// Swap the active screen. Mirrors iOS's `select_screen`:
///
/// - The outgoing screen's cached `effective_policy` decides whether
///   it's hidden (Persistent) or torn down (LazyDisposing).
/// - A cache hit on the incoming route reuses the cached subtree
///   (setVisibility(VISIBLE)); a miss mounts fresh and caches with
///   its own effective_policy (per-screen override or
///   navigator-global fallback).
fn swap_body(
    instance: &Rc<RefCell<TabDrawerInstance>>,
    name: &'static str,
    params: Box<dyn Any>,
) {
    // Snapshot what we need outside the borrow window.
    let (body, context, is_drawer, prev_route, nav_global_policy) = {
        let inst = instance.borrow();
        (
            inst.body.clone(),
            inst.context.clone(),
            matches!(inst.kind, DrawerKind::Drawer { .. }),
            inst.current_route,
            inst.nav_global_policy,
        )
    };

    // Step 1: Hide or dispose the outgoing screen per its own
    // effective_policy.
    if let Some(prev) = prev_route {
        if prev != name {
            let prev_entry = instance.borrow_mut().mounted.remove(prev);
            if let Some(m) = prev_entry {
                match m.effective_policy {
                    MountPolicy::LazyDisposing => {
                        // Disposing: remove view + toolbar from body
                        // and release the reactive scope.
                        with_jni_env(|env| {
                            let _ = env.call_method(
                                body.as_obj(),
                                "removeView",
                                "(Landroid/view/View;)V",
                                &[JValue::Object(&m.view.as_obj())],
                            );
                            if let Some(tb) = &m.toolbar {
                                let _ = env.call_method(
                                    body.as_obj(),
                                    "removeView",
                                    "(Landroid/view/View;)V",
                                    &[JValue::Object(&tb.as_obj())],
                                );
                            }
                        });
                        let release = instance.borrow().release_screen.clone();
                        release(m.scope_id);
                    }
                    MountPolicy::LazyPersistent | MountPolicy::EagerPersistent => {
                        // Persistent: hide via `setVisibility(GONE)`
                        // and keep the entry alive.
                        with_jni_env(|env| {
                            set_visibility_gone(env, &m.view);
                            if let Some(tb) = &m.toolbar {
                                set_visibility_gone(env, tb);
                            }
                        });
                        instance.borrow_mut().mounted.insert(prev, m);
                    }
                }
            }
        }
    }

    // Step 2: Cache hit on the incoming route → unhide + done.
    if instance.borrow().mounted.contains_key(name) {
        let inst = instance.borrow();
        let m = inst.mounted.get(name).expect("just checked contains_key");
        with_jni_env(|env| {
            set_visibility_visible(env, &m.view);
            if let Some(tb) = &m.toolbar {
                set_visibility_visible(env, tb);
            }
        });
        drop(inst);
        instance.borrow_mut().current_route = Some(name);
        return;
    }

    // Step 3: Cache miss → mount fresh, attach to body, cache.
    let result = {
        // mount_screen re-enters the build walker which calls
        // backend.borrow_mut(). Safe here because dispatcher
        // callbacks fire outside any active borrow window (Kotlin
        // event handler → JNI → Rust).
        let mount = instance.borrow().mount_screen.clone();
        mount(name, params)
    };
    let new_view = result.node;
    let new_scope = result.scope_id;
    let new_options = result
        .options
        .downcast::<AndroidScreenOptions>()
        .map(|b| *b)
        .unwrap_or_default();
    let effective_policy = new_options.mount_policy.unwrap_or(nav_global_policy);
    // Run Taffy synchronously BEFORE the new view enters the visible
    // tree. The new screen's Taffy nodes need `compute(root, vw, vh)`
    // so each sub-view's LayoutParams reflect the intended frame.
    // Without this, the user sees one frame of wrong positions
    // before the deferred handler-posted layout pass overwrites them.
    backend_android::run_layout_now();
    let (new_toolbar, body_child) = with_jni_env(|env| {
        // Build a new toolbar for this screen (drawer only; tabs
        // don't have per-screen chrome).
        let tb = if is_drawer {
            attach_toolbar_to_body(env, &context, &body, &new_options)
        } else {
            None
        };
        // Defensive detach in case mount_screen returned a view that
        // already has a parent (e.g. from a stale cache or re-entry).
        let parent_check = env
            .call_method(new_view.as_obj(), "getParent", "()Landroid/view/ViewParent;", &[])
            .ok();
        if let Some(jni::objects::JValueGen::Object(ref p)) = parent_check {
            if !p.is_null() {
                let _ = env.call_method(
                    p,
                    "removeView",
                    "(Landroid/view/View;)V",
                    &[JValue::Object(&new_view.as_obj())],
                );
            }
        }
        // Wrap the screen in a ScrollView so content taller than the
        // viewport (the website's hero + simulator + sections) is
        // actually scrollable. Without this, body's LinearLayout
        // clips at the viewport edge — visible bug: can't scroll
        // past the embedded simulator to see content below. Drawer
        // only — tabs don't have per-screen scroll semantics yet.
        let body_child = if is_drawer {
            wrap_in_scroll_view(env, &context, &new_view).unwrap_or_else(|| new_view.clone())
        } else {
            new_view.clone()
        };
        if let Err(e) = env.call_method(
            body.as_obj(),
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&body_child.as_obj())],
        ) {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            log::error!("swap_body addView failed: {:?}", e);
        }
        (tb, body_child)
    });
    {
        let mut inst = instance.borrow_mut();
        inst.current_route = Some(name);
        inst.mounted.insert(
            name,
            MountedScreen {
                // Cache the BODY-side child (ScrollView wrapper for
                // drawer, raw view for tabs) — `removeView` /
                // `setVisibility` later in swap_body operate on what's
                // actually parented to `body`.
                view: body_child,
                toolbar: new_toolbar,
                scope_id: new_scope,
                options: new_options,
                effective_policy,
            },
        );
    }
}

fn set_visibility_gone(env: &mut jni::JNIEnv, view: &GlobalRef) {
    let _ = env.call_method(view.as_obj(), "setVisibility", "(I)V", &[JValue::Int(8)]);
}

fn set_visibility_visible(env: &mut jni::JNIEnv, view: &GlobalRef) {
    let _ = env.call_method(view.as_obj(), "setVisibility", "(I)V", &[JValue::Int(0)]);
}

/// Invoke a no-arg method on the `RustDrawerLayout` (open/close/toggle).
/// No-op for tab navigators.
fn drawer_jni_call(instance: &Rc<RefCell<TabDrawerInstance>>, method: &str) {
    let drawer_view = {
        let inst = instance.borrow();
        match &inst.kind {
            DrawerKind::Drawer { drawer_view, .. } => drawer_view.clone(),
            DrawerKind::Tab => {
                panic!(
                    "TabNavigator received a drawer command ({}). \
                     Drawer commands (Custom(DrawerCmd::Open/Close/Toggle)) \
                     are only valid against a DrawerNavigator.",
                    method
                );
            }
        }
    };
    with_jni_env(|env| {
        if let Err(e) = env.call_method(drawer_view.as_obj(), method, "()V", &[]) {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            log::error!("RustDrawerLayout.{} JNI call failed: {:?}", method, e);
        }
    });
}

/// Attach the framework-built sidebar to the drawer's
/// `RustDrawerLayout`. Calls `attachDrawer(view)` which adds the view
/// as the drawer-child (gravity = START or END as configured).
pub(crate) fn attach_sidebar(navigator: &GlobalRef, sidebar: GlobalRef) {
    let key = node_key(navigator);
    let lookup = TAB_DRAWER_INSTANCES.with(|m| -> Option<(GlobalRef, f32)> {
        let map = m.borrow();
        let entry = map.get(&key)?;
        let result = match &entry.instance.borrow().kind {
            DrawerKind::Drawer { drawer_view, drawer_width, .. } => {
                Some((drawer_view.clone(), *drawer_width))
            }
            DrawerKind::Tab => None,
        };
        result
    });
    let Some((drawer_view, drawer_width)) = lookup else {
        log::warn!("tab_drawer attach_sidebar: not a drawer navigator");
        return;
    };
    with_jni_env(|env| {
        if let Err(e) = env.call_method(
            drawer_view.as_obj(),
            "attachDrawer",
            "(Landroid/view/View;F)V",
            &[
                JValue::Object(&sidebar.as_obj()),
                JValue::Float(drawer_width),
            ],
        ) {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            log::error!("RustDrawerLayout.attachDrawer JNI call failed: {:?}", e);
        }
    });
    // `attachDrawer` writes a DrawerLayout.LayoutParams with width
    // = the SDK's `drawer_width` (dp→px on the Kotlin side). Schedule
    // a layout pass so apply_frames runs against the now-installed
    // sidebar subtree — the sidebar's own Taffy children (header,
    // links, footer) still need their frames written, even though
    // the outer LP is now author-explicit.
    backend_android::schedule_layout_pass();
}

/// Attach the framework-built initial screen to the body. Called by
/// the framework after `create_*` returns, outside any active backend
/// borrow.
///
/// Returns `true` when `navigator` refers to a tab/drawer instance;
/// `false` otherwise (no-op).
pub(crate) fn attach_initial(
    navigator: &GlobalRef,
    screen: GlobalRef,
    scope_id: u64,
    options: &AndroidScreenOptions,
) -> bool {
    let key = node_key(navigator);
    let entry_data = TAB_DRAWER_INSTANCES.with(|m| -> Option<(GlobalRef, GlobalRef, bool)> {
        let map = m.borrow();
        let entry = map.get(&key)?;
        let inst = entry.instance.borrow();
        let is_drawer = matches!(inst.kind, DrawerKind::Drawer { .. });
        Some((inst.body.clone(), inst.context.clone(), is_drawer))
    });
    let Some((body, context, is_drawer)) = entry_data else { return false };
    let (new_toolbar, body_child) = with_jni_env(|env| {
        let tb = if is_drawer {
            attach_toolbar_to_body(env, &context, &body, options)
        } else {
            None
        };
        // Mirror swap_body: wrap the screen in a ScrollView for
        // drawer navigators so taller-than-viewport content scrolls.
        let body_child = if is_drawer {
            wrap_in_scroll_view(env, &context, &screen).unwrap_or_else(|| screen.clone())
        } else {
            screen.clone()
        };
        let _ = env.call_method(
            body.as_obj(),
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&body_child.as_obj())],
        );
        (tb, body_child)
    });
    TAB_DRAWER_INSTANCES.with(|m| {
        let map = m.borrow();
        if let Some(entry) = map.get(&key) {
            let mut inst = entry.instance.borrow_mut();
            // The initial screen's route name comes from the
            // active_route signal the framework wired into the
            // NavState at navigator creation time. Per-screen
            // mount_policy overrides ride in on subsequent
            // swap_body mounts; the initial mount uses whatever
            // override the options carry (or the navigator-global
            // default).
            let initial_name = inst.active_route_sig.get();
            let effective_policy =
                options.mount_policy.unwrap_or(inst.nav_global_policy);
            inst.current_route = Some(initial_name);
            inst.mounted.insert(
                initial_name,
                MountedScreen {
                    view: body_child,
                    toolbar: new_toolbar,
                    scope_id,
                    options: options.clone(),
                    effective_policy,
                },
            );
        }
    });
    true
}

/// Build a Toolbar from screen options (via the Kotlin shim) and add
/// it to `body` as its first child. No-op if neither a title nor a
/// header_left button is set. Returns the toolbar's GlobalRef so the
/// caller can persist it on the navigator instance.
fn attach_toolbar_to_body(
    env: &mut jni::JNIEnv,
    context: &GlobalRef,
    body: &GlobalRef,
    options: &AndroidScreenOptions,
) -> Option<GlobalRef> {
    if options.title.is_none()
        && options.header_left.is_none()
        && options.header_background.is_none()
        && options.title_color.is_none()
        && options.header_tint.is_none()
    {
        return None;
    }

    let left_ptr: jlong = match options.header_left.as_ref() {
        Some(btn) => {
            // Leak the callback so the Toolbar's OnClickListener can
            // call back any number of times. See HeaderButtonCallback's
            // lifetime note for why we don't free the previous one.
            Box::into_raw(Box::new(HeaderButtonCallback(btn.on_press.clone()))) as jlong
        }
        None => 0,
    };
    let title_jstring = options.title.as_ref().and_then(|t| env.new_string(t).ok());
    // Color fields are closures so the framework's reactive plumbing can
    // re-resolve them on theme change. Invoke each one here and stringify
    // the CSS for the Kotlin-side parser.
    let bg_jstring = options
        .header_background
        .as_ref()
        .and_then(|f| env.new_string(&f().0).ok());
    let title_color_jstring = options
        .title_color
        .as_ref()
        .and_then(|f| env.new_string(&f().0).ok());
    let tint_jstring = options
        .header_tint
        .as_ref()
        .and_then(|f| env.new_string(&f().0).ok());

    let helper_class = match env.find_class("io/idealyst/runtime/RustActionBarHelper") {
        Ok(c) => c,
        Err(e) => {
            log::error!("RustActionBarHelper class not found: {:?}", e);
            return None;
        }
    };
    let null_obj = JObject::null();
    let title_arg: &JObject = match title_jstring.as_ref() {
        Some(s) => s.as_ref(),
        None => &null_obj,
    };
    let bg_arg: &JObject = match bg_jstring.as_ref() {
        Some(s) => s.as_ref(),
        None => &null_obj,
    };
    let title_color_arg: &JObject = match title_color_jstring.as_ref() {
        Some(s) => s.as_ref(),
        None => &null_obj,
    };
    let tint_arg: &JObject = match tint_jstring.as_ref() {
        Some(s) => s.as_ref(),
        None => &null_obj,
    };
    let toolbar_obj = match env.call_static_method(
        helper_class,
        "buildToolbar",
        "(Landroid/content/Context;Ljava/lang/String;JLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)Landroid/widget/Toolbar;",
        &[
            JValue::Object(&context.as_obj()),
            JValue::Object(title_arg),
            JValue::Long(left_ptr),
            JValue::Object(bg_arg),
            JValue::Object(title_color_arg),
            JValue::Object(tint_arg),
        ],
    ) {
        Ok(jni::objects::JValueGen::Object(o)) => o,
        Ok(_) => return None,
        Err(e) => {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            log::error!("RustActionBarHelper.buildToolbar failed: {:?}", e);
            return None;
        }
    };
    if let Err(e) = env.call_method(
        body.as_obj(),
        "addView",
        "(Landroid/view/View;)V",
        &[JValue::Object(&toolbar_obj)],
    ) {
        log::error!("Toolbar addView failed: {:?}", e);
        return None;
    }
    env.new_global_ref(&toolbar_obj).ok()
}

// =============================================================================
// Slot styling
// =============================================================================

pub(crate) fn apply_header_style(
    navigator: &GlobalRef,
    rules: &Rc<runtime_core::StyleRules>,
) {
    let toolbar = lookup_toolbar(navigator);
    let Some(toolbar) = toolbar else { return };
    let Some(color) = rules.background.as_ref() else { return };
    let css = color.resolve().0;
    with_jni_env(|env| {
        let Ok(jstr) = env.new_string(&css) else { return };
        let Ok(helper) = env.find_class("io/idealyst/runtime/RustActionBarHelper") else { return };
        let _ = env.call_static_method(
            helper,
            "setToolbarBackground",
            "(Landroid/widget/Toolbar;Ljava/lang/String;)V",
            &[JValue::Object(&toolbar.as_obj()), JValue::Object(&jstr.as_ref())],
        );
    });
}

pub(crate) fn apply_title_style(
    navigator: &GlobalRef,
    rules: &Rc<runtime_core::StyleRules>,
) {
    let toolbar = lookup_toolbar(navigator);
    let Some(toolbar) = toolbar else { return };
    let Some(color) = rules.color.as_ref() else { return };
    let css = color.resolve().0;
    with_jni_env(|env| {
        let Ok(jstr) = env.new_string(&css) else { return };
        let Ok(helper) = env.find_class("io/idealyst/runtime/RustActionBarHelper") else { return };
        let _ = env.call_static_method(
            helper,
            "setToolbarTitleColor",
            "(Landroid/widget/Toolbar;Ljava/lang/String;)V",
            &[JValue::Object(&toolbar.as_obj()), JValue::Object(&jstr.as_ref())],
        );
    });
}

pub(crate) fn apply_button_style(
    navigator: &GlobalRef,
    rules: &Rc<runtime_core::StyleRules>,
) {
    let toolbar = lookup_toolbar(navigator);
    let Some(toolbar) = toolbar else { return };
    let Some(color) = rules.color.as_ref() else { return };
    let css = color.resolve().0;
    with_jni_env(|env| {
        let Ok(jstr) = env.new_string(&css) else { return };
        let Ok(helper) = env.find_class("io/idealyst/runtime/RustActionBarHelper") else { return };
        let _ = env.call_static_method(
            helper,
            "setToolbarNavIconTint",
            "(Landroid/widget/Toolbar;Ljava/lang/String;)V",
            &[JValue::Object(&toolbar.as_obj()), JValue::Object(&jstr.as_ref())],
        );
    });
}

pub(crate) fn apply_body_style(
    navigator: &GlobalRef,
    rules: &Rc<runtime_core::StyleRules>,
) {
    let body = TAB_DRAWER_INSTANCES.with(|m| -> Option<GlobalRef> {
        let map = m.borrow();
        let entry = map.get(&node_key(navigator))?;
        let b = entry.instance.borrow().body.clone();
        Some(b)
    });
    let Some(body) = body else { return };
    let Some(color) = rules.background.as_ref() else { return };
    let css = color.resolve().0;
    with_jni_env(|env| {
        let Ok(jstr) = env.new_string(&css) else { return };
        let Ok(helper) = env.find_class("io/idealyst/runtime/RustActionBarHelper") else { return };
        let _ = env.call_static_method(
            helper,
            "setViewBackground",
            "(Landroid/view/View;Ljava/lang/String;)V",
            &[JValue::Object(&body.as_obj()), JValue::Object(&jstr.as_ref())],
        );
    });
}

fn lookup_toolbar(navigator: &GlobalRef) -> Option<GlobalRef> {
    TAB_DRAWER_INSTANCES.with(|m| -> Option<GlobalRef> {
        let map = m.borrow();
        let entry = map.get(&node_key(navigator))?;
        let inst = entry.instance.borrow();
        let active = inst.current_route?;
        inst.mounted.get(active)?.toolbar.clone()
    })
}

// =============================================================================
// Release / handle
// =============================================================================

pub(crate) fn release(node: &GlobalRef) -> bool {
    let key = node_key(node);
    let Some(entry) = TAB_DRAWER_INSTANCES.with(|m| m.borrow_mut().remove(&key)) else {
        return false;
    };
    // Release every cached screen's scope (persistent screens were
    // hidden but still live; disposing screens were already
    // released when blurred and never made it into the cache).
    let scopes_to_release: Vec<u64> = {
        let inst = entry.instance.borrow();
        inst.mounted.values().map(|m| m.scope_id).collect()
    };
    let release = entry.instance.borrow().release_screen.clone();
    for scope in scopes_to_release {
        release(scope);
    }
    entry.instance.borrow_mut().mounted.clear();
    let listener_ptr = match entry.instance.borrow().kind {
        DrawerKind::Drawer { listener_ptr, .. } => listener_ptr,
        DrawerKind::Tab => 0,
    };
    drop(entry);
    if listener_ptr != 0 {
        unsafe {
            drop(Box::from_raw(listener_ptr as *mut DrawerListenerBox));
        }
    }
    true
}

pub(crate) fn make_handle(node: &GlobalRef) -> Option<NavigatorHandle> {
    let key = node_key(node);
    TAB_DRAWER_INSTANCES.with(|m| {
        m.borrow().get(&key).map(|entry| {
            NavigatorHandle::with_control(Rc::new(()), &TAB_DRAWER_OPS, entry.control.clone())
        })
    })
}

struct TabDrawerOps;
impl NavigatorOps for TabDrawerOps {}
static TAB_DRAWER_OPS: TabDrawerOps = TabDrawerOps;

// =============================================================================
// JNI exports — RustDrawerLayout listener callbacks
// =============================================================================
//
// DrawerLayout fires its listener on every state transition. The Kotlin
// `RustDrawerLayout` forwards the open/closed events here so we can
// update the framework's reactive `is_open` signal.

/// # Safety
///
/// `ptr` must be the live pointer produced by `Box::into_raw` on a
/// `Box<DrawerListenerBox>` in [`create_drawer`]. Freed by [`release`]
/// when the navigator scope drops.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustDrawerLayout_nativeOnDrawerOpened(
    _env: jni::JNIEnv,
    _this: jni::objects::JObject,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    let listener = &*(ptr as *const DrawerListenerBox);
    listener.is_open.set(true);
    (listener.open_changed)(true);
}

#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustDrawerLayout_nativeOnDrawerClosed(
    _env: jni::JNIEnv,
    _this: jni::objects::JObject,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    let listener = &*(ptr as *const DrawerListenerBox);
    listener.is_open.set(false);
    (listener.open_changed)(false);
}

/// Companion to the `RustDrawerLayout.nativeDrop` Kotlin declaration.
/// Currently unused — [`release`] frees the listener box. Provided so
/// the Kotlin class's `external fun nativeDrop(...)` resolves at link
/// time.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustDrawerLayout_nativeDrop(
    _env: jni::JNIEnv,
    _this: jni::objects::JObject,
    _ptr: jlong,
) {
    // No-op.
}
