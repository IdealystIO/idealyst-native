//! The "private layer" — an overlay subtree that screen recordings do
//! **not** capture. This is the cross-platform generalization of the iOS
//! ReplayKit pattern (put the not-recorded chrome on a *separate*
//! `UIWindow`, since ReplayKit only captures the key window).
//!
//! # Why this needs zero framework-core changes
//!
//! It rides the framework's existing third-party extension mechanism:
//! it's an [`Element::External`] whose payload is [`PrivateLayerProps`]
//! and whose children are parented into whatever native surface the
//! registered backend handler returns. Core already supports exactly
//! this (`Element::External` + per-backend `ExternalRegistry` +
//! `RegisterExternal`); we add no enum variant and no `Backend` method.
//!
//! The per-platform surface the handler creates is the *only* thing that
//! differs by backend, and each maps onto a native capture-exclusion
//! mechanism:
//!
//! | backend | private surface | how the recorder excludes it |
//! |---------|-----------------|------------------------------|
//! | iOS     | separate `UIWindow` (high `windowLevel`) | ReplayKit records key window only |
//! | macOS   | separate `NSWindow`/`NSPanel` | registered into `SCContentFilter(excludingWindows:)` |
//! | Windows | sibling HWND | `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` |
//! | Android | stays on the default display | recordable content goes to a captured secondary `VirtualDisplay` |
//! | web     | DOM sibling outside the captured element | `track.restrictTo(RestrictionTarget.fromElement(content))` |
//! | Linux   | (no exclusion available — renders inline) | — |
//!
//! # Per-backend registration
//!
//! The handler that creates the capture-excluded surface needs the
//! *concrete* backend type — it builds platform windows (a second
//! `UIWindow` on iOS, a `WindowManager` window on Android) that the
//! generic [`RegisterExternal`] surface can't express. So [`register`]
//! is backend-concrete on native (it takes the platform's backend type,
//! exactly like `video::register`) and dispatches per-`cfg` to the
//! matching `imp` module's `register_private_layer`. On web the handler
//! is the documented inline no-op (capture exclusion via Element
//! Capture `restrictTo` is a later addition), and on platforms with no
//! backend support `register` is a generic no-op so author code still
//! compiles.
//!
//! | backend | status |
//! |---------|--------|
//! | iOS     | separate `UIWindow` — ReplayKit-excluded (device-verified by the orchestrator) |
//! | Android | separate `WindowManager` window — PixelCopy-excluded |
//! | web     | inline no-op (TODO: Element Capture `restrictTo`) |
//! | others  | inline no-op |

use runtime_core::{external, Bound, Element, ExternalHandle};

/// Marker payload for the private layer. No fields yet — the layer's
/// behavior is entirely in the backend handler. Kept as a named struct
/// so its [`std::any::TypeId`] is the registry dispatch key.
pub struct PrivateLayerProps {
    _private: (),
}

impl Default for PrivateLayerProps {
    fn default() -> Self {
        Self { _private: () }
    }
}

/// Wrap `children` in a private (non-recorded) overlay surface.
///
/// PascalCase to read like a first-party container inside a `ui!`/`jsx!`
/// tree. Mirrors the iOS "second `UIWindow`" pattern on every backend
/// that supports capture exclusion.
///
/// ```ignore
/// ui! {
///     view {
///         RecordableContent()
///         { screen_recorder::PrivateLayer(vec![ ui! { RecordingControls() } ]) }
///     }
/// }
/// ```
#[allow(non_snake_case)]
pub fn PrivateLayer(children: Vec<Element>) -> Bound<ExternalHandle<PrivateLayerProps>> {
    external(PrivateLayerProps::default()).children(children)
}

// `register` is provided by the per-target `imp` module (selected by
// the `#[cfg_attr(... path = ...)]` on `mod imp` in `lib.rs`) and
// re-exported from the crate root next to `ScreenRecorder`. iOS/Android
// install the capture-excluded-window handler; web + unsupported
// targets install the inline no-op. See [`crate::register`].
