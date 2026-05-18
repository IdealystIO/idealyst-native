//! TabNavigator + DrawerNavigator on Android.
//!
//! These navigator kinds don't use `RustNavigator` / `FragmentManager`
//! — they're simpler. Each one builds a small native subtree
//! described below; the *body* region holds exactly one child View
//! (the currently-active screen) which swaps on Select.
//!
//! # Per-kind shapes
//!
//! - **TabNavigator**: navigator node is just a body `FrameLayout`.
//!   Tab chrome (a tab bar) is the author's responsibility via
//!   `.layout(...)`. Native Android *doesn't* currently render the
//!   author's layout slot for tab navigators — see the TODO at the
//!   end of this file. For now, calling `select(...)` on the handle
//!   works, but there's no native UI to drive it from.
//!
//! - **DrawerNavigator**: navigator node is a horizontal
//!   `LinearLayout` with two children:
//!     1. The author's `.sidebar(...)` subtree (built from
//!        `callbacks.build_sidebar`, or omitted if no sidebar was
//!        registered).
//!     2. A body `FrameLayout` where the active screen mounts.
//!
//!   This is the simplest pinned-sidebar layout — the drawer is
//!   always visible beside the body. Open/close commands flip the
//!   `is_open` signal but don't currently animate the sidebar in
//!   and out; that's a follow-up that needs `DrawerLayout` (or a
//!   hand-rolled translation animation).
//!
//! # Why no FragmentManager
//!
//! Nesting `RustNavigator` would put the drawer's screens on the
//! *same* activity-level back stack as the root stack navigator —
//! tapping a drawer item would push a fragment that the system Back
//! button would later pop, conflicting with stack-navigator
//! semantics. View-swap avoids the conflict entirely: drawer/tab
//! selections never touch the back stack.

use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use framework_core::primitives::navigator::{
    DrawerHandle, DrawerNavigatorCallbacks, MountResult, NavCommand, NavigatorCallbacks,
    NavigatorControl, NavigatorHandle, NavigatorOps, TabNavigatorCallbacks, TabsHandle,
};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::jlong;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Leaked-box payload the `RustDrawerLayout`'s JNI callbacks
/// (`nativeOnDrawerOpened` / `nativeOnDrawerClosed`) dereference
/// to flip the framework's `is_open` signal + fire the
/// `open_changed` callback. The pointer is freed in
/// [`release`] when the navigator's enclosing scope drops.
///
/// We use a dedicated box (rather than reusing the navigator's
/// `TabDrawerEntry`) so the JNI side only sees a stable pointer
/// it can dereference cheaply, with no hashmap lookup.
struct DrawerListenerBox {
    is_open: framework_core::Signal<bool>,
    open_changed: Rc<dyn Fn(bool)>,
}

/// Per-kind metadata stashed on the `TabDrawerInstance`. Distinguishes
/// tab navigators (just a body FrameLayout, no drawer state) from
/// drawer navigators (RustDrawerLayout with attached listener box).
enum DrawerKind {
    /// Tab navigator — no drawer commands, no listener pointer.
    Tab,
    /// Drawer navigator. The navigator's `outer` view is a
    /// `RustExactFrameLayout` wrapper (needed because
    /// `DrawerLayout.onMeasure` insists on EXACTLY-mode measure
    /// specs that we can't guarantee through the framework's
    /// default layout pipeline). `drawer_view` is the actual
    /// `RustDrawerLayout` instance — drawer commands and
    /// `attach_sidebar` target this rather than `outer`.
    /// `listener_ptr` is freed in `release`.
    Drawer {
        drawer_view: GlobalRef,
        listener_ptr: jlong,
        #[allow(dead_code)] // wired through for future re-application
        swipe_to_open: bool,
    },
}

