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
//! # Skeleton status
//!
//! Today the handler renders children **inline in an ordinary view** —
//! exclusion is NOT yet active. Each backend replaces the handler body
//! with its real capture-excluded surface as it's implemented. The
//! author-facing API is final; only the handler body changes.

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{external, Bound, Element, ExternalHandle, RegisterExternal};

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

/// Install the private-layer handler on a backend. Call once at app
/// bootstrap, exactly like `webview::register(&mut backend)`.
///
/// Generic over [`RegisterExternal`] so it works on any backend without
/// naming a concrete backend type.
pub fn register<B: RegisterExternal>(backend: &mut B) {
    backend.register_external::<PrivateLayerProps, _>(|_props, backend| {
        // SKELETON: render children inline in a plain view. Capture
        // exclusion is not active yet. Per-platform impls replace this
        // body with a real capture-excluded surface (see the module
        // table above) and parent the external's children into it.
        backend.create_view(&AccessibilityProps::default())
    });
}
