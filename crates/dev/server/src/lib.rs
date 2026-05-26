//! Dev-side runtime for the hot-reload wire protocol.
//!
//! Provides a [`WireRecordingBackend`] that implements
//! [`runtime_core::Backend`] with `Node = NodeId`. Each method
//! emits one [`Command`] (or a small cluster) into the recorder's
//! outbound queue, plus registers any closures the walker hands it
//! into a [`HandlerTable`].
//!
//! When the app fires an event, the dev side looks up the
//! `HandlerId` in the handler table and runs the captured closure.
//! That closure mutates signals; signal-driven effects re-fire
//! through the walker; the walker calls more `Backend` methods on
//! this same `WireRecordingBackend`, producing more `Command`s;
//! those flush back to the app.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use runtime_core::primitives;
use runtime_core::{
    Backend, Color, ColorScheme, StateBits, StyleRules, TextHandle, TextOps, ViewHandle, ViewOps,
};
use wire::{
    Command, EventArgs, HandlerId, NodeId, StyleId, WireColor, WireStateBit,
};

pub mod convert_out;
// runtime-server dev-host driver. Pulled in only when the consumer activates
// the `runtime-server` feature — `host::run` and `HotPatchAdapter`
// depend on `anyhow` + `subsecond_types`, which are optional deps.
// Recorder-only consumers (tests, `examples/welcome`) don't pay
// the cost.
#[cfg(feature = "runtime-server")]
pub mod host;
mod scene_model;
// Sidecar `runtime_core::scheduling::Scheduler` impl — installed
// once at sidecar startup so `raf_loop_scoped`, `after_ms`, etc.
// fire on the dev side. Otherwise the framework's animation clock
// gets an inert handle and any author code using raf-driven custom
// math (welcome's planets) does nothing.
pub mod scheduler;
#[cfg(feature = "runtime-server")]
pub mod crash_handler;
pub mod sidecar;
// Always compiled — the test-support module is small and its only
// non-trivial cost is the `tungstenite` symbols, which the crate
// already pulls in for the production server. Keeping it
// unconditionally available means integration tests, examples, and
// downstream consumers all see the same surface without juggling a
// feature flag through cargo invocations.
pub mod test_support;
pub mod transport;
pub mod watch;

use scene_model::SceneModel;

pub use sidecar::{Sidecar, SessionTracker, SidecarIn, SidecarOut, SidecarSlot};
pub use transport::{
    serve, serve_with_port_mirror, serve_with_sidecar, serve_with_sidecar_and_tracker,
    serve_with_tick, serve_with_tick_and_port, serve_with_tick_and_port_and_mode, SessionMode,
};
#[cfg(feature = "robot")]
pub use transport::serve_with_robot_bridge;
pub use watch::{spawn_change_loop, spawn_rebuild_loop, RebuildCommand, RebuildConfig};

/// The runtime-server (Application-as-a-Server) **server-side backend** —
/// implements `runtime_core::Backend` with `Node = NodeId`. Plug
/// this into `runtime_core::render(...)` exactly like you'd plug
/// in `WebBackend` / `IosBackend` / `AndroidBackend`. Instead of
/// driving native widgets it records every walker call as a wire
/// [`wire::Command`] for transport to one or more
/// [`RuntimeServerClient`](dev_client::RuntimeServerClient)s.
///
/// `AasBackend` is the heart of the runtime-server architecture:
///
/// ```text
/// UI tree → AasBackend → Wire (Commands) → RuntimeServerClient → Platform Backend → Native
/// ```
///
/// The same `Primitive` tree your iOS/web app would render natively
/// is what the server runs against this backend. The wire output is
/// platform-agnostic; an `RuntimeServerClient` wrapping any platform backend
/// can replay it.
pub use crate::WireRecordingBackend as AasBackend;

/// Stores the live dev-side closures the walker has handed us. Each
/// gets a `HandlerId` minted by the recorder; events arriving back
/// from the app look up the entry and invoke the captured closure.
///
/// **Identity-keyed dedup.** Closures registered with an
/// [`runtime_core::Identity`] reuse the same `HandlerId` across
/// hot-reload rebuilds: the table keeps `identity_to_id` populated
/// across [`Self::clear_closures`] (called from
/// `reset_log_and_scene`), so a re-register from the freshly-walked
/// tree drops back into the existing slot. This is what keeps
/// **client-side leaked callbacks valid** across server respawns —
/// without it, every hot-patch would invalidate every Toolbar
/// hamburger, Button click handler, and other primitive event ids
/// the client cached at install time.
#[derive(Default)]
pub struct HandlerTable {
    next: u64,
    closures: HashMap<HandlerId, Handler>,
    /// Identity → `HandlerId` memo. Persists across
    /// [`Self::clear_closures`] so the *fresh* render that follows a
    /// reset reuses the same id for the same logical emission site
    /// — and the leaked closure on the client (which captured the
    /// original id at first install) keeps routing to the right
    /// handler. Closure replacement happens in
    /// [`Self::register_unit_for_identity`] / friends, where the
    /// existing slot is overwritten with the freshly-walked closure
    /// (capturing the post-reset `Rc<NavigatorControl>` etc.).
    identity_to_id: HashMap<runtime_core::Identity, HandlerId>,
}

enum Handler {
    Unit(Rc<dyn Fn()>),
    Bool(Rc<dyn Fn(bool)>),
    Float(Rc<dyn Fn(f32)>),
    StringFn(Rc<dyn Fn(String)>),
    States(Rc<dyn Fn(StateBits, bool)>),
}

impl HandlerTable {
    fn mint(&mut self) -> HandlerId {
        self.next += 1;
        HandlerId(self.next)
    }

    pub fn register_unit(&mut self, f: Rc<dyn Fn()>) -> HandlerId {
        let id = self.mint();
        self.closures.insert(id, Handler::Unit(f));
        id
    }

    pub fn register_bool(&mut self, f: Rc<dyn Fn(bool)>) -> HandlerId {
        let id = self.mint();
        self.closures.insert(id, Handler::Bool(f));
        id
    }

    pub fn register_float(&mut self, f: Rc<dyn Fn(f32)>) -> HandlerId {
        let id = self.mint();
        self.closures.insert(id, Handler::Float(f));
        id
    }

    pub fn register_string(&mut self, f: Rc<dyn Fn(String)>) -> HandlerId {
        let id = self.mint();
        self.closures.insert(id, Handler::StringFn(f));
        id
    }

    pub fn register_states(&mut self, f: Rc<dyn Fn(StateBits, bool)>) -> HandlerId {
        let id = self.mint();
        self.closures.insert(id, Handler::States(f));
        id
    }

    /// Identity-keyed register. Same logical emission site (same
    /// [`runtime_core::Identity`]) across rebuilds reuses the same
    /// `HandlerId` — the closure under the id is replaced with the
    /// freshly-walked one. Cross-rebuild stability is what keeps the
    /// client's leaked `HeaderButtonCallback`, button click pointer,
    /// etc. valid after a hot-patch.
    ///
    /// `UNIDENTIFIED` identities fall back to the unkeyed
    /// [`Self::register_unit`] path (fresh id every call). Callers
    /// without ambient identity get this gracefully.
    pub fn register_unit_for_identity(
        &mut self,
        identity: runtime_core::Identity,
        f: Rc<dyn Fn()>,
    ) -> HandlerId {
        if identity == runtime_core::Identity::UNIDENTIFIED {
            return self.register_unit(f);
        }
        let id = *self.identity_to_id.entry(identity).or_insert_with(|| {
            self.next += 1;
            HandlerId(self.next)
        });
        self.closures.insert(id, Handler::Unit(f));
        id
    }

