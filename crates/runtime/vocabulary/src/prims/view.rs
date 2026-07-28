//! Container payloads: `view`, `pressable`, `scroll_view`.

use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::scroll_view::ScrollViewHandle;
use runtime_core::{
    FileDropHandler, HoverHandler, PressableHandle, SafeAreaSides, TouchHandler, ViewHandle,
    WheelHandler,
};
use runtime_world::Value;

use crate::style_attach::StyleProp;

/// The `view` primitive — plain container. Fields mirror what
/// `walker/view.rs::build` receives/forwards.
///
/// `is_container` marks a container-query containment context
/// (`StyleOps::mark_container`); the native inline-size feedback loop the
/// old walker wires on top is deferred with the style-engine port (see
/// crate docs' deferred set).
pub struct ViewPrim {
    /// Robot/automation anchor (`test_id = …`). Always present so the
    /// builder setter compiles in every build; read only by the
    /// `robot`-feature registration in the mount handler.
    pub test_id: Option<&'static str>,
    pub style: Option<StyleProp>,
    pub safe_area: SafeAreaSides,
    pub on_touch: Option<TouchHandler>,
    pub on_wheel: Option<WheelHandler>,
    pub on_hover: Option<HoverHandler>,
    pub on_file_drop: Option<FileDropHandler>,
    pub preserves_focus: bool,
    pub is_container: bool,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(ViewHandle)>>,
}

/// The `pressable` primitive — tappable container
/// (`walker/pressable.rs`). `disabled` drives both the backend's
/// `set_disabled` and the handler-level press-block flag (a bare
/// pressable is not a native form control, so the callback must be
/// blocked uniformly — the walker's rationale, ported).
pub struct PressablePrim {
    /// Robot/automation anchor (`test_id = …`). Always present so the
    /// builder setter compiles in every build; read only by the
    /// `robot`-feature registration in the mount handler.
    pub test_id: Option<&'static str>,
    pub on_press: Rc<dyn Fn()>,
    pub disabled: Option<Value<bool>>,
    pub preserves_focus: bool,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(PressableHandle)>>,
}

/// The `scroll_view` primitive (`walker/scroll_view.rs`). Safe-area
/// opt-in routes through the *contentInset* path
/// (`apply_scroll_view_safe_area_inset`), the one place it diverges from
/// `view`.
pub struct ScrollViewPrim {
    /// Robot/automation anchor (`test_id = …`). Always present so the
    /// builder setter compiles in every build; read only by the
    /// `robot`-feature registration in the mount handler.
    pub test_id: Option<&'static str>,
    pub horizontal: bool,
    pub on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
    pub safe_area: SafeAreaSides,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(ScrollViewHandle)>>,
}
