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
mod virtual_grid;
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
pub use virtual_grid::{mount_virtual_grid, register_virtual_grid};
pub use virtualizer::{mount_virtualizer, register_virtualizer};
pub use text::{mount_button, mount_text};
pub use view::{mount_pressable, mount_scroll_view, mount_view};
pub use widgets::{
    mount_activity_indicator, mount_slider, mount_text_area, mount_text_input, mount_toggle,
};

/// Install all 22 built-in handlers on `registry`: 21 single-node (the 13
/// P2 primitives + the P3-set `virtualizer`, `graphics`, `portal` — which
/// also serves the `overlay`/`anchored_overlay` compositions —
/// `presence`, the three navigator prims, and the `lazy` chunk boundary)
/// plus 1 multi-node (`repeat`). Backends call this once at startup,
/// alongside their platform-specific registrations.
///
/// This set is the framework's **always-resident bundle floor**: scene
/// registration is boot-only (`realize` takes an `Rc<Registry<H>>` with no
/// interior mutability), so every handler here — and everything it
/// transitively reaches — is statically reachable from the boot entry in
/// every app on every target. Growing the set grows every app's binary.
/// `tests/builtin_surface.rs` pins it.
///
/// Debug builds also install the kernel's diagnostic log bridge here — see
/// [`install_kernel_diagnostic_bridge`] for why this seam rather than each
/// backend's `start_in`.
pub fn register_builtins<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    register_builtins_with::<H, AllBuiltins>(registry)
}

// ---------------------------------------------------------------------------
// BuiltinSet — per-primitive, opt-in bundle seam
// ---------------------------------------------------------------------------

/// Register the `view` primitive handler.
pub fn register_view<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<ViewPrim>, _>(|cx, p, children| mount_view(cx, p.take(), children));
}
/// Register the `text` primitive handler.
pub fn register_text<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<TextPrim>, _>(|cx, p, children| mount_text(cx, p.take(), children));
}
/// Register the `button` primitive handler.
pub fn register_button<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<ButtonPrim>, _>(|cx, p, children| mount_button(cx, p.take(), children));
}
/// Register the `pressable` primitive handler.
pub fn register_pressable<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<PressablePrim>, _>(|cx, p, children| mount_pressable(cx, p.take(), children));
}
/// Register the `image` primitive handler.
pub fn register_image<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<ImagePrim>, _>(|cx, p, children| mount_image(cx, p.take(), children));
}
/// Register the `icon` primitive handler.
pub fn register_icon<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<IconPrim>, _>(|cx, p, children| mount_icon(cx, p.take(), children));
}
/// Register the `link` primitive handler.
pub fn register_link<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<LinkPrim>, _>(|cx, p, children| mount_link(cx, p.take(), children));
}
/// Register the `toggle` primitive handler.
pub fn register_toggle<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<TogglePrim>, _>(|cx, p, children| mount_toggle(cx, p.take(), children));
}
/// Register the `slider` primitive handler.
pub fn register_slider<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<SliderPrim>, _>(|cx, p, children| mount_slider(cx, p.take(), children));
}
/// Register the `activity_indicator` primitive handler.
pub fn register_activity_indicator<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<ActivityIndicatorPrim>, _>(|cx, p, children| mount_activity_indicator(cx, p.take(), children));
}
/// Register the `text_input` primitive handler.
pub fn register_text_input<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<TextInputPrim>, _>(|cx, p, children| mount_text_input(cx, p.take(), children));
}
/// Register the `text_area` primitive handler.
pub fn register_text_area<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<TextAreaPrim>, _>(|cx, p, children| mount_text_area(cx, p.take(), children));
}
/// Register the `scroll_view` primitive handler.
pub fn register_scroll_view<H: AllCaps + 'static>(registry: &mut Registry<H>) {
    registry.register::<PrimCell<ScrollViewPrim>, _>(|cx, p, children| mount_scroll_view(cx, p.take(), children));
}