/// Per-instance state for a tab or drawer navigator. The `body` is
/// a `FrameLayout` that holds exactly one child (the active
/// screen's native node). `outer` is the framework-visible
/// navigator node — for tabs this is the same as `body`; for
/// drawer this is a `LinearLayout` containing the sidebar + body.
///
/// `current_scope` is the scope id the framework returned for the
/// active screen — we release it on swap so the old screen's
/// signals/effects free deterministically.
pub(crate) struct TabDrawerInstance {
    /// The framework-visible navigator container — the
    /// `RustDrawerLayout` for drawer, or the body `FrameLayout`
    /// for tabs. Drawer commands (open/close/toggle) call methods
    /// on this object via JNI.
    outer: GlobalRef,
    /// The FrameLayout that holds the active screen. Same as
    /// `outer` for tabs; the DrawerLayout's content child for
    /// drawer.
    body: GlobalRef,
    /// Currently-mounted screen's view + scope id. `None` only
    /// between creation and the first `attach_initial` call.
    current: Option<(GlobalRef, u64)>,
    /// Used to release the previous scope when we swap screens.
    release_screen: Rc<dyn Fn(u64)>,
    /// Used to mount the next screen on `Select`.
    mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<GlobalRef>>,
    /// Per-kind metadata. Drawer carries the leaked listener
    /// pointer that needs freeing on release.
    kind: DrawerKind,
}

pub(crate) type TabDrawerInstances = HashMap<usize, TabDrawerEntry>;

pub(crate) struct TabDrawerEntry {
    pub(crate) instance: Rc<RefCell<TabDrawerInstance>>,
    pub(crate) control: Rc<NavigatorControl>,
}

// ---------------------------------------------------------------------------
// Tab navigator
// ---------------------------------------------------------------------------

/// Create a tab navigator. Returns a `FrameLayout` GlobalRef (the
/// body, since tabs don't have a sidebar at this layer).
///
/// **Important**: like the stack navigator's `create`, this MUST NOT
/// call `mount_screen` synchronously — the framework holds
/// `backend.borrow_mut()` for the entire `create_*` call and
/// `mount_screen` re-enters the build walker which also borrow_muts.
/// The framework calls `attach_initial` after `create_*` returns
/// with the freshly-built initial screen.
pub(crate) fn create_tab(
    b: &mut AndroidBackend,
    callbacks: TabNavigatorCallbacks<GlobalRef>,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    let TabNavigatorCallbacks { navigator, .. } = callbacks;
    // No sidebar for tabs. The framework-visible outer node and the
    // body are the same FrameLayout.
    let body = make_frame_layout(b);
    install_instance(
        b,
        navigator,
        control,
        body.clone(),
        body,
        DrawerKind::Tab,
    )
}