    /// See [`Self::register_unit_for_identity`]. Bool variant for
    /// `Toggle.on_change`.
    pub fn register_bool_for_identity(
        &mut self,
        identity: runtime_core::Identity,
        f: Rc<dyn Fn(bool)>,
    ) -> HandlerId {
        if identity == runtime_core::Identity::UNIDENTIFIED {
            return self.register_bool(f);
        }
        let id = *self.identity_to_id.entry(identity).or_insert_with(|| {
            self.next += 1;
            HandlerId(self.next)
        });
        self.closures.insert(id, Handler::Bool(f));
        id
    }

    /// See [`Self::register_unit_for_identity`]. Float variant for
    /// `Slider.on_change`.
    pub fn register_float_for_identity(
        &mut self,
        identity: runtime_core::Identity,
        f: Rc<dyn Fn(f32)>,
    ) -> HandlerId {
        if identity == runtime_core::Identity::UNIDENTIFIED {
            return self.register_float(f);
        }
        let id = *self.identity_to_id.entry(identity).or_insert_with(|| {
            self.next += 1;
            HandlerId(self.next)
        });
        self.closures.insert(id, Handler::Float(f));
        id
    }

    /// See [`Self::register_unit_for_identity`]. String variant for
    /// `TextInput.on_change`.
    pub fn register_string_for_identity(
        &mut self,
        identity: runtime_core::Identity,
        f: Rc<dyn Fn(String)>,
    ) -> HandlerId {
        if identity == runtime_core::Identity::UNIDENTIFIED {
            return self.register_string(f);
        }
        let id = *self.identity_to_id.entry(identity).or_insert_with(|| {
            self.next += 1;
            HandlerId(self.next)
        });
        self.closures.insert(id, Handler::StringFn(f));
        id
    }

    /// Drop all live closures but keep `next` and `identity_to_id`.
    /// Used by [`WireRecordingBackend::reset_log_and_scene`] so a
    /// hot-patch rebuild re-registers the same emission sites under
    /// their original ids — the closures themselves are replaced
    /// because they capture the *new* render's
    /// `Rc<NavigatorControl>` / signals. Anything that touched the
    /// old captures (and would have panicked on the next signal
    /// read) is freed here before the next render starts.
    pub fn clear_closures(&mut self) {
        self.closures.clear();
        // `next` and `identity_to_id` deliberately preserved.
    }

    pub fn release(&mut self, id: HandlerId) {
        self.closures.remove(&id);
    }
}

/// The dev-side backend. Implements `Backend` with `Node = NodeId`;
/// each method emits one or more `Command`s into the recorder.
///
/// Cloning shares the underlying state (Rc<RefCell<…>>) so callers
/// can hand out clones of the recorder to multiple owners — the
/// usual pattern when the walker holds one ref and the dispatch
/// loop holds another.
pub struct WireRecordingBackend {
    inner: Rc<RefCell<RecorderState>>,
    /// Send+Sync mirror of per-navigator URL stacks. Updated
    /// synchronously by the recorder's dispatcher (main thread)
    /// every time a navigator's stack changes. Read by the file
    /// watch + rebuild thread just before `exec`, to serialize and
    /// pass forward as an env var. Survives the process image
    /// swap, letting the freshly-started server restore the
    /// navigator hierarchy.
    nav_state_mirror: Arc<Mutex<NavStateSnapshot>>,
}

/// Map: navigator `NodeId.0` → stack of route URLs (initial route
/// first, top-of-stack last). Snapshotted across `exec` to recover
/// the navigation hierarchy after a hot-reload restart.
pub type NavStateSnapshot = HashMap<u64, Vec<String>>;

struct RecorderState {
    next_node: u64,
    next_style: u64,
    /// Identity → NodeId memo. Keyed by [`runtime_core::Identity`]
    /// (the structural identity the walker sets via
    /// `with_current_identity` before every `backend.create_*` call).
    /// Survives [`WireRecordingBackend::reset_log_and_scene`] so that
    /// across sidecar respawns the same structural emission lands on
    /// the same wire `NodeId` — that's what makes incremental hot
    /// reload incremental (`CreateView` for the same node is a no-op
    /// on the client; new `ApplyStyle` for the same node lands on
    /// the right native view).
    ///
    /// Emissions that arrive under [`runtime_core::Identity::UNIDENTIFIED`]
    /// bypass dedup (mint a fresh id every time). Used as a
    /// pressure-release for any emission site that hasn't been
    /// migrated to set an identity yet.
    identity_to_node: HashMap<runtime_core::Identity, NodeId>,
    /// Monotonic generation counter. Bumped by
    /// [`WireRecordingBackend::reset_log_and_scene`] each time the
    /// scene is wiped + re-rendered (typically after a hot-reload
    /// patch apply). The serve loop snapshots this onto each
    /// connected client and compares per-tick — when the recorder's
    /// epoch moves ahead of a client's, that client gets a fresh
    /// scene snapshot instead of the usual delta broadcast.
    epoch: u64,
    handlers: HandlerTable,
    /// Pre-registered styles. Each `Rc<StyleRules>` pointer identity
    /// gets mapped to a `StyleId` on first encounter so the wire
    /// never re-serializes the same rules. **Per-process** — Rc
    /// pointers don't survive sidecar respawns, so this map is
    /// rebuilt on every walk.
    ///
    /// Holds a `Weak` alongside the id so a stale ptr key (an Rc
    /// dropped and its allocation recycled by the allocator) is
    /// detected and treated as a miss. Without the `Weak`, an
    /// ephemeral `Rc::new(rules)` created+dropped inside an
    /// `Effect` body would leave its address in this map, and the
    /// next `Rc::new(other_rules)` that lands at the same recycled
    /// address would falsely hit and reuse the prior `StyleId` —
    /// silently aliasing distinct styles together over the wire.
    /// (Concrete bug profile: navigator chrome
    /// `apply_navigator_*_style` emissions all collided onto the
    /// first slot's `StyleId`.)
    styles_by_ptr: HashMap<usize, (std::rc::Weak<StyleRules>, StyleId)>,
    /// Content-addressed style-id memo. Keyed by the JSON-serialized
    /// hash of the wire-form `WireStyleRules`. Survives
    /// [`WireRecordingBackend::reset_log_and_scene`] so that across
    /// sidecar respawns the same stylesheet (same resolved values)
    /// lands on the same wire `StyleId` — that's what keeps theme
    /// toggles incremental rather than scrambling everything by
    /// overwriting `styles[0]` with a new sidecar's `RegisterStyle
    /// { id: 0, … }`. Unchanged styles get skipped entirely; changed
    /// styles re-emit `RegisterStyle` with the previously-assigned
    /// id so the client's `styles.insert(id, …)` lands at the right
    /// slot.
    styles_by_content: HashMap<u64, StyleId>,
    out: Vec<Command>,
    color_scheme: ColorScheme,
    /// Maps from a `NodeId` to the most recent state-attach handler.
    /// Lets `dispatch_state` look up the closure to invoke when the
    /// app reports a state-bit transition.
    state_handlers: HashMap<NodeId, HandlerId>,
    /// Per-navigator dev-side state. Used by the dispatcher to call
    /// the framework's screen-mount callbacks and to track the local
    /// scope stack for app-initiated release events.
    navigators: HashMap<NodeId, NavigatorRecState>,
    /// Reverse lookup: scope_id → navigator that owns it. Lets the
    /// recorder route an `AppToDev::ScreenReleased { scope }` to the
    /// right navigator's `release_screen` callback.
    scope_to_navigator: HashMap<u64, NodeId>,
    /// Shared handle to the Send+Sync nav-state mirror, so the
    /// dispatcher can push updates from inside `borrow_mut` without
    /// going through the outer `WireRecordingBackend`.
    nav_state_mirror: Arc<Mutex<NavStateSnapshot>>,
    /// Mirror of the live scene — the source of truth for catch-up
    /// replay. Each recorder method that emits a `Command` also
    /// updates this model so a freshly connecting client can be
    /// brought up to the current state without replaying historical
    /// transients (pushed-then-popped screens, typed-then-deleted
    /// text, scroll positions that moved many times, etc.).
    /// `out` is still used for incremental broadcast to clients
    /// already past the snapshot point.
    scene: SceneModel,
}

