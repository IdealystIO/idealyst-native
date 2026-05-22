//! Dev-side runtime for the hot-reload wire protocol.
//!
//! Provides a [`WireRecordingBackend`] that implements
//! [`framework_core::Backend`] with `Node = NodeId`. Each method
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

use framework_core::primitives;
use framework_core::{Backend, Color, ColorScheme, StateBits, StyleRules, ViewHandle, ViewOps};
use wire::{
    Command, EventArgs, HandlerId, NodeId, StyleId, WireColor, WireStateBit,
};

pub mod convert_out;
mod scene_model;
pub mod sidecar;
pub mod transport;
pub mod watch;

use scene_model::SceneModel;

pub use sidecar::{Sidecar, SidecarIn, SidecarOut, SidecarSlot};
pub use transport::{
    serve, serve_with_port_mirror, serve_with_sidecar, serve_with_tick,
    serve_with_tick_and_port,
};
#[cfg(feature = "robot")]
pub use transport::serve_with_robot_bridge;
pub use watch::{spawn_change_loop, spawn_rebuild_loop, RebuildCommand, RebuildConfig};

/// The AAS (Application-as-a-Server) **server-side backend** —
/// implements `framework_core::Backend` with `Node = NodeId`. Plug
/// this into `framework_core::render(...)` exactly like you'd plug
/// in `WebBackend` / `IosBackend` / `AndroidBackend`. Instead of
/// driving native widgets it records every walker call as a wire
/// [`wire::Command`] for transport to one or more
/// [`AasClient`](dev_client::AasClient)s.
///
/// `AasBackend` is the heart of the AAS architecture:
///
/// ```text
/// UI tree → AasBackend → Wire (Commands) → AasClient → Platform Backend → Native
/// ```
///
/// The same `Primitive` tree your iOS/web app would render natively
/// is what the server runs against this backend. The wire output is
/// platform-agnostic; an `AasClient` wrapping any platform backend
/// can replay it.
pub use crate::WireRecordingBackend as AasBackend;