/// Compile-time selection of which builtin primitives
/// [`register_builtins_with`] installs — one method per primitive.
///
/// The framework's whole builtin vocabulary is optional. An app that never
/// renders a `slider` should not carry the slider handler, its backend
/// implementation, or the web-sys imports and JS glue they reach. The
/// always-on set costs ~65 KB brotli on a hello-world; the three navigator
/// prims alone are ~13 KB of that.
///
/// **Every method defaults to registering nothing.** A set declares what it
/// KEEPS, which is what "don't ship what we don't use" actually means — and
/// it means a primitive added to the vocabulary later does not silently
/// appear in an existing app's binary. Declare one with [`builtin_set!`]
/// rather than writing the impl by hand:
///
/// ```ignore
/// builtin_set!(pub Landing: view, text, image, link);
/// ```
///
/// [`AllBuiltins`] (every primitive — the default every backend boots with)
/// and [`CoreOnly`] (`view` + `text`) are the ready-made ends of the range.
///
/// # Why methods and not `const VIEW: bool`
///
/// A `const` selector is the obvious design and it **does not work** — this
/// was measured, not assumed. With `if S::NAV { register_navigator(r) }`,
/// rustc's monomorphization collector walks the generic MIR and instantiates
/// `register_navigator::<H>` *before* the associated const is substituted
/// and the branch folded. The handler is emitted, stays reachable, and the
/// bundle does not shrink: a `NAV = false` probe measured 562,759 bytes of
/// wasm against 564,846 all-on — 0.4%, versus 513,845 when the call was
/// physically deleted.
///
/// An empty default (or an override) makes the decision in trait resolution,
/// which *is* a monomorphization-time question: nothing names
/// `register_navigator::<H>` for that instantiation, so it is never emitted.
///
/// # Every boot path must pass the SAME set
///
/// A backend that exposes more than one boot entry (web has `start_in_with`
/// *and* `hydrate_in_with`, and a CLI-generated wrapper compiles in both)
/// re-anchors the full vocabulary if *either* still names [`AllBuiltins`].
/// Not theoretical: converting `start_in` alone moved a hello-world by 0.4%
/// because `hydrate_in` was holding everything alive.
///
/// # Dropping something you use
///
/// Realizing an unregistered payload panics at mount — the same loud failure
/// a missing third-party payload gets, deliberately not a silent fallback.
/// That is the signal that the set is too narrow.
pub trait BuiltinSet {
    /// `view` — the structural container.
    fn view<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `text`.
    fn text<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `button`.
    fn button<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `pressable`.
    fn pressable<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `image`.
    fn image<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `icon`.
    fn icon<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `link`.
    fn link<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `toggle`.
    fn toggle<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `slider`.
    fn slider<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `activity_indicator`.
    fn activity_indicator<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `text_input`.
    fn text_input<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `text_area`.
    fn text_area<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `scroll_view`.
    fn scroll_view<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `repeat` — the multi-node driver behind `ui!`/`jsx!` `for`.
    fn repeat<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `lazy` — the chunk boundary behind `lazy! { … }` / `#[component(lazy)]`.
    fn lazy<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `flat_list` / `virtualizer`.
    fn virtualizer<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `virtual_grid` — the two-axis virtualized grid.
    fn virtual_grid<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `graphics` / canvas.
    fn graphics<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `portal`, which also serves `overlay` / `anchored_overlay`.
    fn portal<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// `presence`.
    fn presence<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}
    /// The three navigator prims (`swap`, `stack`, outlet) — the single largest entry, ~52 KB raw / ~13 KB brotli on web.
    fn nav<H: AllCaps + 'static>(_registry: &mut Registry<H>) {}

    /// Backend services only the navigator family needs — on web, browser
    /// URL sync (`popstate` listener + the `UrlSyncService` impl).
    ///
    /// The backend passes a closure that installs them; a set without `nav`
    /// keeps this default, which never calls it, so the closure body — and
    /// every backend symbol it names — is never codegen'd.
    ///
    /// This exists because the install is otherwise UNCONDITIONAL at the
    /// boot seam, and it reaches `runtime_shared::primitives::navigator`.
    /// Dropping the three nav prims while still installing their URL-sync
    /// service left 16,153 bytes of navigator code in a `--primitives core`
    /// bundle — `NavigatorControl::dispatch` and its drop glue among them.
    /// Taking a closure rather than a `bool` is deliberate: a `const NAV`
    /// branch does NOT prune (rustc's monomorphization collector walks the
    /// MIR before the const folds) — the same trap documented on this trait.
    fn nav_services<F: FnOnce()>(_install: F) {}
}

