//! Debug-stats helpers used by the build walker.
//!
//! Everything here is gated on the `debug-stats` Cargo feature. The
//! `time_backend_create` wrapper has a no-op variant for the
//! feature-off build so call sites stay identical.

use crate::element::Element;

#[cfg(feature = "debug-stats")]
use crate::debug;

/// Wrap a backend create call with BackendCreate enter/exit recording.
/// When `debug-stats` is off this is a transparent passthrough — both
/// the kind argument and the wrapper itself become no-ops the compiler
/// inlines away.
#[inline(always)]
#[cfg(feature = "debug-stats")]
pub(super) fn time_backend_create<R>(kind: debug::PrimitiveKind, f: impl FnOnce() -> R) -> R {
    debug::record_backend_create_enter(kind);
    let r = f();
    debug::record_backend_create_exit(kind);
    r
}

/// No-op variant: the `kind` parameter doesn't even exist, so call
/// sites pass `()` instead. Keeps the call-site shape identical to the
/// debug-on path while emitting nothing when off.
#[inline(always)]
#[cfg(not(feature = "debug-stats"))]
pub(super) fn time_backend_create<R>(_kind: (), f: impl FnOnce() -> R) -> R {
    f()
}

/// Map a primitive to the coarse-grained `PrimitiveKind` tag used by
/// debug events. Only compiled when `debug-stats` is enabled.
#[cfg(feature = "debug-stats")]
pub(super) fn debug_kind_of(node: &Element) -> debug::PrimitiveKind {
    use debug::PrimitiveKind;
    match node {
        Element::Text { .. } => PrimitiveKind::Text,
        Element::View { .. } => PrimitiveKind::View,
        Element::Button { .. } => PrimitiveKind::Button,
        Element::Pressable { .. } => PrimitiveKind::Pressable,
        Element::Image { .. } => PrimitiveKind::Image,
        Element::Icon { .. } => PrimitiveKind::Icon,
        Element::TextInput { .. } => PrimitiveKind::TextInput,
        Element::TextArea { .. } => PrimitiveKind::TextArea,
        Element::Toggle { .. } => PrimitiveKind::Toggle,
        Element::ScrollView { .. } => PrimitiveKind::ScrollView,
        Element::Slider { .. } => PrimitiveKind::Slider,
        Element::ActivityIndicator { .. } => PrimitiveKind::ActivityIndicator,
        Element::Virtualizer { .. } => PrimitiveKind::Virtualizer,
        Element::Graphics { .. } => PrimitiveKind::Graphics,
        Element::Navigator { .. } => PrimitiveKind::Navigator,
        Element::When { .. } => PrimitiveKind::When,
        Element::Switch { .. } => PrimitiveKind::Switch,
        Element::Each { .. } => PrimitiveKind::Each,
        Element::Link { .. } => PrimitiveKind::Link,
        Element::Portal { .. } => PrimitiveKind::Portal,
        Element::External { .. } => PrimitiveKind::External,
        Element::Presence { .. } => PrimitiveKind::Presence,
        Element::Lazy { .. } => PrimitiveKind::Lazy,
        // Repeat is expanded into siblings by `insert_children`
        // and never reaches the build walker as a standalone
        // subtree, so this arm is dead in practice. Tag as View
        // to keep the debug timing breakdown defined.
        Element::Repeat { .. } => PrimitiveKind::View,
        // Fragment splices inline via `insert_children` (no node); a
        // standalone fragment builds a transparent anchor. Tag as View to
        // keep the debug timing breakdown defined, same as Repeat.
        Element::Fragment { .. } => PrimitiveKind::View,
        // Robot wrapper — unwrapped before the walker times anything; tag as
        // View for completeness (never actually timed).
        #[cfg(feature = "robot")]
        Element::Component { .. } => PrimitiveKind::View,
    }
}

// Suppress unused-import warning when debug-stats is off (the only
// users of `Element` here are inside the cfg-gated fn).
#[cfg(not(feature = "debug-stats"))]
#[allow(dead_code)]
fn _unused_primitive_marker(_: &Element) {}
