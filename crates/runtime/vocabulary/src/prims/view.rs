//! Container payloads: `view`, `pressable`, `scroll_view`.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::scroll_view::ScrollViewHandle;
use runtime_shared::{
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
    /// Called when the reader arrives within `end_reached_threshold` of
    /// the end of the scroll axis — once per arrival, not per event.
    ///
    /// This exists because `on_scroll` cannot answer it. An offset says
    /// where the reader is, and "how much is left" needs the viewport
    /// and the content extent too, neither of which an app can measure.
    ///
    /// Answered today by the **iOS-mobile, web and macOS** backends.
    /// Elsewhere — Android, Linux, Windows — the default
    /// `ScrollOps::observe_scroll_end` is a no-op and this never fires,
    /// so a list that ONLY grows this way stops growing rather than
    /// degrading to something. (Android needs its Kotlin scroll
    /// listeners to carry the content extent, and to install on an
    /// `observe_scroll_end` that arrives after creation; neither is in
    /// place yet.)
    pub on_end_reached: Option<Rc<dyn Fn()>>,
    /// How close to the end counts as arriving, in logical px. `0`
    /// means the very end; a screenful is the usual choice for
    /// prefetching a list so the next page is there before the reader
    /// is.
    pub end_reached_threshold: f32,
    /// Safe-area treatment, as a THREE-state value.
    ///
    /// - `None` — the author said nothing. The backend is not called and
    ///   the platform default stands (on iOS, UIKit's
    ///   `contentInsetAdjustmentBehavior = .automatic`).
    /// - `Some(sides)` — the author opted in for those sides.
    /// - `Some(SafeAreaSides::NONE)` — the author opted OUT explicitly.
    ///   The scroller bleeds edge to edge and owns its own content
    ///   offset.
    ///
    /// The third state is the reason this is an `Option` rather than a
    /// bare `SafeAreaSides`. The backends have always implemented the
    /// opt-out — `apply_scroll_view_safe_area_inset` maps an empty
    /// `sides` to "never adjust" — but with a bare set there was no way
    /// to REACH it: an empty set was indistinguishable from silence, the
    /// call was skipped, and the platform default survived. That made a
    /// container's inset depend on whether some ancestor scroller
    /// happened to span the window, which is not a property anything
    /// declares.
    pub safe_area: Option<SafeAreaSides>,
    /// Whether the scroller may travel past its content and spring back
    /// — iOS rubber-banding, Android's stretch, the browser's bounce.
    ///
    /// Three-state for the same reason `safe_area` is: `None` is
    /// silence and the platform default stands, `Some(false)` clamps
    /// the scroll to its content, `Some(true)` asks for the bounce
    /// where a backend would otherwise suppress it.
    ///
    /// Bounce is a good default for a PAGE — it signals "you are at the
    /// end" and it is what a native app does. It reads as a glitch on a
    /// bounded pane inside a page: a table that springs away from its
    /// own header has no end to signal, because the thing that scrolls
    /// is not the thing the gesture appears to grab.
    pub bounces: Option<bool>,
    /// Whether the scroller bounces even when there is NOTHING to
    /// scroll — content shorter than the scrollport.
    ///
    /// Distinct from [`Self::bounces`], which asks whether the spring
    /// exists at all. This asks whether it fires against an edge that
    /// is also the other edge. UIKit splits the same way
    /// (`bounces` / `alwaysBounceVertical`), and iOS is the backend
    /// that needs the distinction: the framework turns
    /// `alwaysBounce*` ON for every scroller so short content does not
    /// feel dead, which is right for a PAGE and wrong for a bounded
    /// pane whose content usually fits.
    ///
    /// A bottom sheet is the case that named it: sized to its content,
    /// it has nothing to scroll almost always, and a sheet that
    /// rubber-bands under the finger reads as "this scrolls" when it
    /// does not — and as a failed drag-to-dismiss when it might.
    /// `Some(false)` leaves the spring intact for the sheet that IS
    /// long enough to scroll.
    ///
    /// Three-state like its neighbours: `None` is silence and the
    /// backend's default stands.
    pub always_bounce: Option<bool>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(ScrollViewHandle)>>,
}