/// Create a drawer navigator backed by an `androidx.drawerlayout.
/// widget.DrawerLayout` (via our `RustDrawerLayout` subclass).
///
/// The native shape: a `RustDrawerLayout` whose two children are
///   1. a body `FrameLayout` (the active screen's container), and
///   2. the sidebar View (attached later by `attach_sidebar`).
///
/// Open/close commands trigger DrawerLayout's animations + scrim
/// for free. Edge-swipe to open is on by default; gated by
/// `swipe_to_open`. DrawerLayout's `onDrawerOpened` /
/// `onDrawerClosed` listener fires `nativeOnDrawerOpened` /
/// `nativeOnDrawerClosed` back to Rust, which updates the
/// framework's reactive `is_open` signal.
pub(crate) fn create_drawer(
    b: &mut AndroidBackend,
    callbacks: DrawerNavigatorCallbacks<GlobalRef>,
    control: Rc<NavigatorControl>,
) -> GlobalRef {
    let DrawerNavigatorCallbacks {
        navigator,
        is_open,
        open_changed,
        build_content,
        side,
        swipe_to_open,
        ..
    } = callbacks;
    // build_content runs from the walker after create_drawer
    // returns (outside the borrow_mut window). Forget it here.
    let _ = build_content;

    // Leak a listener box so the RustDrawerLayout's JNI callbacks
    // can find the `is_open` signal + `open_changed` callback.
    // Freed in `release` below.
    let listener_box = Box::new(DrawerListenerBox {
        is_open,
        open_changed,
    });
    let listener_ptr = Box::into_raw(listener_box) as jlong;

    // Build the RustDrawerLayout itself (constructor takes Context
    // + listener pointer).
    let drawer_layout_ref = with_env(|env| {
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
                    JValue::Object(&b.context.as_obj()),
                    JValue::Long(listener_ptr),
                ],
            )
            .expect("new RustDrawerLayout failed");
        // DrawerLayout requires its parent to measure it with
        // MeasureSpec.EXACTLY. `apply_default_layout_params`
        // (MATCH_PARENT x WRAP_CONTENT) gives WRAP_CONTENT which
        // measures as AT_MOST and trips DrawerLayout's
        // `onMeasure` assertion. Force MATCH_PARENT on both axes
        // so the parent always measures us exactly.
        let lp_class = env
            .find_class("android/view/ViewGroup$LayoutParams")
            .expect("ViewGroup$LayoutParams class");
        let lp = env
            .new_object(
                &lp_class,
                "(II)V",
                // (width=MATCH_PARENT=-1, height=MATCH_PARENT=-1)
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
            framework_core::DrawerSide::Start => 0x00800003i32,
            framework_core::DrawerSide::End => 0x00800005i32,
        };
        let _ = env.call_method(
            &local,
            "setDrawerGravity",
            "(I)V",
            &[JValue::Int(gravity)],
        );

        // Initial swipe-to-open lock mode. Applies globally until
        // a drawer is attached; then we re-apply per-drawer.
        let _ = env.call_method(
            &local,
            "setSwipeEnabled",
            "(Z)V",
            &[JValue::Bool(if swipe_to_open { 1 } else { 0 })],
        );

        env.new_global_ref(local).expect("global_ref RustDrawerLayout")
    });

    // Body LinearLayout (the active-screen container, attached as
    // DrawerLayout's "content" child — the one that fills the
    // frame and gets covered by the drawer on open). Vertical so a
    // per-screen Toolbar (built from `ScreenOptions` in
    // `attach_initial`) stacks above the screen.
    let body = make_body_linear(b);
    with_env(|env| {
        let _ = env.call_method(
            drawer_layout_ref.as_obj(),
            "attachContent",
            "(Landroid/view/View;)V",
            &[JValue::Object(&body.as_obj())],
        );
    });

    // Wrap the DrawerLayout in a `RustExactFrameLayout`. This
    // wrapper measures its child with EXACTLY no matter what spec
    // its own parent passes in, which is required by
    // `DrawerLayout.onMeasure`. Without this wrapper, when the
    // ambient parent (e.g. the root stack navigator's
    // FrameLayout with MATCH_PARENT × WRAP_CONTENT) measures
    // AT_MOST, the assertion in DrawerLayout fires and the app
    // crashes.
    let outer = with_env(|env| {
        let class = env
            .find_class("io/idealyst/runtime/RustExactFrameLayout")
            .expect("RustExactFrameLayout class not found");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .expect("new RustExactFrameLayout failed");
        apply_default_layout_params(env, &local);
        // Add the DrawerLayout as the wrapper's only child.
        let _ = env.call_method(
            &local,
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&drawer_layout_ref.as_obj())],
        );
        env.new_global_ref(local).expect("global_ref RustExactFrameLayout")
    });

    install_instance(
        b,
        navigator,
        control,
        outer,
        body,
        DrawerKind::Drawer {
            drawer_view: drawer_layout_ref,
            listener_ptr,
            swipe_to_open,
        },
    )
}

fn make_frame_layout(b: &AndroidBackend) -> GlobalRef {
    with_env(|env| {
        let class = env
            .find_class("android/widget/FrameLayout")
            .expect("FrameLayout class not found");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .expect("new FrameLayout failed");
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).expect("global_ref FrameLayout")
    })
}

/// Build a vertical `LinearLayout` for the drawer body. Children are
/// [Toolbar, screen] stacked top-to-bottom. The Toolbar is added by
/// [`attach_initial`] from `ScreenOptions`; the screen view is the
/// framework-built navigator content. Using a vertical LinearLayout
/// (not a FrameLayout) lets the Toolbar reserve its measured height
/// at the top without overlapping the screen.
fn make_body_linear(b: &AndroidBackend) -> GlobalRef {
    with_env(|env| {
        let class = env
            .find_class("android/widget/LinearLayout")
            .expect("LinearLayout class not found");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .expect("new LinearLayout failed");
        // Vertical orientation: setOrientation(LinearLayout.VERTICAL=1).
        let _ = env.call_method(&local, "setOrientation", "(I)V", &[JValue::Int(1)]);
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).expect("global_ref LinearLayout")
    })
}

