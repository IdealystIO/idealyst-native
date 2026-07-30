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
