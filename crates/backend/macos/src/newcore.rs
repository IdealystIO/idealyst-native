//! New-core adoption for the macOS backend (idea-lite migration, P4a).
//!
//! Implements [`runtime_scene::Host`] plus **all 30** capability traits
//! (`runtime_vocabulary::caps`) directly on [`MacosBackend`] — the
//! production shape of the migration: no `LegacyBridge` wrapper in the
//! render path. Every trait method delegates via UFCS
//! (`<MacosBackend as Backend>::method(self, …)`) to the existing
//! `Backend` impl, so the AppKit mechanism code (NSView creation, Taffy
//! layout, style translation, transitions, virtualizer cells) is REUSED
//! verbatim — this module adds a second *front* onto the same machinery,
//! exactly like `backend-web/src/newcore.rs` does for the DOM (this file
//! mirrors that module mechanically; keep the two in sync).
//!
//! # Delegation status — every capability accounted for
//!
//! | Trait | Status |
//! |---|---|
//! | `runtime_scene::Host` (7 ops) | direct (`create_anchor` → `create_reactive_anchor`, `supports_splice` → `supports_child_splice` — the P1 renames) |
//! | `AppEnvOps` | direct |
//! | `LifecycleOps` | direct (`is_hydrating` is always `false` on this backend — no hydration on native) |
//! | `ViewOps` | direct |
//! | `InputOps` | direct |
//! | `PressableOps` | direct |
//! | `TextOps` | direct (the js-binding methods resolve to the trait-default no-ops, same as the old walker: `supports_js_text_bindings` is `false` here) |
//! | `ButtonOps` | direct |
//! | `ImageOps` | direct |
//! | `IconOps` | direct |
//! | `LinkOps` | direct |
//! | `TextInputOps` | direct |
//! | `ToggleOps` | direct |
//! | `SliderOps` | direct |
//! | `ActivityIndicatorOps` | direct |
//! | `ScrollOps` | direct |
//! | `SafeAreaOps` | direct |
//! | `VirtualizerOps` | direct |
//! | `GraphicsOps` | direct |
//! | `PortalOps` | direct |
//! | `PresenceOps` | direct |
//! | `NavigatorOps` | direct |
//! | `ExternalOps` | direct |
//! | `DocumentOps` | direct (web-flavored methods — `create_element`, `attach_html_*`, `register_raw_css` — resolve to the same trait-default no-ops the old walker hit on this backend) |
//! | `StyleOps` | direct (class-minting methods return the trait-default `None` on native; the vocabulary's `attach_style` then takes the `apply_styled_variants` path — identical routing to the old walker. See *Styling* below.) |
//! | `AssetOps` | direct |
//! | `A11yOps` | direct |
//! | `AnimationOps` | direct |
//! | `IntrospectionOps` | direct |
//! | `BatchOps` | direct |
//! | `WireBindingOps` | direct (wire-recorder no-ops on this backend, same as today) |
//!
//! **30/30 direct, 0 adapted, 0 stubbed.** Nothing panics, nothing
//! silently no-ops beyond what the wrapped `Backend` impl already does.
//! Where the macOS backend does not override a `Backend` method (the
//! DOM-only and wire-only families), the UFCS call resolves to the same
//! trait-default fallback the old walker would hit — behavior is
//! identical by construction. The impl bodies below are generated
//! mechanically from `backend-web/src/newcore.rs` (itself generated from
//! `runtime_vocabulary::bridge`, the compile-time proof of the signature
//! freeze), with `WebBackend` → `MacosBackend`.
//!
//! # Styling note for the parallel P3c style-engine work
//!
//! On this backend `mint_style_class` / `mint_class_for_app` /
//! `supports_js_class_bindings` all take the trait-default path (`None` /
//! `false`): macOS styles are applied per-node via
//! `apply_style`/`apply_styled_variants` and re-fired through the
//! theme-cohort driver on token updates
//! (`token_updates_propagate_via_cascade` is `false`, so cohort fan-out —
//! not CSS cascade — is the token-update mechanism). Any new-core style
//! engine must keep driving the cohort re-apply path on native.
//!
//! # Boot path — [`start`]
//!
//! Client-render-only mount of a `runtime_scene::Element` tree against
//! live AppKit through the registry. Unlike web (where the browser is the
//! host and `backend_web::newcore::start` owns the whole boot), macOS
//! splits host duties: `host-appkit` owns NSApplication/NSWindow and the
//! content-view handoff (`create_host_root` → `setContentView` →
//! `set_host_root`), and THIS function owns everything from "backend is
//! wired to a window" onward. `host_appkit::newcore::run` is the
//! all-in-one entry apps call; it constructs the backend exactly like the
//! old-core `host_appkit::run_with` and then calls [`start`].
//!
//! Sequence (mirrors `runtime_core::mount`'s ordering where they overlap):
//!
//! 1. Install the default monotonic time source (the macOS analogue of
//!    web `start_in`'s `install_time_source` — without it the animation
//!    clock and `PhaseTimer` read 0). The old `mount` preamble's other
//!    ambient installs (current platform, color scheme, URL opener,
//!    announcer) are runtime-core-private and skipped here, exactly as
//!    on the web new-core boot — public seams for them are a
//!    later-phase migration item.
//! 2. `Registry` (`register_builtins` + the `register` seam) + `World` +
//!    `world.enter(realize)`.
//! 3. `runtime_core::scheduling::drain_buffered_microtasks()` — the host
//!    opened a mount-buffering window (`begin_mount_buffering`) before
//!    calling in, so microtasks scheduled during the build (deferred
//!    chrome, follow-up layout passes) run HERE, synchronously, before
//!    `finish` — landing in the first layout/paint exactly like the old
//!    boot (`runtime_core::mount` drains at the same point; see
//!    `backend_apple_core::scheduler::MOUNT_BUFFER` and the macOS
//!    chrome-first-paint-buffering note). A [`schedule_flush`] issued
//!    during the window also buffers and therefore commits inside this
//!    drain — staged writes cannot leak past the first paint.
//! 4. `Backend::finish(root)` — parents the single root NSView into the
//!    host root (`addSubview`) and runs the first Taffy layout pass: the
//!    exact root-attachment mechanism of the old mount. `finish` is
//!    called with the backend `RefCell` borrowed and defers its
//!    viewport-signal mirror to a microtask for that reason — do not
//!    drain microtasks while the borrow is held.
//! 5. `world.flush()` — commit anything staged during mount (ref-fill
//!    callbacks, handler setup) before the first paint.
//! 6. Install the flush pump, retain `{Realized, pump, backend,
//!    registry, world}` in the returned [`NewCoreApp`]. The host closes
//!    the buffering window (`end_mount_buffering`) after this returns —
//!    leftover microtasks (e.g. `finish`'s viewport mirror) dispatch
//!    normally onto the main queue, same as the old boot.
//!
//! **Hydration is NOT in scope** (native never hydrates); navigator /
//! portal / presence payload handlers are not ported yet (later phase) —
//! the smoke app avoids them.
//!
//! # Flush driver (design §3: Apple = runloop turn boundary)
//!
//! The new kernel stages writes; nothing is observable until the host
//! driver calls [`World::flush`]. Two hooks, mirroring the web driver's
//! event-listener + rAF pair with this platform's equivalents:
//!
//! 1. **Local NSEvent monitor → microtask.** `install_flush_pump`
//!    registers ONE `NSEvent addLocalMonitorForEventsMatchingMask:`
//!    monitor for the discrete event families that reach author callbacks
//!    (mouse down/up ×3 buttons, key down/up — the macOS analogue of
//!    web's `click`/`input`/`keydown`/… window listeners). A local
//!    monitor fires BEFORE `NSApplication sendEvent:` dispatches the
//!    event; the monitor calls [`schedule_flush`], which queues ONE
//!    deduped `runtime_core::scheduling::schedule_microtask` — on this
//!    platform that is `dispatch_async(main_queue)` (see
//!    `backend_apple_core::scheduler`), which drains on a LATER run-loop
//!    iteration, i.e. strictly AFTER the current event's synchronous
//!    dispatch (responder chain, target-action, author `on_press`
//!    closures) completes. Net effect: stage during dispatch, commit at
//!    the run-loop turn boundary right after — the idea-lite contract.
//!    Same precedent as `imp/keyboard.rs`'s app-key monitor.
//! 2. **Frame tick.** A `runtime_core::scheduling::raf_loop` (on macOS: a
//!    common-modes 60 Hz NSTimer — the platform's CADisplayLink stand-in,
//!    see the scheduler's `raf_loop` note) flushes once per frame. This
//!    is the animation-tick driver from the design AND the safety net
//!    for staged writes whose event never crosses the monitor: AppKit
//!    control **tracking loops** (NSButton/NSSlider pull events via
//!    `nextEventMatchingMask:` during a press/drag, bypassing
//!    `sendEvent:` and therefore local monitors) and `after_ms` timer
//!    callbacks both commit at the next frame boundary instead of never.
//!    Common modes matter: the timer keeps firing during those same
//!    tracking loops. An empty flush is a cheap early-out, so the idle
//!    cost is one vec-drain per frame.
//!
//! Both hooks funnel through [`schedule_flush`]/`flush_now`, which skips
//! re-entrant flushes (`world.is_flushing()`) — belt and braces; a
//! main-queue microtask can't actually preempt a synchronous flush.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{class, msg_send};
use objc2_foundation::NSObject;
use runtime_core::accessibility::{AccessibilityProps, AccessibilityTree, LiveRegionPriority, Role};
use runtime_core::animation::AnimProp;
use runtime_core::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId,
};
use runtime_core::breakpoint::Breakpoint;
use runtime_core::introspect::NativeNode;
use runtime_core::primitives;
use runtime_core::primitives::portal::ViewportRect;
use runtime_core::styled_text::TextRun;
use runtime_core::{
    Action, Backend, BackendBatch, Color, ColorScheme, Easing, FileDropHandler, FontFamily,
    HoverHandler, ImageErrorHandler, ImageLoadHandler, PageMetadata, Platform, SafeAreaSides,
    Screenshot, StateBits, StyleApplication, StyleRules, TokenEntry, Tokenized, TouchHandler,
    TouchId, VirtualizerCallbacks, WheelHandler,
};
use runtime_scene::{realize, Element, Host, Realized, Registry};
use runtime_vocabulary::caps;
use runtime_world::World;

