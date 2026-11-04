//! `VirtualizerPrim` — `androidx.recyclerview.widget.RecyclerView`
//! + a Kotlin `RustListAdapter` that trampolines every lifecycle event
//! back to a leaked `VirtualizerCallbacks` box.
//!
//! The framework still owns mount/release ordering — it hands us
//! callbacks, we hand them to Kotlin, Kotlin calls back through JNI
//! on every `onBindViewHolder` / `onViewRecycled`.

use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use runtime_shared::{Lanes, VirtualizerCallbacks, VirtualLayout};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use std::cell::RefCell;
use std::collections::HashMap;

// A mounted row is a Taffy root: it is built detached and hosted in a
// `RustRowContainer`, outside the framework tree. The layout pass computes
// every root against a box, and a row's box is its slot in the list, never
// the viewport: computed against the viewport, the first pass after a row
// mounted wrote the row a screen tall, so the list's first row filled it and
// no later row was bound at all.

/// What the layout pass needs about a mounted row.
#[derive(Clone)]
pub(crate) struct Row {
    view: GlobalRef,
    list: jlong,
    scope_id: u64,
    pub(crate) horizontal: bool,
    /// The author's main-axis size, in dp. `None` in a measured list, whose
    /// rows take their content's size.
    pub(crate) known_main: Option<f32>,
}

thread_local! {
    /// Mounted rows by their root view's key. An entry lives exactly as long
    /// as the row is bound.
    static ROWS: RefCell<HashMap<usize, Row>> = RefCell::new(HashMap::new());
    /// The callbacks pointer each live list's rows are recorded under, by the
    /// list's view key.
    static LIST_CALLBACKS: RefCell<HashMap<usize, jlong>> = RefCell::new(HashMap::new());
}

/// Record a row just mounted, and ask for the layout pass that gives it its
/// box: nothing else requests one when a row binds during a scroll.
pub(crate) fn note_row(
    view: &GlobalRef,
    list: jlong,
    scope_id: u64,
    horizontal: bool,
    known_main: Option<f32>,
) {
    let row = Row { view: view.clone(), list, scope_id, horizontal, known_main };
    ROWS.with(|r| r.borrow_mut().insert(AndroidBackend::node_key_of(view), row));
    crate::schedule_layout_pass();
}

/// Tear down the list `node`: its adapter goes, recycling every bound row
/// (each release is a no-op by now, the handler drained them first), so
/// nothing the toolkit still has queued can ask freed state for a count, and
/// no row stays behind in [`ROWS`].
pub(crate) fn release(node: &GlobalRef) {
    let key = AndroidBackend::node_key_of(node);
    with_env(|env| {
        let none = jni::objects::JObject::null();
        let detached = env.call_method(
            node.as_obj(),
            "setAdapter",
            "(Landroidx/recyclerview/widget/RecyclerView$Adapter;)V",
            &[JValue::Object(&none)],
        );
        if detached.is_err() {
            let _ = env.exception_clear();
        }
    });
    if let Some(list) = LIST_CALLBACKS.with(|l| l.borrow_mut().remove(&key)) {
        ROWS.with(|r| r.borrow_mut().retain(|_, row| row.list != list));
    }
}

/// Forget the row `scope_id` of `list`, which is being released.
pub(crate) fn forget_row(list: jlong, scope_id: u64) {
    ROWS.with(|r| r.borrow_mut().retain(|_, row| row.list != list || row.scope_id != scope_id));
}

/// The row whose root view has key `key`, if it is one.
pub(crate) fn row(key: usize) -> Option<Row> {
    ROWS.with(|r| r.borrow().get(&key).cloned())
}

/// The cross extent of `row`'s slot in dp: what the list gave its container.
/// `None` until the list has laid that container out.
pub(crate) fn row_cross(row: &Row) -> Option<f32> {
    with_env(|env| {
        let parent = env
            .call_method(row.view.as_obj(), "getParent", "()Landroid/view/ViewParent;", &[])
            .and_then(|v| v.l());
        let Ok(parent) = parent else {
            let _ = env.exception_clear();
            return None;
        };
        if parent.is_null() {
            return None;
        }
        let getter = if row.horizontal { "getHeight" } else { "getWidth" };
        let px = env.call_method(&parent, getter, "()I", &[]).and_then(|v| v.i());
        let Ok(px) = px else {
            let _ = env.exception_clear();
            return None;
        };
        let density = crate::imp::density_of(env, &parent).unwrap_or(1.0);
        (px > 0).then(|| px as f32 / density)
    })
}

