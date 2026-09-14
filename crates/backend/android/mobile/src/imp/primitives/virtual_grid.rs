//! `virtual_grid` on Android — the Kotlin `RustVirtualGrid` ViewGroup
//! plus the framework-side windowing.
//!
//! See `RustVirtualGrid.kt` for why this is a hand-rolled two-axis
//! `ViewGroup` rather than a `RecyclerView` with a custom
//! `LayoutManager`. The split of responsibility here is the same as
//! every other backend's grid engine: Kotlin owns the gesture, the
//! fling and the child canvas offset; Rust owns which cells exist and
//! where, via the shared
//! `runtime_shared::primitives::virtual_grid::GridMetrics`.
//!
//! ## Units
//!
//! Metrics are dp (what `StyleRules` and Taffy reason in). Android
//! view geometry is device pixels, so every frame and content extent
//! handed across the JNI boundary is converted at the call site —
//! never the inputs, matching the sticky module's discipline
//! ([[project_android_setTranslation_device_px]]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use runtime_shared::primitives::virtual_grid::{GridCallbacks, GridMetrics, GridWindow};

struct MountedCell {
    view: GlobalRef,
    scope_id: u64,
}

/// Per-grid state. Boxed and leaked to give Kotlin a stable pointer,
/// exactly like the 1-D adapter's callbacks box; `release` reclaims it.
pub(crate) struct GridState {
    pub(crate) grid_view: GlobalRef,
    pub(crate) callbacks: RefCell<Option<GridCallbacks<GlobalRef>>>,
    pub(crate) metrics: RefCell<GridMetrics>,
    mounted: RefCell<HashMap<(usize, usize), MountedCell>>,
    last_window: RefCell<Option<GridWindow>>,
    /// Viewport in dp, reported by Kotlin's `onLayout`. Zero until the
    /// first layout pass — windowing against zero would mount nothing
    /// and cache that as current, so `sync` bails instead.
    viewport: RefCell<(f32, f32)>,
    overscan: f32,
    density: RefCell<f32>,
}

/// Registry keyed by the grid view's JObject pointer, mirroring the
/// sticky registry's keying scheme.
pub(crate) type GridRegistry = HashMap<usize, Rc<GridState>>;

pub(crate) fn create(
    b: &AndroidBackend,
    registry: &mut GridRegistry,
    callbacks: GridCallbacks<GlobalRef>,
    overscan: f32,
) -> GlobalRef {
    let metrics = GridMetrics::build(
        (callbacks.col_count)(),
        (callbacks.row_count)(),
        &*callbacks.col_width,
        &*callbacks.row_height,
    );

    let state = Rc::new(GridState {
        // Filled in below once the view exists; a placeholder global
        // ref would need a JObject we don't have yet, so the field is
        // written after construction via `Rc::get_mut`-free interior
        // mutability on the registry entry instead.
        grid_view: with_env(|env| {
            // Construct the Kotlin ViewGroup with a null native
            // pointer first; the real pointer is set below, once the
            // `Rc` has an address. Kotlin never dereferences it before
            // its first layout pass, which cannot happen inside this
            // function.
            let cls = env
                .find_class("io/idealyst/runtime/RustVirtualGrid")
                .expect("RustVirtualGrid class — staged with the Kotlin runtime");
            let obj = env
                .new_object(
                    &cls,
                    "(Landroid/content/Context;J)V",
                    &[JValue::Object(&b.context.as_obj()), JValue::Long(0)],
                )
                .expect("RustVirtualGrid construction");
            env.new_global_ref(obj).expect("global ref for grid view")
        }),
        callbacks: RefCell::new(Some(callbacks)),
        metrics: RefCell::new(metrics),
        mounted: RefCell::new(HashMap::new()),
        last_window: RefCell::new(None),
        viewport: RefCell::new((0.0, 0.0)),
        overscan,
        density: RefCell::new(1.0),
    });

    let key = state.grid_view.as_obj().as_raw() as usize;
    // Hand Kotlin the registry KEY, not a raw `Rc` pointer. The key is
    // stable, and a stale callback after teardown looks up a missing
    // entry and no-ops — where a raw pointer would be a use-after-free.
    with_env(|env| {
        let _ = env.set_field(
            state.grid_view.as_obj(),
            "nativePtr",
            "J",
            JValue::Long(key as jlong),
        );
        // `nativePtr` is a `val` constructor property on the Kotlin
        // side, so the field write above can fail on a runtime that
        // enforces finality. Clear any pending exception so it can't
        // leak into the next JNI call
        // ([[project_android_net_pending_exception_clear]]).
        let _ = env.exception_clear();
        apply_content_size(env, &state);
    });

    let view = state.grid_view.clone();
    registry.insert(key, state);
    view
}