use crate::imp::{MacosBackend, MacosNode};

// Re-exported so the host shell (`host-appkit`) and app wrappers can
// name the boot-path types without a direct runtime-scene dependency —
// mirrors how consumers reach the old core's `Element` through
// `runtime_core`.
pub use runtime_scene::Element as SceneElement;
pub use runtime_scene::Registry as SceneRegistry;

// ===========================================================================
// Boot path
// ===========================================================================

thread_local! {
    /// The world the flush driver commits. Kept out of [`NewCoreApp`]'s
    /// custody so [`schedule_flush`] never touches app state (a flush can
    /// run while the app value is mid-construction inside [`start`]).
    static FLUSH_WORLD: RefCell<Option<World>> = const { RefCell::new(None) };
    /// Dedup flag: one queued flush microtask at a time.
    static FLUSH_QUEUED: Cell<bool> = const { Cell::new(false) };
}

/// Everything the boot path must keep alive. Field order is drop order:
/// the realized tree unmounts before the world (its slots' owner) dies,
/// and the flush pump's monitor/timer cancel before the world they flush
/// is gone. The host typically `std::mem::forget`s this before entering
/// the run loop (same retention as the old boot's `forget(owner)` — the
/// process exits with the run loop).
pub struct NewCoreApp {
    realized: Realized<MacosNode>,
    _pump: FlushPump,
    _backend: Rc<RefCell<MacosBackend>>,
    _registry: Rc<Registry<MacosBackend>>,
    world: World,
}