impl RecorderState {
    /// Single emit point: the scene model interprets the command to
    /// stay in sync, then the command lands in the broadcast log.
    /// Every Backend method should funnel through here instead of
    /// pushing to `out` directly — that's how the snapshot stays
    /// current and clients connecting mid-session see the right
    /// thing.
    fn emit(&mut self, cmd: Command) {
        self.scene.apply(&cmd);
        self.out.push(cmd);
    }

    /// Build a `WireAccessibilityProps` from an in-memory
    /// `AccessibilityProps`, registering each action's handler into
    /// `self.handlers` so the reverse channel can dispatch it. Equivalent
    /// to calling `convert_out::a11y_to_wire(p, &mut self.handlers)` but
    /// borrows `self` once — call this and bind the result before any
    /// `self.emit(...)` to avoid double mutable-borrow conflicts on
    /// `state`.
    fn wire_a11y(
        &mut self,
        p: &runtime_core::accessibility::AccessibilityProps,
    ) -> wire::WireAccessibilityProps {
        convert_out::a11y_to_wire(p, &mut self.handlers)
    }
}

/// Per-navigator dev-side state used by the recording backend's
/// dispatcher. The framework callback substrate has been refactored
/// out of runtime-core; this struct is now a stub kept for the
/// stack-mirror plumbing until the SDK-handler-level recording lands.
pub struct NavigatorRecState {
    /// Scope ids of the screens currently on the navigator's stack,
    /// top of stack = end of vec. Updated by the dispatcher on push
    /// and by app-event handlers on swipe-back.
    pub stack: Vec<u64>,
    /// URL paths of the screens on the navigator's stack, in lock
    /// step with `stack`. Persisted across `exec` so the navigation
    /// hierarchy can be restored on the freshly-started server.
    pub stack_urls: Vec<String>,
}

impl WireRecordingBackend {
    pub fn new() -> Self {
        let nav_state_mirror = Arc::new(Mutex::new(NavStateSnapshot::new()));
        let inner = Rc::new(RefCell::new(RecorderState {
            next_node: 0,
            next_style: 0,
            identity_to_node: HashMap::new(),
            styles_by_content: HashMap::new(),
            epoch: 0,
            handlers: HandlerTable::default(),
            styles_by_ptr: HashMap::new(),
            out: Vec::new(),
            color_scheme: ColorScheme::Auto,
            state_handlers: HashMap::new(),
            navigators: HashMap::new(),
            scope_to_navigator: HashMap::new(),
            nav_state_mirror: nav_state_mirror.clone(),
            scene: SceneModel::new(),
        }));
        // Install a per-thread weak handle so `RecordingViewOps`
        // (called from `AnimatedValue::bind` -> `ViewHandle::set_animated_*`)
        // can reach this recorder to emit `SetAnimated*` wire commands.
        // The runtime-server sidecar runs one session per thread, so per-thread
        // is the right scope — host process for the dev server.
        RECORDER_HANDLE.with(|slot| {
            *slot.borrow_mut() = Some(Rc::downgrade(&inner));
        });
        Self {
            inner,
            nav_state_mirror,
        }
    }

    /// Public handle to the per-navigator URL stack mirror. Send + Sync
    /// — safe to share with the file-watch / rebuild thread so it can
    /// serialize the current navigation hierarchy before `exec`.
    pub fn nav_state_mirror(&self) -> Arc<Mutex<NavStateSnapshot>> {
        self.nav_state_mirror.clone()
    }

    /// Restore a previously-snapshotted navigator stack. Stubbed —
    /// the legacy callback-driven navigator recording was removed
    /// from runtime-core, and the replacement SDK-handler-level
    /// recording is not yet wired up. Until then, restore is a no-op
    /// and nav hierarchies do not survive hot-reload restarts.
    pub fn restore_nav_state(&self, _saved: &NavStateSnapshot) {
        // Pending SDK-handler-level navigator recording.
    }

    pub fn set_color_scheme(&self, scheme: ColorScheme) {
        self.inner.borrow_mut().color_scheme = scheme;
    }

    /// Append a `Command` that was produced by some *external* source
    /// (e.g. a sidecar process running the user's app code) rather
    /// than by a local `Backend` method call. Updates the scene model
    /// and the broadcast log identically to a normally-emitted
    /// command, so catch-up and incremental-broadcast paths work
    /// unchanged.
    ///
    /// The infra-only runtime-server host uses this to mirror the sidecar's
    /// command stream into its own recorder without running the user
    /// code itself. Local renders should always go through the
    /// `Backend` trait — this method bypasses the framework runtime
    /// and is purely a transport hook.
    pub fn push_external_command(&self, cmd: Command) {
        self.inner.borrow_mut().emit(cmd);
    }

    /// Advance the per-thread animation clock by `dt` and fire any
    /// registered tick closures + scheduler-stored raf-loop closures
    /// + expired `after_ms` deadlines. This is the explicit
    /// equivalent of a platform `raf_loop` callback firing — used by
    /// the runtime-server sidecar when a client sends
    /// `AppToDev::RequestFrame { dt_ms }` to drive the next frame.
    ///
    /// Order: scheduler-stored closures first (raf_loop callbacks
    /// that write to AVs via `av.set(...)`), then the animation clock
    /// (drives declarative animators — tweens/springs registered via
    /// `av.animate(...)`). This way author code that imperatively
    /// updates AV inside `raf_loop_scoped` sees its value land
    /// *before* the declarative tween for that frame's tick — same
    /// ordering the browser uses (raf callbacks before
    /// CSS-transition advancing).
    ///
    /// Returns the number of declarative tick closures that survived
    /// this tick (zero = no active animators on this thread; idle
    /// sessions don't need further RequestFrames if no
    /// raf_loop_scoped callbacks are registered either).
    ///
    /// Must run on the same thread that owns this recorder — the
    /// animation clock and scheduler closures are both `thread_local!`,
    /// so calling from a different thread would drive an empty/wrong
    /// registry.
    pub fn tick_animations(&self, dt: std::time::Duration) -> usize {
        scheduler::drive_pending();
        runtime_core::animation::clock::tick_for_test(dt)
    }

    /// Drop every command from the log and reset the scene to empty.
    /// Used by the infra host when it swaps to a freshly-spawned
    /// sidecar (or hot-reloads a dylib patch) — the next render
    /// will emit a fresh command stream and re-using the old log
    /// would produce stale catch-up snapshots for clients that
    /// connect after the swap.
    ///
    /// Increments the scene epoch so the serve loop's broadcast
    /// can detect the reset and force every already-connected
    /// client to re-snapshot from scratch (cursor-based delta
    /// broadcast doesn't work across a log truncation — the
    /// client's stored cursor is ahead of the new log's length).
    pub fn reset_log_and_scene(&self) {
        let mut state = self.inner.borrow_mut();
        state.out.clear();
        state.scene = SceneModel::new();
        // `next_node` is *not* reset for the same reason `next_style` and
        // `handlers.next` aren't: `identity_to_node` survives the reset,
        // and minting fresh ids from 0 would recycle ids that cached
        // identities are still using — every emission with a new identity
        // (a row added to a previously-rendered list, for example) would
        // collide with whatever existing identity holds `NodeId(1)`,
        // `NodeId(2)`, … The high-water mark is the only collision-safe
        // start point. (Regression test: `reset_log_and_scene_does_not_
        // collide_minted_ids_with_cached_identities` in `aas_headless`.)
        // `next_style` is *not* reset: the new walk's content-addressed
        // dedup (see [`Self::intern_style`]) reuses the previously-
        // assigned `StyleId` for any unchanged stylesheet. New
        // styles that didn't exist before mint a fresh id past the
        // high-water mark — keeps everything unique without ever
        // overwriting a still-referenced id on the client.
        // Deliberately NOT cleared: `identity_to_node` persists across
        // sidecar respawns. The fresh walk re-emits the same
        // structural identities (`Identity::node(parent, slot, ...)`
        // is a pure function of position), and we want them to land
        // on the same wire `NodeId`s as before — that's how the
        // client's idempotent apply correctly skips `CreateView` for
        // unchanged nodes while still receiving new
        // `ApplyStyle`/`UpdateText` deltas for the *same* native view.
        // Removed-from-the-new-walk identities stay in the map; they
        // just won't be referenced. A future pass can prune them by
        // diffing visit sets between walks.
        // `styles_by_ptr` is per-process (Rc identities won't match
        // across sidecar respawns), so it's safe to clear. The
        // cross-process `styles_by_content` map below is *not*
        // cleared — that's what reuses ids for unchanged styles.
        state.styles_by_ptr.clear();
        // `styles_by_content` deliberately survives: the next walk
        // will rebuild `styles_by_ptr` lazily on each style
        // encounter, but the content hash → `StyleId` mapping must
        // persist so unchanged stylesheets reuse the same wire id
        // and don't trigger a `RegisterStyle` overwrite of a still-
        // referenced slot.
        state.state_handlers.clear();
        state.navigators.clear();
        state.scope_to_navigator.clear();
        // Drop old closures (frees their captured Rcs — old
        // NavigatorControl, signal handles, etc. — before the
        // next render starts) but keep `next` + `identity_to_id`
        // so identity-keyed re-registers land on the same
        // `HandlerId`s the previous walk minted. That's what makes
        // leaked client-side callbacks (Android Toolbar hamburger,
        // primitive event ptrs) survive a hot-patch.
        state.handlers.clear_closures();
        state.epoch = state.epoch.wrapping_add(1);
    }

