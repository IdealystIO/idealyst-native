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
//! Everything in this crate is pure data — no `framework-core`
//! dependency. Conversion to/from in-memory types lives in
//! `dev-client` (app side) and `dev-server` (dev
//! side).

#![deny(missing_debug_implementations)]

use serde::{Deserialize, Serialize};

/// Protocol version. Bumped on any breaking wire change. Dev/app
/// versions must match exactly — this is a dev-mode tool, so we don't
/// pay for backward compatibility.
pub const PROTOCOL_VERSION: u32 = 1;

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

    /// App-side error. Lets dev surface backend panics.
    Error { message: String },
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
    CreateView {
        id: NodeId,
    },
    CreateText {
        id: NodeId,
        content: String,
    },
    CreateButton {
        id: NodeId,
        label: String,
        on_click: HandlerId,
        leading_icon: Option<WireIconData>,
        trailing_icon: Option<WireIconData>,
    },
    CreatePressable {
        id: NodeId,
        on_click: HandlerId,
    },
    CreateReactiveAnchor {
        id: NodeId,
    },
    CreateImage {
        id: NodeId,
        src: String,
        alt: Option<String>,
    },
    CreateIcon {
        id: NodeId,
        data: WireIconData,
        color: Option<WireColor>,
    },
    CreateTextInput {
        id: NodeId,
        initial_value: String,
        placeholder: Option<String>,
        on_change: HandlerId,
    },
    CreateToggle {
        id: NodeId,
        initial_value: bool,
        on_change: HandlerId,
    },
    CreateSlider {
        id: NodeId,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: HandlerId,
    },
    CreateScrollView {
        id: NodeId,
        horizontal: bool,
    },
    CreateWebView {
        id: NodeId,
        url: String,
    },
    CreateVideo {
        id: NodeId,
        src: String,
        autoplay: bool,
        controls: bool,
        loop_playback: bool,
    },
    CreateActivityIndicator {
        id: NodeId,
        size: WireActivityIndicatorSize,
        color: Option<WireColor>,
    },
    CreateLink {
        id: NodeId,
        route: String,
        url: String,
        kind: WireNavKind,
        on_activate: HandlerId,
    },
    CreatePortal {
        id: NodeId,
        target: WirePortalTarget,
        on_dismiss: Option<HandlerId>,
        trap_focus: bool,
    },
    /// GPU surface. The render closures are bound app-locally by
    /// name — the dev side carries no GPU code. This is the one
    /// place where the wire "handler" resolves to an app-side
    /// registration rather than a dev-side closure.
    CreateGraphics {
        id: NodeId,
        renderer: String,
    },
    CreateVirtualizer {
        id: NodeId,
        overscan: f32,
        horizontal: bool,
        initial_size: WireItemSize,
        initial_keys: Vec<u64>,
    },
    CreateNavigator {
        id: NodeId,
        initial_route: String,
        initial_path: String,
    },
    CreateTabNavigator {
        id: NodeId,
        initial_route: String,
        initial_path: String,
        tabs: Vec<WireTabRegistration>,
        placement: WireTabPlacement,
        mount_policy: WireMountPolicy,
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
    UpdateTextInputValue {
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
    UpdateWebViewUrl {
        node: NodeId,
        url: String,
    },
    UpdateVideoSrc {
        node: NodeId,
        src: String,
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
}

// ---------------------------------------------------------------------------
// Wire styles and theme
// ---------------------------------------------------------------------------

/// Subset of `framework_core::StyleRules` carried over the wire.
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
}

/// Wire mirror of `framework_core::FontFamily`. The `Typeface`
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

/// Maps to `framework_core::StateBits` flags.
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

/// Wire mirror of `framework_core::assets::AssetTag`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WireAssetTag {
    Font,
    Image,
    Audio,
    Video,
    Blob,
}

/// Wire mirror of `framework_core::assets::AssetSource`. `Embedded`
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

/// Wire mirror of `framework_core::assets::TypefaceFace`. The face's
/// asset has been registered separately via `Command::RegisterAsset`;
/// this struct only carries the cross-reference plus weight/style.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireTypefaceFace {
    pub weight: WireFontWeight,
    pub style: WireFontStyle,
    pub asset: AssetId,
}

/// Wire mirror of `framework_core::style::FontStyle`. (Mirrors the
/// existing `WireFontWeight` shape — split out because the framework's
/// `FontStyle` enum is distinct from `FontWeight`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireFontStyle {
    Normal,
    Italic,
}

/// Wire mirror of `framework_core::assets::SystemFallback`.
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