impl NewCoreApp {
    /// Borrow the live tree (tests, diagnostics).
    pub fn with_realized<R>(&self, f: impl FnOnce(&Realized<MacosNode>) -> R) -> R {
        f(&self.realized)
    }

    /// The mounted world (tests can flush it explicitly).
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Unmount: drops the `Realized` (cleanups fire, views detach from
    /// the live tree's point of view), the flush pump, and the world —
    /// after unhooking the flush driver's world reference. Primarily for
    /// tests; a windowed app forgets the value instead.
    pub fn stop(self) {
        set_flush_world(None);
        drop(self);
    }
}

/// Mount a new-core element tree into an already-window-wired backend.
///
/// The host must have: constructed the backend, parented its host root
/// into an NSWindow (`create_host_root` → `setContentView` →
/// `set_host_root`), installed the scheduler + global self-handle, and
/// opened a mount-buffering window. See the module docs for the exact
/// sequence and `host_appkit::newcore::run` for the canonical caller.
///
/// `register` runs after [`runtime_vocabulary::register_builtins`], so
/// apps/SDKs can register their own payload handlers on the same
/// registry before the tree realizes. The build closure runs inside
/// `world.enter`, so free `signal()`/`effect()` calls work; top-level
/// creations are world-root-owned (they live until app teardown).
pub fn start(
    backend: Rc<RefCell<MacosBackend>>,
    register: impl FnOnce(&mut Registry<MacosBackend>),
    build: impl FnOnce() -> Element,
) -> NewCoreApp {
    // Monotonic clock (step 1 in the module docs) — the macOS analogue
    // of web `start_in`'s `install_time_source`. Idempotent, first
    // install wins. The old `mount` preamble's other ambient installs
    // (`install_current_platform` / color scheme / URL opener /
    // announcer) live in a runtime-core-private module and are NOT
    // reachable from a backend crate — same situation as
    // `backend_web::newcore::start`, which also boots without them.
    // Author code reading `runtime_core::platform()` on the new core
    // gets the uninstalled default until the migration gives those
    // installs a public seam (later-phase item, noted in module docs).
    let platform = backend.borrow().platform();
    runtime_core::time::install_default_time_source(platform);

    let mut registry: Registry<MacosBackend> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    register(&mut registry);
    let registry = Rc::new(registry);

    let world = World::new();
    let realized = world.enter(|| {
        let element = build();
        realize(&backend, &registry, element)
    });

    // Pre-`finish` buffered drain (step 3) — deferred chrome and any
    // build-time `schedule_flush` land before the first layout. Must run
    // with NO backend borrow held (drained tasks re-borrow the backend).
    runtime_core::scheduling::drain_buffered_microtasks();

    // Single-root contract, matching the old-core mount: `finish`
    // parents the root view into the host root and runs the first
    // layout pass.
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "backend_macos::newcore::start: the app root must contribute exactly one \
             top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    Backend::finish(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount before the first paint.
    world.flush();

    let pump = install_flush_pump();
    set_flush_world(Some(world.clone()));
    NewCoreApp {
        realized,
        _pump: pump,
        _backend: backend,
        _registry: registry,
        world,
    }
}

fn set_flush_world(world: Option<World>) {
    FLUSH_WORLD.with(|w| *w.borrow_mut() = world);
}

// ===========================================================================
// Flush driver
// ===========================================================================

/// Queue one flush of the mounted world on the framework microtask
/// queue (deduped). Safe to call any time; a no-op before [`start`].
/// The event monitor and future new-core wrappers call this right
/// after author-visible dispatch. During a mount-buffering window the
/// microtask buffers and commits inside the host's synchronous drain
/// (module docs, step 3).
pub fn schedule_flush() {
    if FLUSH_QUEUED.with(|q| q.replace(true)) {
        return;
    }
    runtime_core::scheduling::schedule_microtask(|| {
        FLUSH_QUEUED.with(|q| q.set(false));
        flush_now();
    });
}

/// Flush the mounted world immediately (skipped while it is already
/// mid-flush).
fn flush_now() {
    let world = FLUSH_WORLD.with(|w| w.borrow().clone());
    if let Some(world) = world {
        if !world.is_flushing() {
            world.flush();
        }
    }
}

/// `NSEventMask` bits for the discrete event families that reach author
/// callbacks — the macOS analogue of the web driver's `FLUSH_EVENTS`
/// window listeners. Continuous streams (mouseMoved, scrollWheel) are
/// deliberately absent: they are frame-paced by the timer hook, which is
/// the right cadence for drag/scroll-driven state. Bit positions are
/// `1 << NSEventType` (same convention as `imp/keyboard.rs`'s
/// `NS_EVENT_MASK_KEY_DOWN`).
const FLUSH_EVENT_MASK: usize = (1 << 1)  // leftMouseDown
    | (1 << 2)   // leftMouseUp
    | (1 << 3)   // rightMouseDown
    | (1 << 4)   // rightMouseUp
    | (1 << 25)  // otherMouseDown
    | (1 << 26)  // otherMouseUp
    | (1 << 10)  // keyDown
    | (1 << 11); // keyUp

/// The two flush hooks (module docs): a local NSEvent monitor for
/// discrete events + the frame-tick loop. Dropping cancels both (the
/// `RafLoop` handle invalidates its NSTimer on drop; the monitor is
/// removed explicitly).
struct FlushPump {
    _raf: runtime_core::scheduling::RafLoop,
    monitor: Option<Retained<NSObject>>,
}

fn install_flush_pump() -> FlushPump {
    // The monitor block: nudge the flush driver, return the event
    // UNCHANGED so normal routing continues (never swallow). The body is
    // two thread-local ops + at most one dispatch_async — panic-free by
    // inspection, so no catch_unwind shim is needed here (compare
    // `imp/keyboard.rs`, whose block runs arbitrary author handlers).
    let block = RcBlock::new(move |event: *mut NSObject| -> *mut NSObject {
        schedule_flush();
        event
    });
    // `addLocalMonitor…` copies the handler block internally, so the
    // local `block` may drop after this; we retain the returned monitor
    // token to feed `removeMonitor:` later. Same shape as
    // `imp/keyboard.rs::set_app_key_handler`.
    let monitor: *mut NSObject = unsafe {
        msg_send![
            class!(NSEvent),
            addLocalMonitorForEventsMatchingMask: FLUSH_EVENT_MASK,
            handler: &*block,
        ]
    };
    FlushPump {
        _raf: runtime_core::scheduling::raf_loop(flush_now),
        monitor: unsafe { Retained::retain(monitor) },
    }
}

impl Drop for FlushPump {
    fn drop(&mut self) {
        if let Some(monitor) = self.monitor.take() {
            unsafe {
                let _: () = msg_send![class!(NSEvent), removeMonitor: &*monitor];
            }
        }
    }
}

// ===========================================================================
// Host + capability-trait delegation (generated from
// runtime_vocabulary::bridge — keep mechanically in sync; the scene-parity
// goldens + the AllCaps bound on register_builtins are the compile gates)
// ===========================================================================

// ---------------------------------------------------------------------------
// Host — the P1 structural seam
// ---------------------------------------------------------------------------

impl Host for MacosBackend {
    type Node = MacosNode;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        <MacosBackend as Backend>::insert(self, parent, child)
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        <MacosBackend as Backend>::insert_many(self, parent, children)
    }

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        <MacosBackend as Backend>::insert_at(self, parent, child, index)
    }

    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        <MacosBackend as Backend>::remove_child(self, parent, child)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        <MacosBackend as Backend>::clear_children(self, node)
    }

    fn create_anchor(&mut self) -> Self::Node {
        <MacosBackend as Backend>::create_reactive_anchor(self)
    }

    fn supports_splice(&self) -> bool {
        <MacosBackend as Backend>::supports_child_splice(self)
    }
}

