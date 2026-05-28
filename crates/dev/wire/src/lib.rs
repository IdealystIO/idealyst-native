//! Dev-mode hot-reload wire protocol.
//!
//! The dev machine runs the user's component code in a normal Rust
//! process; a `WireRecordingBackend` translates each `Backend` trait
//! call the walker makes into a [`Command`] on the wire. The app's
//! `WireBackend<B>` replays those commands against its real
//! platform backend.
//!
//! Three id namespaces are minted on the dev side and held opaquely
//! on the app side:
//!
//! - [`NodeId`] — backend nodes (every `create_*` call mints one).
//! - [`HandlerId`] — closures (every primitive callback gets one).
//!   Most resolve back to dev-side closures via the reverse channel;
//!   GPU-bound callbacks resolve to app-local registered renderers.
//! - [`StyleId`] — pre-registered styles. The dev side ships the
//!   rule body once via [`Command::RegisterStyle`]; subsequent
//!   [`Command::ApplyStyle`]s reference by id.
//!
//! Everything in this crate is pure data — no `runtime-core`
//! dependency. Conversion to/from in-memory types lives in
//! `dev-client` (app side) and `dev-server` (dev
//! side).

#![deny(missing_debug_implementations)]

use serde::{Deserialize, Serialize};

/// Protocol version. Bumped on any breaking wire change. Dev/app
/// versions must match exactly — this is a dev-mode tool, so we don't
/// pay for backward compatibility.
///
/// Bump policy:
/// - Adding a new `Command` / `DevToApp` / `AppToDev` variant: BUMP.
///   serde uses external tagging, so an older peer would silently
///   drop-on-decode the new variant. Every new variant breaks
///   older-peer decode.
/// - Adding a `#[serde(default)]` field to an existing variant: no
///   bump required (defaulting fields ride through).
/// - Removing/renaming a variant or field: BUMP.
/// - Changing semantics of an existing variant (e.g. payload shape):
///   BUMP.
///
/// Bumped to 2 with the audit-driven additions: `CreateTextArea`,
/// `UpdateTextAreaValue`, `CreateExternal`, `NavigatorSelect`, plus
/// scene-model handling of theme tokens and drawer-open state.
///
/// Bumped to 3 to carry accessibility props end-to-end: every `Create*`
/// command grew an `a11y: WireAccessibilityProps` field, and two new
/// variants — `UpdateAccessibility` and `AnnounceForAccessibility` —
/// were added so the dev-server's `update_accessibility` /
/// `announce_for_accessibility` Backend calls reach the app side
/// instead of being dropped on the floor.
///
/// Bumped to 4 to carry `AccessibilityAction` handlers across the wire.
/// Previously `WireAccessibilityProps::actions` was `Vec<String>` —
/// only the names survived, and the replayer reconstructed actions with
/// no-op handlers. v4 changes the shape to
/// `Vec<WireAccessibilityAction { name, handler: HandlerId }>`, mirroring
/// the `on_click` / `on_change` trampoline pattern: the recorder mints a
/// `HandlerId` for each action and the replayer builds a closure that
/// posts `AppToDev::Event { handler, args: Unit }` back over the reverse
/// channel. Removes the documented gap in
/// `docs/accessibility-design.md`.
///
/// Bumped to 5 to carry per-client runtime-server sessions across the Hello
/// exchange. Sessions are entirely server-assigned — the client never
/// names one. Instead the client sends an `identity: ClientIdentity`
/// (platform + optional human-readable device label) that the server
/// uses for log lines and the future session-picker dev tool. The
/// server's `DevToApp::Hello.session: String` reports back the
/// server-minted id so the client can show it in dev tools.
///
/// "Multi vs. single session" is a server-side knob, not a client
/// preference: in the default *per-client* mode every connection gets
/// a fresh isolated session; in *shared* mode all connections land on
/// one common scene (the legacy collaborative-devices mode).
///
/// Bumped to 6 to carry per-frame animation writes. `AnimatedValue::bind`
/// subscribes a callback to the animation clock that calls
/// `backend.set_animated_f32`/`set_animated_color` each tick. In runtime-server
/// mode the sidecar runs that callback; without a wire path the
/// server-side `WireRecordingBackend` would fall through to the
/// trait-default no-op and the client would render the initial
/// (often `opacity: 0`) state forever. `SetAnimatedF32` and
/// `SetAnimatedColor` (plus their `WireAnimProp` discriminator)
/// shuttle each tick's value to the client, where the client's
/// `WireBackend::apply` dispatches to the wrapped backend's
/// `set_animated_f32`/`set_animated_color` — those have working
/// per-platform impls (DOM inline `style.transform` on web, CALayer
/// on iOS, etc.).
///
/// Bumped to 7 to flip the animation tick cadence from sidecar-self-
/// paced to **client-driven** via `AppToDev::RequestFrame { dt_ms }`.
/// Each client fires `RequestFrame` from its native raf; the dev side
/// runs one animation clock tick per request and ships the resulting
/// `SetAnimated*` commands back. Animations now stop when the tab is
/// backgrounded (browser throttles raf) and adapt to client framerate
/// — no wasted dev-host CPU on a quiet client.
pub const PROTOCOL_VERSION: u32 = 9;

/// Alias retained for code/docs that reference `WIRE_VERSION` rather
/// than the canonical [`PROTOCOL_VERSION`] name. Both point at the same
/// integer.
pub const WIRE_VERSION: u32 = PROTOCOL_VERSION;

// ---------------------------------------------------------------------------
// ID namespaces
// ---------------------------------------------------------------------------

macro_rules! define_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub u64);

        impl $name {
            pub const ZERO: Self = Self(0);

            pub fn next(self) -> Self {
                Self(self.0 + 1)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
    };
}

define_id!(NodeId, "Backend node identity. Minted by the dev side; opaque on the app side.");
define_id!(HandlerId, "Closure identity. Used for events and ops dispatching.");
define_id!(StyleId, "Pre-registered style identity.");
define_id!(StylesheetId, "Stylesheet identity for grouped registration.");
define_id!(ScopeId, "Per-screen / per-item framework scope. Minted by dev; used by app to request release.");
define_id!(AssetId, "Static asset identity (font / image / audio / video / blob).");
define_id!(TypefaceId, "Static typeface identity (a font family + a weight/style table).");

// ---------------------------------------------------------------------------
// Top-level message envelopes
// ---------------------------------------------------------------------------

