//! Generic mount handlers for the built-in primitives (P2b) — the old
//! walker's per-primitive build modules, re-homed as registry handlers.
//!
//! Each `mount_*` fn is bounded on exactly the capability traits it
//! calls, ports its walker module's mount sequence faithfully (create →
//! attach_style → handlers/ref-fill → binding effects → teardown
//! registration — each fn's docs note its walker source and any
//! deviation), and returns the real node. `scene-parity`'s full-op
//! goldens pin the resulting backend-call streams against the old
//! walker's.
//!
//! Binding effects, state signals, and teardown probes are created inside
//! the handler body, i.e. inside the `MountCx` collector — they die with
//! the realized subtree (the P1 rule; `MountCx` already wraps every
//! realization in `collect_owned`).

use runtime_scene::Registry;
use runtime_world::{effect, Value};

use crate::caps::AllCaps;
use crate::prims::{
    ActivityIndicatorPrim, ButtonPrim, IconPrim, ImagePrim, LinkPrim, PressablePrim, PrimCell,
    ScrollViewPrim, SliderPrim, TextAreaPrim, TextInputPrim, TextPrim, TogglePrim, ViewPrim,
};

mod graphics;
mod lazy;
mod media;
mod navigator;
mod portal;
mod presence;
mod repeat;
mod text;
mod view;
mod virtualizer;
mod widgets;

pub use graphics::{mount_graphics, register_graphics};
pub use lazy::{mount_lazy, register_lazy};
pub use media::{mount_icon, mount_image, mount_link};
pub use navigator::{
    mount_navigator_outlet, mount_stack_navigator, mount_swap_navigator, register_navigator,
    NavCaps,
};
/// The platform-URL synchronization seam a URL-bearing host installs
/// (see `navigator::url_sync`).
pub use navigator::url_sync as nav_url_sync;
pub use portal::{mount_portal, register_portal};
pub use presence::{mount_presence, register_presence};
pub use repeat::{mount_repeat, register_repeat};
pub use virtualizer::{mount_virtualizer, register_virtualizer};
pub use text::{mount_button, mount_text};
pub use view::{mount_pressable, mount_scroll_view, mount_view};
pub use widgets::{
    mount_activity_indicator, mount_slider, mount_text_area, mount_text_input, mount_toggle,
};

/// Install all 18 built-in handlers (the 13 P2 primitives + the P3-set
/// `virtualizer`, `graphics`, `portal` — which also serves the
/// `overlay`/`anchored_overlay` compositions — `presence`, and the
/// `lazy` chunk boundary) on `registry`. Backends call this
/// once at startup (alongside their platform-specific registrations);
/// [`LegacyBridge`](crate::bridge::LegacyBridge) around any existing
/// `Backend` satisfies the bound automatically.
pub fn register_builtins<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<ViewPrim>, _>(|cx, p, children| {
        mount_view(cx, p.take(), children)
    });
    registry.register::<PrimCell<TextPrim>, _>(|cx, p, children| mount_text(cx, p.take(), children));
    registry.register::<PrimCell<ButtonPrim>, _>(|cx, p, children| {
        mount_button(cx, p.take(), children)
    });
    registry.register::<PrimCell<PressablePrim>, _>(|cx, p, children| {
        mount_pressable(cx, p.take(), children)
    });
    registry.register::<PrimCell<ImagePrim>, _>(|cx, p, children| {
        mount_image(cx, p.take(), children)
    });
    registry.register::<PrimCell<IconPrim>, _>(|cx, p, children| mount_icon(cx, p.take(), children));
    registry.register::<PrimCell<TogglePrim>, _>(|cx, p, children| {
        mount_toggle(cx, p.take(), children)
    });
    registry.register::<PrimCell<SliderPrim>, _>(|cx, p, children| {
        mount_slider(cx, p.take(), children)
    });
    registry.register::<PrimCell<ActivityIndicatorPrim>, _>(|cx, p, children| {
        mount_activity_indicator(cx, p.take(), children)
    });
    registry.register::<PrimCell<LinkPrim>, _>(|cx, p, children| mount_link(cx, p.take(), children));
    registry.register::<PrimCell<ScrollViewPrim>, _>(|cx, p, children| {
        mount_scroll_view(cx, p.take(), children)
    });
    registry.register::<PrimCell<TextInputPrim>, _>(|cx, p, children| {
        mount_text_input(cx, p.take(), children)
    });
    registry.register::<PrimCell<TextAreaPrim>, _>(|cx, p, children| {
        mount_text_area(cx, p.take(), children)
    });
    register_virtualizer(registry);
    register_graphics(registry);
    register_portal(registry);
    register_presence(registry);
    register_navigator(registry);
    register_repeat(registry);
    register_lazy(registry);
}

/// Bind a `Value` prop that the `create_*` call did NOT consume: the
/// apply runs once for `Const` and per fire (including an immediate
/// first fire) for `Dyn` — the shape of the old walker's unconditional
/// update effects (image `src`, controlled `value` write-backs), where
/// mount always emits one initial `update_*`.
pub(crate) fn bind_value<T: 'static>(value: Value<T>, mut apply: impl FnMut(&T) + 'static) {
    match value {
        Value::Const(v) => apply(&v),
        Value::Dyn(f) => {
            let _ = effect(move || apply(&f()));
        }
    }
}

/// Bind a `Value` prop whose initial value the `create_*` call ALREADY
/// consumed: `Const` installs nothing (the widget was born correct);
/// `Dyn` installs the update effect, whose first fire re-applies at
/// mount — exactly the old walker's `Reactive::Dynamic`-gated effects
/// (`secure`, `placeholder`) and closure-prop effects (button label,
/// icon color/data, link url, activity size).
pub(crate) fn bind_dyn<T: 'static>(value: Value<T>, mut apply: impl FnMut(&T) + 'static) {
    if let Value::Dyn(f) = value {
        let _ = effect(move || apply(&f()));
    }
}

/// Read a `Value`'s current value by reference-shape match without
/// consuming it (`Const` clones; `Dyn` invokes the closure — the same
/// initial-read the old walker does before `create_*`).
pub(crate) fn initial_of<T: Clone>(value: &Value<T>) -> T {
    match value {
        Value::Const(v) => v.clone(),
        Value::Dyn(f) => f(),
    }
}
