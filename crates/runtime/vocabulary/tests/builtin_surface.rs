//! Bundle-floor gate: pins the ALWAYS-RESIDENT built-in handler set.
//!
//! # Why this test exists
//!
//! On the old core, a heavy third-party renderer (canvas-vello → vello →
//! wgpu, the PDF stack, maps) reached `main.wasm` only through its
//! `Element::External` handler registration, and an SDK could *defer* that
//! registration into a `#[wasm_split]` chunk body
//! (`runtime_shared::defer_external_registration`) so the payload shipped in
//! the chunk instead. Four hand-built `examples/*-canvas` fixtures probed
//! that axis by eye, and `tests/lazy-external-split` + the
//! `prune-regression` runner measured it in bytes (a 512 KiB fake SDK; the
//! lazy variant's `main.wasm` had to be ≥ 400 KiB smaller).
//!
//! Runtime v2 removes that axis entirely. [`runtime_scene::realize`] takes
//! an `Rc<Registry<H>>` with **no interior mutability** and
//! `Registry::register` needs `&mut self`, so handlers can only be
//! installed at the boot seam (`register_builtins` + the app's
//! `register_scene_extensions`) before the tree goes live. Every
//! registered handler is therefore statically reachable from the entry
//! point, in every build, on every target — registration IS the bundle
//! floor, and there is no lazy escape hatch left to measure.
//!
//! So the byte-diff experiment has nothing left to vary, but the thing it
//! ultimately protected — "the framework does not silently grow what every
//! app must carry" — is now decided entirely by *how many handlers
//! `register_builtins` installs and what they reach*. That is what this
//! file gates, deterministically and offline, instead of building two wasm
//! artifacts and diffing them.
//!
//! # What a failure means
//!
//! - **Count changed.** A primitive was added to (or removed from) the
//!   unconditional built-in set. Adding one means every app on every
//!   target now links that handler and its transitive dependencies. That
//!   is a deliberate framework-wide decision (CLAUDE.md §3: peripheral
//!   features belong in an SDK that registers through the app's
//!   `register_scene_extensions`, not in `register_builtins`) — make it
//!   consciously, then update the expectations here and the doc comment on
//!   `register_builtins`.
//! - **Identity changed.** A payload type was renamed or its handler
//!   dropped; the `has::<T>()` assertions say which.
//!
//! The reachability floor *below* the handlers (a built-in handler gaining
//! a heavy dependency) is bounded by runtime-vocabulary's own dependency
//! list, which is `runtime-shared` / `runtime-scene` / `runtime-world` and
//! nothing else — see this crate's `Cargo.toml`, where every heavier
//! anchor (`mcp-catalog`'s `inventory` ctors, `runtime-shared`'s `linkme`
//! premint registry, `runtime-core`'s `LegacyBridge`) is behind an
//! off-by-default feature so shipped graphs carry none of it.

use runtime_scene::Registry;
use runtime_vocabulary::caps::ViewOps;
use runtime_vocabulary::prims::{
    ActivityIndicatorPrim, ButtonPrim, GraphicsPrim, IconPrim, ImagePrim, LazyPrim, LinkPrim,
    NavigatorOutletPrim, PortalPrim, PresencePrim, PressablePrim, PrimCell, RepeatPrim,
    ScrollViewPrim, SliderPrim, StackNavigatorPrim, SwapNavigatorPrim, TextAreaPrim, TextInputPrim,
    TextPrim, TogglePrim, ViewPrim, VirtualizerPrim,
};
use runtime_vocabulary::register_builtins;

/// Single-node handlers installed by `register_builtins`. Every entry is a
/// symbol the boot entry statically reaches in EVERY app.
const EXPECTED_SINGLE_HANDLERS: usize = 21;

/// Multi-node (`Element::Many`) handlers — currently only `repeat`.
const EXPECTED_MANY_HANDLERS: usize = 1;

fn builtins() -> Registry<host_mock::HostMock> {
    let mut registry: Registry<host_mock::HostMock> = Registry::new();
    register_builtins(&mut registry);
    registry
}