// ---------------------------------------------------------------------------
// App environment + lifecycle
// ---------------------------------------------------------------------------

impl caps::AppEnvOps for MacosBackend {
    fn color_scheme(&self) -> ColorScheme {
        <MacosBackend as Backend>::color_scheme(self)
    }

    fn platform(&self) -> Platform {
        <MacosBackend as Backend>::platform(self)
    }

    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
        <MacosBackend as Backend>::url_opener(self)
    }

    fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
        <MacosBackend as Backend>::fullscreen_setter(self)
    }

    fn set_page_metadata(&mut self, meta: &PageMetadata) {
        <MacosBackend as Backend>::set_page_metadata(self, meta)
    }

    fn set_app_background(&mut self, color: &Tokenized<Color>) {
        <MacosBackend as Backend>::set_app_background(self, color)
    }

    fn set_scrollbar_theme(&mut self, thumb: &Tokenized<Color>, track: &Tokenized<Color>) {
        <MacosBackend as Backend>::set_scrollbar_theme(self, thumb, track)
    }

    fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
        <MacosBackend as Backend>::set_app_key_handler(self, handler)
    }
}

impl caps::LifecycleOps for MacosBackend {
    fn finish(&mut self, root: Self::Node) {
        <MacosBackend as Backend>::finish(self, root)
    }

    fn run_layout(&mut self) {
        <MacosBackend as Backend>::run_layout(self)
    }

    fn schedule_layout_pass() {
        <MacosBackend as Backend>::schedule_layout_pass()
    }

    fn is_hydrating(&self) -> bool {
        <MacosBackend as Backend>::is_hydrating(self)
    }

    fn renders_lazy_chunks(&self) -> bool {
        <MacosBackend as Backend>::renders_lazy_chunks(self)
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for MacosBackend {
    fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        <MacosBackend as Backend>::create_view(self, a11y)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        <MacosBackend as Backend>::make_view_handle(self, node)
    }
}

impl caps::InputOps for MacosBackend {
    fn install_touch_handler(&mut self, node: &Self::Node, handler: TouchHandler) {
        <MacosBackend as Backend>::install_touch_handler(self, node, handler)
    }

    fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId) {
        <MacosBackend as Backend>::claim_touch(self, node, touch_id)
    }

    fn install_wheel_handler(&mut self, node: &Self::Node, handler: WheelHandler) {
        <MacosBackend as Backend>::install_wheel_handler(self, node, handler)
    }

    fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler) {
        <MacosBackend as Backend>::install_hover_handler(self, node, handler)
    }

    fn mark_preserves_focus(&mut self, node: &Self::Node) {
        <MacosBackend as Backend>::mark_preserves_focus(self, node)
    }

    fn install_file_drop_handler(&mut self, node: &Self::Node, handler: FileDropHandler) {
        <MacosBackend as Backend>::install_file_drop_handler(self, node, handler)
    }
}

impl caps::PressableOps for MacosBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node {
        <MacosBackend as Backend>::create_pressable(self, on_click, a11y)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> runtime_core::PressableHandle {
        <MacosBackend as Backend>::make_pressable_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Text + button
// ---------------------------------------------------------------------------

impl caps::TextOps for MacosBackend {
    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node {
        <MacosBackend as Backend>::create_text(self, content, a11y)
    }

    fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node {
        <MacosBackend as Backend>::create_styled_text(self, runs, a11y)
    }

    fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]) {
        <MacosBackend as Backend>::update_styled_text(self, node, runs)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        <MacosBackend as Backend>::update_text(self, node, content)
    }

    fn create_text_with_id(
        &mut self,
        content: &str,
        a11y: &AccessibilityProps,
    ) -> Option<(Self::Node, u32)> {
        <MacosBackend as Backend>::create_text_with_id(self, content, a11y)
    }

    fn update_text_by_id(&mut self, id: u32, content: String) {
        <MacosBackend as Backend>::update_text_by_id(self, id, content)
    }

    fn release_text_id(&mut self, id: u32) {
        <MacosBackend as Backend>::release_text_id(self, id)
    }

    fn supports_js_text_bindings(&self) -> bool {
        <MacosBackend as Backend>::supports_js_text_bindings(self)
    }

    fn register_reactive_text_binding(
        &mut self,
        text_id: u32,
        signal_ids: &[u64],
        template_parts: &[&str],
        initial_values: &[&str],
        stringifiers: &[Rc<dyn Fn() -> String>],
    ) {
        <MacosBackend as Backend>::register_reactive_text_binding(
            self,
            text_id,
            signal_ids,
            template_parts,
            initial_values,
            stringifiers,
        )
    }

    fn release_reactive_text_binding(&mut self, text_id: u32) {
        <MacosBackend as Backend>::release_reactive_text_binding(self, text_id)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        <MacosBackend as Backend>::make_text_handle(self, node)
    }
}

impl caps::ButtonOps for MacosBackend {
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_button(self, label, on_click, leading_icon, trailing_icon, a11y)
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        <MacosBackend as Backend>::update_button_label(self, node, label)
    }

    fn make_button_handle(&self, node: &Self::Node) -> runtime_core::ButtonHandle {
        <MacosBackend as Backend>::make_button_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl caps::ImageOps for MacosBackend {
    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &AccessibilityProps) -> Self::Node {
        <MacosBackend as Backend>::create_image(self, src, alt, a11y)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        <MacosBackend as Backend>::update_image_src(self, node, src)
    }

    fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>) {
        <MacosBackend as Backend>::update_image_alt(self, node, alt)
    }

    fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler) {
        <MacosBackend as Backend>::install_image_load_handler(self, node, handler)
    }

    fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler) {
        <MacosBackend as Backend>::install_image_error_handler(self, node, handler)
    }

    fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
        <MacosBackend as Backend>::make_image_handle(self, node)
    }
}

impl caps::IconOps for MacosBackend {
    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_icon(self, data, color, a11y)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        <MacosBackend as Backend>::update_icon_color(self, node, color)
    }

    fn update_icon_data(&mut self, node: &Self::Node, data: &primitives::icon::IconData) {
        <MacosBackend as Backend>::update_icon_data(self, node, data)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        <MacosBackend as Backend>::update_icon_stroke(self, node, progress)
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        <MacosBackend as Backend>::animate_icon_stroke(
            self,
            node,
            from,
            to,
            duration_ms,
            easing,
            infinite,
            autoreverses,
        )
    }

    fn make_icon_handle(&self, node: &Self::Node) -> primitives::icon::IconHandle {
        <MacosBackend as Backend>::make_icon_handle(self, node)
    }
}

impl caps::LinkOps for MacosBackend {
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_link(self, config, a11y)
    }

    fn update_link_url(&mut self, node: &Self::Node, url: &str) {
        <MacosBackend as Backend>::update_link_url(self, node, url)
    }

    fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle {
        <MacosBackend as Backend>::make_link_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Form widgets
// ---------------------------------------------------------------------------

impl caps::TextInputOps for MacosBackend {
    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        secure: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_text_input(
            self,
            initial_value,
            placeholder,
            on_change,
            on_key_down,
            on_blur,
            secure,
            a11y,
        )
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        <MacosBackend as Backend>::update_text_input_value(self, node, value)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        <MacosBackend as Backend>::update_text_input_secure(self, node, secure)
    }

    fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>) {
        <MacosBackend as Backend>::set_text_input_focus_handler(self, node, handler)
    }

    fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {
        <MacosBackend as Backend>::update_text_input_placeholder(self, node, placeholder)
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        wrap: bool,
        min_rows: Option<u32>,
        max_rows: Option<u32>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_text_area(
            self,
            initial_value,
            placeholder,
            wrap,
            min_rows,
            max_rows,
            on_change,
            on_key_down,
            a11y,
        )
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        <MacosBackend as Backend>::update_text_area_value(self, node, value)
    }

    fn make_text_input_handle(&self, node: &Self::Node) -> primitives::text_input::TextInputHandle {
        <MacosBackend as Backend>::make_text_input_handle(self, node)
    }

    fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
        <MacosBackend as Backend>::make_text_area_handle(self, node)
    }
}