/// Messages sent from the dev process to the app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DevToApp {
    /// Sent once on connection. Establishes protocol version and
    /// initial theme / stylesheet state.
    Hello {
        protocol_version: u32,
        theme: WireTheme,
        /// If this dev-server process was started as a self-restart
        /// after a source change, this is the ms-since-epoch
        /// timestamp captured *when the change was detected*. The
        /// app uses it to log the end-to-end "change → apply"
        /// latency. `None` on a cold start.
        #[serde(default)]
        rebuilt_at_ms: Option<u64>,
        /// Session id this client has been attached to. Echoes back
        /// the value the client passed in [`AppToDev::Hello::session`]
        /// (or the server-minted id when the client sent `None`). Same
        /// id passed by a second client puts both on a synced scene.
        /// Empty string is a sentinel for "this server build doesn't
        /// support sessions" — older sidecars/hosts that haven't grown
        /// session support yet send `""` here and the client should
        /// treat the connection as the legacy shared-state behavior.
        #[serde(default)]
        session: String,
    },

    /// A batch of backend commands to apply atomically. Batching
    /// matters: a single user-facing event often produces many
    /// walker calls (mounting a screen → 30+ create_/insert calls).
    Commands(Vec<Command>),

    /// Dev process is rebuilding the user crate. App freezes input
    /// dispatch and shows a subtle indicator. Followed by a fresh
    /// `Hello` once rebuild completes.
    Rebuilding,

    /// Dev process hit an error during rebuild / render. App displays
    /// the message; existing UI stays mounted but inert.
    Error { message: String },

    /// Theme swap. Includes re-resolved stylesheets so the app has
    /// updated styles before the dev side issues any new ApplyStyle.
    ThemeChanged {
        theme: WireTheme,
        styles: Vec<(StyleId, WireStyleRules)>,
    },
}

/// Messages sent from the app to the dev process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AppToDev {
    /// Sent once on connection.
    Hello {
        app_name: String,
        color_scheme: WireColorScheme,
        /// Web only: `Some(window.location.pathname)`. Lets the
        /// server reconcile its persisted nav stack with the URL
        /// the browser actually has after a reload. Native clients
        /// send `None`.
        #[serde(default)]
        initial_url: Option<String>,
        /// Self-description for the server's logs and the future
        /// session-picker dev tool. Never affects session assignment
        /// — that's purely a server-side decision (per-client vs
        /// shared, plus the assigned id). Clients should fill in as
        /// much as they reasonably know; the server tolerates default
        /// values for any field.
        #[serde(default)]
        identity: ClientIdentity,
        /// Initial viewport in CSS pixels (web: `window.innerWidth`
        /// / `innerHeight`; native: the root window's content size).
        /// The sidecar feeds this to `RecordingViewOps::frame(...)`
        /// so author code reading `page_ref.with(|h| h.frame())`
        /// gets a sensible answer per-session — without it, the
        /// recorder returns `None` and welcome's planet orbits fall
        /// back to a hardcoded 393×800 (wrong for any other
        /// browser size). `None` from older clients means "use the
        /// fallback".
        #[serde(default)]
        viewport: Option<WireViewport>,
    },

    /// A user-driven event fired against a registered handler.
    Event {
        handler: HandlerId,
        args: EventArgs,
    },

    /// State bit transition for a node previously registered via
    /// [`Command::AttachStates`].
    StateChanged {
        node: NodeId,
        bit: WireStateBit,
        on: bool,
    },

    /// Platform color scheme changed mid-session.
    ColorSchemeChanged { scheme: WireColorScheme },

    /// User popped a navigator screen via a platform gesture (swipe,
    /// back button). Dev framework drops the matching scope.
    ScreenReleased { scope: ScopeId },

    /// Navigator depth changed (after push/pop/swipe-back/reset). Dev
    /// framework uses this to keep its NavState mirror in sync.
    NavigatorDepthChanged { navigator: NodeId, depth: u32 },

    /// Drawer state flipped from a platform gesture.
    DrawerStateChanged { navigator: NodeId, is_open: bool },

    /// Tab activation triggered a lazy mount need.
    TabSelected { navigator: NodeId, index: u32 },

    /// Virtualizer needs an item index mounted.
    VirtualizerMountItem { virtualizer: NodeId, index: usize },

    /// Virtualizer is recycling an item.
    VirtualizerReleaseItem { scope: ScopeId },

    /// An item's measured size changed (Measured mode).
    VirtualizerMeasuredSize { scope: ScopeId, size: f32 },

    /// Request that the dev side advance its animation clock by
    /// `dt_ms` milliseconds and ship any resulting per-frame writes
    /// (`Command::SetAnimatedF32` / `SetAnimatedColor`) back. Cadence
    /// is **client-driven**: each browser/native client fires this
    /// from its own `requestAnimationFrame`-equivalent, supplying
    /// the wall-clock elapsed since its last frame. The dev side
    /// runs one `runtime_core::animation::clock` tick per request
    /// and emits the produced commands through the normal broadcast
    /// path.
    ///
    /// Client-driven (vs. sidecar-self-paced) buys two things:
    /// 1. The dev side stops ticking when the client's tab is
    ///    backgrounded or throttled — no wasted CPU on the dev host.
    /// 2. Frame production matches frame paint 1:1 — no drift from
    ///    independent clocks producing visible double-frames or
    ///    skipped frames.
    ///
    /// In shared-session mode multiple clients drive the same
    /// session's clock; the server dedups (only the first
    /// `RequestFrame` per ~16ms window actually ticks). In per-client
    /// mode every session has its own client, so no dedup needed.
    RequestFrame { dt_ms: u32 },

    /// Viewport changed (resize, orientation change, devtools toggle).
    /// Sidecar updates its per-session viewport so subsequent
    /// `RecordingViewOps::frame(...)` calls return the new size and
    /// raf-driven positioning math re-centres correctly. Web sends
    /// from a `resize` event listener; native sends on window /
    /// trait collection changes.
    ViewportChanged { width: f32, height: f32 },

    /// App-side error. Lets dev surface backend panics.
    Error { message: String },
}

/// Initial viewport in CSS pixels, attached to [`AppToDev::Hello`].
/// Same shape as a [`ViewportChanged`](AppToDev::ViewportChanged) but
/// inside the connection handshake so the sidecar has a sane viewport
/// from frame zero — without this, the first raf tick would compute
/// positions against the 393×800 fallback before the resize listener
/// has a chance to send a corrective `ViewportChanged`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WireViewport {
    pub width: f32,
    pub height: f32,
}

/// Self-description a client sends in [`AppToDev::Hello`]. The server
/// uses it to attach human-readable context to log lines and to
/// surface in a future session-picker dev tool. Session assignment
/// itself doesn't consult this struct — it's metadata, not protocol.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientIdentity {
    /// Platform the client is running on. Defaults to
    /// [`WirePlatform::Other`] when the client doesn't know or hasn't
    /// been updated to populate this.
    #[serde(default)]
    pub platform: WirePlatform,
    /// Free-form device label for display: "iPhone 15 Pro
    /// Simulator", "Pixel 9", "MacBook Air (M2)", "Chrome 132 on
    /// Linux". `None` is fine; the server falls back to
    /// `format!("{:?}", platform)` for log lines.
    #[serde(default)]
    pub device_label: Option<String>,
}

/// Closed enum mirror of the platforms that can host an runtime-server client.
/// Used for log/display only — never affects server logic. New
/// variants land here as new platforms get an runtime-server client; the
/// `#[serde(other)]` catch-all keeps older servers tolerant of newer
/// clients that name an unknown platform.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WirePlatform {
    Web,
    Ios,
    Android,
    MacOs,
    Linux,
    Windows,
    /// Catch-all for an unknown platform. Default for old clients
    /// that don't fill the field in.
    #[default]
    #[serde(other)]
    Other,
}