fn apply_content_size(env: &mut jni::JNIEnv, state: &GridState) {
    let density = *state.density.borrow();
    let (w, h) = state.metrics.borrow().content_size();
    let _ = env.call_method(
        state.grid_view.as_obj(),
        "setContentSize",
        "(II)V",
        &[
            JValue::Int((w * density).round() as i32),
            JValue::Int((h * density).round() as i32),
        ],
    );
    let _ = env.exception_clear();
}

/// Kotlin reports a new viewport (first layout, rotation, resize).
pub(crate) fn viewport_changed(
    backend: &mut AndroidBackend,
    key: usize,
    width_px: i32,
    height_px: i32,
) {
    let Some(state) = backend.virtual_grid_registry.get(&key).cloned() else {
        return;
    };
    let density = with_env(|env| {
        crate::imp::density_of(env, &state.grid_view.as_obj()).unwrap_or(1.0)
    });
    let density = if density <= 0.0 { 1.0 } else { density };
    *state.density.borrow_mut() = density;
    *state.viewport.borrow_mut() = (width_px as f32 / density, height_px as f32 / density);
    // The cached window is keyed to a viewport size, so a new viewport
    // must invalidate it — a resize that leaves the window's INDICES
    // unchanged still needs the content extent re-applied.
    *state.last_window.borrow_mut() = None;
    with_env(|env| apply_content_size(env, &state));
    sync(backend, key);
}

/// Counts or sizes changed.
pub(crate) fn data_changed(backend: &mut AndroidBackend, node: &GlobalRef) {
    let key = node.as_obj().as_raw() as usize;
    let Some(state) = backend.virtual_grid_registry.get(&key).cloned() else {
        return;
    };
    let rebuilt = state.callbacks.borrow().as_ref().map(|cb| {
        GridMetrics::build(
            (cb.col_count)(),
            (cb.row_count)(),
            &*cb.col_width,
            &*cb.row_height,
        )
    });
    let Some(m) = rebuilt else { return };
    *state.metrics.borrow_mut() = m;
    *state.last_window.borrow_mut() = None;
    with_env(|env| apply_content_size(env, &state));
    sync(backend, key);
    // The one queueing path with no drain already behind it —
    // `viewport_changed` and `on_scroll` are both drained by their JNI
    // trampolines. Safe where `queue_sync` is not: this fires on a real
    // data change, not once per pass.
    crate::schedule_layout_pass();
}

/// Re-window: diff the visible cell rect and mount/release the delta.
enum PendingJob {
    Sync(usize, Rc<GridState>),
    Teardown(TeardownJob),
}

/// A grid's cells, taken out of the registry, waiting to be dropped.
struct TeardownJob {
    grid_view: GlobalRef,
    cells: Vec<MountedCell>,
    release_cell: Option<Rc<dyn Fn(u64)>>,
}