/// Stores the live dev-side closures the walker has handed us. Each
/// gets a `HandlerId` minted by the recorder; events arriving back
/// from the app look up the entry and invoke the captured closure.
///
/// **Identity-keyed dedup.** Closures registered with an
/// [`framework_core::Identity`] reuse the same `HandlerId` across
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
    identity_to_id: HashMap<framework_core::Identity, HandlerId>,
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
    /// [`framework_core::Identity`]) across rebuilds reuses the same
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
        identity: framework_core::Identity,
        f: Rc<dyn Fn()>,
    ) -> HandlerId {
        if identity == framework_core::Identity::UNIDENTIFIED {
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
        identity: framework_core::Identity,
        f: Rc<dyn Fn(bool)>,
    ) -> HandlerId {
        if identity == framework_core::Identity::UNIDENTIFIED {
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
        identity: framework_core::Identity,
        f: Rc<dyn Fn(f32)>,
    ) -> HandlerId {
        if identity == framework_core::Identity::UNIDENTIFIED {
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
        identity: framework_core::Identity,
        f: Rc<dyn Fn(String)>,
    ) -> HandlerId {
        if identity == framework_core::Identity::UNIDENTIFIED {
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
    /// Identity → NodeId memo. Keyed by [`framework_core::Identity`]
    /// (the structural identity the walker sets via
    /// `with_current_identity` before every `backend.create_*` call).
    /// Survives [`WireRecordingBackend::reset_log_and_scene`] so that
    /// across sidecar respawns the same structural emission lands on
    /// the same wire `NodeId` — that's what makes incremental hot
    /// reload incremental (`CreateView` for the same node is a no-op
    /// on the client; new `ApplyStyle` for the same node lands on
    /// the right native view).
    ///
    /// Emissions that arrive under [`framework_core::Identity::UNIDENTIFIED`]
    /// bypass dedup (mint a fresh id every time). Used as a
    /// pressure-release for any emission site that hasn't been
    /// migrated to set an identity yet.
    identity_to_node: HashMap<framework_core::Identity, NodeId>,
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
}

/// Per-navigator dev-side state used by the recording backend's
/// dispatcher. The callbacks come from the framework when
/// `create_navigator` is called; the stack is the recorder's own
/// model of the navigator's screen stack (so it can call
/// `release_screen` for the right scope when handling Pop / swipe-back).
pub struct NavigatorRecState {
    /// Framework-supplied callbacks. `Rc`'d because individual fields
    /// are already `Rc<dyn Fn(...)>`; the wrapping `Rc` makes whole-
    /// struct clones cheap from inside the dispatcher closure.
    pub callbacks: Rc<framework_core::primitives::navigator::NavigatorCallbacks<NodeId>>,
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
        Self {
            inner: Rc::new(RefCell::new(RecorderState {
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
            })),
            nav_state_mirror,
        }
    }

    /// Public handle to the per-navigator URL stack mirror. Send + Sync
    /// — safe to share with the file-watch / rebuild thread so it can
    /// serialize the current navigation hierarchy before `exec`.
    pub fn nav_state_mirror(&self) -> Arc<Mutex<NavStateSnapshot>> {
        self.nav_state_mirror.clone()
    }

    /// Restore a previously-snapshotted navigator stack. Called by
    /// the dev-server's main on startup, after the framework's
    /// initial render has produced fresh navigators at the same
    /// `NodeId`s. For each saved URL beyond the initial route, we
    /// look up the matching `(name, params)` via `match_path` and
    /// dispatch a `Push` — which goes through the regular dispatcher
    /// and emits real `NavigatorPush` wire commands, so the same
    /// screens come back without any client-side cooperation.
    pub fn restore_nav_state(&self, saved: &NavStateSnapshot) {
        use framework_core::primitives::navigator::NavCommand;
        for (nav_id_raw, urls) in saved {
            let nav_id = NodeId(*nav_id_raw);
            // Snapshot the callbacks under a short borrow.
            let callbacks = {
                let state = self.inner.borrow();
                let Some(nav) = state.navigators.get(&nav_id) else {
                    eprintln!(
                        "[dev-server] restore: no navigator at id {}; the route table may have changed",
                        nav_id_raw
                    );
                    continue;
                };
                nav.callbacks.clone()
            };
            // Skip the initial route — `navigator_attach_initial`
            // already put it in place.
            for url in urls.iter().skip(1) {
                let Some((name, params)) = (callbacks.match_path)(url) else {
                    eprintln!(
                        "[dev-server] restore: no route matches {:?} — stopping replay for navigator {}",
                        url, nav_id_raw
                    );
                    break;
                };
                navigator_dispatcher_handle(
                    &self.inner,
                    nav_id,
                    callbacks.clone(),
                    NavCommand::Push {
                        name,
                        url: url.clone(),
                        params,
                    },
                );
            }
        }
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
    /// The infra-only AAS host uses this to mirror the sidecar's
    /// command stream into its own recorder without running the user
    /// code itself. Local renders should always go through the
    /// `Backend` trait — this method bypasses the framework runtime
    /// and is purely a transport hook.
    pub fn push_external_command(&self, cmd: Command) {
        self.inner.borrow_mut().emit(cmd);
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
    /// ambient [`framework_core::current_identity`] to dedup across
    /// sidecar respawns: the same structural emission always gets the
    /// same `NodeId`, which is what makes hot reload incremental
    /// rather than a full-scene reset. Emissions under
    /// [`framework_core::Identity::UNIDENTIFIED`] (legacy path that
    /// hasn't been migrated yet) always mint fresh — no dedup.
    fn mint_node(state: &mut RecorderState) -> NodeId {
        let id = framework_core::current_identity();
        if id != framework_core::Identity::UNIDENTIFIED {
            if let Some(&existing) = state.identity_to_node.get(&id) {
                return existing;
            }
        }
        state.next_node += 1;
        let assigned = NodeId(state.next_node);
        if id != framework_core::Identity::UNIDENTIFIED {
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
impl ViewOps for RecordingViewOps {}

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

    fn create_view(
        &mut self,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.emit(Command::CreateView {
            id,
            a11y: convert_out::a11y_to_wire(a11y),
        });
        id
    }

    fn create_text(
        &mut self,
        content: &str,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.emit(Command::CreateText {
            id,
            content: content.to_string(),
            a11y: convert_out::a11y_to_wire(a11y),
        });
        id
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &framework_core::Action,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let identity = framework_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_unit_for_identity(identity, on_click.fire.clone());
        let leading = leading_icon.map(convert_out::icon_data_to_wire);
        let trailing = trailing_icon.map(convert_out::icon_data_to_wire);
        state.emit(Command::CreateButton {
            id,
            label: label.to_string(),
            on_click: handler,
            leading_icon: leading,
            trailing_icon: trailing,
            a11y: convert_out::a11y_to_wire(a11y),
        });
        id
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let identity = framework_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_unit_for_identity(identity, on_click);
        state.emit(Command::CreatePressable {
            id,
            on_click: handler,
            a11y: convert_out::a11y_to_wire(a11y),
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
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.emit(Command::CreateImage {
            id,
            src: src.to_string(),
            alt: alt.map(str::to_string),
            a11y: convert_out::a11y_to_wire(a11y),
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
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let wire_data = convert_out::icon_data_to_wire(data);
        state.emit(Command::CreateIcon {
            id,
            data: wire_data,
            color: color.map(|c| WireColor(c.0.clone())),
            a11y: convert_out::a11y_to_wire(a11y),
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
        easing: framework_core::Easing,
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
        _on_key_down: Option<framework_core::primitives::key::KeyDownHandler>,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // `_on_key_down` is not yet wired across the AAS protocol
        // (would require a new wire op + per-frame key event
        // dispatch). Snapshot/replay clients don't observe key
        // interception — they see the resulting Signal updates only.
        let mut state = self.inner.borrow_mut();
        let identity = framework_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_string_for_identity(identity, on_change);
        state.emit(Command::CreateTextInput {
            id,
            initial_value: initial_value.to_string(),
            placeholder: placeholder.map(str::to_string),
            on_change: handler,
            a11y: convert_out::a11y_to_wire(a11y),
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
        _on_key_down: Option<framework_core::primitives::key::KeyDownHandler>,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // `_on_key_down` is dropped on the wire for the same reason as
        // `create_text_input` above — AAS doesn't yet carry intercepted
        // key events. Snapshot/replay clients still see the resulting
        // Signal updates.
        let mut state = self.inner.borrow_mut();
        let identity = framework_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_string_for_identity(identity, on_change);
        state.emit(Command::CreateTextArea {
            id,
            initial_value: initial_value.to_string(),
            placeholder: placeholder.map(str::to_string),
            on_change: handler,
            a11y: convert_out::a11y_to_wire(a11y),
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
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.emit(Command::CreateExternal {
            id,
            type_name: type_name.to_string(),
            a11y: convert_out::a11y_to_wire(a11y),
        });
        id
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let identity = framework_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_bool_for_identity(identity, on_change);
        state.emit(Command::CreateToggle {
            id,
            initial_value,
            on_change: handler,
            a11y: convert_out::a11y_to_wire(a11y),
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
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.emit(Command::CreateScrollView {
            id,
            horizontal,
            a11y: convert_out::a11y_to_wire(a11y),
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
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let identity = framework_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_float_for_identity(identity, on_change);
        state.emit(Command::CreateSlider {
            id,
            initial_value,
            min,
            max,
            step,
            on_change: handler,
            a11y: convert_out::a11y_to_wire(a11y),
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

    fn create_video(
        &mut self,
        src: &str,
        autoplay: bool,
        controls: bool,
        loop_playback: bool,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.emit(Command::CreateVideo {
            id,
            src: src.to_string(),
            autoplay,
            controls,
            loop_playback,
            a11y: convert_out::a11y_to_wire(a11y),
        });
        id
    }

    fn update_video_src(&mut self, node: &Self::Node, src: &str) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UpdateVideoSrc {
            node: *node,
            src: src.to_string(),
        });
    }

    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &framework_core::accessibility::AccessibilityProps,
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
        state.emit(Command::CreateActivityIndicator {
            id,
            size: wire_size,
            color: color.map(|c| WireColor(c.0.clone())),
            a11y: convert_out::a11y_to_wire(a11y),
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
        id: framework_core::AssetId,
        kind: framework_core::AssetTag,
        source: &framework_core::AssetSource,
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
        id: framework_core::AssetId,
        kind: framework_core::AssetTag,
    ) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UnregisterAsset {
            id: convert_out::asset_id_to_wire(id),
            kind: convert_out::asset_tag_to_wire(kind),
        });
    }

    fn register_typeface(
        &mut self,
        id: framework_core::TypefaceId,
        family_name: &str,
        faces: &[framework_core::TypefaceFace],
        fallback: framework_core::SystemFallback,
    ) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::RegisterTypeface {
            id: convert_out::typeface_id_to_wire(id),
            family_name: family_name.to_string(),
            faces: faces.iter().map(convert_out::typeface_face_to_wire).collect(),
            fallback: convert_out::system_fallback_to_wire(fallback),
        });
    }

    fn unregister_typeface(&mut self, id: framework_core::TypefaceId) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UnregisterTypeface {
            id: convert_out::typeface_id_to_wire(id),
        });
    }

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
        transition: Option<(u32, framework_core::Easing)>,
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
        a11y: &framework_core::accessibility::AccessibilityProps,
        inferred_role: Option<framework_core::accessibility::Role>,
    ) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::UpdateAccessibility {
            id: *node,
            a11y: convert_out::a11y_to_wire(a11y),
            inferred_role: inferred_role.map(convert_out::role_to_wire),
        });
    }

    fn announce_for_accessibility(
        &mut self,
        msg: &str,
        priority: framework_core::accessibility::LiveRegionPriority,
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
    /// no-op and any author-installed theme tokens never reached AAS
    /// clients — token-keyed `Tokenized<T>` references silently fell
    /// back to literals on every replay.
    fn install_tokens(&mut self, tokens: &[framework_core::TokenEntry]) {
        let mut state = self.inner.borrow_mut();
        state.emit(Command::InstallThemeVariables {
            tokens: tokens.iter().map(token_entry_to_wire).collect(),
        });
    }

    /// Push updated token values onto the wire. Same shape as
    /// [`install_tokens`]; mirrors `update_tokens` on the framework
    /// side. Clients with a variable store update in place; clients
    /// without one re-resolve via the framework's token-version signal.
    fn update_tokens(&mut self, tokens: &[framework_core::TokenEntry]) {
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
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let identity = framework_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = on_dismiss
            .map(|cb| state.handlers.register_unit_for_identity(identity, cb));
        let wire_target = match target {
            primitives::portal::PortalTarget::Viewport(placement) => {
                wire::WirePortalTarget::Viewport(wire_viewport_placement(placement))
            }
            primitives::portal::PortalTarget::Anchor { target: _, side, align, offset } => {
                // AAS can't track anchor rects on the device — the
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
        state.emit(Command::CreatePortal {
            id,
            target: wire_target,
            on_dismiss: handler,
            trap_focus,
            a11y: convert_out::a11y_to_wire(a11y),
        });
        id
    }

    fn create_graphics(
        &mut self,
        _on_ready: primitives::graphics::OnReady,
        _on_resize: primitives::graphics::OnResize,
        _on_lost: primitives::graphics::OnLost,
        a11y: &framework_core::accessibility::AccessibilityProps,
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
        state.emit(Command::CreateGraphics {
            id,
            renderer: "<unnamed>".to_string(),
            a11y: convert_out::a11y_to_wire(a11y),
        });
        id
    }

    fn create_navigator(
        &mut self,
        callbacks: framework_core::primitives::navigator::NavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::primitives::navigator::NavigatorControl>,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let nav_id;
        let initial_route = callbacks.initial_route;
        let initial_path = callbacks.initial_path;
        let callbacks_rc = Rc::new(callbacks);
        {
            let mut state = self.inner.borrow_mut();
            nav_id = Self::mint_node(&mut state);
            state.emit(Command::CreateNavigator {
                id: nav_id,
                initial_route: initial_route.to_string(),
                initial_path: initial_path.to_string(),
                a11y: convert_out::a11y_to_wire(a11y),
            });
            state.navigators.insert(
                nav_id,
                NavigatorRecState {
                    callbacks: callbacks_rc.clone(),
                    stack: Vec::new(), stack_urls: Vec::new(),
                },
            );
        }
        // Install dispatcher. The closure captures a Weak ref to the
        // shared state so it doesn't keep the recorder alive past
        // teardown.
        let weak_inner = Rc::downgrade(&self.inner);
        let cbs = callbacks_rc.clone();
        control.install(Box::new(move |cmd| {
            let Some(inner) = weak_inner.upgrade() else {
                return;
            };
            navigator_dispatcher_handle(&inner, nav_id, cbs.clone(), cmd);
        }));
        nav_id
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        let mirror = self.nav_state_mirror.clone();
        let mut urls_snapshot: Option<Vec<String>> = None;
        {
            let mut state = self.inner.borrow_mut();
            if let Some(nav) = state.navigators.get_mut(navigator) {
                nav.stack.push(scope_id);
                // Seed `stack_urls` with the navigator's declared
                // initial path so `restore_nav_state` knows what URL
                // the bottom-of-stack screen corresponds to.
                let initial_path = nav.callbacks.initial_path.to_string();
                nav.stack_urls.push(initial_path);
                urls_snapshot = Some(nav.stack_urls.clone());
            }
            state.scope_to_navigator.insert(scope_id, *navigator);
            let wire_options = screen_options_to_wire(&mut state, &options);
            state.emit(Command::NavigatorAttachInitial {
                navigator: *navigator,
                screen,
                scope: wire::ScopeId(scope_id),
                options: wire_options,
            });
        }
        if let Some(urls) = urls_snapshot {
            if let Ok(mut m) = mirror.lock() {
                m.insert(navigator.0, urls);
            }
        }
    }

    fn create_tab_navigator(
        &mut self,
        callbacks: framework_core::primitives::navigator::TabNavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::primitives::navigator::NavigatorControl>,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let nav_id;
        let initial_route = callbacks.navigator.initial_route;
        let initial_path = callbacks.navigator.initial_path;
        let tabs_wire: Vec<wire::WireTabRegistration> = callbacks
            .tabs
            .iter()
            .map(|t| wire::WireTabRegistration {
                route: t.route.to_string(),
                label: t.label.clone(),
                icon: t.icon.clone(),
            })
            .collect();
        let placement = match callbacks.placement {
            framework_core::primitives::navigator::TabPlacement::Bottom
            | framework_core::primitives::navigator::TabPlacement::Auto => {
                wire::WireTabPlacement::Bottom
            }
            framework_core::primitives::navigator::TabPlacement::Top
            | framework_core::primitives::navigator::TabPlacement::Sidebar => {
                wire::WireTabPlacement::Top
            }
        };
        let mount_policy = match callbacks.mount_policy {
            framework_core::primitives::navigator::MountPolicy::EagerPersistent => {
                wire::WireMountPolicy::EagerPersistent
            }
            framework_core::primitives::navigator::MountPolicy::LazyPersistent => {
                wire::WireMountPolicy::LazyPersistent
            }
            framework_core::primitives::navigator::MountPolicy::LazyDisposing => {
                wire::WireMountPolicy::LazyDisposing
            }
        };
        let inner_callbacks_rc = Rc::new(callbacks.navigator);
        {
            let mut state = self.inner.borrow_mut();
            nav_id = Self::mint_node(&mut state);
            state.emit(Command::CreateTabNavigator {
                id: nav_id,
                initial_route: initial_route.to_string(),
                initial_path: initial_path.to_string(),
                tabs: tabs_wire,
                placement,
                mount_policy,
                a11y: convert_out::a11y_to_wire(a11y),
            });
            state.navigators.insert(
                nav_id,
                NavigatorRecState {
                    callbacks: inner_callbacks_rc.clone(),
                    stack: Vec::new(), stack_urls: Vec::new(),
                },
            );
        }
        let weak_inner = Rc::downgrade(&self.inner);
        let cbs = inner_callbacks_rc.clone();
        control.install(Box::new(move |cmd| {
            let Some(inner) = weak_inner.upgrade() else {
                return;
            };
            navigator_dispatcher_handle(&inner, nav_id, cbs.clone(), cmd);
        }));
        nav_id
    }

    fn tab_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        // Same wire shape as the stack-navigator initial attach; the
        // app side dispatches based on what kind of navigator the
        // navigator NodeId belongs to.
        self.navigator_attach_initial(navigator, screen, scope_id, options);
    }

    fn create_drawer_navigator(
        &mut self,
        callbacks: framework_core::primitives::navigator::DrawerNavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::primitives::navigator::NavigatorControl>,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let nav_id;
        let initial_route = callbacks.navigator.initial_route;
        let initial_path = callbacks.navigator.initial_path;
        let side = match callbacks.side {
            framework_core::primitives::navigator::DrawerSide::Start => {
                wire::WireDrawerSide::Left
            }
            framework_core::primitives::navigator::DrawerSide::End => {
                wire::WireDrawerSide::Right
            }
        };
        let drawer_type = match callbacks.drawer_type {
            framework_core::primitives::navigator::DrawerType::Front => {
                wire::WireDrawerType::Front
            }
            framework_core::primitives::navigator::DrawerType::Slide => {
                wire::WireDrawerType::Slide
            }
        };
        let mount_policy = match callbacks.mount_policy {
            framework_core::primitives::navigator::MountPolicy::EagerPersistent => {
                wire::WireMountPolicy::EagerPersistent
            }
            framework_core::primitives::navigator::MountPolicy::LazyPersistent => {
                wire::WireMountPolicy::LazyPersistent
            }
            framework_core::primitives::navigator::MountPolicy::LazyDisposing => {
                wire::WireMountPolicy::LazyDisposing
            }
        };
        let drawer_width = callbacks.drawer_width;
        let swipe_to_open = callbacks.swipe_to_open;
        let inner_callbacks_rc = Rc::new(callbacks.navigator);
        {
            let mut state = self.inner.borrow_mut();
            nav_id = Self::mint_node(&mut state);
            state.emit(Command::CreateDrawerNavigator {
                id: nav_id,
                initial_route: initial_route.to_string(),
                initial_path: initial_path.to_string(),
                side,
                drawer_type,
                drawer_width,
                swipe_to_open,
                mount_policy,
                a11y: convert_out::a11y_to_wire(a11y),
            });
            state.navigators.insert(
                nav_id,
                NavigatorRecState {
                    callbacks: inner_callbacks_rc.clone(),
                    stack: Vec::new(), stack_urls: Vec::new(),
                },
            );
        }
        let weak_inner = Rc::downgrade(&self.inner);
        let cbs = inner_callbacks_rc.clone();
        control.install(Box::new(move |cmd| {
            let Some(inner) = weak_inner.upgrade() else {
                return;
            };
            navigator_dispatcher_handle(&inner, nav_id, cbs.clone(), cmd);
        }));
        nav_id
    }

    fn drawer_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        self.navigator_attach_initial(navigator, screen, scope_id, options);
    }

    fn drawer_navigator_attach_sidebar(
        &mut self,
        navigator: &Self::Node,
        sidebar: Self::Node,
    ) {
        self.inner.borrow_mut().emit(Command::DrawerAttachSidebar {
            navigator: *navigator,
            sidebar,
        });
    }

    fn attach_navigator_layout(
        &mut self,
        navigator: &Self::Node,
        root: Self::Node,
        outlet: Self::Node,
    ) {
        self.inner.borrow_mut().emit(Command::AttachNavigatorLayout {
            navigator: *navigator,
            root,
            outlet,
        });
    }

    fn create_virtualizer(
        &mut self,
        callbacks: framework_core::VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
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
        state.emit(Command::CreateVirtualizer {
            id,
            overscan,
            horizontal,
            initial_size: wire::WireItemSize { measured, sizes },
            initial_keys: keys,
            a11y: convert_out::a11y_to_wire(a11y),
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

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        // Re-snapshot count for now — keys/sizes refresh in a follow-up
        // alongside mount-on-demand wiring above.
        self.inner.borrow_mut().emit(Command::VirtualizerDataChanged {
            node: *node,
            item_count: 0,
        });
    }

    fn apply_navigator_header_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.emit(Command::ApplyNavigatorHeaderStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_navigator_title_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.emit(Command::ApplyNavigatorTitleStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_navigator_button_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.emit(Command::ApplyNavigatorButtonStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_navigator_body_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.emit(Command::ApplyNavigatorBodyStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_drawer_sidebar_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.emit(Command::ApplyDrawerSidebarStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_drawer_scrim_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.emit(Command::ApplyDrawerScrimStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_tab_bar_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.emit(Command::ApplyTabBarStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_tab_icon_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.emit(Command::ApplyTabIconStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_tab_label_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.emit(Command::ApplyTabLabelStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let identity = framework_core::current_identity();
        let id = Self::mint_node(&mut state);
        let handler = state
            .handlers
            .register_unit_for_identity(identity, config.on_activate);
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
            a11y: convert_out::a11y_to_wire(a11y),
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
    fn release_navigator(&mut self, node: &Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.navigators.remove(node);
        state.emit(Command::ReleaseNode { node: *node });
    }

    fn release_tab_navigator(&mut self, node: &Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.navigators.remove(node);
        state.emit(Command::ReleaseNode { node: *node });
    }

    fn release_drawer_navigator(&mut self, node: &Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.navigators.remove(node);
        state.emit(Command::ReleaseNode { node: *node });
    }

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

fn token_entry_to_wire(entry: &framework_core::TokenEntry) -> wire::WireTokenEntry {
    wire::WireTokenEntry {
        name: entry.name.to_string(),
        value: token_value_to_wire(&entry.value),
    }
}

fn token_value_to_wire(value: &framework_core::TokenValue) -> wire::WireTokenValue {
    match value {
        framework_core::TokenValue::Color(c) => {
            wire::WireTokenValue::Color(wire::WireColor(c.0.clone()))
        }
        framework_core::TokenValue::Number(n) => wire::WireTokenValue::Number(*n),
        framework_core::TokenValue::Length(l) => {
            wire::WireTokenValue::Length(length_to_wire_token(*l))
        }
    }
}

fn length_to_wire_token(l: framework_core::Length) -> wire::WireLength {
    match l {
        framework_core::Length::Px(v) => wire::WireLength::Px(v),
        framework_core::Length::Percent(v) => wire::WireLength::Pct(v),
        framework_core::Length::Auto => wire::WireLength::Auto,
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
// Navigator dispatcher — called from the closure installed on the
// NavigatorControl in `create_navigator`. Free function rather than
// method so we don't get tangled up in lifetimes between the closure
// and `self`.
// ---------------------------------------------------------------------------

fn navigator_dispatcher_handle(
    inner: &Rc<RefCell<RecorderState>>,
    nav_id: NodeId,
    cbs: Rc<framework_core::primitives::navigator::NavigatorCallbacks<NodeId>>,
    cmd: framework_core::primitives::navigator::NavCommand,
) {
    use framework_core::primitives::navigator::NavCommand;
    // Helper closure: build the screen subtree by invoking
    // `mount_screen` (which calls back into the recording backend),
    // then translate into wire form. Borrows are released BEFORE
    // mount_screen runs so the walker can re-enter `&mut self`.
    let push_like = |kind: PushLikeKind,
                     name: &'static str,
                     url: String,
                     params: Box<dyn std::any::Any>,
                     restore: bool| {
        let mount = (cbs.mount_screen)(name, params);
        let scope = wire::ScopeId(mount.scope_id);
        let (released_scopes, new_depth, urls_snapshot, mirror) = {
            let mut state = inner.borrow_mut();
            let wire_options = screen_options_to_wire(&mut state, &mount.options);
            let nav_state = state.navigators.get_mut(&nav_id).unwrap();
            let mut released = Vec::new();
            match kind {
                PushLikeKind::Push | PushLikeKind::Select => {
                    nav_state.stack.push(mount.scope_id);
                    nav_state.stack_urls.push(url.clone());
                }
                PushLikeKind::Replace => {
                    if let Some(popped) = nav_state.stack.pop() {
                        released.push(popped);
                    }
                    let _ = nav_state.stack_urls.pop();
                    nav_state.stack.push(mount.scope_id);
                    nav_state.stack_urls.push(url.clone());
                }
                PushLikeKind::Reset => {
                    released.extend(nav_state.stack.drain(..));
                    nav_state.stack_urls.clear();
                    nav_state.stack.push(mount.scope_id);
                    nav_state.stack_urls.push(url.clone());
                }
            }
            let depth = nav_state.stack.len();
            let urls_snapshot = nav_state.stack_urls.clone();
            state.scope_to_navigator.insert(mount.scope_id, nav_id);
            for r in &released {
                state.scope_to_navigator.remove(r);
            }
            state.emit(match kind {
                PushLikeKind::Push => Command::NavigatorPush {
                    navigator: nav_id,
                    screen: mount.node,
                    scope,
                    options: wire_options,
                    url: url.clone(),
                    restore,
                },
                PushLikeKind::Replace => Command::NavigatorReplace {
                    navigator: nav_id,
                    screen: mount.node,
                    scope,
                    options: wire_options,
                    url: url.clone(),
                    restore,
                },
                PushLikeKind::Reset => Command::NavigatorReset {
                    navigator: nav_id,
                    screen: mount.node,
                    scope,
                    options: wire_options,
                    url: url.clone(),
                    restore,
                },
                // Select on a tab/drawer navigator is a single-screen
                // "swap to this route" — semantically distinct from
                // `Reset` (which drains the whole stack). Pre-fix we
                // masqueraded as `NavigatorReset`, which caused
                // `SceneModel::apply(NavigatorReset)` to drain the
                // navigator's per-screen state. Emitting the new
                // explicit `NavigatorSelect` keeps the snapshot model
                // accurate and the dev-client dispatches `NavCommand::
                // Select` directly without disambiguating by navigator
                // kind.
                PushLikeKind::Select => Command::NavigatorSelect {
                    navigator: nav_id,
                    screen: mount.node,
                    scope,
                    options: wire_options,
                    url: url.clone(),
                },
            });
            let mirror = state.nav_state_mirror.clone();
            (released, depth, urls_snapshot, mirror)
        };
        // Sync the Send+Sync mirror after the borrow is released —
        // the rebuild thread can read this snapshot just before
        // `exec` to persist the navigation hierarchy.
        if let Ok(mut m) = mirror.lock() {
            m.insert(nav_id.0, urls_snapshot);
        }
        for scope_id in released_scopes {
            (cbs.release_screen)(scope_id);
        }
        (cbs.depth_changed)(new_depth);
    };

    match cmd {
        NavCommand::Push { name, url, params } => {
            push_like(PushLikeKind::Push, name, url, params, false)
        }
        NavCommand::Replace { name, url, params } => {
            push_like(PushLikeKind::Replace, name, url, params, false)
        }
        NavCommand::Reset { name, url, params } => {
            push_like(PushLikeKind::Reset, name, url, params, false)
        }
        NavCommand::Select { name, url, params } => {
            push_like(PushLikeKind::Select, name, url, params, false)
        }
        NavCommand::Pop => {
            let (popped_scope, new_depth) = {
                let mut state = inner.borrow_mut();
                let nav_state = state.navigators.get_mut(&nav_id).unwrap();
                let popped = nav_state.stack.pop();
                let _ = nav_state.stack_urls.pop();
                let depth = nav_state.stack.len();
                let urls_snapshot = nav_state.stack_urls.clone();
                let mirror = state.nav_state_mirror.clone();
                state.emit(Command::NavigatorPop {
                    navigator: nav_id,
                    count: 1,
                });
                // Update mirror outside inner borrow path.
                if let Ok(mut m) = mirror.lock() {
                    m.insert(nav_id.0, urls_snapshot);
                }
                (popped, depth)
            };
            if let Some(scope) = popped_scope {
                inner.borrow_mut().scope_to_navigator.remove(&scope);
                (cbs.release_screen)(scope);
            }
            (cbs.depth_changed)(new_depth);
        }
        NavCommand::OpenDrawer => {
            inner
                .borrow_mut()
                .emit(Command::OpenDrawer { navigator: nav_id });
        }
        NavCommand::CloseDrawer => {
            inner.borrow_mut().emit(Command::CloseDrawer {
                navigator: nav_id,
            });
        }
        NavCommand::ToggleDrawer => {
            inner.borrow_mut().emit(Command::ToggleDrawer {
                navigator: nav_id,
            });
        }
    }
}

#[derive(Copy, Clone)]
enum PushLikeKind {
    Push,
    Replace,
    Reset,
    Select,
}

fn screen_options_to_wire(
    state: &mut RecorderState,
    options: &framework_core::primitives::navigator::ScreenOptions,
) -> wire::WireScreenOptions {
    wire::WireScreenOptions {
        title: options.title.clone(),
        header_shown: options.header_shown,
        header_left: options
            .header_left
            .as_ref()
            .map(|btn| header_button_to_wire(state, btn, 0)),
        header_right: options
            .header_right
            .as_ref()
            .map(|btn| header_button_to_wire(state, btn, 1)),
    }
}

fn header_button_to_wire(
    state: &mut RecorderState,
    btn: &framework_core::primitives::navigator::HeaderButton,
    slot: u32,
) -> wire::WireHeaderButton {
    // Derive a stable identity for this header button from the
    // ambient walker identity (set by the caller — typically the
    // navigator's identity, since `screen_options_to_wire` runs
    // inside the `drawer_navigator_attach_initial` impl which is
    // called by the walker after restoring identity post-mount).
    // `slot` distinguishes left vs. right; the (parent, slot) pair
    // is what the rest of the identity tree uses, so the resulting
    // identity stays stable across hot-patches and the matching
    // `HandlerId` survives `clear_closures`.
    //
    // Hot-patch hamburger fix: the Android Toolbar's leaked
    // `HeaderButtonCallback` captured the original `HandlerId` at
    // install time and never updates. After a hot-patch the
    // server's freshly-walked button re-registers under the *same*
    // id (identity match) with the *new* `Rc<NavigatorControl>` in
    // its captured closure — so a tap still fires `ToggleDrawer`
    // on the live navigator instead of getting silently dropped.
    let identity = framework_core::Identity::node(
        framework_core::current_identity(),
        slot,
        None,
        None,
    );
    let handler = state
        .handlers
        .register_unit_for_identity(identity, btn.on_press.clone());
    wire::WireHeaderButton {
        icon: btn.icon.clone(),
        on_press: handler,
        tint: btn.tint.as_ref().map(|c| wire::WireColor(c.0.clone())),
    }
}

// ---------------------------------------------------------------------------
// App→Dev event dispatch helpers exposed on WireRecordingBackend.
// ---------------------------------------------------------------------------

impl WireRecordingBackend {
    /// Handle an `AppToDev::ScreenReleased { scope }` event arriving
    /// from the app side. Looks up which navigator owns the scope,
    /// pops it off the stack model, and invokes the framework's
    /// `release_screen` callback to drop the scope on dev.
    ///
    /// This is the path for *client-initiated* pops — iOS swipe-back
    /// or any platform back gesture that pops a screen without going
    /// through `NavCommand::Pop`. We mirror what the server-side Pop
    /// dispatch does: trim `stack_urls`, sync the snapshot mirror so
    /// the next rebuild-exec snapshot is accurate, and emit a
    /// `Command::NavigatorPop` into the append-only log so other
    /// connected clients (and any fresh client that reconnects)
    /// learn about the pop.
    pub fn handle_screen_released(&self, scope: u64) -> bool {
        let (cbs, new_depth, urls_snapshot, nav_id, mirror) = {
            let mut state = self.inner.borrow_mut();
            let Some(&nav_id) = state.scope_to_navigator.get(&scope) else {
                return false;
            };
            let Some(nav) = state.navigators.get_mut(&nav_id) else {
                return false;
            };
            nav.stack.retain(|&s| s != scope);
            // Stack navigators only release from the top, so the
            // popped url is the tail of stack_urls. If swap-style
            // releases ever land, this needs to be by-position.
            let _ = nav.stack_urls.pop();
            let depth = nav.stack.len();
            let urls = nav.stack_urls.clone();
            let cbs = nav.callbacks.clone();
            state.scope_to_navigator.remove(&scope);
            state.emit(Command::NavigatorPop {
                navigator: nav_id,
                count: 1,
            });
            let mirror = state.nav_state_mirror.clone();
            (cbs, depth, urls, nav_id, mirror)
        };
        if let Ok(mut m) = mirror.lock() {
            m.insert(nav_id.0, urls_snapshot);
        }
        (cbs.release_screen)(scope);
        (cbs.depth_changed)(new_depth);
        true
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