/// Argument bundle for a fired handler. Keeps the wire types small
/// without requiring per-handler argument-type tagging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventArgs {
    Unit,
    Bool(bool),
    Float(f32),
    String(String),
}

// ---------------------------------------------------------------------------
// Commands — the meat of the protocol.
// ---------------------------------------------------------------------------

/// One command corresponds to one (or one cluster of) `Backend` trait
/// method call(s) on the dev side. Variants are roughly grouped:
/// create_, insert, update_, apply_, release_, plus the navigator /
/// virtualizer / overlay control planes.
///
/// Add a variant when the framework adds a primitive or extends an
/// existing primitive's mutation surface. Wire bumps are cheap in
/// dev-mode-only land; both sides ship from the same commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    // --- Create commands: every primitive's `create_*` ---
    //
    // Every Create* variant carries an `a11y: WireAccessibilityProps`
    // so the app-side replayer can pass the matching
    // `AccessibilityProps` into the wrapped backend's `create_*`.
    // `#[serde(default)]` lets older recordings (which were emitted
    // before WIRE_VERSION=3) deserialize as `Default::default()`. New
    // emissions always carry the field.
    CreateView {
        id: NodeId,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateText {
        id: NodeId,
        content: String,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateButton {
        id: NodeId,
        label: String,
        on_click: HandlerId,
        leading_icon: Option<WireIconData>,
        trailing_icon: Option<WireIconData>,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreatePressable {
        id: NodeId,
        on_click: HandlerId,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateReactiveAnchor {
        id: NodeId,
    },
    CreateImage {
        id: NodeId,
        src: String,
        alt: Option<String>,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateIcon {
        id: NodeId,
        data: WireIconData,
        color: Option<WireColor>,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateTextInput {
        id: NodeId,
        initial_value: String,
        placeholder: Option<String>,
        on_change: HandlerId,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    /// Multi-line text-entry primitive — mirrors `CreateTextInput` but
    /// produces a backend node that renders newlines and (typically)
    /// resizes vertically. Key-intercept (`on_key_down`) plumbing isn't
    /// yet on the wire; framework drops it on the recorder side until
    /// the runtime-server protocol grows a key-event reverse channel.
    CreateTextArea {
        id: NodeId,
        initial_value: String,
        placeholder: Option<String>,
        on_change: HandlerId,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    /// Third-party `Element::External` node. Only `type_name` crosses
    /// the wire because the underlying `Rc<dyn Any>` props are arbitrary
    /// Rust types with no serialization contract. Clients that have
    /// registered an external factory under `type_name` may consult it
    /// to render a sensible default; clients that haven't render an
    /// empty placeholder so the tree stays well-formed.
    CreateExternal {
        id: NodeId,
        type_name: String,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateToggle {
        id: NodeId,
        initial_value: bool,
        on_change: HandlerId,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateSlider {
        id: NodeId,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: HandlerId,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateScrollView {
        id: NodeId,
        horizontal: bool,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateActivityIndicator {
        id: NodeId,
        size: WireActivityIndicatorSize,
        color: Option<WireColor>,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateLink {
        id: NodeId,
        route: String,
        url: String,
        kind: WireNavKind,
        on_activate: HandlerId,
        /// `true` ⇒ external (off-app) link. The replay client
        /// reconstructs it via `LinkConfig { external: true, .. }` so
        /// the web backend emits `<a target="_blank">`. `#[serde(default)]`
        /// keeps older payloads (no field) deserializing as in-app.
        #[serde(default)]
        external: bool,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreatePortal {
        id: NodeId,
        target: WirePortalTarget,
        on_dismiss: Option<HandlerId>,
        trap_focus: bool,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    /// GPU surface. The render closures are bound app-locally by
    /// name — the dev side carries no GPU code. This is the one
    /// place where the wire "handler" resolves to an app-side
    /// registration rather than a dev-side closure.
    CreateGraphics {
        id: NodeId,
        renderer: String,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateVirtualizer {
        id: NodeId,
        overscan: f32,
        horizontal: bool,
        initial_size: WireItemSize,
        initial_keys: Vec<u64>,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateNavigator {
        id: NodeId,
        initial_route: String,
        initial_path: String,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateTabNavigator {
        id: NodeId,
        initial_route: String,
        initial_path: String,
        tabs: Vec<WireTabRegistration>,
        placement: WireTabPlacement,
        mount_policy: WireMountPolicy,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },
    CreateDrawerNavigator {
        id: NodeId,
        initial_route: String,
        initial_path: String,
        side: WireDrawerSide,
        drawer_type: WireDrawerType,
        drawer_width: f32,
        swipe_to_open: bool,
        mount_policy: WireMountPolicy,
        #[serde(default)]
        a11y: WireAccessibilityProps,
    },

    // --- Tree mutation ---
    Insert {
        parent: NodeId,
        child: NodeId,
    },
    InsertMany {
        parent: NodeId,
        children: Vec<NodeId>,
    },
    ClearChildren {
        node: NodeId,
    },

    // --- Reactive updates ---
    UpdateText {
        node: NodeId,
        content: String,
    },
    UpdateButtonLabel {
        node: NodeId,
        label: String,
    },
    UpdateImageSrc {
        node: NodeId,
        src: String,
    },
    UpdateIconColor {
        node: NodeId,
        color: WireColor,
    },
    UpdateIconStroke {
        node: NodeId,
        progress: f32,
    },
    AnimateIconStroke {
        node: NodeId,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: WireEasing,
        infinite: bool,
        autoreverses: bool,
    },

    /// Per-frame animation tick — scalar family. Mirrors
    /// `Backend::set_animated_f32`. Emitted by the dev-side
    /// `WireRecordingBackend` every time the sidecar's animation
    /// clock fires an `AnimatedValue::bind` callback that writes a
    /// scalar (`Opacity`, `TranslateX`, `RotateZ`, `ZIndex`, …).
    /// Apply on the client by dispatching to the wrapped backend's
    /// `set_animated_f32(node, prop, value)`.
    ///
    /// High-frequency: 60fps × N animated props per scene. The wire
    /// can batch (multiple per `DevToApp::Commands` frame) but each
    /// individual tick still ships one variant — there's no
    /// further-coalescing within a single frame, since the prop +
    /// value tuple is already the minimum representable update.
    SetAnimatedF32 {
        node: NodeId,
        prop: WireAnimProp,
        value: f32,
    },

    /// Per-frame animation tick — color family. Mirrors
    /// `Backend::set_animated_color`. `value` is sRGB
    /// `[r, g, b, a]` with channels in `0..=1`. See
    /// [`Command::SetAnimatedF32`] for the surrounding contract.
    SetAnimatedColor {
        node: NodeId,
        prop: WireAnimProp,
        value: [f32; 4],
    },
    UpdateTextInputValue {
        node: NodeId,
        value: String,
    },
    UpdateTextAreaValue {
        node: NodeId,
        value: String,
    },
    UpdateToggleValue {
        node: NodeId,
        value: bool,
    },
    UpdateSliderValue {
        node: NodeId,
        value: f32,
    },
    SetDisabled {
        node: NodeId,
        disabled: bool,
    },

    // --- Styles ---
    /// Register a style under an id. Subsequent ApplyStyle commands
    /// reference by id. Sent before any ApplyStyle that uses it.
    RegisterStyle {
        id: StyleId,
        rules: WireStyleRules,
    },
    /// Drop a previously-registered style.
    UnregisterStyle {
        id: StyleId,
    },
    /// Apply a registered style to a node.
    ApplyStyle {
        node: NodeId,
        style: StyleId,
    },
    /// Apply a base style plus state overlays.
    ApplyStyledStates {
        node: NodeId,
        base: StyleId,
        overlays: Vec<(WireStateBit, StyleId)>,
    },
    /// Hook up native state events for the node (hover, press,
    /// focus). The app reports state transitions via
    /// [`AppToDev::StateChanged`].
    AttachStates {
        node: NodeId,
    },
    OnNodeUnstyled {
        node: NodeId,
    },

    // --- Presence transforms ---
    ApplyPresence {
        node: NodeId,
        state: WirePresenceState,
        transition: Option<(u32, WireEasing)>,
    },

    // --- Navigator control plane ---
    NavigatorAttachInitial {
        navigator: NodeId,
        screen: NodeId,
        scope: ScopeId,
        options: WireScreenOptions,
    },
    NavigatorPush {
        navigator: NodeId,
        screen: NodeId,
        scope: ScopeId,
        options: WireScreenOptions,
        /// URL of the screen being pushed. Drives `history.pushState`
        /// on web; informational on native.
        #[serde(default)]
        url: String,
        /// `true` when the server is rebuilding stack state after a
        /// reconnect (the screens were already there before the
        /// rebuild). Web backends MUST NOT call `history.pushState`
        /// in this case — the browser already has the URL and the
        /// history entries. Native backends ignore this flag.
        #[serde(default)]
        restore: bool,
    },
    NavigatorPop {
        navigator: NodeId,
        count: u32,
    },
    NavigatorReplace {
        navigator: NodeId,
        screen: NodeId,
        scope: ScopeId,
        options: WireScreenOptions,
        #[serde(default)]
        url: String,
        #[serde(default)]
        restore: bool,
    },
    NavigatorReset {
        navigator: NodeId,
        screen: NodeId,
        scope: ScopeId,
        options: WireScreenOptions,
        #[serde(default)]
        url: String,
        #[serde(default)]
        restore: bool,
    },
    /// Switch a tab/drawer navigator to a different mounted screen
    /// without tearing down the rest of the stack. Mirrors a
    /// stack-`Push` payload but the client side dispatches
    /// `NavCommand::Select` instead. Pre-fix this was conflated with
    /// `NavigatorReset`; SceneModel's snapshot would treat a tab swap
    /// as a full stack reset, dropping any persisted state on the same
    /// navigator. See the wire-protocol audit (`PushLikeKind::Select`
    /// masquerade).
    NavigatorSelect {
        navigator: NodeId,
        screen: NodeId,
        scope: ScopeId,
        options: WireScreenOptions,
        #[serde(default)]
        url: String,
    },
    /// Mount a lazy tab's content after the app reports activation.
    NavigatorMountTab {
        navigator: NodeId,
        index: u32,
        screen: NodeId,
        scope: ScopeId,
    },
    DrawerAttachSidebar {
        navigator: NodeId,
        sidebar: NodeId,
    },
    /// Attach a pre-built layout subtree to a navigator. Web-only
    /// semantically — the recording backend invokes the author's
    /// `.layout(...)` closure, emits the resulting subtree's
    /// `CreateView`/`Insert`/`ApplyStyle` commands like any other tree,
    /// then emits this command to tell the wire client which built
    /// node is the layout root (inserted into the navigator's
    /// container) and which node is the outlet (where subsequent
    /// screen attaches land). Backends that don't render through a
    /// layout (iOS/Android/Roku) treat this as a no-op.
    AttachNavigatorLayout {
        navigator: NodeId,
        root: NodeId,
        outlet: NodeId,
    },
    OpenDrawer {
        navigator: NodeId,
    },
    CloseDrawer {
        navigator: NodeId,
    },
    ToggleDrawer {
        navigator: NodeId,
    },
    ApplyNavigatorHeaderStyle {
        navigator: NodeId,
        style: StyleId,
    },
    ApplyNavigatorTitleStyle {
        navigator: NodeId,
        style: StyleId,
    },
    ApplyNavigatorButtonStyle {
        navigator: NodeId,
        style: StyleId,
    },
    ApplyNavigatorBodyStyle {
        navigator: NodeId,
        style: StyleId,
    },
    ApplyDrawerSidebarStyle {
        navigator: NodeId,
        style: StyleId,
    },
    ApplyDrawerScrimStyle {
        navigator: NodeId,
        style: StyleId,
    },
    ApplyTabBarStyle {
        navigator: NodeId,
        style: StyleId,
    },
    ApplyTabIconStyle {
        navigator: NodeId,
        style: StyleId,
    },
    ApplyTabLabelStyle {
        navigator: NodeId,
        style: StyleId,
    },

    // --- Virtualizer control plane ---
    VirtualizerDataChanged {
        node: NodeId,
        item_count: usize,
    },
    /// Reply to a `VirtualizerMountItem` request: attach the freshly
    /// built subtree at the given index.
    VirtualizerAttachItem {
        virtualizer: NodeId,
        index: usize,
        child: NodeId,
        scope: ScopeId,
    },

    // --- Lifecycle ---
    /// Terminal command emitted by the walker's final `finish(root)`
    /// call. App applies any remaining flush work and marks the
    /// initial mount complete.
    Finish {
        root: NodeId,
    },
    /// Release a node and any backend-side state. Used for navigator
    /// screens, virtualizer items, and overlays whose scopes have
    /// dropped on the dev side.
    ReleaseNode {
        node: NodeId,
    },
    /// Apply a freshly resolved theme's tokens. Backends with
    /// runtime variable stores (web's CSS custom properties) update
    /// in place; others ignore.
    InstallThemeVariables {
        tokens: Vec<WireTokenEntry>,
    },

    // --- Assets ---
    /// Register a static asset (font / image / etc.) under `id`. Sent
    /// before any command (typeface, style, image primitive) that
    /// references the asset. The app side caches by id; subsequent
    /// references resolve through the cache. Bytes ride inline for
    /// `WireAssetSource::Embedded`; `Bundled` paths and `Remote` URLs
    /// are resolved by the app-side backend.
    RegisterAsset {
        id: AssetId,
        kind: WireAssetTag,
        source: WireAssetSource,
    },
    /// Drop a previously-registered asset. Mirror of
    /// [`Command::UnregisterStyle`]; cache-bounded backends evict.
    UnregisterAsset {
        id: AssetId,
        kind: WireAssetTag,
    },
    /// Register a typeface (font family with a weight/style table).
    /// Sent after the per-face assets have been registered. Subsequent
    /// `ApplyStyle` commands reference the family by [`TypefaceId`]
    /// once the wire's `WireStyleRules` grows a `font_family` slot.
    RegisterTypeface {
        id: TypefaceId,
        family_name: String,
        faces: Vec<WireTypefaceFace>,
        fallback: WireSystemFallback,
    },
    UnregisterTypeface {
        id: TypefaceId,
    },

    // --- Accessibility ---
    /// Replace the accessibility prop bag on an existing node. Mirrors
    /// `Backend::update_accessibility(&node, &props, inferred_role)` —
    /// the walker's reactive a11y Effect emits this when any field of
    /// the resolved [`AccessibilityProps`] changes for a mounted node.
    ///
    /// `inferred_role` is the primitive's default role (the same value
    /// the walker computed via
    /// `runtime_core::accessibility::default_role`). Backends fall
    /// back to it when `a11y.role.is_none()`.
    ///
    /// [`AccessibilityProps`]: WireAccessibilityProps
    UpdateAccessibility {
        id: NodeId,
        a11y: WireAccessibilityProps,
        inferred_role: Option<WireRole>,
    },

    /// Imperative live-region announcement. Mirrors
    /// `Backend::announce_for_accessibility(msg, priority)` — no node
    /// target, just a one-shot post into the platform's AX subsystem.
    AnnounceForAccessibility {
        msg: String,
        priority: WireLiveRegionPriority,
    },
}

// ---------------------------------------------------------------------------
// Wire styles and theme
// ---------------------------------------------------------------------------

/// Subset of `runtime_core::StyleRules` carried over the wire.
///
/// **Prototype scope.** Only the fields the iOS / Android backends
/// look at most often are mirrored. Tokenized values are resolved on
/// the dev side before serialization — the wire only carries
/// concrete literals. Extending this is mechanical.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WireStyleRules {
    pub background: Option<WireColor>,
    pub color: Option<WireColor>,
    pub font_size: Option<WireLength>,

    pub flex_direction: Option<WireFlexDirection>,
    pub justify_content: Option<WireJustifyContent>,
    pub align_items: Option<WireAlignItems>,
    pub gap: Option<WireLength>,

    pub flex_grow: Option<f32>,
    pub flex_shrink: Option<f32>,
    pub flex_basis: Option<WireLength>,

    pub width: Option<WireLength>,
    pub height: Option<WireLength>,
    pub min_width: Option<WireLength>,
    pub min_height: Option<WireLength>,
    pub max_width: Option<WireLength>,
    pub max_height: Option<WireLength>,
    pub aspect_ratio: Option<f32>,

    pub padding_top: Option<WireLength>,
    pub padding_right: Option<WireLength>,
    pub padding_bottom: Option<WireLength>,
    pub padding_left: Option<WireLength>,
    pub margin_top: Option<WireLength>,
    pub margin_right: Option<WireLength>,
    pub margin_bottom: Option<WireLength>,
    pub margin_left: Option<WireLength>,

    pub border_top_left_radius: Option<WireLength>,
    pub border_top_right_radius: Option<WireLength>,
    pub border_bottom_left_radius: Option<WireLength>,
    pub border_bottom_right_radius: Option<WireLength>,

    pub opacity: Option<f32>,
    pub font_weight: Option<WireFontWeight>,
    pub font_family: Option<WireFontFamily>,
    pub text_align: Option<WireTextAlign>,

    // --- Position (added in PROTOCOL_VERSION 8). `#[serde(default)]`
    // so v7 sidecars/clients that omit these fields still decode. ---
    #[serde(default)]
    pub position: Option<WirePosition>,
    #[serde(default)]
    pub top: Option<WireLength>,
    #[serde(default)]
    pub right: Option<WireLength>,
    #[serde(default)]
    pub bottom: Option<WireLength>,
    #[serde(default)]
    pub left: Option<WireLength>,

    // --- Visual extras the welcome example (and most apps) need to
    // render correctly. Pre-v8 sidecars dropped these on the floor,
    // which is why runtime-server-hosted welcome showed only the headline text:
    // sun glare / vignette / planets all relied on background_gradient
    // + position:absolute + transform + overflow:hidden, none of which
    // crossed the wire. ---
    #[serde(default)]
    pub overflow: Option<WireOverflow>,
    #[serde(default)]
    pub transform: Option<Vec<WireTransform>>,
    #[serde(default)]
    pub background_gradient: Option<WireGradient>,
}

/// Wire mirror of `runtime_core::Position`. The mobile-flavored
/// subset — only the two positioning models that have consistent
/// semantics across UIKit, Android view system, and CSS.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WirePosition {
    Relative,
    Absolute,
    /// Scroll-relative pinning. Wire counterpart of
    /// `runtime_core::Position::Sticky`. Web clients honour this
    /// directly via CSS; native clients fall back to `Relative`
    /// until the per-platform scroll-listener implementation lands.
    Sticky,
}

/// Wire mirror of `runtime_core::Overflow`. Currently `Visible` /
/// `Hidden` only — matches the local enum.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireOverflow {
    Visible,
    Hidden,
}

/// Wire mirror of `runtime_core::Transform`. The stack of transform
/// operations applied to a view's layer/element in order — same
/// shape as React Native's `transform: [...]`. Each variant carries
/// only the parameters specific to that operation.
///
/// Translates carry [`WireLength`] (rather than raw f32) so the dev
/// side can emit `px` or `%` and the client picks the right CSS /
/// CALayer translation form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireTransform {
    TranslateX(WireLength),
    TranslateY(WireLength),
    Scale(f32),
    ScaleXY { x: f32, y: f32 },
    Rotate(f32),
    SkewX(f32),
    SkewY(f32),
}

/// Wire mirror of `runtime_core::Gradient`. See the local type for
/// per-backend mapping; on the wire we just ship the kind + the
/// pre-resolved color stops. The dev side has already run token
/// resolution against the active theme by the time this serializes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireGradient {
    pub kind: WireGradientKind,
    pub stops: Vec<WireGradientStop>,
}

/// Wire mirror of `runtime_core::GradientStop`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireGradientStop {
    pub offset: f32,
    pub color: WireColor,
}

/// Wire mirror of `runtime_core::GradientKind`. Linear gradients
/// carry an axis angle (CSS conventions: 0 = bottom→top, 90 =
/// left→right). Radials carry a normalized center, a radius
/// multiplier, and a reference-distance keyword.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireGradientKind {
    Linear {
        angle_deg: f32,
    },
    Radial {
        center: (f32, f32),
        radius: f32,
        extent: WireRadialExtent,
    },
}

/// Wire mirror of `runtime_core::RadialExtent`. Mirrors CSS's
/// `radial-gradient` extent keywords.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireRadialExtent {
    ClosestSide,
    FarthestCorner,
}

/// Wire mirror of `runtime_core::FontFamily`. The `Typeface`
/// variant carries both the [`TypefaceId`] (for identity / dedup
/// purposes against earlier `Command::RegisterTypeface`) and the
/// family name (so replay backends can emit `font-family: "name"`
/// without keeping a side table). The redundancy is small and keeps
/// the wire side stateless.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireFontFamily {
    /// CSS-style family-name string. Passed through verbatim — may
    /// contain a comma-separated fallback stack.
    System(String),
    /// Registered typeface, identified by id and named for direct
    /// `font-family` emission. The matching `Command::RegisterTypeface`
    /// has already been shipped earlier in the command stream.
    Typeface {
        id: TypefaceId,
        family_name: String,
    },
}

/// CSS color string ("#ff8800", "rgba(...)", "red"). Wire keeps it
/// as a string to dodge per-backend color parsing differences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireColor(pub String);

/// Length values are pre-resolved to one of three forms on the wire.
/// The dev side runs the token resolution against the active theme.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireLength {
    Px(f32),
    Pct(f32),
    /// CSS-style auto / intrinsic, where applicable.
    Auto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireFlexDirection {
    Row,
    Column,
    RowReverse,
    ColumnReverse,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireJustifyContent {
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireAlignItems {
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    Baseline,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireFontWeight {
    Thin,
    ExtraLight,
    Light,
    Regular,
    Medium,
    SemiBold,
    Bold,
    ExtraBold,
    Black,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireTextAlign {
    Left,
    Right,
    Center,
    Justify,
    Start,
    End,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireEasing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Cubic(f32, f32, f32, f32),
}

/// Wire mirror of `runtime_core::animation::AnimProp`. Variants
/// carrying a payload (the `GradientStopColor(u8)` stop index) embed
/// it inline. Scalar vs color split is implicit on the receiving end
/// via the carrying command — `SetAnimatedF32` always pairs with a
/// scalar variant, `SetAnimatedColor` always pairs with a color
/// variant. The client's `WireBackend` reconstructs the framework's
/// `AnimProp` from this and dispatches to the wrapped backend's
/// `set_animated_*` method.
///
/// New variants here track new entries in
/// `runtime_core::animation::AnimProp`. The `#[serde(other)]` arm
/// on `Unknown` keeps older peers tolerant of newer enumerants —
/// they'll drop the tick rather than abort the batch (the next tick's
/// value supersedes anyway, so a one-frame skip is invisible).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireAnimProp {
    // --- Scalar (f32) family ---
    Opacity,
    TranslateX,
    TranslateY,
    Scale,
    ScaleX,
    ScaleY,
    RotateZ,
    ZIndex,
    // --- Color ([f32; 4]) family ---
    BackgroundColor,
    ForegroundColor,
    /// Per-stop gradient color; stop index inline.
    GradientStopColor(u8),
    /// Catch-all for forward-compat — see type-level docs.
    #[serde(other)]
    Unknown,
}

/// Maps to `runtime_core::StateBits` flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WireStateBit {
    Hovered,
    Pressed,
    Focused,
    Disabled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireColorScheme {
    Light,
    Dark,
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireTheme {
    pub name: String,
    pub color_scheme: WireColorScheme,
    pub tokens: Vec<WireTokenEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireTokenEntry {
    pub name: String,
    pub value: WireTokenValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireTokenValue {
    Color(WireColor),
    Number(f32),
    Length(WireLength),
    String(String),
}

// ---------------------------------------------------------------------------
// Accessibility (a11y) wire mirror.
// ---------------------------------------------------------------------------

/// Wire mirror of `runtime_core::accessibility::AccessibilityProps`.
///
/// Every `Create*` command carries one of these so the app side can
/// pass the right `AccessibilityProps` into `Backend::create_*`. The
/// dev-server's recorder serializes from `&AccessibilityProps`; the
/// dev-client's replayer deserializes back into `AccessibilityProps`.
///
/// Actions ride the wire as [`WireAccessibilityAction`] entries — each
/// carries a `name` and a [`HandlerId`] trampoline. The recorder mints
/// a fresh `HandlerId` for every action and registers the in-memory
/// closure into its `HandlerTable`; the replayer reconstructs a
/// `Rc<dyn Fn()>` that posts `AppToDev::Event { handler, args: Unit }`
/// over the reverse channel exactly like an `on_click` handler. The
/// dispatch path is the same trampoline used by every other primitive
/// callback — assistive-technology triggers on the app side reach the
/// dev-side closure that was attached when the primitive was built.
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireAccessibilityProps {
    pub label: Option<String>,
    pub hint: Option<String>,
    pub identifier: Option<String>,
    pub hidden: bool,
    pub role: Option<WireRole>,
    /// Raw bits of `runtime_core::accessibility::AccessibilityTraits`.
    /// Reconstructed on the receiving side via
    /// `AccessibilityTraits::from_bits_truncate`.
    pub traits: u16,
    pub live_region: Option<WireLiveRegionPriority>,
    /// Custom AX actions exposed to assistive technology. Each entry
    /// carries the action's localized `name` plus a [`HandlerId`]
    /// trampoline; see [`WireAccessibilityAction`].
    pub actions: Vec<WireAccessibilityAction>,
}

/// Wire mirror of `runtime_core::accessibility::AccessibilityAction`.
/// Mirrors how `on_click` / `on_change` cross the wire: the recorder
/// allocates a [`HandlerId`] for the action's `Rc<dyn Fn()>` and the
/// replayer hands the backend a closure that posts
/// `AppToDev::Event { handler, args: Unit }` over the reverse channel.
/// When AT fires the action, that closure invocation dispatches
/// through the dev-side `HandlerTable` to the originally-captured
/// `Rc<dyn Fn()>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireAccessibilityAction {
    /// Localized name shown in the rotor / context menu
    /// ("Delete", "Archive", "Show details").
    pub name: String,
    /// Trampoline id; resolves back to the dev-side closure via
    /// `AppToDev::Event { handler, args: Unit }`.
    pub handler: HandlerId,
}

/// Wire mirror of `runtime_core::accessibility::Role`. Source `Role`
/// is `#[non_exhaustive]`; we mirror with a closed enum here, plus an
/// `Unknown` fallback so a newer-version peer's unrecognized variants
/// decode without aborting the batch. (Older peers that don't know a
/// freshly-added variant simply see `Unknown` on this side.)
#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum WireRole {
    // Structural
    Button,
    Link,
    Image,
    Text,
    Header,
    List,
    ListItem,
    Group,
    Separator,

    // Input
    TextField,
    TextArea,
    Switch,
    Slider,
    Checkbox,
    RadioButton,
    RadioGroup,
    ComboBox,
    SearchField,

    // Disclosure / navigation
    Tab,
    TabList,
    TabPanel,
    NavigationLink,
    MenuItem,
    Menu,
    MenuBar,
    Toolbar,

    // Feedback
    Alert,
    Status,
    ProgressBar,
    Spinner,

    // Container / overlay
    Dialog,
    AlertDialog,
    Drawer,
    Popover,
    Tooltip,
    Region,

    /// Catch-all for forward-compatibility — a newer-version dev side
    /// shipped a role this side doesn't know about. Backends translate
    /// `Unknown` as "no explicit role override; fall back to the
    /// primitive's inferred default" (same observable behavior as
    /// `role: None`).
    #[serde(other)]
    Unknown,
}

/// Wire mirror of `runtime_core::accessibility::LiveRegionPriority`.
#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum WireLiveRegionPriority {
    Polite,
    Assertive,
}

// ---------------------------------------------------------------------------
// Wire primitive-specific types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireIconData {
    pub view_box: (u16, u16),
    pub paths: Vec<String>,
    pub fill_rule: WireFillRule,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireFillRule {
    NonZero,
    EvenOdd,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireActivityIndicatorSize {
    Small,
    Large,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireNavKind {
    Push,
    Replace,
    Reset,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireMountPolicy {
    EagerPersistent,
    LazyPersistent,
    LazyDisposing,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WireScreenOptions {
    pub title: Option<String>,
    pub header_shown: Option<bool>,
    pub header_left: Option<WireHeaderButton>,
    pub header_right: Option<WireHeaderButton>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireHeaderButton {
    pub icon: String,
    pub on_press: HandlerId,
    pub tint: Option<WireColor>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireDrawerSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireDrawerType {
    Front,
    Back,
    Slide,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireTabPlacement {
    Bottom,
    Top,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireTabRegistration {
    pub route: String,
    pub label: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireItemSize {
    /// True for `ItemSize::Measured` — the backend should observe
    /// the mounted node's size and call back with the measurement.
    /// False for `ItemSize::Known` — the size is authoritative.
    pub measured: bool,
    /// Per-index sizes, pre-evaluated on the dev side at the time of
    /// the latest data snapshot. Indexed by item index.
    pub sizes: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WirePortalTarget {
    Viewport(WireViewportPlacement),
    Anchor {
        node: NodeId,
        side: WireElementSide,
        align: WireElementAlign,
        offset: f32,
    },
    Named(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireViewportPlacement {
    Center,
    Top,
    Bottom,
    Left,
    Right,
    FullScreen,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireElementSide {
    Above,
    Below,
    Start,
    End,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireElementAlign {
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct WirePresenceState {
    pub opacity: Option<f32>,
    pub tx: Option<f32>,
    pub ty: Option<f32>,
    pub scale: Option<f32>,
}

// ---------------------------------------------------------------------------
// Wire asset types
// ---------------------------------------------------------------------------

/// Wire mirror of `runtime_core::assets::AssetTag`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WireAssetTag {
    Font,
    Image,
    Audio,
    Video,
    Blob,
}

/// Wire mirror of `runtime_core::assets::AssetSource`. `Embedded`
/// carries bytes inline (base64-friendly via serde's default `Vec<u8>`
/// encoding — switch to the binary codec for production). `Bundled`
/// and `Remote` are pointer-only — the app-side backend resolves them
/// against whatever bundle / network it has.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireAssetSource {
    Embedded {
        bytes: Vec<u8>,
        extension: String,
    },
    Bundled {
        path: String,
    },
    Remote {
        url: String,
    },
}

/// Wire mirror of `runtime_core::assets::TypefaceFace`. The face's
/// asset has been registered separately via `Command::RegisterAsset`;
/// this struct only carries the cross-reference plus weight/style.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireTypefaceFace {
    pub weight: WireFontWeight,
    pub style: WireFontStyle,
    pub asset: AssetId,
}

/// Wire mirror of `runtime_core::style::FontStyle`. (Mirrors the
/// existing `WireFontWeight` shape — split out because the framework's
/// `FontStyle` enum is distinct from `FontWeight`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireFontStyle {
    Normal,
    Italic,
}

/// Wire mirror of `runtime_core::assets::SystemFallback`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireSystemFallback {
    Serif,
    SansSerif,
    Monospace,
    None,
}

// ---------------------------------------------------------------------------
// Frame format
// ---------------------------------------------------------------------------

/// Serialize/deserialize helpers for one-shot encoding of a message.
/// Length-prefixed binary frames over WebSocket / TCP / pipe should
/// be straightforward to layer on top.
pub mod codec {
    use super::*;

    /// Encode any wire message to JSON bytes (prototype). Swap for
    /// CBOR/bincode/postcard when going to real transport.
    pub fn encode<T: Serialize>(msg: &T) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec(msg)
    }

    pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> serde_json::Result<T> {
        serde_json::from_slice(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: Serialize + for<'de> Deserialize<'de>>(value: &T) -> serde_json::Value {
        let bytes = codec::encode(value).expect("encode");
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("decode");
        // Re-encode and parse the actual T to ensure decode succeeds
        // against the strongly-typed schema as well.
        let _: T = codec::decode(&bytes).expect("decode strong-typed");
        decoded
    }

    #[test]
    fn register_asset_bundled_roundtrip() {
        let cmd = Command::RegisterAsset {
            id: AssetId(42),
            kind: WireAssetTag::Font,
            source: WireAssetSource::Bundled {
                path: "fonts/Inter-Regular.ttf".to_string(),
            },
        };
        let bytes = codec::encode(&cmd).expect("encode");
        let decoded: Command = codec::decode(&bytes).expect("decode");
        match decoded {
            Command::RegisterAsset { id, kind, source } => {
                assert_eq!(id, AssetId(42));
                assert_eq!(kind, WireAssetTag::Font);
                match source {
                    WireAssetSource::Bundled { path } => {
                        assert_eq!(path, "fonts/Inter-Regular.ttf");
                    }
                    _ => panic!("expected Bundled"),
                }
            }
            _ => panic!("expected RegisterAsset variant"),
        }
    }

    #[test]
    fn register_asset_embedded_preserves_bytes() {
        let cmd = Command::RegisterAsset {
            id: AssetId(7),
            kind: WireAssetTag::Image,
            source: WireAssetSource::Embedded {
                bytes: vec![0xDE, 0xAD, 0xBE, 0xEF],
                extension: "png".to_string(),
            },
        };
        let bytes = codec::encode(&cmd).expect("encode");
        let decoded: Command = codec::decode(&bytes).expect("decode");
        match decoded {
            Command::RegisterAsset {
                source:
                    WireAssetSource::Embedded { bytes, extension },
                ..
            } => {
                assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);
                assert_eq!(extension, "png");
            }
            _ => panic!("expected Embedded RegisterAsset"),
        }
    }

    #[test]
    fn register_typeface_carries_faces() {
        let cmd = Command::RegisterTypeface {
            id: TypefaceId(99),
            family_name: "Inter".to_string(),
            faces: vec![
                WireTypefaceFace {
                    weight: WireFontWeight::Regular,
                    style: WireFontStyle::Normal,
                    asset: AssetId(1),
                },
                WireTypefaceFace {
                    weight: WireFontWeight::Bold,
                    style: WireFontStyle::Italic,
                    asset: AssetId(2),
                },
            ],
            fallback: WireSystemFallback::SansSerif,
        };
        let bytes = codec::encode(&cmd).expect("encode");
        let decoded: Command = codec::decode(&bytes).expect("decode");
        match decoded {
            Command::RegisterTypeface {
                id,
                family_name,
                faces,
                fallback,
            } => {
                assert_eq!(id, TypefaceId(99));
                assert_eq!(family_name, "Inter");
                assert_eq!(faces.len(), 2);
                assert_eq!(faces[1].asset, AssetId(2));
                assert!(matches!(faces[1].style, WireFontStyle::Italic));
                assert!(matches!(fallback, WireSystemFallback::SansSerif));
            }
            _ => panic!("expected RegisterTypeface"),
        }
    }

    #[test]
    fn wire_font_family_typeface_carries_id_and_name() {
        let ff = WireFontFamily::Typeface {
            id: TypefaceId(123),
            family_name: "Inter".to_string(),
        };
        let v = roundtrip(&ff);
        // JSON shape sanity-check — the enum is tagged with the
        // variant name and carries the two fields. `TypefaceId` is a
        // tuple struct with one field, which serde-json flattens to
        // a bare number.
        let obj = v.get("Typeface").expect("Typeface variant");
        assert_eq!(obj.get("id").and_then(|n| n.as_u64()), Some(123));
        assert_eq!(obj.get("family_name").and_then(|s| s.as_str()), Some("Inter"));
    }

    #[test]
    fn wire_font_family_system_passes_through() {
        let ff = WireFontFamily::System("ui-monospace, Menlo, monospace".to_string());
        let bytes = codec::encode(&ff).expect("encode");
        let decoded: WireFontFamily = codec::decode(&bytes).expect("decode");
        match decoded {
            WireFontFamily::System(s) => {
                assert_eq!(s, "ui-monospace, Menlo, monospace");
            }
            _ => panic!("expected System"),
        }
    }

    #[test]
    fn wire_style_rules_carries_font_family() {
        let rules = WireStyleRules {
            font_family: Some(WireFontFamily::Typeface {
                id: TypefaceId(5),
                family_name: "Inter".to_string(),
            }),
            font_weight: Some(WireFontWeight::Bold),
            ..Default::default()
        };
        let bytes = codec::encode(&rules).expect("encode");
        let decoded: WireStyleRules = codec::decode(&bytes).expect("decode");
        match decoded.font_family {
            Some(WireFontFamily::Typeface { id, family_name }) => {
                assert_eq!(id, TypefaceId(5));
                assert_eq!(family_name, "Inter");
            }
            _ => panic!("expected Typeface variant"),
        }
        assert!(matches!(decoded.font_weight, Some(WireFontWeight::Bold)));
    }

    #[test]
    fn app_hello_identity_roundtrips() {
        // Default identity — old clients that don't populate the
        // field decode cleanly via `#[serde(default)]`.
        let h = AppToDev::Hello {
            app_name: "x".into(),
            color_scheme: WireColorScheme::Auto,
            initial_url: None,
            identity: ClientIdentity::default(),
        };
        let bytes = codec::encode(&h).unwrap();
        match codec::decode::<AppToDev>(&bytes).unwrap() {
            AppToDev::Hello { identity, .. } => {
                assert_eq!(identity.platform, WirePlatform::Other);
                assert!(identity.device_label.is_none());
            }
            _ => panic!("expected Hello"),
        }

        // Populated identity.
        let h = AppToDev::Hello {
            app_name: "x".into(),
            color_scheme: WireColorScheme::Auto,
            initial_url: None,
            identity: ClientIdentity {
                platform: WirePlatform::Ios,
                device_label: Some("iPhone 15 Pro Sim".into()),
            },
        };
        let bytes = codec::encode(&h).unwrap();
        match codec::decode::<AppToDev>(&bytes).unwrap() {
            AppToDev::Hello { identity, .. } => {
                assert_eq!(identity.platform, WirePlatform::Ios);
                assert_eq!(identity.device_label.as_deref(), Some("iPhone 15 Pro Sim"));
            }
            _ => panic!("expected Hello"),
        }
    }

    #[test]
    fn app_hello_legacy_decode_identity_defaults() {
        // Pre-v5 clients sent no `identity` field — must decode
        // as default.
        let bytes = serde_json::to_vec(&serde_json::json!({
            "Hello": {
                "app_name": "x",
                "color_scheme": "Auto"
            }
        }))
        .unwrap();
        match codec::decode::<AppToDev>(&bytes).unwrap() {
            AppToDev::Hello { identity, .. } => {
                assert_eq!(identity.platform, WirePlatform::Other);
            }
            _ => panic!("expected Hello"),
        }
    }

    #[test]
    fn wire_platform_unknown_decodes_as_other() {
        // Future clients may name platforms we don't know — should
        // decode as Other rather than fail the whole batch.
        let bytes = b"\"WeirdPlatformFromTheFuture\"";
        let p: WirePlatform = codec::decode(bytes).unwrap();
        assert_eq!(p, WirePlatform::Other);
    }

    #[test]
    fn dev_hello_session_field_roundtrips() {
        let h = DevToApp::Hello {
            protocol_version: PROTOCOL_VERSION,
            theme: WireTheme {
                name: "t".into(),
                color_scheme: WireColorScheme::Auto,
                tokens: Vec::new(),
            },
            rebuilt_at_ms: None,
            session: "s_abc123".into(),
        };
        let bytes = codec::encode(&h).unwrap();
        match codec::decode::<DevToApp>(&bytes).unwrap() {
            DevToApp::Hello { session, .. } => assert_eq!(session, "s_abc123"),
            _ => panic!("expected Hello"),
        }
    }

    #[test]
    fn dev_hello_legacy_decode_session_defaults_empty() {
        // Older sidecar (pre-v5) emitted no `session` field. The
        // `#[serde(default)]` should fill it with "".
        let bytes = serde_json::to_vec(&serde_json::json!({
            "Hello": {
                "protocol_version": PROTOCOL_VERSION,
                "theme": { "name": "t", "color_scheme": "Auto", "tokens": [] },
            }
        }))
        .unwrap();
        match codec::decode::<DevToApp>(&bytes).unwrap() {
            DevToApp::Hello { session, .. } => assert_eq!(session, ""),
            _ => panic!("expected Hello"),
        }
    }

    #[test]
    fn unregister_commands_roundtrip() {
        let a = Command::UnregisterAsset {
            id: AssetId(1),
            kind: WireAssetTag::Audio,
        };
        let t = Command::UnregisterTypeface { id: TypefaceId(2) };
        let _: Command = codec::decode(&codec::encode(&a).unwrap()).unwrap();
        let _: Command = codec::decode(&codec::encode(&t).unwrap()).unwrap();
    }
}

