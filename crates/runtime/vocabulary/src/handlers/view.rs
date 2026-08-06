//! Container handlers: `view`, `pressable`, `scroll_view`.

use std::cell::Cell;
use std::rc::Rc;

use runtime_scene::{Element, MountCx};

use crate::caps::{InputOps, IntrospectionOps, PressableOps, SafeAreaOps, ScrollOps, ViewOps};
use crate::prims::{PressablePrim, ScrollViewPrim, ViewPrim};
use crate::style_attach::{attach_style, StyleServices};

use super::bind_value;

/// Mount a `view` — port of `walker/view.rs::build`.
///
/// Sequence: `create_view` → (`mark_container`) → children →
/// attach_style → safe-area → touch/wheel/hover/file-drop →
/// `mark_preserves_focus` → ref-fill.
///
/// Deviations from the walker, per the P2 scope (crate docs):
/// - safe-area applies ONCE at mount (`apply_safe_area_padding`); the
///   old inset-signal re-application effect is deferred with the
///   flush-driver work (P3) — the insets signal lives in the old
///   reactive world.
/// - `mark_container` is emitted, but the native inline-size feedback
///   signal the old walker builds on non-cascade backends is deferred
///   with the style engine.
pub fn mount_view<H>(cx: &mut MountCx<'_, H>, prim: ViewPrim, children: Vec<Element>) -> H::Node
where
    H: ViewOps + InputOps + StyleServices + SafeAreaOps + IntrospectionOps,
{
    let backend = cx.backend().clone();
    let mut node = backend.borrow_mut().create_view(&prim.a11y);
    // Identity/robot registration: BEFORE children realize, so they
    // link to this view on the parent stack (guard pops on scope exit).
    #[cfg(feature = "robot")]
    let _robot = crate::robot::register_mount(
        &backend,
        &node,
        crate::robot::ElementKind::View,
        prim.test_id,
        None,
        None,
        crate::robot::MountActions::default(),
    );
    if prim.is_container {
        // Before children, so descendants build inside the containment
        // context (walker `build_view` ordering).
        backend.borrow_mut().mark_container(&node);
    }
    cx.realize_children_into(&mut node, children);
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }
    if !prim.safe_area.is_empty() {
        backend
            .borrow_mut()
            .apply_safe_area_padding(&node, prim.safe_area);
    }
    // Input handlers are the longest-lived callbacks a backend holds: they
    // live on gesture recognizers / event controllers whose lifetime the
    // toolkit owns, so they are the most likely to be invoked after the
    // scope dies. Guarded before hand-off — see `callback_guard`.
    if prim.on_touch.is_some()
        || prim.on_wheel.is_some()
        || prim.on_hover.is_some()
        || prim.on_file_drop.is_some()
    {
        let alive = crate::callback_guard::ScopeAlive::current();
        if let Some(h) = prim.on_touch {
            backend
                .borrow_mut()
                .install_touch_handler(&node, alive.wrap_touch(h));
        }
        if let Some(h) = prim.on_wheel {
            backend
                .borrow_mut()
                .install_wheel_handler(&node, alive.wrap_wheel(h));
        }
        if let Some(h) = prim.on_hover {
            backend
                .borrow_mut()
                .install_hover_handler(&node, alive.wrap_hover(h));
        }
        if let Some(h) = prim.on_file_drop {
            backend
                .borrow_mut()
                .install_file_drop_handler(&node, alive.wrap_file_drop(h));
        }
    }
    if prim.preserves_focus {
        backend.borrow_mut().mark_preserves_focus(&node);
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_view_handle(&node);
        fill(handle);
    }
    node
}

