//! Accessibility surface — moved to `runtime-shared` (the walker-free
//! half). This core-side module re-exports it at the old paths and
//! keeps ONLY [`primitive_kind`], which pattern-matches the old-core
//! `Element` wire tree (an old-walker concern that dies with it).
pub use runtime_shared::accessibility::*;

/// Map a `Element` reference to its [`PrimitiveKind`]. Used by the
/// walker's a11y plumbing to look up the default role for the
/// primitive's variant without exposing every primitive's internal
/// structure to the accessibility module.
///
/// Returns `None` for control-flow primitives (`When`, `Switch`,
/// `Repeat`) which are transparent containers with no a11y identity
/// of their own — the walker reads the actually-mounted subtree's
/// kind instead.
pub fn primitive_kind(p: &crate::Element) -> Option<PrimitiveKind> {
    use crate::Element;
    match p {
        Element::View { .. } => Some(PrimitiveKind::View),
        Element::Text { .. } => Some(PrimitiveKind::Text),
        Element::Button { .. } => Some(PrimitiveKind::Button),
        Element::Pressable { .. } => Some(PrimitiveKind::Pressable),
        Element::Image { .. } => Some(PrimitiveKind::Image),
        Element::Icon { .. } => Some(PrimitiveKind::Icon),
        Element::Link { .. } => Some(PrimitiveKind::Link),
        Element::TextInput { .. } => Some(PrimitiveKind::TextInput),
        Element::TextArea { .. } => Some(PrimitiveKind::TextArea),
        Element::Toggle { .. } => Some(PrimitiveKind::Toggle),
        Element::Slider { .. } => Some(PrimitiveKind::Slider),
        Element::ScrollView { .. } => Some(PrimitiveKind::ScrollView),
        Element::ActivityIndicator { .. } => Some(PrimitiveKind::ActivityIndicator),
        Element::Virtualizer { .. } => Some(PrimitiveKind::Virtualizer),
        Element::Graphics { .. } => Some(PrimitiveKind::Graphics),
        Element::Portal { .. } => Some(PrimitiveKind::Portal),
        Element::Presence { .. } => Some(PrimitiveKind::Presence),
        Element::External { .. } => Some(PrimitiveKind::External),
        Element::Navigator { .. } => Some(PrimitiveKind::Navigator),
        Element::NavigatorOutlet { .. } => Some(PrimitiveKind::View),
        Element::Lazy { .. } => Some(PrimitiveKind::Lazy),
        // Control flow + fragment — layout-transparent, no node of their own.
        Element::When { .. }
        | Element::Switch { .. }
        | Element::Each { .. }
        | Element::Dynamic { .. }
        | Element::Repeat { .. }
        | Element::Fragment { .. } => None,
        // Robot wrapper — transparent; unwrapped before this is consulted.
        #[cfg(feature = "robot")]
        Element::Component { .. } => None,
    }
}
