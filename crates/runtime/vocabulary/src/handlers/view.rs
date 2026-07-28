//! Container handlers: `view`, `pressable`, `scroll_view`.

use std::cell::Cell;
use std::rc::Rc;

use runtime_scene::{Element, MountCx};

use crate::caps::{InputOps, PressableOps, SafeAreaOps, ScrollOps, ViewOps};
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
    H: ViewOps + InputOps + StyleServices + SafeAreaOps,
{
    let backend = cx.backend().clone();
    let mut node = backend.borrow_mut().create_view(&prim.a11y);
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
    if let Some(h) = prim.on_touch {
        backend.borrow_mut().install_touch_handler(&node, h);
    }
    if let Some(h) = prim.on_wheel {
        backend.borrow_mut().install_wheel_handler(&node, h);
    }
    if let Some(h) = prim.on_hover {
        backend.borrow_mut().install_hover_handler(&node, h);
    }
    if let Some(h) = prim.on_file_drop {
        backend.borrow_mut().install_file_drop_handler(&node, h);
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
    H: PressableOps + InputOps + StyleServices,
{
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
    let mut node = backend.borrow_mut().create_pressable(on_press, &prim.a11y);
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
                setter(runtime_core::StateBits::DISABLED, d);
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
    H: ScrollOps + SafeAreaOps + StyleServices,
{
    let backend = cx.backend().clone();
    let mut node = backend
        .borrow_mut()
        .create_scroll_view(prim.horizontal, prim.on_scroll, &prim.a11y);
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