/// Mount a `pressable` — port of `walker/pressable.rs::build`.
///
/// Sequence: (press-block wrap) → `create_pressable` → children →
/// attach_style → ref-fill → disabled binding → `mark_preserves_focus`.
///
/// The press-block wrap is the walker's uniform-disable mechanism: a
/// bare pressable is not a native form control, so `set_disabled` alone
/// can't make it inert — the shared flag blocks the callback across
/// mouse/keyboard/programmatic activation on every backend.
pub fn mount_pressable<H>(
    cx: &mut MountCx<'_, H>,
    prim: PressablePrim,
    children: Vec<Element>,
) -> H::Node
where
    H: PressableOps + InputOps + StyleServices + IntrospectionOps,
{
    // Robot `click` gets the RAW author callback (pre press-block wrap),
    // mirroring the old walker's `robot_extract_meta`, which read the
    // element's `on_click` before the build wrapped it.
    #[cfg(feature = "robot")]
    let robot_click = prim.on_press.clone();
    let (on_press, press_block): (Rc<dyn Fn()>, Option<Rc<Cell<bool>>>) = if prim.disabled.is_some()
    {
        let flag = Rc::new(Cell::new(false));
        let flag_for_press = flag.clone();
        let inner = prim.on_press;
        let wrapped: Rc<dyn Fn()> = Rc::new(move || {
            if !flag_for_press.get() {
                (inner)();
            }
        });
        (wrapped, Some(flag))
    } else {
        (prim.on_press, None)
    };

    let backend = cx.backend().clone();
    // Scope-guard before the backend ever sees it: a native toolkit can
    // invoke a stored callback after this node's scope dies (GTK fires
    // `focus-leave` mid-unparent; a deferred run-loop source outlives a
    // route change), and the resulting stale-signal panic is raised inside
    // a non-unwinding C trampoline, which ABORTS. See `callback_guard`.
    let alive = crate::callback_guard::ScopeAlive::current();
    let on_press = alive.wrap0(on_press);
    let mut node = backend.borrow_mut().create_pressable(on_press, &prim.a11y);
    #[cfg(feature = "robot")]
    let _robot = crate::robot::register_mount(
        &backend,
        &node,
        crate::robot::ElementKind::Pressable,
        prim.test_id,
        None,
        None,
        crate::robot::MountActions {
            click: Some(robot_click),
            ..Default::default()
        },
    );
    cx.realize_children_into(&mut node, children);
    let state_setter = prim
        .style
        .map(|style| attach_style(&backend, &node, style));
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_pressable_handle(&node);
        fill(handle);
    }
    if let Some(disabled) = prim.disabled {
        let b = backend.clone();
        let n = node.clone();
        // Old `attach_disabled` ordering: press-block flag, native
        // set_disabled, then the DISABLED state-bit flip so a
        // `state disabled { … }` overlay applies via the state machinery.
        bind_value(disabled, move |&d| {
            if let Some(flag) = press_block.as_ref() {
                flag.set(d);
            }
            b.borrow_mut().set_disabled(&n, d);
            if let Some(setter) = state_setter.as_ref() {
                setter(runtime_shared::StateBits::DISABLED, d);
            }
        });
    }
    if prim.preserves_focus {
        backend.borrow_mut().mark_preserves_focus(&node);
    }
    node
}

/// Mount a `scroll_view` — port of `walker/scroll_view.rs::build`.
///
/// Sequence: `create_scroll_view` → children → attach_style →
/// safe-area (contentInset path, once — reactive insets deferred like
/// `view`) → ref-fill.
pub fn mount_scroll_view<H>(
    cx: &mut MountCx<'_, H>,
    prim: ScrollViewPrim,
    children: Vec<Element>,
) -> H::Node
where
    H: ScrollOps + SafeAreaOps + StyleServices + IntrospectionOps,
{
    let backend = cx.backend().clone();
    // `on_scroll` is the worst offender for post-teardown delivery: some
    // backends must defer it to a run-loop source (calling it inline
    // re-enters the reactive runtime mid-allocation), so the call can land
    // well after a route change dropped the screen. See `callback_guard`.
    let alive = crate::callback_guard::ScopeAlive::current();
    let on_scroll = alive.wrap2_opt(prim.on_scroll);
    let mut node = backend
        .borrow_mut()
        .create_scroll_view(prim.horizontal, on_scroll, &prim.a11y);
    // Robot `set_scroll` routes through the scroll HANDLE (whose ops
    // take no backend borrow), NOT `set_node_scroll` under a live
    // `borrow_mut`: native scroll writes fire scroll notifications
    // synchronously (AppKit `reflectScrolledClipView:`), whose reactive
    // effects re-borrow the backend to re-style — a held borrow aborts
    // with "RefCell already borrowed" (the old walker's exact rule).
    #[cfg(feature = "robot")]
    let _robot = {
        let handle = backend.borrow().make_scroll_view_handle(&node);
        crate::robot::register_mount(
            &backend,
            &node,
            crate::robot::ElementKind::ScrollView,
            prim.test_id,
            None,
            None,
            crate::robot::MountActions {
                set_scroll: Some(Rc::new(move |x, y| handle.scroll_to(x, y))),
                ..Default::default()
            },
        )
    };
    cx.realize_children_into(&mut node, children);
    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }
    if !prim.safe_area.is_empty() {
        backend
            .borrow_mut()
            .apply_scroll_view_safe_area_inset(&node, prim.safe_area);
    }
    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_scroll_view_handle(&node);
        fill(handle);
    }
    node
}