thread_local! {
    /// Work that must run with NO backend borrow held.
    ///
    /// `mount_cell` realizes a subtree and `release_cell` drops one, and
    /// both re-enter the backend through `create_*` / scope cleanups —
    /// `handlers::view::mount_view` opens with `backend.borrow_mut()`.
    /// Every path into a sync already holds that borrow: the layout pass,
    /// the `virtual_grid_data_changed` dispatch, and the scroll callback
    /// from `RustVirtualGrid`'s JNI export. Cloning `GridState` out of
    /// the registry does not end the borrow — it is the CALLER's.
    ///
    /// So the borrow-free half is queued here and drained where the
    /// borrow is provably gone. Cells mount one pass later than the
    /// layout that discovered them and mark the tree dirty on mount, so
    /// the following pass frames them.
    ///
    /// Proven on iOS 2026-09-14, where wiring `GridOps` up for the first
    /// time aborted the app with `RefCell already borrowed` inside
    /// `virtual_grid::mount_cell` the moment a grid mounted. This
    /// backend had the identical structure and the identical latent bug.
    static PENDING: RefCell<Vec<PendingJob>> = const { RefCell::new(Vec::new()) };

    /// Mounted cells' view keys → the box this engine framed them at,
    /// keyed the way `view_to_layout` is.
    ///
    /// A mounted cell's root view has no Taffy PARENT — the engine
    /// places it in the grid's content space — so the layout pass sees
    /// it as a ROOT, computes it against the VIEWPORT and writes the
    /// result over the engine's frame. On iOS that turned every 40x44
    /// cell into 40x956 stacked at the origin. The pass consults this
    /// map to compute a cell against its own box and to leave its frame
    /// alone; the cell's subtree still lays out, inside the cell.
    static CELL_BOXES: RefCell<HashMap<usize, (f32, f32)>> = RefCell::new(HashMap::new());
}

/// The box a mounted grid cell was framed at, or `None` for any view
/// that is not a grid cell root. Consulted by `run_layout_pass`.
pub(crate) fn cell_box(view_key: usize) -> Option<(f32, f32)> {
    CELL_BOXES.with(|m| m.borrow().get(&view_key).copied())
}

/// Queue a sync, replacing any already queued for the same grid — the
/// diff is computed at drain time, so the later subsumes the earlier.
fn queue_sync(key: usize, state: Rc<GridState>) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if let Some(slot) = p.iter_mut().find(
            |j| matches!(j, PendingJob::Sync(k, _) if *k == key),
        ) {
            *slot = PendingJob::Sync(key, state);
        } else {
            p.push(PendingJob::Sync(key, state));
        }
    });
    // NO `schedule_layout_pass()` here, and the omission is
    // load-bearing. `sync_all` queues EVERY registered grid on every
    // layout pass, so a schedule from this function makes each pass
    // arm the next one and the app spins the main thread forever —
    // measured at 25,488 passes in 90s, a constant view count, taps
    // and the robot bridge both dead. The queue does not need it: the
    // pass that queued this drains it on the way out, and the mounts
    // that drain performs dirty the tree themselves, which schedules
    // the pass that frames them. That settles, because a re-queued
    // sync whose window has not moved returns without mounting.
}

/// Run the queued mounts, releases and teardowns.
///
/// **Call only where the backend borrow is provably released**, or this
/// puts back the abort the queue exists to prevent. Loops because a
/// teardown's scope cleanups can queue another grid's sync.
pub(crate) fn drain_pending() {
    for _ in 0..8 {
        let jobs: Vec<PendingJob> = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
        if jobs.is_empty() {
            return;
        }
        for job in jobs {
            match job {
                PendingJob::Sync(_, state) => sync_now(&state),
                PendingJob::Teardown(job) => teardown_now(job),
            }
        }
    }
}