#[test]
fn regression_builtin_handler_set_size_is_pinned() {
    let registry = builtins();

    assert_eq!(
        registry.handler_count(),
        EXPECTED_SINGLE_HANDLERS,
        "register_builtins now installs {} single-node handlers (expected {}). \
         `handler_count` counts BOOT registration only, and every built-in \
         registers at boot, so this set is resident in every app's binary on \
         every target — it IS the framework's bundle floor. (Handlers installed \
         later via Registry::register_deferred are deliberately excluded: not \
         being reachable from boot is their purpose.) If the change is intended, \
         update EXPECTED_SINGLE_HANDLERS and the doc comment on \
         register_builtins; if the new primitive is a peripheral feature, ship it \
         as an SDK that registers through the app's register_scene_extensions \
         instead (CLAUDE.md §3).",
        registry.handler_count(),
        EXPECTED_SINGLE_HANDLERS,
    );
    assert_eq!(
        registry.many_handler_count(),
        EXPECTED_MANY_HANDLERS,
        "register_builtins now installs {} multi-node handlers (expected {}) — \
         same bundle-floor reasoning as the single-node set",
        registry.many_handler_count(),
        EXPECTED_MANY_HANDLERS,
    );
}

#[test]
fn regression_builtin_handler_identities_are_pinned() {
    let registry = builtins();

    // The 13 P2 primitives.
    assert!(registry.has::<PrimCell<ViewPrim>>(), "view");
    assert!(registry.has::<PrimCell<TextPrim>>(), "text");
    assert!(registry.has::<PrimCell<ButtonPrim>>(), "button");
    assert!(registry.has::<PrimCell<PressablePrim>>(), "pressable");
    assert!(registry.has::<PrimCell<ImagePrim>>(), "image");
    assert!(registry.has::<PrimCell<IconPrim>>(), "icon");
    assert!(registry.has::<PrimCell<TogglePrim>>(), "toggle");
    assert!(registry.has::<PrimCell<SliderPrim>>(), "slider");
    assert!(
        registry.has::<PrimCell<ActivityIndicatorPrim>>(),
        "activity_indicator"
    );
    assert!(registry.has::<PrimCell<LinkPrim>>(), "link");
    assert!(registry.has::<PrimCell<ScrollViewPrim>>(), "scroll_view");
    assert!(registry.has::<PrimCell<TextInputPrim>>(), "text_input");
    assert!(registry.has::<PrimCell<TextAreaPrim>>(), "text_area");

    // The P3 set + the navigator prims + the lazy chunk boundary.
    assert!(registry.has::<PrimCell<VirtualizerPrim>>(), "virtualizer");
    assert!(registry.has::<PrimCell<GraphicsPrim>>(), "graphics");
    assert!(registry.has::<PrimCell<PortalPrim>>(), "portal");
    assert!(registry.has::<PrimCell<PresencePrim>>(), "presence");
    assert!(
        registry.has::<PrimCell<SwapNavigatorPrim>>(),
        "swap_navigator"
    );
    assert!(
        registry.has::<PrimCell<StackNavigatorPrim>>(),
        "stack_navigator"
    );
    assert!(
        registry.has::<PrimCell<NavigatorOutletPrim>>(),
        "navigator_outlet"
    );
    assert!(registry.has::<PrimCell<LazyPrim>>(), "lazy");

    // Multi-node.
    assert!(
        registry
            .get_many(std::any::TypeId::of::<PrimCell<RepeatPrim>>())
            .is_some(),
        "repeat (Element::Many handler)"
    );
}