pub(crate) fn create(
    b: &AndroidBackend,
    callbacks: VirtualizerCallbacks<GlobalRef>,
    overscan: f32,
    layout: VirtualLayout,
) -> GlobalRef {
    // Read before the box moves — the scroll listener is installed
    // conditionally and the callbacks are unreachable afterwards.
    let has_on_scroll = callbacks.on_scroll.is_some();

    // We leak the box to get a stable pointer; `nativeDrop` (called
    // from the adapter teardown path, if ever wired) frees it. The
    // Activity outlives the list in this demo so the leak is bounded.
    let boxed = Box::new(callbacks);
    let ptr = Box::into_raw(boxed) as jlong;

    // RecyclerView orientation constants: VERTICAL=1, HORIZONTAL=0
    // (shared by LinearLayoutManager + GridLayoutManager).
    let orientation_int = if layout.axis.is_horizontal() { 0 } else { 1 };

    with_env(|env| {
        // RecyclerView(Context).
        let rv_class = env
            .find_class("androidx/recyclerview/widget/RecyclerView")
            .expect("RecyclerView class — add androidx.recyclerview to the consuming app's Gradle deps");
        let rv = env
            .new_object(
                &rv_class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();

        // Pick the layout manager by lane config:
        //   - one lane  → RustLinearLayoutManager (a plain list)
        //   - Fixed(N)   → RustGridLayoutManager(spanCount = N)
        //   - AutoFit    → RustAutofitGridLayoutManager(minCross)
        // All three honor the overscan factor via the shared
        // `calculateExtraLayoutSpace` override.
        let lm = match layout.lanes {
            Lanes::Fixed(1) => {
                let lm_class = env
                    .find_class("io/idealyst/runtime/RustLinearLayoutManager")
                    .unwrap();
                env.new_object(
                    &lm_class,
                    "(Landroid/content/Context;IF)V",
                    &[
                        JValue::Object(&b.context.as_obj()),
                        JValue::Int(orientation_int),
                        JValue::Float(overscan),
                    ],
                )
                .unwrap()
            }
            Lanes::Fixed(n) => {
                let lm_class = env
                    .find_class("io/idealyst/runtime/RustGridLayoutManager")
                    .unwrap();
                env.new_object(
                    &lm_class,
                    "(Landroid/content/Context;IIF)V",
                    &[
                        JValue::Object(&b.context.as_obj()),
                        JValue::Int(n.max(1) as i32),
                        JValue::Int(orientation_int),
                        JValue::Float(overscan),
                    ],
                )
                .unwrap()
            }
            Lanes::AutoFit { min_cross } => {
                let lm_class = env
                    .find_class("io/idealyst/runtime/RustAutofitGridLayoutManager")
                    .unwrap();
                env.new_object(
                    &lm_class,
                    "(Landroid/content/Context;FFIF)V",
                    &[
                        JValue::Object(&b.context.as_obj()),
                        JValue::Float(min_cross),
                        JValue::Float(layout.cross_spacing),
                        JValue::Int(orientation_int),
                        JValue::Float(overscan),
                    ],
                )
                .unwrap()
            }
        };
        env.call_method(
            &rv,
            "setLayoutManager",
            "(Landroidx/recyclerview/widget/RecyclerView$LayoutManager;)V",
            &[JValue::Object(&lm)],
        )
        .unwrap();

        // Inter-item gaps. A single ItemDecoration distributes the
        // main-axis (inter-row) and cross-axis (inter-lane) spacing;
        // it reads the live span count from the layout manager so the
        // AutoFit case works too. Negative gaps go through it too
        // (rows overlap); see `needs_spacing_decoration`.
        if crate::layout_policy::needs_spacing_decoration(layout) {
            let dec_class = env
                .find_class("io/idealyst/runtime/RustGridSpacingDecoration")
                .unwrap();
            let dec = env
                .new_object(
                    &dec_class,
                    "(FFI)V",
                    &[
                        JValue::Float(layout.main_spacing),
                        JValue::Float(layout.cross_spacing),
                        JValue::Int(orientation_int),
                    ],
                )
                .unwrap();
            env.call_method(
                &rv,
                "addItemDecoration",
                "(Landroidx/recyclerview/widget/RecyclerView$ItemDecoration;)V",
                &[JValue::Object(&dec)],
            )
            .unwrap();
        }

        // RustListAdapter(nativePtr).
        let adapter_class = env
            .find_class("io/idealyst/runtime/RustListAdapter")
            .unwrap();
        let adapter = env
            .new_object(&adapter_class, "(J)V", &[JValue::Long(ptr)])
            .unwrap();
        env.call_method(
            &rv,
            "setAdapter",
            "(Landroidx/recyclerview/widget/RecyclerView$Adapter;)V",
            &[JValue::Object(&adapter)],
        )
        .unwrap();

        // Author scroll observer. Installed ONLY when the author
        // supplied one — a RecyclerView scroll listener fires on every
        // frame of a fling, so an unconditional listener would pay a
        // JNI crossing per frame for every list in the app.
        //
        // `has_on_scroll` is read off the leaked box (the callbacks
        // themselves already moved into it); the listener shares that
        // same pointer, so it must not outlive `nativeDrop`.
        if has_on_scroll {
            let listener_class = env
                .find_class("io/idealyst/runtime/RustScrollListener")
                .expect("RustScrollListener class — staged with the Kotlin runtime");
            let listener = env
                .new_object(&listener_class, "(J)V", &[JValue::Long(ptr)])
                .unwrap();
            env.call_method(
                &rv,
                "addOnScrollListener",
                "(Landroidx/recyclerview/widget/RecyclerView$OnScrollListener;)V",
                &[JValue::Object(&listener)],
            )
            .unwrap();
        }

        apply_default_layout_params(env, &rv);
        let node = env.new_global_ref(rv).unwrap();
        LIST_CALLBACKS.with(|l| l.borrow_mut().insert(AndroidBackend::node_key_of(&node), ptr));
        node
    })
}

/// Scroll the list so `index` is on screen. `scrollToPosition` hands
/// the work to the layout manager, which knows the live item extents —
/// recomputing an offset in Rust would drift once measured sizes
/// refine.
pub(crate) fn scroll_to_index(node: &GlobalRef, index: usize) {
    with_env(|env| {
        let _ = env.call_method(
            node.as_obj(),
            "scrollToPosition",
            "(I)V",
            &[JValue::Int(index as i32)],
        );
    });
}

/// Current scroll offset in density-independent units — the same space
/// `RustScrollListener` reports and the same space web/iOS/macOS use.
/// Density comes from the view's own display (`density_of`), matching
/// `AndroidScrollViewOps::scroll_to`.
pub(crate) fn scroll_offset(node: &GlobalRef) -> (f32, f32) {
    with_env(|env| {
        let view = node.as_obj();
        let d = crate::imp::density_of(env, &view).unwrap_or(1.0);
        let x = env
            .call_method(&view, "computeHorizontalScrollOffset", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0) as f32;
        let y = env
            .call_method(&view, "computeVerticalScrollOffset", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0) as f32;
        (x / d, y / d)
    })
}

/// Scroll to an absolute offset. RecyclerView deliberately does NOT
/// support `View.scrollTo` (it throws / corrupts its own layout
/// bookkeeping) — the supported form is the relative `scrollBy`, so
/// the delta is computed against the current offset. That's why this
/// reads the offset first instead of assuming a starting position.
pub(crate) fn scroll_to(node: &GlobalRef, x: f32, y: f32) {
    let (cur_x, cur_y) = scroll_offset(node);
    with_env(|env| {
        let view = node.as_obj();
        let d = crate::imp::density_of(env, &view).unwrap_or(1.0);
        let dx = ((x - cur_x) * d).round() as i32;
        let dy = ((y - cur_y) * d).round() as i32;
        if dx == 0 && dy == 0 {
            return;
        }
        let _ = env.call_method(
            &view,
            "scrollBy",
            "(II)V",
            &[JValue::Int(dx), JValue::Int(dy)],
        );
    });
}

pub(crate) fn data_changed(node: &GlobalRef) {
    // Fetch the RecyclerView's adapter and tell it the data changed.
    // The adapter computes a key diff against its last snapshot and
    // dispatches granular updates so surviving items don't rebind.
    with_env(|env| {
        let adapter = match env.call_method(
            node.as_obj(),
            "getAdapter",
            "()Landroidx/recyclerview/widget/RecyclerView$Adapter;",
            &[],
        ) {
            Ok(v) => v.l().unwrap(),
            Err(_) => return,
        };
        if adapter.is_null() {
            return;
        }
        // `dataChanged()` lives on RustListAdapter, not the base
        // Adapter type — Java dispatch finds it by the runtime class.
        let _ = env.call_method(&adapter, "dataChanged", "()V", &[]);
    });
}