/// Install the per-instance state on the backend, wire up the
/// dispatcher, and return the framework-visible outer node.
fn install_instance(
    b: &mut AndroidBackend,
    callbacks: NavigatorCallbacks<GlobalRef>,
    control: Rc<NavigatorControl>,
    outer: GlobalRef,
    body: GlobalRef,
    kind: DrawerKind,
) -> GlobalRef {
    let is_drawer = matches!(kind, DrawerKind::Drawer { .. });
    let instance = Rc::new(RefCell::new(TabDrawerInstance {
        outer: outer.clone(),
        body,
        current: None,
        release_screen: callbacks.release_screen.clone(),
        mount_screen: callbacks.mount_screen.clone(),
        kind,
    }));

    let dispatcher_instance = instance.clone();
    control.install(Box::new(move |cmd| {
        match cmd {
            NavCommand::Select { name, params, url: _ } => {
                swap_body(&dispatcher_instance, name, params);
                // Auto-close the drawer after selecting an item. On
                // Android the drawer is a real overlay (DrawerLayout)
                // that visually covers the body, so leaving it open
                // would hide the newly-mounted screen. Matches the
                // web dispatcher's auto-close behavior. No-op for
                // tab navigators (drawer_jni_call panics on tabs;
                // gate on kind).
                if matches!(dispatcher_instance.borrow().kind, DrawerKind::Drawer { .. }) {
                    drawer_jni_call(&dispatcher_instance, "closeDrawerProgrammatic");
                }
            }
            NavCommand::Reset { name, params, url: _ } => {
                swap_body(&dispatcher_instance, name, params);
                if matches!(dispatcher_instance.borrow().kind, DrawerKind::Drawer { .. }) {
                    drawer_jni_call(&dispatcher_instance, "closeDrawerProgrammatic");
                }
            }
            NavCommand::OpenDrawer => {
                drawer_jni_call(&dispatcher_instance, "openDrawerProgrammatic");
            }
            NavCommand::CloseDrawer => {
                drawer_jni_call(&dispatcher_instance, "closeDrawerProgrammatic");
            }
            NavCommand::ToggleDrawer => {
                drawer_jni_call(&dispatcher_instance, "toggleDrawer");
            }
            NavCommand::Push { .. } | NavCommand::Pop | NavCommand::Replace { .. } => {
                let kind = if is_drawer { "DrawerNavigator" } else { "TabNavigator" };
                panic!(
                    "{} received an unsupported NavCommand — \
                     tabs/drawer accept Select (+ Reset for go-home, \
                     and drawer accepts Open/Close/ToggleDrawer). \
                     Push/Pop/Replace belong on a stack navigator.",
                    kind
                );
            }
        }
    }));

    let key = AndroidBackend::node_key_of(&outer);
    b.tab_drawer_instances
        .insert(key, TabDrawerEntry { instance, control });

    outer
}

/// Mount a new screen, addView to the body, release the previous
/// scope. Used by both Select and Reset (same dispatch shape on
/// Android — no back-stack distinction here).
fn swap_body(
    instance: &Rc<RefCell<TabDrawerInstance>>,
    name: &'static str,
    params: Box<dyn Any>,
) {
    let result = {
        let inst = instance.borrow();
        // mount_screen re-enters the build walker which calls
        // backend.borrow_mut(). Safe here because dispatcher
        // callbacks fire outside any active borrow window (Kotlin
        // event handler → JNI → Rust).
        (inst.mount_screen)(name, params)
    };
    let new_view = result.node;
    let new_scope = result.scope_id;
    let (body, old_scope) = {
        let mut inst = instance.borrow_mut();
        let old = inst.current.take().map(|(_, s)| s);
        (inst.body.clone(), old)
    };
    with_env(|env| {
        let _ = env.call_method(body.as_obj(), "removeAllViews", "()V", &[]);
        let _ = env.call_method(
            body.as_obj(),
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&new_view.as_obj())],
        );
    });
    if let Some(scope) = old_scope {
        // Release the previous scope AFTER the new view is in
        // place. Releasing first would let the old view's
        // reactive effects fire one more time during the swap
        // and crash on freed state.
        let release = instance.borrow().release_screen.clone();
        release(scope);
    }
    instance.borrow_mut().current = Some((new_view, new_scope));
}