/// The mechanism the bundle floor rests on: a payload type NOBODY
/// registered has no handler, and giving it one at boot costs an
/// exclusive borrow — so the boot set is closed and countable.
///
/// A post-boot seam DOES exist now (`Registry::defer` +
/// `register_deferred`, the runtime-v2 successor to
/// `defer_external_registration`, measured by
/// `tests/lazy-payload-split`), which is exactly how a heavy SDK leaves
/// `main.wasm`. It does not dilute this gate: late handlers are counted
/// separately (`late_handler_count`), and parking a payload requires an
/// explicit `defer::<T>()` declaration — the built-ins declare none, as
/// asserted below.
#[test]
fn regression_builtin_registration_is_boot_only_and_declares_no_deferred_kinds() {
    struct UnregisteredPayload;

    let mut registry = builtins();
    assert!(
        !registry.has::<UnregisteredPayload>(),
        "a payload nobody registered must have no handler — realize panics on it \
         (the scene contract), which is precisely why every BUILT-IN handler is \
         installed at the boot seam and is therefore main-resident"
    );
    assert_eq!(
        registry.deferred_kind_count(),
        0,
        "no built-in primitive may be late-bound: deferral is the third-party \
         bundle-size seam, and a parked built-in would mean the framework's own \
         vocabulary is not resident at boot"
    );
    assert_eq!(registry.late_handler_count(), 0);

    // The boot installation path: an exclusive borrow, before the registry
    // is handed to `realize` as a shared `Rc`.
    let previous = registry.register::<UnregisteredPayload, _>(|cx, _p, _children| {
        cx.backend().borrow_mut().create_view(&Default::default())
    });
    assert!(previous.is_none(), "first registration returns no predecessor");
    assert!(registry.has::<UnregisteredPayload>());
}

// ===========================================================================
// BuiltinSet — per-primitive, opt-in bundle seam
// ===========================================================================

use runtime_vocabulary::{
    builtin_set, register_builtins_with, AllBuiltins, BuiltinSet, CoreOnly,
};

/// `AllBuiltins` must be exactly what `register_builtins` has always
/// installed. If these diverge, every existing backend boot silently changes
/// behavior, because `register_builtins` IS
/// `register_builtins_with::<_, AllBuiltins>`.
///
/// This also guards the `builtin_set!` primitive list: `AllBuiltins` is
/// declared through the macro, so a primitive added to `BuiltinSet` but left
/// out of that list would show up here as a count mismatch rather than
/// silently never registering.
#[test]
fn all_builtins_is_identical_to_the_historical_set() {
    let mut selected: Registry<host_mock::HostMock> = Registry::new();
    register_builtins_with::<host_mock::HostMock, AllBuiltins>(&mut selected);

    let legacy = builtins();
    assert_eq!(selected.handler_count(), legacy.handler_count());
    assert_eq!(selected.many_handler_count(), legacy.many_handler_count());
    assert_eq!(selected.handler_count(), EXPECTED_SINGLE_HANDLERS);
    assert_eq!(selected.many_handler_count(), EXPECTED_MANY_HANDLERS);
}

/// `CoreOnly` is the "just the core framework" floor: the reactive kernel,
/// scene model, style engine and backend, with `view` + `text` and nothing
/// else composable on top.
#[test]
fn core_only_registers_view_and_text_and_nothing_else() {
    let mut registry: Registry<host_mock::HostMock> = Registry::new();
    register_builtins_with::<host_mock::HostMock, CoreOnly>(&mut registry);

    assert!(registry.has::<PrimCell<ViewPrim>>());
    assert!(registry.has::<PrimCell<TextPrim>>());
    assert_eq!(registry.handler_count(), 2, "view + text only");
    assert_eq!(
        registry.many_handler_count(),
        0,
        "`repeat` is opt-in too — a core-only app has no `for`"
    );

    // Nothing became late-bound: opting down is still boot-only registration.
    assert_eq!(registry.deferred_kind_count(), 0);
    assert_eq!(registry.late_handler_count(), 0);
}