    /// Monotonically-increasing scene generation. Bumped every time
    /// [`Self::reset_log_and_scene`] runs. The serve loop uses this
    /// to decide whether a connected client needs a fresh snapshot
    /// (its remembered epoch falls behind the recorder's).
    pub fn epoch(&self) -> u64 {
        self.inner.borrow().epoch
    }

    /// Drain any pending commands, removing them from the recorder's
    /// log. Used by the legacy single-snapshot path; new code should
    /// prefer the append-only [`Self::commands_since`] /
    /// [`Self::command_count`] API which lets multiple clients each
    /// hold their own cursor into the same shared log.
    pub fn drain_commands(&self) -> Vec<Command> {
        std::mem::take(&mut self.inner.borrow_mut().out)
    }

    /// Total number of commands ever emitted by the walker into this
    /// recorder's log. Pair with [`Self::commands_since`] to ferry
    /// "everything past my cursor" to a connected client.
    pub fn command_count(&self) -> usize {
        self.inner.borrow().out.len()
    }

    /// Return a clone of every command emitted at or after `cursor`.
    /// Used for incremental broadcast to clients already past the
    /// initial snapshot. The recorder's log is append-only; cursors
    /// only ever move forward.
    ///
    /// Note: this does NOT filter out commands superseded by later
    /// state (a Push followed by a Pop both stay in the log). Fresh
    /// clients should instead start from [`Self::snapshot`], which
    /// serializes the *current* scene state. Live clients are
    /// presumed to have already received the historical commands,
    /// so incremental replay from their cursor is correct.
    pub fn commands_since(&self, cursor: usize) -> Vec<Command> {
        let state = self.inner.borrow();
        if cursor >= state.out.len() {
            Vec::new()
        } else {
            state.out[cursor..].to_vec()
        }
    }

    /// Build a fresh wire-command stream that, applied to an empty
    /// client, reproduces the *current* scene. Catchup uses this
    /// instead of `commands_since(0)` so transient history (mounts
    /// later popped, signal mutations later overwritten) doesn't
    /// re-play on every reconnect.
    ///
    /// The client should set its replay cursor to whatever
    /// [`Self::command_count`] returned at the moment this snapshot
    /// was taken — subsequent `commands_since(cursor)` calls then
    /// pick up live mutations that happened after the snapshot.
    pub fn snapshot(&self) -> Vec<Command> {
        self.inner.borrow().scene.snapshot_commands()
    }

    /// Dispatch an event received from the app. Returns true if the
    /// handler was found and invoked. The closure may mutate signals
    /// (triggering further walker work + commands); callers should
    /// `drain_commands()` after this returns to flush.
    pub fn dispatch_event(&self, id: HandlerId, args: EventArgs) -> bool {
        // Snapshot the closure ref out of the borrow first — the
        // closure body may re-enter this backend through the walker.
        let handler_ref = {
            let state = self.inner.borrow();
            state.handlers.closures.get(&id).map(|h| match h {
                Handler::Unit(f) => HandlerSnapshot::Unit(f.clone()),
                Handler::Bool(f) => HandlerSnapshot::Bool(f.clone()),
                Handler::Float(f) => HandlerSnapshot::Float(f.clone()),
                Handler::StringFn(f) => HandlerSnapshot::StringFn(f.clone()),
                Handler::States(f) => HandlerSnapshot::States(f.clone()),
            })
        };
        let Some(handler) = handler_ref else {
            return false;
        };
        match (handler, args) {
            (HandlerSnapshot::Unit(f), EventArgs::Unit) => {
                f();
                true
            }
            (HandlerSnapshot::Bool(f), EventArgs::Bool(v)) => {
                f(v);
                true
            }
            (HandlerSnapshot::Float(f), EventArgs::Float(v)) => {
                f(v);
                true
            }
            (HandlerSnapshot::StringFn(f), EventArgs::String(v)) => {
                f(v);
                true
            }
            _ => false,
        }
    }

    /// Dispatch a state-bit transition reported from the app.
    pub fn dispatch_state(&self, node: NodeId, bit: WireStateBit, on: bool) -> bool {
        let cb = {
            let state = self.inner.borrow();
            let Some(&handler_id) = state.state_handlers.get(&node) else {
                return false;
            };
            state.handlers.closures.get(&handler_id).and_then(|h| {
                if let Handler::States(f) = h {
                    Some(f.clone())
                } else {
                    None
                }
            })
        };
        if let Some(cb) = cb {
            cb(convert_out::wire_state_bit_to_bits(bit), on);
            true
        } else {
            false
        }
    }

    /// Allocate a `NodeId` for the current emission site. Uses the
    /// ambient [`runtime_core::current_identity`] to dedup across
    /// sidecar respawns: the same structural emission always gets the
    /// same `NodeId`, which is what makes hot reload incremental
    /// rather than a full-scene reset. Emissions under
    /// [`runtime_core::Identity::UNIDENTIFIED`] (legacy path that
    /// hasn't been migrated yet) always mint fresh — no dedup.
    fn mint_node(state: &mut RecorderState) -> NodeId {
        let id = runtime_core::current_identity();
        if id != runtime_core::Identity::UNIDENTIFIED {
            if let Some(&existing) = state.identity_to_node.get(&id) {
                return existing;
            }
        }
        state.next_node += 1;
        let assigned = NodeId(state.next_node);
        if id != runtime_core::Identity::UNIDENTIFIED {
            state.identity_to_node.insert(id, assigned);
        }
        assigned
    }