impl caps::ToggleOps for MacosBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_toggle(self, initial_value, on_change, a11y)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        <MacosBackend as Backend>::update_toggle_value(self, node, value)
    }

    fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
        <MacosBackend as Backend>::make_toggle_handle(self, node)
    }
}

impl caps::SliderOps for MacosBackend {
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_slider(self, initial_value, min, max, step, on_change, a11y)
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        <MacosBackend as Backend>::update_slider_value(self, node, value)
    }

    fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
        <MacosBackend as Backend>::make_slider_handle(self, node)
    }
}

impl caps::ActivityIndicatorOps for MacosBackend {
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_activity_indicator(self, size, color, a11y)
    }

    fn update_activity_indicator_size(
        &mut self,
        node: &Self::Node,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        <MacosBackend as Backend>::update_activity_indicator_size(self, node, size)
    }

    fn make_activity_indicator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::activity_indicator::ActivityIndicatorHandle {
        <MacosBackend as Backend>::make_activity_indicator_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Scroll + safe area + virtualizer
// ---------------------------------------------------------------------------

impl caps::ScrollOps for MacosBackend {
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_scroll_view(self, horizontal, on_scroll, a11y)
    }

    fn node_scroll(&self, node: &Self::Node) -> (f32, f32) {
        <MacosBackend as Backend>::node_scroll(self, node)
    }

    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
        <MacosBackend as Backend>::set_node_scroll(self, node, x, y)
    }

    fn make_scroll_view_handle(&self, node: &Self::Node) -> primitives::scroll_view::ScrollViewHandle {
        <MacosBackend as Backend>::make_scroll_view_handle(self, node)
    }
}

impl caps::SafeAreaOps for MacosBackend {
    fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        <MacosBackend as Backend>::apply_safe_area_padding(self, node, sides)
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        <MacosBackend as Backend>::apply_scroll_view_safe_area_inset(self, node, sides)
    }
}

impl caps::VirtualizerOps for MacosBackend {
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_virtualizer(self, callbacks, overscan, layout, a11y)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        <MacosBackend as Backend>::virtualizer_data_changed(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        <MacosBackend as Backend>::release_virtualizer(self, node)
    }

    fn make_virtualizer_handle(&self, node: &Self::Node) -> primitives::virtualizer::VirtualizerHandle {
        <MacosBackend as Backend>::make_virtualizer_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Graphics + portal + presence + navigator
// ---------------------------------------------------------------------------

impl caps::GraphicsOps for MacosBackend {
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_graphics(self, on_ready, on_resize, on_lost, a11y)
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        <MacosBackend as Backend>::release_graphics(self, node)
    }

    fn make_graphics_handle(&self, node: &Self::Node) -> primitives::graphics::GraphicsHandle {
        <MacosBackend as Backend>::make_graphics_handle(self, node)
    }
}

impl caps::PortalOps for MacosBackend {
    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_portal(self, target, on_dismiss, trap_focus, a11y)
    }

    fn release_portal(&mut self, node: &Self::Node) {
        <MacosBackend as Backend>::release_portal(self, node)
    }

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        <MacosBackend as Backend>::set_portal_hidden(self, node, hidden)
    }

    fn make_portal_handle(&self, node: &Self::Node) -> primitives::portal::PortalHandle {
        <MacosBackend as Backend>::make_portal_handle(self, node)
    }
}

impl caps::PresenceOps for MacosBackend {
    fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        <MacosBackend as Backend>::create_presence_placeholder(self, a11y)
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        <MacosBackend as Backend>::apply_presence(self, node, state, transition)
    }

    fn make_presence_handle(&self, node: &Self::Node) -> primitives::presence::PresenceHandle {
        <MacosBackend as Backend>::make_presence_handle(self, node)
    }
}

impl caps::NavigatorOps for MacosBackend {
    fn create_navigator(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        presentation: Rc<dyn Any>,
        host: primitives::navigator::NavigatorHost<Self::Node>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_navigator(self, type_id, type_name, presentation, host, a11y)
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        <MacosBackend as Backend>::release_navigator(self, node)
    }

    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<StyleRules>,
    ) {
        <MacosBackend as Backend>::apply_navigator_slot_style(self, node, slot, style)
    }

    fn make_navigator_handle(&self, node: &Self::Node) -> primitives::navigator::NavigatorHandle {
        <MacosBackend as Backend>::make_navigator_handle(self, node)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        <MacosBackend as Backend>::navigator_attach_initial(self, navigator, screen, scope_id, options)
    }
}

// ---------------------------------------------------------------------------
// External + document
// ---------------------------------------------------------------------------

impl caps::ExternalOps for MacosBackend {
    fn create_external(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        payload: &Rc<dyn Any>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <MacosBackend as Backend>::create_external(self, type_id, type_name, payload, a11y)
    }

    fn release_external(&mut self, node: &Self::Node) {
        <MacosBackend as Backend>::release_external(self, node)
    }

    fn missing_primitive_placeholder(&mut self, label: &'static str) -> Self::Node {
        <MacosBackend as Backend>::missing_primitive_placeholder(self, label)
    }
}

impl caps::DocumentOps for MacosBackend {
    fn create_element(&mut self, tag: &str) -> Self::Node {
        <MacosBackend as Backend>::create_element(self, tag)
    }

    fn attach_html_id(&self, node: &Self::Node, id: &str) {
        <MacosBackend as Backend>::attach_html_id(self, node, id)
    }

