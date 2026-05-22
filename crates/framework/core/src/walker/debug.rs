//! Debug-stats helpers used by the build walker.
//!
//! Everything here is gated on the `debug-stats` Cargo feature. The
//! `time_backend_create` wrapper has a no-op variant for the
//! feature-off build so call sites stay identical.

use crate::primitive::Primitive;

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
pub(super) fn debug_kind_of(node: &Primitive) -> debug::PrimitiveKind {
    use debug::PrimitiveKind;
    match node {
        Primitive::Text { .. } => PrimitiveKind::Text,
        Primitive::View { .. } => PrimitiveKind::View,
        Primitive::Button { .. } => PrimitiveKind::Button,
        Primitive::Pressable { .. } => PrimitiveKind::Pressable,
        Primitive::Image { .. } => PrimitiveKind::Image,
        Primitive::Icon { .. } => PrimitiveKind::Icon,
        Primitive::TextInput { .. } => PrimitiveKind::TextInput,
        Primitive::TextArea { .. } => PrimitiveKind::TextArea,
        Primitive::Toggle { .. } => PrimitiveKind::Toggle,
        Primitive::ScrollView { .. } => PrimitiveKind::ScrollView,
        Primitive::Slider { .. } => PrimitiveKind::Slider,
        Primitive::Video { .. } => PrimitiveKind::Video,
        Primitive::ActivityIndicator { .. } => PrimitiveKind::ActivityIndicator,
        Primitive::Virtualizer { .. } => PrimitiveKind::Virtualizer,
        Primitive::Graphics { .. } => PrimitiveKind::Graphics,
        Primitive::Navigator(_) => PrimitiveKind::Navigator,
        Primitive::TabNavigator(_) => PrimitiveKind::TabNavigator,
        Primitive::DrawerNavigator(_) => PrimitiveKind::DrawerNavigator,
        Primitive::When { .. } => PrimitiveKind::When,
        Primitive::Switch { .. } => PrimitiveKind::Switch,
        Primitive::Link { .. } => PrimitiveKind::Link,
        Primitive::Portal { .. } => PrimitiveKind::Portal,
        Primitive::External { .. } => PrimitiveKind::External,
        Primitive::Presence { .. } => PrimitiveKind::Presence,
        // Repeat is expanded into siblings by `insert_children`
        // and never reaches the build walker as a standalone
        // subtree, so this arm is dead in practice. Tag as View
        // to keep the debug timing breakdown defined.
        Primitive::Repeat { .. } => PrimitiveKind::View,
    }
}

// Suppress unused-import warning when debug-stats is off (the only
// users of `Primitive` here are inside the cfg-gated fn).
#[cfg(not(feature = "debug-stats"))]
#[allow(dead_code)]
fn _unused_primitive_marker(_: &Primitive) {}