/// A set declares what it KEEPS. Everything unlisted must be absent — that
/// absence is what lets LLVM drop the handler and everything it reaches.
#[test]
fn builtin_set_keeps_exactly_what_it_lists() {
    builtin_set!(Landing: view, text, image, link);

    let mut registry: Registry<host_mock::HostMock> = Registry::new();
    register_builtins_with::<host_mock::HostMock, Landing>(&mut registry);

    assert!(registry.has::<PrimCell<ViewPrim>>());
    assert!(registry.has::<PrimCell<TextPrim>>());
    assert!(registry.has::<PrimCell<ImagePrim>>());
    assert!(registry.has::<PrimCell<LinkPrim>>());
    assert_eq!(registry.handler_count(), 4);

    // The big-ticket drops the seam exists for.
    assert!(!registry.has::<PrimCell<StackNavigatorPrim>>());
    assert!(!registry.has::<PrimCell<SwapNavigatorPrim>>());
    assert!(!registry.has::<PrimCell<NavigatorOutletPrim>>());
    assert!(!registry.has::<PrimCell<VirtualizerPrim>>());
    assert!(!registry.has::<PrimCell<GraphicsPrim>>());
    assert!(!registry.has::<PrimCell<PortalPrim>>());
    assert!(!registry.has::<PrimCell<ButtonPrim>>());
    assert_eq!(registry.many_handler_count(), 0);
}

/// Dropping only the navigator — the single largest entry (~52 KB raw /
/// ~13 KB brotli on web) and the motivating case for this seam.
#[test]
fn dropping_nav_removes_exactly_the_three_navigator_prims() {
    builtin_set!(NoNav:
        view, text, button, pressable, image, icon, link, toggle, slider,
        activity_indicator, text_input, text_area, scroll_view, repeat, lazy,
        virtualizer, graphics, portal, presence,
    );

    let mut registry: Registry<host_mock::HostMock> = Registry::new();
    register_builtins_with::<host_mock::HostMock, NoNav>(&mut registry);

    assert_eq!(
        registry.handler_count(),
        EXPECTED_SINGLE_HANDLERS - 3,
        "swap + stack + outlet, and nothing else"
    );
    assert!(!registry.has::<PrimCell<StackNavigatorPrim>>());
    assert!(!registry.has::<PrimCell<SwapNavigatorPrim>>());
    assert!(!registry.has::<PrimCell<NavigatorOutletPrim>>());

    // Everything else survives, including the multi-node `repeat`.
    assert!(registry.has::<PrimCell<ViewPrim>>());
    assert!(registry.has::<PrimCell<PortalPrim>>());
    assert!(registry.has::<PrimCell<VirtualizerPrim>>());
    assert_eq!(registry.many_handler_count(), EXPECTED_MANY_HANDLERS);
}

/// `nav_services` must ride the `nav` selection.
///
/// The web backend's URL-sync install reaches
/// `runtime_shared::primitives::navigator`, so calling it unconditionally
/// re-anchored navigator code in bundles that had dropped the nav prims —
/// `NavigatorControl::dispatch` and its drop glue survived a
/// `--primitives core` build until this hook existed (measured: 359,684 →
/// 351,538 bytes of wasm once gated).
///
/// A `bool` would not do: the backend must never *name* the install for a
/// set without nav, or the collector emits it regardless. Hence a closure
/// the default simply never calls.
#[test]
fn nav_services_run_only_for_sets_that_keep_nav() {
    use std::cell::Cell;

    // A set WITH nav runs the backend's installer.
    let ran = Cell::new(false);
    AllBuiltins::nav_services(|| ran.set(true));
    assert!(
        ran.get(),
        "AllBuiltins keeps `nav`, so navigator backend services must install"
    );

    // A set WITHOUT nav never invokes it — which is what lets the linker
    // drop the closure body and everything it names.
    let ran = Cell::new(false);
    CoreOnly::nav_services(|| ran.set(true));
    assert!(
        !ran.get(),
        "CoreOnly drops `nav`, so its backend services must not install — \
         calling them would re-anchor navigator code the set excluded"
    );

    // Explicitly selecting nav opts back in.
    builtin_set!(WithNav: view, text, nav);
    let ran = Cell::new(false);
    WithNav::nav_services(|| ran.set(true));
    assert!(ran.get(), "an explicit `nav` keep must install its services");
}