/// Drop a torn-down grid's cells, with no backend borrow held — the
/// two-axis form of the virtualizer teardown bug (FRAMEWORK-NOTES
/// Wave-19).
fn teardown_now(job: TeardownJob) {
    for cell in job.cells {
        CELL_BOXES.with(|m| {
            m.borrow_mut()
                .remove(&crate::imp::AndroidBackend::node_key(&cell.view))
        });
        with_env(|env| {
            let _ = env.call_method(
                job.grid_view.as_obj(),
                "removeView",
                "(Landroid/view/View;)V",
                &[JValue::Object(&cell.view.as_obj())],
            );
        });
        if let Some(release) = job.release_cell.as_ref() {
            release(cell.scope_id);
        }
    }
}

pub(crate) fn sync(backend: &mut AndroidBackend, key: usize) {
    let Some(state) = backend.virtual_grid_registry.get(&key).cloned() else {
        return;
    };
    // The registry lookup is the only part needing the backend, and the
    // caller holds its borrow — hand the rest to the queue. See `PENDING`.
    queue_sync(key, state);
}

/// The body of the old `sync`, with no backend borrow held.
fn sync_now(state: &GridState) {
    let viewport = *state.viewport.borrow();
    if viewport.0 <= 0.0 || viewport.1 <= 0.0 {
        // Layout hasn't run yet. Bail WITHOUT caching so the next
        // viewport report retries.
        return;
    }
    let density = *state.density.borrow();
    let scroll = with_env(|env| {
        let x = env
            .call_method(state.grid_view.as_obj(), "getScrollX", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0) as f32;
        let y = env
            .call_method(state.grid_view.as_obj(), "getScrollY", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0) as f32;
        (x / density, y / density)
    });

    let window = state
        .metrics
        .borrow()
        .visible_window(scroll, viewport, state.overscan);
    if *state.last_window.borrow() == Some(window) {
        return;
    }
    *state.last_window.borrow_mut() = Some(window);

    // Release cells that left the window first, so their scopes free
    // before the incoming ones allocate.
    let leaving: Vec<(usize, usize)> = state
        .mounted
        .borrow()
        .keys()
        .copied()
        .filter(|(c, r)| !window.contains(*c, *r))
        .collect();
    for slot in leaving {
        let cell = state.mounted.borrow_mut().remove(&slot);
        let Some(cell) = cell else { continue };
        CELL_BOXES.with(|m| {
            m.borrow_mut()
                .remove(&crate::imp::AndroidBackend::node_key(&cell.view))
        });
        with_env(|env| {
            let _ = env.call_method(
                state.grid_view.as_obj(),
                "removeView",
                "(Landroid/view/View;)V",
                &[JValue::Object(&cell.view.as_obj())],
            );
            let _ = env.exception_clear();
        });
        let release = state.callbacks.borrow().as_ref().map(|c| c.release_cell.clone());
        if let Some(release) = release {
            release(cell.scope_id);
        }
    }

    // Mount cells that entered.
    for (col, row) in window.cells() {
        if state.mounted.borrow().contains_key(&(col, row)) {
            continue;
        }
        let mount = state.callbacks.borrow().as_ref().map(|c| c.mount_cell.clone());
        let Some(mount) = mount else { break };
        let (view, scope_id) = mount(col, row);
        let (x, y) = state.metrics.borrow().cell_origin(col, row);
        let (w, h) = state.metrics.borrow().cell_size(col, row);
        // Tell the layout pass this root is a cell, not a screen — see
        // `CELL_BOXES`. Kotlin's `setCellFrame` re-applies the FRAME on
        // every pass, so the visible symptom here is milder than iOS's;
        // the cell's SUBTREE would still be computed against the
        // viewport without this.
        CELL_BOXES.with(|m| {
            m.borrow_mut()
                .insert(crate::imp::AndroidBackend::node_key(&view), (w, h))
        });
        with_env(|env| {
            let _ = env.call_method(
                state.grid_view.as_obj(),
                "addView",
                "(Landroid/view/View;)V",
                &[JValue::Object(&view.as_obj())],
            );
            // Content-space frame in device pixels; `RustVirtualGrid`
            // stores it and re-applies it on every layout pass, so the
            // frame survives Android re-laying the container out.
            let _ = env.call_method(
                state.grid_view.as_obj(),
                "setCellFrame",
                "(Landroid/view/View;IIII)V",
                &[
                    JValue::Object(&view.as_obj()),
                    JValue::Int((x * density).round() as i32),
                    JValue::Int((y * density).round() as i32),
                    JValue::Int((w * density).round() as i32),
                    JValue::Int((h * density).round() as i32),
                ],
            );
            let _ = env.exception_clear();
        });
        state
            .mounted
            .borrow_mut()
            .insert((col, row), MountedCell { view, scope_id });
    }
}