/// Declare a [`BuiltinSet`] by the primitives it KEEPS.
///
/// ```ignore
/// builtin_set!(pub Landing: view, text, image, link);
/// builtin_set!(Admin: view, text, button, text_input, scroll_view, repeat, nav);
/// ```
///
/// Anything not listed is never named for this set, so it is never emitted.
/// Valid names are the methods of [`BuiltinSet`]; an unknown one is a
/// compile error rather than a silent no-op.
#[macro_export]
macro_rules! builtin_set {
    ($(#[$meta:meta])* $vis:vis $name:ident : $($keep:ident),* $(,)?) => {
        $(#[$meta])*
        $vis struct $name;
        impl $crate::BuiltinSet for $name {
            $( $crate::builtin_set_keep!(@keep $keep); )*
        }
    };
}

/// Per-primitive override emitted by [`builtin_set!`].
///
/// Implementation detail: one arm per [`BuiltinSet`] method. An arm missing
/// here makes `builtin_set!(X: that_name)` fail to compile, which is the
/// intended failure mode — `builtin_set_covers_every_builtin` in
/// `tests/builtin_surface.rs` pins the arms against the trait so a new
/// primitive cannot be left un-selectable.
#[macro_export]
#[doc(hidden)]
macro_rules! builtin_set_keep {
    (@keep view) => {
        fn view<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_view(registry)
        }
    };
    (@keep text) => {
        fn text<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_text(registry)
        }
    };
    (@keep button) => {
        fn button<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_button(registry)
        }
    };
    (@keep pressable) => {
        fn pressable<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_pressable(registry)
        }
    };
    (@keep image) => {
        fn image<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_image(registry)
        }
    };
    (@keep icon) => {
        fn icon<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_icon(registry)
        }
    };
    (@keep link) => {
        fn link<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_link(registry)
        }
    };
    (@keep toggle) => {
        fn toggle<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_toggle(registry)
        }
    };
    (@keep slider) => {
        fn slider<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_slider(registry)
        }
    };
    (@keep activity_indicator) => {
        fn activity_indicator<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_activity_indicator(registry)
        }
    };
    (@keep text_input) => {
        fn text_input<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_text_input(registry)
        }
    };
    (@keep text_area) => {
        fn text_area<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_text_area(registry)
        }
    };
    (@keep scroll_view) => {
        fn scroll_view<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_scroll_view(registry)
        }
    };
    (@keep repeat) => {
        fn repeat<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_repeat(registry)
        }
    };
    (@keep lazy) => {
        fn lazy<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_lazy(registry)
        }
    };
    (@keep virtualizer) => {
        fn virtualizer<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_virtualizer(registry)
        }
    };
    (@keep virtual_grid) => {
        fn virtual_grid<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_virtual_grid(registry)
        }
    };
    (@keep graphics) => {
        fn graphics<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_graphics(registry)
        }
    };
    (@keep portal) => {
        fn portal<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_portal(registry)
        }
    };
    (@keep presence) => {
        fn presence<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_presence(registry)
        }
    };
    (@keep nav) => {
        fn nav<H: $crate::caps::AllCaps + 'static>(
            registry: &mut $crate::__scene::Registry<H>,
        ) {
            $crate::handlers::register_navigator(registry)
        }
        // Selecting `nav` also opts into the backend services it needs.
        fn nav_services<F: ::std::ops::FnOnce()>(install: F) {
            install()
        }
    };
}

builtin_set!(
    /// Every builtin installed — the historical behavior of
    /// [`register_builtins`], and what every backend boots with unless the
    /// app opts down.
    pub AllBuiltins: view, text, button, pressable, image, icon, link, toggle, slider, activity_indicator, text_input, text_area, scroll_view, repeat, lazy, virtualizer, virtual_grid, graphics, portal, presence, nav
);

builtin_set!(
    /// The framework floor: `view` + `text`, nothing else.
    ///
    /// What remains is the reactive kernel, the scene model, the style
    /// engine and the backend — "just the core framework". Everything
    /// composable on top is dropped from the binary.
    pub CoreOnly: view, text
);

/// [`register_builtins`], restricted to the primitives `S` selects.
///
/// See [`BuiltinSet`] for why the selector is a type rather than a value.
pub fn register_builtins_with<H: AllCaps + 'static, S: BuiltinSet>(registry: &mut Registry<H>) {
    // Every primitive goes through `S`'s method — NOT an `if S::FLAG`.
    // That distinction is the whole mechanism; see `BuiltinSet`.
    S::view(registry);
    S::text(registry);
    S::button(registry);
    S::pressable(registry);
    S::image(registry);
    S::icon(registry);
    S::link(registry);
    S::toggle(registry);
    S::slider(registry);
    S::activity_indicator(registry);
    S::text_input(registry);
    S::text_area(registry);
    S::scroll_view(registry);
    S::repeat(registry);
    S::lazy(registry);
    S::virtualizer(registry);
    S::virtual_grid(registry);
    S::graphics(registry);
    S::portal(registry);
    S::presence(registry);
    S::nav(registry);
    install_kernel_diagnostic_bridge();
}

/// Route runtime-world's dev diagnostics (today: the staged-read warning)
/// into runtime-shared's `Logger`, so they surface on `console.warn` /
/// `NSLog` / `__android_log_print` instead of `eprintln!` — which is a
/// silent no-op sink on `wasm32-unknown-unknown`, i.e. invisible on the
/// most common dev target.
///
/// The bridge lives here because runtime-world is the BOTTOM of the new
/// core's dependency chain (world ← scene ← vocabulary ← backends) and
/// deliberately depends on nothing but `rustc-hash`; it cannot call
/// `log_warn!` itself. `register_builtins` is the one seam every backend's
/// boot passes through, which is why the install rides along here rather
/// than being repeated in each backend's `start_in`.
///
/// Debug builds only — in release the kernel diagnostic does not exist, so
/// neither does `install_diagnostic_sink`.
#[cfg(debug_assertions)]
fn install_kernel_diagnostic_bridge() {
    runtime_world::install_diagnostic_sink(Box::new(|msg: &str| {
        runtime_shared::logging::log(runtime_shared::logging::LogLevel::Warn, msg);
    }));
}

#[cfg(not(debug_assertions))]
#[inline(always)]
fn install_kernel_diagnostic_bridge() {}

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