    fn attach_html_class(&self, node: &Self::Node, class: &str) {
        <MacosBackend as Backend>::attach_html_class(self, node, class)
    }

    fn attach_html_style(&self, node: &Self::Node, prop: &str, value: &str) {
        <MacosBackend as Backend>::attach_html_style(self, node, prop, value)
    }

    fn register_raw_css(&mut self, css: &str) {
        <MacosBackend as Backend>::register_raw_css(self, css)
    }
}

// ---------------------------------------------------------------------------
// Style + assets
// ---------------------------------------------------------------------------

impl caps::StyleOps for MacosBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        <MacosBackend as Backend>::apply_style(self, node, style)
    }

    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
        <MacosBackend as Backend>::mint_style_class(self, style)
    }

    fn mint_class_for_app(&mut self, app: &StyleApplication) -> Option<String> {
        <MacosBackend as Backend>::mint_class_for_app(self, app)
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        <MacosBackend as Backend>::apply_styled_states(self, node, base, overlays)
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        state_overlays: &[(StateBits, Rc<StyleRules>)],
        breakpoint_overlays: &[(Breakpoint, Rc<StyleRules>)],
        container_overlays: &[(f32, Rc<StyleRules>)],
    ) {
        <MacosBackend as Backend>::apply_styled_variants(
            self,
            node,
            base,
            state_overlays,
            breakpoint_overlays,
            container_overlays,
        )
    }

    fn mark_container(&mut self, node: &Self::Node) {
        <MacosBackend as Backend>::mark_container(self, node)
    }

    fn handles_states_natively(&self) -> bool {
        <MacosBackend as Backend>::handles_states_natively(self)
    }

    fn token_updates_propagate_via_cascade(&self) -> bool {
        <MacosBackend as Backend>::token_updates_propagate_via_cascade(self)
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        <MacosBackend as Backend>::register_stylesheet(self, rules)
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        <MacosBackend as Backend>::unregister_stylesheet(self, rules)
    }

    fn install_tokens(&mut self, tokens: &[TokenEntry]) {
        <MacosBackend as Backend>::install_tokens(self, tokens)
    }

    fn update_tokens(&mut self, tokens: &[TokenEntry]) {
        <MacosBackend as Backend>::update_tokens(self, tokens)
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        <MacosBackend as Backend>::on_node_unstyled(self, node)
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        <MacosBackend as Backend>::attach_states(self, node, setter)
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        <MacosBackend as Backend>::set_disabled(self, node, disabled)
    }

    fn supports_preminted_styles(&self) -> bool {
        <MacosBackend as Backend>::supports_preminted_styles(self)
    }

    fn apply_default_text_font(&mut self, font: Option<&FontFamily>) {
        <MacosBackend as Backend>::apply_default_text_font(self, font)
    }

    fn supports_js_class_bindings(&self) -> bool {
        <MacosBackend as Backend>::supports_js_class_bindings(self)
    }

    fn register_reactive_class_binding(
        &mut self,
        node: &Self::Node,
        signal_id: u64,
        values: &[u32],
        classes: &[&str],
        value_reader: Rc<dyn Fn() -> u32>,
    ) -> u32 {
        <MacosBackend as Backend>::register_reactive_class_binding(
            self,
            node,
            signal_id,
            values,
            classes,
            value_reader,
        )
    }

    fn release_reactive_class_binding(&mut self, binding_id: u32) {
        <MacosBackend as Backend>::release_reactive_class_binding(self, binding_id)
    }
}

impl caps::AssetOps for MacosBackend {
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        <MacosBackend as Backend>::register_asset(self, id, kind, source)
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        <MacosBackend as Backend>::unregister_asset(self, id, kind)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        <MacosBackend as Backend>::register_typeface(self, id, family_name, faces, fallback)
    }

    fn unregister_typeface(&mut self, id: TypefaceId) {
        <MacosBackend as Backend>::unregister_typeface(self, id)
    }
}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl caps::A11yOps for MacosBackend {
    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &AccessibilityProps,
        inferred_role: Option<Role>,
    ) {
        <MacosBackend as Backend>::update_accessibility(self, node, a11y, inferred_role)
    }

    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
        <MacosBackend as Backend>::announce_for_accessibility(self, msg, priority)
    }

    fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> {
        <MacosBackend as Backend>::dump_accessibility_tree(self)
    }
}

impl caps::AnimationOps for MacosBackend {
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        <MacosBackend as Backend>::set_animated_f32(self, node, prop, value)
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        <MacosBackend as Backend>::set_animated_color(self, node, prop, value)
    }
}

impl caps::IntrospectionOps for MacosBackend {
    fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <MacosBackend as Backend>::frame(self, node)
    }

    fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <MacosBackend as Backend>::absolute_frame(self, node)
    }

    fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <MacosBackend as Backend>::device_frame(self, node)
    }

    fn supports_native_introspection(&self) -> bool {
        <MacosBackend as Backend>::supports_native_introspection(self)
    }

    fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
        <MacosBackend as Backend>::introspect_native(self, node)
    }

    fn note_introspection_root(&self, node: &Self::Node) {
        <MacosBackend as Backend>::note_introspection_root(self, node)
    }

    fn supports_screenshot(&self) -> bool {
        <MacosBackend as Backend>::supports_screenshot(self)
    }

    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
        <MacosBackend as Backend>::capture_screenshot(self, done)
    }
}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for MacosBackend {
    fn supports_batched_repeat(&self) -> bool {
        <MacosBackend as Backend>::supports_batched_repeat(self)
    }

    fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
        <MacosBackend as Backend>::execute_batch(self, batch)
    }

    fn execute_batch_with_attach(
        &mut self,
        batch: BackendBatch,
        parent: &mut Self::Node,
        attach_locals: &[u32],
    ) -> Vec<Self::Node> {
        <MacosBackend as Backend>::execute_batch_with_attach(self, batch, parent, attach_locals)
    }
}

