//! Framework core: primitives, Backend trait, render walker, reactivity.

mod backend;
mod builder;
mod handles;
mod primitive;
mod reactive;
mod scheduling;
mod sources;
mod style;
mod walker;
pub mod primitives;

// Cross-platform per-frame + async-driver primitives. Off by default;
// see the `async-driver` feature in Cargo.toml.
#[cfg(feature = "async-driver")]
pub mod driver;

#[cfg(feature = "debug-stats")]
pub mod debug;

pub use backend::{Backend, VirtualizerCallbacks};
pub use handles::{
    ButtonHandle, ButtonOps, PressableHandle, PressableOps, RefFill, RefOps, StateBits, TextHandle,
    TextOps, ViewHandle, ViewOps,
};
pub use builder::{
    button, pressable, switch, text, view, when, Bindable, Bound, ChildList, IntoDisabledSource,
    IntoPrimitive,
};
pub use primitive::Primitive;
pub use sources::{IntoStyleSource, IntoTextSource, StyleSource, TextSource};
pub use walker::{render, Owner};
pub use primitives::navigator::{
    match_pattern, DefaultLinkKind, DrawerHandle, DrawerItem, DrawerItemRegistration,
    DrawerNavigator, DrawerNavigatorCallbacks, DrawerSide, DrawerSidebarProps, LayoutPlan,
    LayoutProps, MountPolicy, NavCommand, NavState, Navigator, NavigatorCallbacks,
    NavigatorControl, NavigatorHandle, NavigatorOps, Route, RouteParams, TabNavigator,
    TabNavigatorCallbacks, TabPlacement, TabRegistration, TabSpec, TabsHandle,
};
pub use primitives::overlay::{
    overlay, AnchorTarget, AnchorableHandle, BackdropMode, ElementAlign, ElementAnchor,
    ElementSide, OverlayAnchor, OverlayHandle, OverlayOps, ViewportPlacement, ViewportRect,
};
pub use primitives::presence::{
    presence, PresenceAnim, PresenceHandle, PresenceOps, PresenceState,
};
pub use reactive::{arena_stats, untrack, ArenaStats, Effect, Ref, Signal};
pub use scheduling::{
    after_animation_frame, after_ms, raf_loop, schedule_microtask, RafLoop, ScheduledTask,
};

pub use style::{
    active_theme, derived, install_theme, pregenerate_for_theme, resolve as resolve_style,
    set_theme, AlignContent, AlignItems, AlignSelf, Color, Derive, Easing, FlexDirection,
    FlexWrap, FontStyle, FontWeight, IntoOverrideSource, IntoVariantSource, JustifyContent,
    Length, Overflow, Position, Shadow, StyleApplication, StyleRules, StyleSheet, TextAlign,
    TextTransform, ThemeTokens, TokenEntry, TokenValue, Tokenized, Transform, Transition,
    VariantAxis, VariantEnum, VariantSet, VariantValue,
};

pub use framework_macros::{component, jsx, stylesheet, ui};

// This crate root used to be ~3200 lines containing everything below.
// Each major concern now lives in its own private submodule and the
// crate's public surface is preserved via the `pub use` block above:
//
//   sources.rs   — TextSource / IntoTextSource / StyleSource /
//                  IntoStyleSource
//   handles.rs   — StateBits / ButtonHandle / ViewHandle / TextHandle /
//                  RefOps / RefFill
//   primitive.rs — the `Primitive` enum and `impl Primitive`
//   builder.rs   — Bound<H> / Bindable<H> / ChildList /
//                  IntoDisabledSource / IntoPrimitive plus the
//                  `view`/`text`/`button`/`when`/`switch` constructors
//   walker.rs    — `render`, `Owner`, the `build` walker, per-primitive
//                  builders, style attachment + theme cohort, and the
//                  reactive-branching builders (when/switch/presence).
//
// The `signal!` and `children!` macros stay here — `#[macro_export]`
// registers them at the crate root anyway, and keeping them next to
// the `pub use` block makes the public surface a single grep target.

/// Shorthand for `Signal::new(value)`. Equivalent in every way; just less
/// typing at the call site.
///
/// ```ignore
/// let count = signal!(0);
/// // same as: let count = Signal::new(0);
/// ```
#[macro_export]
macro_rules! signal {
    ($value:expr) => {
        $crate::Signal::new($value)
    };
}

/// Builds a `Vec<Primitive>` from a mixed-shape list of children.
///
/// Each argument must implement [`ChildList`]; the macro flattens
/// `Option<Primitive>` (skipping `None`) and `Vec<Primitive>` (extending
/// inline) so call sites can write conditionals naturally.
///
/// ```ignore
/// view(children![
///     text("always"),
///     logged_in.then(|| text("conditional")),
///     items.into_iter().map(|i| text(i)).collect::<Vec<_>>(),
/// ])
/// ```
#[macro_export]
macro_rules! children {
    ($($child:expr),* $(,)?) => {{
        let mut __c: ::std::vec::Vec<$crate::Primitive> = ::std::vec::Vec::new();
        $( $crate::ChildList::append_to($child, &mut __c); )*
        __c
    }};
}