/// Invoke a no-arg method on the `RustDrawerLayout` (open/close/
/// toggle). No-op for tab navigators (which don't have a drawer
/// shell). The method name is one of `openDrawerProgrammatic`,
/// `closeDrawerProgrammatic`, `toggleDrawer`.
fn drawer_jni_call(instance: &Rc<RefCell<TabDrawerInstance>>, method: &str) {
    let drawer_view = {
        let inst = instance.borrow();
        match &inst.kind {
            DrawerKind::Drawer { drawer_view, .. } => drawer_view.clone(),
            DrawerKind::Tab => {
                // Drawer commands against a tab nav are a
                // programmer error — matches the dispatcher's
                // Push/Pop arm posture for non-stack commands.
                panic!(
                    "TabNavigator received a drawer command ({}). \
                     Drawer commands (Open/Close/ToggleDrawer) are \
                     only valid against a DrawerNavigator.",
                    method
                );
            }
        }
    };
    with_env(|env| {
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
/// `RustDrawerLayout`. Calls `attachDrawer(view)` which adds the
/// view as the drawer-child (gravity = START or END as configured
/// during create_drawer).
///
/// Tabs don't have sidebars; calling this on a tab navigator is a
/// no-op (the walker only calls this for drawer kinds, but defend
/// against future mis-wiring).
pub(crate) fn attach_sidebar(
    b: &mut AndroidBackend,
    navigator: &GlobalRef,
    sidebar: GlobalRef,
) {
    log::info!("[drawer] attach_sidebar called");
    let Some(entry) = b.tab_drawer_instances.get(&AndroidBackend::node_key_of(navigator)) else {
        log::error!("tab_drawer attach_sidebar: no instance for node");
        return;
    };
    let drawer_view = match &entry.instance.borrow().kind {
        DrawerKind::Drawer { drawer_view, .. } => drawer_view.clone(),
        DrawerKind::Tab => {
            log::warn!("tab_drawer attach_sidebar: navigator is not a drawer kind, skipping");
            return;
        }
    };
    log::info!("[drawer] attach_sidebar: calling attachDrawer JNI");
    with_env(|env| {
        if let Err(e) = env.call_method(
            drawer_view.as_obj(),
            "attachDrawer",
            "(Landroid/view/View;)V",
            &[JValue::Object(&sidebar.as_obj())],
        ) {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            log::error!("RustDrawerLayout.attachDrawer JNI call failed: {:?}", e);
        } else {
            log::info!("[drawer] attachDrawer JNI returned OK");
        }
    });
}

/// Attach the framework-built initial screen to the body. Called
/// by the framework after `create_*` returns, outside any active
/// backend borrow.
pub(crate) fn attach_initial(
    b: &mut AndroidBackend,
    navigator: &GlobalRef,
    screen: GlobalRef,
    scope_id: u64,
    options: framework_core::ScreenOptions,
) {
    let Some(entry) = b.tab_drawer_instances.get(&AndroidBackend::node_key_of(navigator)) else {
        log::error!("tab_drawer attach_initial: no instance for node");
        return;
    };
    let body = entry.instance.borrow().body.clone();
    with_env(|env| {
        let _ = env.call_method(
            body.as_obj(),
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&screen.as_obj())],
        );
    });
    entry.instance.borrow_mut().current = Some((screen, scope_id));

    // Push title + header_left through the Activity's system
    // ActionBar — mirrors what iOS does with the screen's
    // navigationItem (`title`, `setLeftBarButtonItem`).
    apply_screen_options(b, &options);
}

/// Mirror `ScreenOptions` onto the host Activity's ActionBar.
/// `RustActionBarHelper.apply` is a static Kotlin shim that takes the
/// title + a raw pointer to a leaked `HeaderButtonCallback` (0 ⇒ no
/// left button). The Activity's `onOptionsItemSelected` override
/// calls back through `RustActionBarHelper.dispatchHomePress` to
/// invoke the callback when the user taps the indicator.
fn apply_screen_options(
    b: &crate::imp::AndroidBackend,
    options: &framework_core::ScreenOptions,
) {
    use crate::imp::callbacks::HeaderButtonCallback;
    use jni::sys::jlong;

    let left_ptr: jlong = match options.header_left.as_ref() {
        Some(btn) => {
            // Leak the callback so the JVM-side helper can call back
            // any number of times. See `HeaderButtonCallback`'s
            // lifetime note for why we don't free the previous one.
            let leaked = Box::into_raw(Box::new(HeaderButtonCallback(btn.on_press.clone())));
            leaked as jlong
        }
        None => 0,
    };
    let title = options.title.clone();

    with_env(|env| {
        let title_jstring = match title {
            Some(ref t) => env.new_string(t).ok(),
            None => None,
        };
        let helper_class = match env.find_class("io/idealyst/runtime/RustActionBarHelper") {
            Ok(c) => c,
            Err(e) => {
                log::error!("RustActionBarHelper class not found: {:?}", e);
                return;
            }
        };
        let null_obj = JObject::null();
        let title_arg: &JObject = match title_jstring.as_ref() {
            Some(s) => s.as_ref(),
            None => &null_obj,
        };
        if let Err(e) = env.call_static_method(
            helper_class,
            "apply",
            "(Landroid/app/Activity;Ljava/lang/String;J)V",
            &[
                JValue::Object(&b.context.as_obj()),
                JValue::Object(title_arg),
                JValue::Long(left_ptr),
            ],
        ) {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            log::error!("RustActionBarHelper.apply failed: {:?}", e);
        }
    });
}

pub(crate) fn release(b: &mut AndroidBackend, node: &GlobalRef) {
    let key = AndroidBackend::node_key_of(node);
    let Some(entry) = b.tab_drawer_instances.remove(&key) else {
        return;
    };
    if let Some((_view, scope)) = entry.instance.borrow_mut().current.take() {
        let release = entry.instance.borrow().release_screen.clone();
        release(scope);
    }
    // Free the leaked DrawerListenerBox if this is a drawer
    // navigator. The Kotlin DrawerLayout still holds a JNI
    // reference to this pointer; any in-flight onDrawerOpened /
    // onDrawerClosed callback after this point would dereference
    // a freed box. In practice the DrawerLayout's listener fires
    // synchronously during user interaction (no async dispatch),
    // and `release` only runs when the enclosing scope drops —
    // at which point the DrawerLayout itself is detached from its
    // parent and not receiving new events.
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
}

pub(crate) fn make_tab_handle(b: &AndroidBackend, node: &GlobalRef) -> TabsHandle {
    TabsHandle::from_inner(make_inner_handle(b, node))
}

pub(crate) fn make_drawer_handle(b: &AndroidBackend, node: &GlobalRef) -> DrawerHandle {
    DrawerHandle::from_inner(
        make_inner_handle(b, node),
        Rc::new(std::cell::Cell::new(false)),
    )
}

fn make_inner_handle(b: &AndroidBackend, node: &GlobalRef) -> NavigatorHandle {
    let key = AndroidBackend::node_key_of(node);
    let Some(entry) = b.tab_drawer_instances.get(&key) else {
        return NavigatorHandle::new(Rc::new(()), &TabDrawerOps);
    };
    NavigatorHandle::with_control(Rc::new(()), &TabDrawerOps, entry.control.clone())
}

struct TabDrawerOps;
impl NavigatorOps for TabDrawerOps {}

// ---------------------------------------------------------------------------
// JNI exports — RustDrawerLayout listener callbacks
// ---------------------------------------------------------------------------
//
// DrawerLayout fires its listener on every state transition. The
// Kotlin `RustDrawerLayout` forwards the open/closed events here so
// we can update the framework's reactive `is_open` signal (which
// drives any author-side effects: hamburger icon toggle, screen
// dim, etc.).

/// # Safety
///
/// `ptr` must be the live pointer produced by `Box::into_raw` on a
/// `Box<DrawerListenerBox>` in `create_drawer`. Freed by `release`
/// when the navigator scope drops; in-flight calls between the
/// release and the Kotlin instance's GC are theoretically possible
/// but the listener is owned by the DrawerLayout, which is
/// detached from its parent before `release` runs.
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

/// Companion to the `RustDrawerLayout.nativeDrop` Kotlin
/// declaration. Currently unused — `release_drawer_navigator`
/// frees the listener box. Provided so the Kotlin class's
/// `external fun nativeDrop(...)` resolves at link time.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustDrawerLayout_nativeDrop(
    _env: jni::JNIEnv,
    _this: jni::objects::JObject,
    _ptr: jlong,
) {
    // No-op. The drawer listener pointer's lifetime is managed
    // from Rust's `release_drawer_navigator`, not from the Kotlin
    // class's finalize path.
}