    fn intern_style(state: &mut RecorderState, rules: &Rc<StyleRules>) -> StyleId {
        // Fast path: same `Rc` pointer we've seen this process →
        // already registered. Skips the wire conversion + hash.
        //
        // The `Weak::upgrade` check is load-bearing: `Rc::as_ptr`
        // returns a `*const StyleRules` that's only meaningful while
        // the allocation is alive. An `Rc` dropped here can have its
        // address recycled by the allocator on the next
        // `Rc::new(...)`, and without a liveness check the map
        // would return the prior `StyleId` for what is actually a
        // brand-new, different-content `Rc` at the same address.
        let ptr = Rc::as_ptr(rules) as usize;
        let stale = match state.styles_by_ptr.get(&ptr) {
            Some((weak, sid)) => {
                if weak.upgrade().is_some() {
                    return *sid;
                }
                // Cached Weak is dead → the prior allocation was
                // dropped. The current `rules` happens to land on
                // the same recycled address. Fall through to
                // content hashing; the stale entry gets overwritten
                // by the insert at the bottom of the miss path.
                true
            }
            None => false,
        };
        if stale {
            // Defensive remove so a content-hash miss below doesn't
            // skip the insert (the insert overwrites anyway, but
            // dropping the dead Weak immediately frees its slot).
            state.styles_by_ptr.remove(&ptr);
        }
        // Convert to wire form and hash the serialized bytes for a
        // content-addressed lookup. This is the cross-process map
        // that makes hot reload incremental: after a sidecar
        // respawn, the new walker re-encounters the same logical
        // stylesheets — different `Rc` pointers but identical
        // wire content — so they should land on the same `StyleId`s
        // the client already has registered.
        let wire = convert_out::style_rules_to_wire(rules);
        let content_hash = {
            use std::hash::{Hash, Hasher};
            let bytes = serde_json::to_vec(&wire).unwrap_or_default();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut h);
            h.finish()
        };
        if let Some(&id) = state.styles_by_content.get(&content_hash) {
            // Known content → reuse the existing wire id. Cache by
            // `Rc` pointer too so we don't re-hash on the next
            // encounter within this process.
            state.styles_by_ptr.insert(ptr, (Rc::downgrade(rules), id));
            return id;
        }
        // New content → mint a fresh id, register on the wire, cache
        // in both maps.
        state.next_style += 1;
        let id = StyleId(state.next_style);
        state.styles_by_ptr.insert(ptr, (Rc::downgrade(rules), id));
        state.styles_by_content.insert(content_hash, id);
        state.emit(Command::RegisterStyle { id, rules: wire });
        id
    }
}

impl Default for WireRecordingBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for WireRecordingBackend {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            nav_state_mirror: self.nav_state_mirror.clone(),
        }
    }
}

enum HandlerSnapshot {
    Unit(Rc<dyn Fn()>),
    Bool(Rc<dyn Fn(bool)>),
    Float(Rc<dyn Fn(f32)>),
    StringFn(Rc<dyn Fn(String)>),
    #[allow(dead_code)]
    States(Rc<dyn Fn(StateBits, bool)>),
}

// ---------------------------------------------------------------------------
// Backend impl
// ---------------------------------------------------------------------------

/// Marker `ViewOps` for the recording backend. The recording side
/// doesn't expose layout rects (no real DOM/UIKit) so every method on
/// `ViewOps` falls back to the trait default; what matters is that
/// the static-dyn ops pointer exists so `ViewHandle::new` accepts it.
struct RecordingViewOps;

impl ViewOps for RecordingViewOps {
    /// Report the per-session viewport for every view on the
    /// recording backend. Welcome's `coordinator::use_welcome`
    /// reads `page_ref.with(|h| h.frame())` once per raf tick to
    /// recentre the planet orbit ellipse; without this override the
    /// trait default returns `None`, the welcome code falls back to
    /// a hardcoded `(393.0, 800.0)`, and the orbits anchor at
    /// (~196, ~400) regardless of the client's actual viewport.
    ///
    /// Returning the viewport for every view (not just the root) is
    /// intentional — the recording backend has no real layout, so
    /// "per-view rect" is undefined here. The viewport is the only
    /// honest answer the sidecar can give. Author code that
    /// genuinely needs sub-view frames (overlay anchoring, etc.) has
    /// to query the rendering backend, not the recorder.
    fn frame(
        &self,
        _node: &dyn std::any::Any,
    ) -> Option<runtime_core::primitives::portal::ViewportRect> {
        let (w, h) = SESSION_VIEWPORT.with(|c| c.get())?;
        Some(runtime_core::primitives::portal::ViewportRect {
            x: 0.0,
            y: 0.0,
            width: w,
            height: h,
        })
    }

    /// Route animated scalar writes to the per-thread recorder's
    /// `set_animated_f32`. Mirrors the pattern in `backend-web`'s
    /// `WebViewOps`, but routes through a thread-local Weak handle
    /// instead of a process-global one — the runtime-server sidecar runs one
    /// reactive runtime per session thread, so the handle must be
    /// per-thread.
    ///
    /// Silently no-ops if the recorder hasn't been installed
    /// (e.g., tests using `RecordingViewOps` without going through
    /// `WireRecordingBackend::new`).
    fn set_animated_f32(
        &self,
        node: &dyn std::any::Any,
        prop: runtime_core::animation::AnimProp,
        value: f32,
    ) {
        let Some(node_id) = node.downcast_ref::<NodeId>() else { return };
        let rc = match RECORDER_HANDLE
            .with(|slot| slot.borrow().as_ref().cloned())
            .and_then(|w| w.upgrade())
        {
            Some(rc) => rc,
            None => return,
        };
        let mut state = match rc.try_borrow_mut() {
            Ok(s) => s,
            Err(_) => return,
        };
        state.emit(Command::SetAnimatedF32 {
            node: *node_id,
            prop: convert_out::anim_prop_to_wire(prop),
            value,
        });
    }

    fn set_animated_color(
        &self,
        node: &dyn std::any::Any,
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        let Some(node_id) = node.downcast_ref::<NodeId>() else { return };
        let rc = match RECORDER_HANDLE
            .with(|slot| slot.borrow().as_ref().cloned())
            .and_then(|w| w.upgrade())
        {
            Some(rc) => rc,
            None => return,
        };
        let mut state = match rc.try_borrow_mut() {
            Ok(s) => s,
            Err(_) => return,
        };
        state.emit(Command::SetAnimatedColor {
            node: *node_id,
            prop: convert_out::anim_prop_to_wire(prop),
            value,
        });
    }
}

thread_local! {
    /// Per-thread handle to the live `WireRecordingBackend` for the
    /// session running on this thread. Set by
    /// `WireRecordingBackend::new` and used by
    /// [`RecordingViewOps::set_animated_f32`] / `set_animated_color`
    /// and [`RecordingTextOps::set_animated_color`] to route per-frame
    /// animation writes — these calls reach the ops impls through
    /// `ViewHandle`/`TextHandle::set_animated_*`, which receive only
    /// `&self` (so we can't pass the recorder directly) and the
    /// `Node` downcast target. A thread-local Weak is the standard
    /// cross-platform pattern; see `backend_web::WEB_BACKEND_HANDLE`
    /// for the same shape on a process-global thread (there's only
    /// one renderer there).
    static RECORDER_HANDLE: RefCell<Option<std::rc::Weak<RefCell<RecorderState>>>> =
        const { RefCell::new(None) };

    /// Per-session-thread viewport (CSS pixels). Set by the session
    /// thread when it receives an `AppToDev::Hello { viewport }` or
    /// `AppToDev::ViewportChanged { width, height }`. Read by
    /// [`RecordingViewOps::frame`] so author code reading
    /// `page_ref.with(|h| h.frame())` gets the *client's* viewport
    /// (not a sidecar-side hardcoded fallback).
    ///
    /// `None` before the client's first Hello — `frame()` returns
    /// `None` in that window and author code falls back to its own
    /// default (welcome uses 393×800). Once Hello lands the value
    /// flips on for the rest of the session.
    static SESSION_VIEWPORT: std::cell::Cell<Option<(f32, f32)>> =
        const { std::cell::Cell::new(None) };
}

/// Update the calling session-thread's viewport. Called from
/// `dispatch_app_to_dev` on Hello / ViewportChanged. Must run on
/// the session thread (the thread-local is per-thread).
pub fn set_session_viewport(width: f32, height: f32) {
    SESSION_VIEWPORT.with(|c| c.set(Some((width, height))));
}

/// `TextOps` mirror of [`RecordingViewOps`]. The framework's
/// `AnimatedValue::bind_text_color` routes through
/// `TextHandle::set_animated_color` → here, *not* through
/// `ViewHandle`. Without this override the welcome example's headline
/// text never gets its color animated in (stays at whatever the static
/// style declared) — same dead-letter behavior the trait-default
/// `Backend::set_animated_color` has for the view path.
struct RecordingTextOps;

