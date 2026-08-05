//! Form-control widgets: text input / text area, toggle, slider, and
//! the activity indicator.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives;

use super::noop;
use super::ExternalOps;

/// The `text_input` + `text_area` primitives (single- and multi-line
/// editors share a walker and a controlled-value pattern). Serves
/// `walker/text_input.rs`.
pub trait TextInputOps: ExternalOps {
    /// Create a single-line text input (controlled `value` pattern).
    #[allow(unused_variables)]
    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        secure: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.missing_primitive_placeholder("text_input (backend compiled without `prim-text-input`)")
    }

    /// Controlled-value write-back.
    #[allow(unused_variables)]
    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {}

    /// Toggle secure-entry (mask) mode in place.
    #[allow(unused_variables)]
    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {}

    /// Install a focus-change notifier (`handler(true)` on focus gain).
    #[allow(unused_variables)]
    fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>) {}

    /// Update the placeholder in place (`None` clears).
    #[allow(unused_variables)]
    fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {}

    /// Create a multi-line text editor (row-bounded intrinsic growth).
    #[allow(unused_variables)]
    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        wrap: bool,
        min_rows: Option<u32>,
        max_rows: Option<u32>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.missing_primitive_placeholder("text_area (backend compiled without `prim-text-input`)")
    }

    /// Controlled-value write-back for the text area.
    #[allow(unused_variables)]
    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {}

    /// Imperative-ref handle for a text input. Default: no-op.
    #[allow(unused_variables)]
    fn make_text_input_handle(&self, node: &Self::Node) -> primitives::text_input::TextInputHandle {
        primitives::text_input::TextInputHandle::new(Rc::new(()), &noop::NoopTextInputOps)
    }

    /// Imperative-ref handle for a text area. Default: no-op.
    #[allow(unused_variables)]
    fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
        primitives::text_area::TextAreaHandle::new(Rc::new(()), &noop::NoopTextAreaOps)
    }
}

/// The `toggle` primitive (switch / checkbox). Serves
/// `walker/toggle.rs`.
pub trait ToggleOps: ExternalOps {
    /// Create a toggle with initial value + change callback.
    #[allow(unused_variables)]
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.missing_primitive_placeholder("toggle (backend compiled without `prim-toggle`)")
    }

    /// Controlled-value write-back.
    #[allow(unused_variables)]
    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {}

    /// Imperative-ref handle for a toggle. Default: no-op.
    #[allow(unused_variables)]
    fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
        primitives::toggle::ToggleHandle::new(Rc::new(()), &noop::NoopToggleOps)
    }
}

/// The `slider` primitive. Serves `walker/slider.rs`.
pub trait SliderOps: ExternalOps {
    /// Create a slider (min/max/step static after creation).
    #[allow(unused_variables)]
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.missing_primitive_placeholder("slider (backend compiled without `prim-slider`)")
    }

    /// Controlled-value write-back.
    #[allow(unused_variables)]
    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {}

    /// Imperative-ref handle for a slider. Default: no-op.
    #[allow(unused_variables)]
    fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
        primitives::slider::SliderHandle::new(Rc::new(()), &noop::NoopSliderOps)
    }
}

/// The `activity_indicator` primitive (loading spinner). Serves
/// `walker/activity_indicator.rs`.
pub trait ActivityIndicatorOps: ExternalOps {
    /// Create a loading spinner (size/color static at construction).
    #[allow(unused_variables)]
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&runtime_shared::Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.missing_primitive_placeholder(
            "activity_indicator (backend compiled without `prim-activity`)",
        )
    }

    /// Re-apply a live `size` source in place (web only in practice).
    #[allow(unused_variables)]
    fn update_activity_indicator_size(
        &mut self,
        node: &Self::Node,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        // default: no-op
    }

    /// Imperative-ref handle for an activity indicator. Default: no-op.
    #[allow(unused_variables)]
    fn make_activity_indicator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::activity_indicator::ActivityIndicatorHandle {
        primitives::activity_indicator::ActivityIndicatorHandle::new(
            Rc::new(()),
            &noop::NoopActivityIndicatorOps,
        )
    }
}
