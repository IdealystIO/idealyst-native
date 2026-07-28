//! Container builders: `view()`, `pressable()`, `scroll_view()`.

use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::scroll_view::ScrollViewHandle;
use runtime_core::{
    FileDropHandler, PressableHandle, SafeAreaSides, TouchHandler, ViewHandle,
    WheelHandler,
};
use runtime_scene::{item, Element};
use runtime_world::IntoValue;

use crate::prims::{PressablePrim, PrimCell, ScrollViewPrim, ViewPrim};
use crate::style_attach::IntoStyleProp;

use super::SceneChild;

/// Start a `view` — the plain container primitive.
pub fn view() -> ViewBuilder {
    ViewBuilder {
        prim: ViewPrim {
            test_id: None,
            style: None,
            safe_area: SafeAreaSides::NONE,
            on_touch: None,
            on_wheel: None,
            on_hover: None,
            on_file_drop: None,
            preserves_focus: false,
            is_container: false,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
        children: Vec::new(),
    }
}

pub struct ViewBuilder {
    prim: ViewPrim,
    children: Vec<Element>,
}

impl ViewBuilder {
    pub fn child(mut self, child: impl SceneChild) -> Self {
        self.children.push(child.into_child());
        self
    }

    /// Append many pre-built children (e.g. from a `.map().collect()`).
    pub fn children(mut self, children: Vec<Element>) -> Self {
        self.children.extend(children);
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn safe_area(mut self, sides: SafeAreaSides) -> Self {
        self.prim.safe_area = sides;
        self
    }

    pub fn on_touch(mut self, handler: TouchHandler) -> Self {
        self.prim.on_touch = Some(handler);
        self
    }

    pub fn on_wheel(mut self, handler: WheelHandler) -> Self {
        self.prim.on_wheel = Some(handler);
        self
    }

    /// Pointer-enter/leave notifications (`true` on enter).
    pub fn on_hover(mut self, handler: impl Fn(bool) + 'static) -> Self {
        self.prim.on_hover = Some(Rc::new(handler));
        self
    }

    pub fn on_file_drop(mut self, handler: FileDropHandler) -> Self {
        self.prim.on_file_drop = Some(handler);
        self
    }

    pub fn preserves_focus(mut self, preserve: bool) -> Self {
        self.prim.preserves_focus = preserve;
        self
    }

    /// Mark this view a container-query containment context.
    pub fn container(mut self) -> Self {
        self.prim.is_container = true;
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Receive the imperative-ref handle at mount (P2 form of `.bind`).
    /// Robot/automation anchor (`test_id = …`): the mount handler
    /// registers this node under `id` in the vocabulary robot registry
    /// (`robot` feature; the slot is inert otherwise).
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.prim.test_id = Some(id);
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(ViewHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), self.children)
    }
}

/// Start a `pressable` — a tappable container (no UA chrome).
pub fn pressable(on_press: impl Fn() + 'static) -> PressableBuilder {
    PressableBuilder {
        prim: PressablePrim {
            test_id: None,
            on_press: Rc::new(on_press),
            disabled: None,
            preserves_focus: false,
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
        children: Vec::new(),
    }
}

pub struct PressableBuilder {
    prim: PressablePrim,
    children: Vec<Element>,
}

impl PressableBuilder {
    pub fn child(mut self, child: impl SceneChild) -> Self {
        self.children.push(child.into_child());
        self
    }

    pub fn children(mut self, children: Vec<Element>) -> Self {
        self.children.extend(children);
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn disabled(mut self, disabled: impl IntoValue<bool>) -> Self {
        self.prim.disabled = Some(disabled.into_value());
        self
    }

    pub fn preserves_focus(mut self, preserve: bool) -> Self {
        self.prim.preserves_focus = preserve;
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Robot/automation anchor (`test_id = …`): the mount handler
    /// registers this node under `id` in the vocabulary robot registry
    /// (`robot` feature; the slot is inert otherwise).
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.prim.test_id = Some(id);
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(PressableHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), self.children)
    }
}

/// Start a `scroll_view` — a scrolling container (vertical by default).
pub fn scroll_view() -> ScrollViewBuilder {
    ScrollViewBuilder {
        prim: ScrollViewPrim {
            test_id: None,
            horizontal: false,
            on_scroll: None,
            safe_area: SafeAreaSides::NONE,
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
        children: Vec::new(),
    }
}

pub struct ScrollViewBuilder {
    prim: ScrollViewPrim,
    children: Vec<Element>,
}

impl ScrollViewBuilder {
    pub fn child(mut self, child: impl SceneChild) -> Self {
        self.children.push(child.into_child());
        self
    }

    pub fn children(mut self, children: Vec<Element>) -> Self {
        self.children.extend(children);
        self
    }

    pub fn horizontal(mut self, horizontal: bool) -> Self {
        self.prim.horizontal = horizontal;
        self
    }

    /// Per-offset-change scroll notifications (CSS px / native points).
    pub fn on_scroll(mut self, handler: impl Fn(f32, f32) + 'static) -> Self {
        self.prim.on_scroll = Some(Rc::new(handler));
        self
    }

    pub fn safe_area(mut self, sides: SafeAreaSides) -> Self {
        self.prim.safe_area = sides;
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Robot/automation anchor (`test_id = …`): the mount handler
    /// registers this node under `id` in the vocabulary robot registry
    /// (`robot` feature; the slot is inert otherwise).
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.prim.test_id = Some(id);
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(ScrollViewHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), self.children)
    }
}