impl TextOps for RecordingTextOps {
    fn set_animated_color(
        &self,
        node: &dyn std::any::Any,
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        let Some(node_id) = node.downcast_ref::<NodeId>() else { return };
        let rc = match RECORDER_HANDLE
            .with(|slot| slot.borrow().as_ref().cloned())
            .and_then(|w| w.upgrade())
        {
            Some(rc) => rc,
            None => return,
        };
        let mut state = match rc.try_borrow_mut() {
            Ok(s) => s,
            Err(_) => return,
        };
        state.emit(Command::SetAnimatedColor {
            node: *node_id,
            prop: convert_out::anim_prop_to_wire(prop),
            value,
        });
    }
}

impl Backend for WireRecordingBackend {
    type Node = NodeId;

    fn color_scheme(&self) -> ColorScheme {
        self.inner.borrow().color_scheme
    }

    /// Wrap the wire `NodeId` so layout authors who bind a
    /// `Ref<ViewHandle>` (notably the framework's `outlet_ref` inside
    /// `build_layout`) can retrieve the assigned wire id by
    /// downcasting `ViewHandle::as_any()` back to `NodeId`. The default
    /// trait impl returns an `Rc<()>` which would make outlet
    /// resolution impossible.
    fn make_view_handle(&self, node: &Self::Node) -> ViewHandle {
        ViewHandle::new(Rc::new(*node), &RecordingViewOps)
    }

    /// Symmetric to [`Self::make_view_handle`] but for text nodes.
    /// Author code that calls `AnimatedValue::bind_text_color` (the
    /// welcome example's headline color fade) reaches `TextOps` via
    /// `TextHandle::set_animated_color`; without this override the
    /// default [`runtime_core::Backend::make_text_handle`] returns
    /// `NoopTextOps` and every animation tick is silently dropped.
    fn make_text_handle(&self, node: &Self::Node) -> TextHandle {
        TextHandle::new(Rc::new(*node), &RecordingTextOps)
    }