/// Author scroll observer + re-window, called from the Kotlin
/// `onScrollChanged` trampoline. One path for drags, flings and
/// programmatic scrolls.
pub(crate) fn on_scroll(backend: &mut AndroidBackend, key: usize, x: f32, y: f32) {
    sync(backend, key);
    let Some(state) = backend.virtual_grid_registry.get(&key).cloned() else {
        return;
    };
    let author = state.callbacks.borrow().as_ref().and_then(|c| c.on_scroll.clone());
    if let Some(f) = author {
        f(x, y);
    }
}

pub(crate) fn release(backend: &mut AndroidBackend, node: &GlobalRef) {
    let key = node.as_obj().as_raw() as usize;
    let Some(state) = backend.virtual_grid_registry.remove(&key) else {
        return;
    };
    // Take the callbacks out FIRST so a scroll event still in flight
    // sees `None` and bails rather than mounting into a torn-down grid
    // — the same guard the 1-D data source uses.
    let cbs = state.callbacks.borrow_mut().take();
    let release_cell = cbs.as_ref().map(|c| c.release_cell.clone());
    let drained: Vec<MountedCell> = state
        .mounted
        .borrow_mut()
        .drain()
        .map(|(_, v)| v)
        .collect();
    with_env(|env| {
        let _ = env.call_method(state.grid_view.as_obj(), "removeAllViews", "()V", &[]);
        let _ = env.exception_clear();
    });
    // Queued, not run: this runs under the caller's backend borrow, and
    // dropping a cell scope re-enters it. `removeAllViews` above already
    // detached them, so the teardown job only drops the scopes.
    PENDING.with(|p| {
        p.borrow_mut().push(PendingJob::Teardown(TeardownJob {
            grid_view: state.grid_view.clone(),
            cells: drained,
            release_cell,
        }))
    });
    crate::schedule_layout_pass();
}

/// Scroll so `(col, row)` sits at the leading corner. The origin comes
/// from the LIVE metrics — column widths may have changed since mount.
pub(crate) fn scroll_to_cell(backend: &AndroidBackend, node: &GlobalRef, col: usize, row: usize) {
    let key = node.as_obj().as_raw() as usize;
    let Some(state) = backend.virtual_grid_registry.get(&key) else {
        return;
    };
    let (x, y) = state.metrics.borrow().cell_origin(col, row);
    scroll_to(node, x, y);
}

pub(crate) fn scroll_offset(node: &GlobalRef) -> (f32, f32) {
    with_env(|env| {
        let view = node.as_obj();
        let d = crate::imp::density_of(env, &view).unwrap_or(1.0);
        let d = if d <= 0.0 { 1.0 } else { d };
        let x = env
            .call_method(&view, "getScrollX", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0) as f32;
        let y = env
            .call_method(&view, "getScrollY", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0) as f32;
        (x / d, y / d)
    })
}

/// Absolute scroll in dp. `RustVirtualGrid.scrollToDp` does the
/// conversion and the clamp on the Kotlin side, where the content
/// extent lives.
pub(crate) fn scroll_to(node: &GlobalRef, x: f32, y: f32) {
    with_env(|env| {
        let _ = env.call_method(
            node.as_obj(),
            "scrollToDp",
            "(FF)V",
            &[JValue::Float(x), JValue::Float(y)],
        );
        let _ = env.exception_clear();
    });
}