impl caps::WireBindingOps for MacosBackend {
    fn note_text_binding(&mut self, node: &Self::Node, signal_ids: &[u64], method: &'static str) {
        <MacosBackend as Backend>::note_text_binding(self, node, signal_ids, method)
    }

    fn note_signal_initial(&mut self, signal_id: u64, value: &runtime_core::__serde_json::Value) {
        <MacosBackend as Backend>::note_signal_initial(self, signal_id, value)
    }

    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ) {
        <MacosBackend as Backend>::note_when_binding(
            self,
            anchor,
            signal_ids,
            cond_method,
            then_node,
            otherwise_node,
        )
    }

    fn note_switch_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        arms: &[(runtime_core::__serde_json::Value, Self::Node)],
        default_node: &Self::Node,
    ) {
        <MacosBackend as Backend>::note_switch_binding(
            self,
            anchor,
            signal_ids,
            cond_method,
            arms,
            default_node,
        )
    }

    fn note_repeat_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
    ) {
        <MacosBackend as Backend>::note_repeat_binding(
            self,
            anchor,
            signal_ids,
            count_method,
            row_template,
            row_index_signal_id,
        )
    }

    fn note_virtualizer_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
        horizontal: bool,
    ) {
        <MacosBackend as Backend>::note_virtualizer_binding(
            self,
            anchor,
            signal_ids,
            count_method,
            row_template,
            row_index_signal_id,
            horizontal,
        )
    }

    fn supports_lazy_slot_capture(&self) -> bool {
        <MacosBackend as Backend>::supports_lazy_slot_capture(self)
    }

    fn begin_slot_capture(&mut self) {
        <MacosBackend as Backend>::begin_slot_capture(self)
    }

    fn end_slot_capture(&mut self, slot_root: &Self::Node) {
        <MacosBackend as Backend>::end_slot_capture(self, slot_root)
    }
}

// ===========================================================================
// Native tests — the flush driver's dedup + mount-buffering interplay.
// These run on the cargo-test thread (no main-thread AppKit required):
// they exercise the scheduler-facing glue only. The Host/caps delegation
// and the boot path are exercised by building + launching
// `newcore-macos-smoke` (AppKit objects can only be constructed on the
// main thread, so there is no unit-test seam for them — same limitation
// as every other imp/ module; see the smoke crate).
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_world::{effect, signal};

    /// `schedule_flush` queues exactly one deduped microtask; during a
    /// mount-buffering window it buffers (instead of dispatching to the
    /// main queue, which no test run loop would ever drain) and commits
    /// synchronously inside `drain_buffered_microtasks` — the exact
    /// interplay the boot path relies on (module docs, step 3).
    #[test]
    fn schedule_flush_dedups_and_commits_on_buffered_drain() {
        // Idempotent global install (first wins); the buffering window
        // below keeps every microtask on THIS thread.
        backend_apple_core::scheduler::install_scheduler();
        backend_apple_core::scheduler::end_mount_buffering(); // clean slate
        backend_apple_core::scheduler::begin_mount_buffering();

        let world = World::new();
        set_flush_world(Some(world.clone()));

        let log: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
        let count = world.enter(|| {
            let count = signal(0i32);
            let log = log.clone();
            effect(move || log.borrow_mut().push(count.get()));
            count
        });
        assert_eq!(*log.borrow(), vec![0], "effect ran once at creation");

        // Stage twice, schedule twice: the second schedule_flush must
        // dedup (flag already set), and nothing commits until the drain.
        count.set(1);
        count.set(2);
        schedule_flush();
        assert!(FLUSH_QUEUED.with(|q| q.get()), "first call arms the flag");
        schedule_flush();
        assert_eq!(*log.borrow(), vec![0], "staged, not committed");

        runtime_core::scheduling::drain_buffered_microtasks();
        assert_eq!(
            *log.borrow(),
            vec![0, 2],
            "ONE flush committed the latest staged value"
        );
        assert!(
            !FLUSH_QUEUED.with(|q| q.get()),
            "drain disarms the dedup flag"
        );

        // The flag re-arms for the next write→flush cycle.
        count.set(3);
        schedule_flush();
        runtime_core::scheduling::drain_buffered_microtasks();
        assert_eq!(*log.borrow(), vec![0, 2, 3]);

        set_flush_world(None);
        backend_apple_core::scheduler::end_mount_buffering();
    }

    /// `flush_now` with no mounted world is a no-op (the pump's timer
    /// tick fires before `start` finishes wiring on a cold boot), and a
    /// re-entrant flush is skipped via `world.is_flushing()`.
    #[test]
    fn flush_now_tolerates_no_world_and_reentry() {
        set_flush_world(None);
        flush_now(); // must not panic

        let world = World::new();
        set_flush_world(Some(world.clone()));
        let observed: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let (sig, obs) = world.enter(|| {
            let sig = signal(0i32);
            let obs = observed.clone();
            effect(move || {
                let v = sig.get();
                if v == 1 {
                    // Re-entrant flush attempt from inside a flush:
                    // world.is_flushing() short-circuits it.
                    flush_now();
                    obs.set(true);
                }
            });
            (sig, observed.clone())
        });
        sig.set(1);
        flush_now();
        assert!(obs.get(), "effect ran; re-entrant flush_now didn't recurse/panic");
        set_flush_world(None);
    }
}