    fn create_view(
        &mut self,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateView {
            id,
            a11y: wire_a11y,
        });
        id
    }

    fn create_text(
        &mut self,
        content: &str,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateText {
            id,
            content: content.to_string(),
            a11y: wire_a11y,
        });
        id
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &runtime_core::Action,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let identity = runtime_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_unit_for_identity(identity, on_click.fire.clone());
        let leading = leading_icon.map(convert_out::icon_data_to_wire);
        let trailing = trailing_icon.map(convert_out::icon_data_to_wire);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateButton {
            id,
            label: label.to_string(),
            on_click: handler,
            leading_icon: leading,
            trailing_icon: trailing,
            a11y: wire_a11y,
        });
        id
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let identity = runtime_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_unit_for_identity(identity, on_click);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreatePressable {
            id,
            on_click: handler,
            a11y: wire_a11y,
        });
        id
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.emit(Command::CreateReactiveAnchor { id });
        id
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::Insert {
            parent: *parent,
            child,
        });
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::InsertMany {
            parent: *parent,
            children,
        });
    }

    fn clear_children(&mut self, node: &Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::ClearChildren { node: *node });
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UpdateText {
            node: *node,
            content: content.to_string(),
        });
    }

    fn create_image(
        &mut self,
        src: &str,
        alt: Option<&str>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateImage {
            id,
            src: src.to_string(),
            alt: alt.map(str::to_string),
            a11y: wire_a11y,
        });
        id
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UpdateImageSrc {
            node: *node,
            src: src.to_string(),
        });
    }

    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let wire_data = convert_out::icon_data_to_wire(data);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateIcon {
            id,
            data: wire_data,
            color: color.map(|c| WireColor(c.0.clone())),
            a11y: wire_a11y,
        });
        id
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UpdateIconColor {
            node: *node,
            color: WireColor(color.0.clone()),
        });
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UpdateIconStroke {
            node: *node,
            progress,
        });
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: runtime_core::Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::AnimateIconStroke {
            node: *node,
            from,
            to,
            duration_ms,
            easing: convert_out::easing_to_wire(easing),
            infinite,
            autoreverses,
        });
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UpdateButtonLabel {
            node: *node,
            label: label.to_string(),
        });
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // `_on_key_down` is not yet wired across the runtime-server protocol
        // (would require a new wire op + per-frame key event
        // dispatch). Snapshot/replay clients don't observe key
        // interception — they see the resulting Signal updates only.
        let mut state = self.inner.borrow_mut();
        let identity = runtime_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_string_for_identity(identity, on_change);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateTextInput {
            id,
            initial_value: initial_value.to_string(),
            placeholder: placeholder.map(str::to_string),
            on_change: handler,
            a11y: wire_a11y,
        });
        id
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UpdateTextInputValue {
            node: *node,
            value: value.to_string(),
        });
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // `_on_key_down` is dropped on the wire for the same reason as
        // `create_text_input` above — runtime-server doesn't yet carry intercepted
        // key events. Snapshot/replay clients still see the resulting
        // Signal updates.
        let mut state = self.inner.borrow_mut();
        let identity = runtime_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_string_for_identity(identity, on_change);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateTextArea {
            id,
            initial_value: initial_value.to_string(),
            placeholder: placeholder.map(str::to_string),
            on_change: handler,
            a11y: wire_a11y,
        });
        id
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UpdateTextAreaValue {
            node: *node,
            value: value.to_string(),
        });
    }

    /// External (third-party) primitive over the wire: only the
    /// `type_name` travels. The `Rc<dyn Any>` props can't cross —
    /// they're arbitrary Rust types with no serialization contract. The
    /// client side may consult its own external registry by `type_name`
    /// and render with default props, or render a placeholder if no
    /// registration matches. Either way, this override stops the
    /// framework's `unimplemented!()` default from aborting the
    /// dev-server walker on every `Primitive::External` mount.
    fn create_external(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _payload: &Rc<dyn std::any::Any>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateExternal {
            id,
            type_name: type_name.to_string(),
            a11y: wire_a11y,
        });
        id
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let identity = runtime_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_bool_for_identity(identity, on_change);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateToggle {
            id,
            initial_value,
            on_change: handler,
            a11y: wire_a11y,
        });
        id
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UpdateToggleValue {
            node: *node,
            value,
        });
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        _on_scroll: Option<std::rc::Rc<dyn Fn(f32, f32)>>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // `on_scroll` cannot cross the wire to a client \u{2014} the
        // callback is a Rust closure with no serialized representation.
        // For runtime-server mode the user's `on_scroll` is accepted
        // but never fires; a future wire-protocol extension can
        // surface client-side scroll positions back to the server
        // for the user-callback dispatch. Sticky positioning
        // (`Position::Sticky`) renders correctly on the client
        // because the client backend handles scroll observation
        // locally; this gap is purely the user-facing `on_scroll`.
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateScrollView {
            id,
            horizontal,
            a11y: wire_a11y,
        });
        id
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let identity = runtime_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_float_for_identity(identity, on_change);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateSlider {
            id,
            initial_value,
            min,
            max,
            step,
            on_change: handler,
            a11y: wire_a11y,
        });
        id
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UpdateSliderValue {
            node: *node,
            value,
        });
    }

    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let wire_size = match size {
            primitives::activity_indicator::ActivityIndicatorSize::Small => {
                wire::WireActivityIndicatorSize::Small
            }
            primitives::activity_indicator::ActivityIndicatorSize::Large => {
                wire::WireActivityIndicatorSize::Large
            }
        };
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateActivityIndicator {
            id,
            size: wire_size,
            color: color.map(|c| WireColor(c.0.clone())),
            a11y: wire_a11y,
        });
        id
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.emit(Command::ApplyStyle {
            node: *node,
            style: sid,
        });
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        let mut state = self.inner.borrow_mut();
        let base_id = Self::intern_style(&mut state, base);
        let mut wire_overlays = Vec::with_capacity(overlays.len());
        for (bits, rules) in overlays {
            let sid = Self::intern_style(&mut state, rules);
            for bit in convert_out::expand_state_bits(*bits) {
                wire_overlays.push((bit, sid));
            }
        }
        state.emit(Command::ApplyStyledStates {
            node: *node,
            base: base_id,
            overlays: wire_overlays,
        });
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        let mut state = self.inner.borrow_mut();
        for r in rules {
            Self::intern_style(&mut state, r);
        }
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        let mut state = self.inner.borrow_mut();
        for r in rules {
            let ptr = Rc::as_ptr(r) as usize;
            if let Some((_, sid)) = state.styles_by_ptr.remove(&ptr) {
                state.emit(Command::UnregisterStyle { id: sid });
            }
        }
    }

    fn register_asset(
        &mut self,
        id: runtime_core::AssetId,
        kind: runtime_core::AssetTag,
        source: &runtime_core::AssetSource,
    ) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::RegisterAsset {
            id: convert_out::asset_id_to_wire(id),
            kind: convert_out::asset_tag_to_wire(kind),
            source: convert_out::asset_source_to_wire(source),
        });
    }

    fn unregister_asset(
        &mut self,
        id: runtime_core::AssetId,
        kind: runtime_core::AssetTag,
    ) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UnregisterAsset {
            id: convert_out::asset_id_to_wire(id),
            kind: convert_out::asset_tag_to_wire(kind),
        });
    }

    fn register_typeface(
        &mut self,
        id: runtime_core::TypefaceId,
        family_name: &str,
        faces: &[runtime_core::TypefaceFace],
        fallback: runtime_core::SystemFallback,
    ) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::RegisterTypeface {
            id: convert_out::typeface_id_to_wire(id),
            family_name: family_name.to_string(),
            faces: faces.iter().map(convert_out::typeface_face_to_wire).collect(),
            fallback: convert_out::system_fallback_to_wire(fallback),
        });
    }

    fn unregister_typeface(&mut self, id: runtime_core::TypefaceId) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UnregisterTypeface {
            id: convert_out::typeface_id_to_wire(id),
        });
    }

    // Per-frame animation writes (`set_animated_f32` /
    // `set_animated_color` on the `Backend` trait) are *not*
    // overridden here. The framework's `AnimatedValue::bind` routes
    // its per-tick writes through `ViewHandle::set_animated_*`
    // (which calls into `ViewOps`), not through the backend's
    // `Backend::set_animated_*` directly. The actual emit point is
    // [`RecordingViewOps`] above — it resolves the per-thread
    // recorder via `RECORDER_HANDLE` and emits `Command::SetAnimated*`
    // wire commands. Adding a `Backend::set_animated_*` override
    // here would be dead code (the trait method on `Backend` is only
    // hit by code paths that have a `&mut Backend` handle, which the
    // animation tick path never does — it has only a `&ViewHandle`).

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        let mut state = self.inner.borrow_mut();
        let handler_id = state.handlers.register_states(setter);
        state.state_handlers.insert(*node, handler_id);
        state.emit(Command::AttachStates { node: *node });
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::SetDisabled {
            node: *node,
            disabled,
        });
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::OnNodeUnstyled { node: *node });
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        s: primitives::presence::PresenceState,
        transition: Option<(u32, runtime_core::Easing)>,
    ) {
        let mut state = self.inner.borrow_mut();
        let wire_state = wire::WirePresenceState {
            opacity: s.opacity,
            tx: s.translate_x,
            ty: s.translate_y,
            scale: s.scale,
        };
        let wire_transition = transition.map(|(d, e)| (d, convert_out::easing_to_wire(e)));
        state.emit(Command::ApplyPresence {
            node: *node,
            state: wire_state,
            transition: wire_transition,
        });
    }

    fn finish(&mut self, root: Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::Finish { root });
    }

    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &runtime_core::accessibility::AccessibilityProps,
        inferred_role: Option<runtime_core::accessibility::Role>,
    ) {
        let mut state = self.inner.borrow_mut();
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::UpdateAccessibility {
            id: *node,
            a11y: wire_a11y,
            inferred_role: inferred_role.map(convert_out::role_to_wire),
        });
    }

    fn announce_for_accessibility(
        &mut self,
        msg: &str,
        priority: runtime_core::accessibility::LiveRegionPriority,
    ) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::AnnounceForAccessibility {
            msg: msg.to_string(),
            priority: convert_out::live_region_to_wire(priority),
        });
    }

    /// Forward installed tokens onto the wire so client-side backends
    /// with a runtime variable store (web's CSS custom properties)
    /// receive them. Without this override, the trait default was a
    /// no-op and any author-installed theme tokens never reached runtime-server
    /// clients — token-keyed `Tokenized<T>` references silently fell
    /// back to literals on every replay.
    fn install_tokens(&mut self, tokens: &[runtime_core::TokenEntry]) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::InstallThemeVariables {
            tokens: tokens.iter().map(token_entry_to_wire).collect(),
        });
    }

    /// Push updated token values onto the wire. Same shape as
    /// [`install_tokens`]; mirrors `update_tokens` on the framework
    /// side. Clients with a variable store update in place; clients
    /// without one re-resolve via the framework's token-version signal.
    fn update_tokens(&mut self, tokens: &[runtime_core::TokenEntry]) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::InstallThemeVariables {
            tokens: tokens.iter().map(token_entry_to_wire).collect(),
        });
    }

    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let identity = runtime_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = on_dismiss
            .map(|cb| state.handlers.register_unit_for_identity(identity, cb));
        let wire_target = match target {
            primitives::portal::PortalTarget::Viewport(placement) => {
                wire::WirePortalTarget::Viewport(wire_viewport_placement(placement))
            }
            primitives::portal::PortalTarget::Anchor { target: _, side, align, offset } => {
                // runtime-server can't track anchor rects on the device — the
                // anchor node id isn't carried in framework's
                // type-erased AnchorTarget. Best-effort: emit a
                // viewport-centered portal with the side/align/offset
                // info so the client can at least position something
                // sensible. (Future: extend AnchorTarget to expose a
                // NodeId for wire backends.)
                wire::WirePortalTarget::Anchor {
                    node: id,
                    side: wire_element_side(side),
                    align: wire_element_align(align),
                    offset,
                }
            }
            primitives::portal::PortalTarget::Named(name) => {
                wire::WirePortalTarget::Named(name.to_string())
            }
        };
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreatePortal {
            id,
            target: wire_target,
            on_dismiss: handler,
            trap_focus,
            a11y: wire_a11y,
        });
        id
    }

    fn create_graphics(
        &mut self,
        _on_ready: primitives::graphics::OnReady,
        _on_resize: primitives::graphics::OnResize,
        _on_lost: primitives::graphics::OnLost,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // The author's `on_ready` / `on_resize` / `on_lost` closures
        // can't travel over the wire (they capture renderer state
        // tied to the dev-side process). Instead, ship the renderer
        // *name* — the client-side `WireBackend::graphics_registry`
        // looks it up against locally-registered renderer factories
        // and stands up the surface on the client.
        //
        // Without a renderer-name plumbing path on `OnReady` we
        // currently emit `<unnamed>` and rely on the client falling
        // back to no-op handlers when no renderer is registered.
        // That's the documented gap captured in the wire-protocol
        // audit (`project_aas_graphics_unsupported` originally
        // described the placeholder-text workaround; this is the
        // forward path that lets registered renderers actually run).
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateGraphics {
            id,
            renderer: "<unnamed>".to_string(),
            a11y: wire_a11y,
        });
        id
    }

    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let identity = runtime_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_unit_for_identity(identity, config.on_activate);
        let wire_a11y = state.wire_a11y(a11y);
        // NavKind isn't carried in LinkConfig (the closure already
        // encodes which command to dispatch); the wire stores a
        // placeholder so a future renderer can target the right
        // accessibility role if it cares. Default Push is fine.
        state.emit(Command::CreateLink {
            id,
            route: config.route.to_string(),
            url: config.url,
            kind: wire::WireNavKind::Push,
            on_activate: handler,
            a11y: wire_a11y,
        });
        id
    }

    // Handle constructors fall through to the trait's no-op defaults.
    // Imperative `handle.click()` from dev-side code is a v1 feature
    // (needs a synchronous round trip we don't implement yet).

    // ---------------------------------------------------------------------
    // Release hooks — emit `Command::ReleaseNode` so the client tears down
    // the matching backend node and the dev-client's per-node bookkeeping
    // is dropped instead of growing across hot-reload cycles.
    //
    // The framework's walker installs RAII cleanup guards per primitive
    // that fire these on `Scope::drop`. Without the overrides below, every
    // unmounted navigator / virtualizer / portal / graphics / external /
    // overlay would leak its `NodeId` on the client side (see
    // `SceneModel::apply` for the matching per-node map clears).

    fn release_virtualizer(&mut self, node: &Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::ReleaseNode { node: *node });
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::ReleaseNode { node: *node });
    }

    fn release_portal(&mut self, node: &Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::ReleaseNode { node: *node });
    }

    fn release_external(&mut self, node: &Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::ReleaseNode { node: *node });
    }
}

// ---------------------------------------------------------------------------
// Wire mappers for the new portal primitive's positioning enums.
// ---------------------------------------------------------------------------

fn token_entry_to_wire(entry: &runtime_core::TokenEntry) -> wire::WireTokenEntry {
    wire::WireTokenEntry {
        name: entry.name.to_string(),
        value: token_value_to_wire(&entry.value),
    }
}

fn token_value_to_wire(value: &runtime_core::TokenValue) -> wire::WireTokenValue {
    match value {
        runtime_core::TokenValue::Color(c) => {
            wire::WireTokenValue::Color(wire::WireColor(c.0.clone()))
        }
        runtime_core::TokenValue::Number(n) => wire::WireTokenValue::Number(*n),
        runtime_core::TokenValue::Length(l) => {
            wire::WireTokenValue::Length(length_to_wire_token(*l))
        }
    }
}

fn length_to_wire_token(l: runtime_core::Length) -> wire::WireLength {
    match l {
        runtime_core::Length::Px(v) => wire::WireLength::Px(v),
        runtime_core::Length::Percent(v) => wire::WireLength::Pct(v),
        runtime_core::Length::Auto => wire::WireLength::Auto,
    }
}

fn wire_viewport_placement(
    p: primitives::portal::ViewportPlacement,
) -> wire::WireViewportPlacement {
    match p {
        primitives::portal::ViewportPlacement::Center => wire::WireViewportPlacement::Center,
        primitives::portal::ViewportPlacement::Top => wire::WireViewportPlacement::Top,
        primitives::portal::ViewportPlacement::Bottom => wire::WireViewportPlacement::Bottom,
        primitives::portal::ViewportPlacement::Left => wire::WireViewportPlacement::Left,
        primitives::portal::ViewportPlacement::Right => wire::WireViewportPlacement::Right,
        primitives::portal::ViewportPlacement::FullScreen => {
            wire::WireViewportPlacement::FullScreen
        }
    }
}

fn wire_element_side(s: primitives::portal::ElementSide) -> wire::WireElementSide {
    match s {
        primitives::portal::ElementSide::Above => wire::WireElementSide::Above,
        primitives::portal::ElementSide::Below => wire::WireElementSide::Below,
        primitives::portal::ElementSide::Start => wire::WireElementSide::Start,
        primitives::portal::ElementSide::End => wire::WireElementSide::End,
    }
}

fn wire_element_align(a: primitives::portal::ElementAlign) -> wire::WireElementAlign {
    match a {
        primitives::portal::ElementAlign::Start => wire::WireElementAlign::Start,
        primitives::portal::ElementAlign::Center => wire::WireElementAlign::Center,
        primitives::portal::ElementAlign::End => wire::WireElementAlign::End,
    }
}

// ---------------------------------------------------------------------------
// Navigator dispatcher — legacy callback path stripped. The
// SDK-handler-level recording will live here once the dev wire is
// rewired against `NavigatorRegistry` / `NavigatorHandler<B>`. For
// now everything below is dead code preserved only so the existing
// inherent methods on `WireRecordingBackend` keep their shape.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// App→Dev event dispatch helpers exposed on WireRecordingBackend.
// ---------------------------------------------------------------------------

impl WireRecordingBackend {
    /// Handle an `AppToDev::ScreenReleased { scope }` event arriving
    /// from the app side. Stubbed pending SDK-handler-level
    /// recording — the legacy callback path was removed when the
    /// navigator substrate was refactored out of runtime-core.
    pub fn handle_screen_released(&self, _scope: u64) -> bool {
        false
    }

    /// Handle `AppToDev::DrawerStateChanged` — informational only;
    /// useful for analytics or for fwd to the framework's drawer
    /// open-state signal in a follow-up.
    pub fn handle_drawer_state_changed(&self, _navigator: NodeId, _is_open: bool) {
        // Reserved for drawer signal sync in a follow-up.
    }

    /// Handle `AppToDev::TabSelected` — used when the platform fires
    /// a tab activation gesture. The framework's tab navigator
    /// dispatcher needs to be told to switch.
    pub fn handle_tab_selected(&self, _navigator: NodeId, _index: u32) {
        // Reserved for tab signal sync in a follow-up.
    }
}

// ==========================================================================
// Legacy nav recorder methods — inherent. Dev wire's navigator recording
// path was driven by Backend trait methods that have been removed; these
// inherent versions are temporarily dead code until the dev wire is
// rewired to record at the SDK-handler level (post-SDK-migration TODO).
// ==========================================================================

impl WireRecordingBackend {
    pub fn create_virtualizer(
        &mut self,
        callbacks: runtime_core::VirtualizerCallbacks<NodeId>,
        overscan: f32,
        horizontal: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> NodeId {
        // Eagerly snapshot the current data set: count + keys +
        // initial sizes. The wire ships these so the app's
        // virtualizer can stand up its scroll math and visible-window
        // tracking. Items themselves mount lazily via
        // `VirtualizerMountItem` reverse-channel events.
        let count = (callbacks.item_count)();
        let keys: Vec<u64> = (0..count).map(|i| (callbacks.item_key)(i)).collect();
        let sizes: Vec<f32> = (0..count).map(|i| (callbacks.item_size)(i)).collect();
        let measured = callbacks.measure_sizes;

        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let wire_a11y = state.wire_a11y(a11y);
        state.emit(Command::CreateVirtualizer {
            id,
            overscan,
            horizontal,
            initial_size: wire::WireItemSize { measured, sizes },
            initial_keys: keys,
            a11y: wire_a11y,
        });
        // Note: callbacks aren't stored on the recorder side yet.
        // Lazy mount-on-demand is the natural next step:
        //   - Stash `callbacks` in a `HashMap<NodeId, _>` similar to
        //     navigators.
        //   - Add `handle_virtualizer_mount_item(node, index)` on the
        //     recorder, mirroring `handle_screen_released`.
        //   - Inside it: `let (n, scope) = (callbacks.mount_item)(idx)`,
        //     emit `VirtualizerAttachItem { node, index, child: n,
        //     scope: ScopeId(scope) }`.
        // Deferred so the comprehensive nav/link/overlay/graphics
        // work ships first.
        let _ = callbacks; // suppress unused
        id
    }

    pub fn virtualizer_data_changed(&mut self, node: &NodeId) {
        // Re-snapshot count for now — keys/sizes refresh in a follow-up
        // alongside mount-on-demand wiring above.
        self.inner.borrow_mut().emit(Command::VirtualizerDataChanged {
            node: *node,
            item_count: 0,
        });
    }

}
