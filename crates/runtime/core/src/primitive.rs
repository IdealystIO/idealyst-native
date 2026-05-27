//! The `Primitive` enum — the structural skeleton of the UI.
//!
//! Every primitive optionally carries a `style` slot — styling is
//! orthogonal to structure, so authors can style any primitive
//! without each primitive having to know about styling. The renderer
//! applies the style via an independent `Effect` per primitive, so a
//! content signal change doesn't re-fire the style effect and vice
//! versa.

use crate::accessibility::AccessibilityProps;
use crate::handles::RefFill;
use crate::primitives;
use crate::sources::{IntoStyleSource, StyleSource, TextSource};
use crate::style::Color;
use crate::Signal;
use std::any::Any;
use std::rc::Rc;

/// Primitives are the structural skeleton of the UI. Every primitive
/// optionally carries a `style` slot — styling is orthogonal to
/// structure, so authors can style any primitive without each primitive
/// having to know about styling. The renderer applies the style via an
/// independent `Effect` per primitive, so a content signal change
/// doesn't re-fire the style effect and vice versa.
pub enum Primitive {
    View {
        children: Vec<Primitive>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        /// Per-side opt-in for safe-area padding. `NONE` means the
        /// view ignores system insets (the default). When non-zero,
        /// the backend adds the platform's safe-area inset to the
        /// matching side of the view's padding, reactively — orientation
        /// flips and dynamic-island changes propagate without a rebuild.
        /// See [`crate::SafeAreaSides`].
        safe_area_sides: crate::SafeAreaSides,
        /// Optional raw-touch handler. When `Some`, the framework
        /// asks the backend to deliver every touch event hitting this
        /// view (or bubbling up from a descendant whose handler
        /// returned `consumed: false`) into the closure. See
        /// [`crate::touch`] for the event model and the claim
        /// protocol.
        on_touch: Option<crate::TouchHandler>,
        /// Accessibility prop bag — label, role override, traits,
        /// hint, etc. Default is `AccessibilityProps::default()` which
        /// tells the backend "infer everything from the primitive type."
        /// See [`crate::accessibility`] for the model.
        accessibility: AccessibilityProps,
        #[cfg(feature = "robot")]
        test_id: Option<&'static str>,
    },
    Text {
        source: TextSource,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
        #[cfg(feature = "robot")]
        test_id: Option<&'static str>,
    },
    Button {
        /// Label source. `TextSource::Static` for a fixed string;
        /// `TextSource::Reactive` for a closure that reads signals
        /// and produces a fresh label string on each fire. The
        /// walker installs an Effect on the latter so the native
        /// widget's text updates when the underlying signals change.
        label: TextSource,
        /// Press handler. Carries both a runtime callable and the
        /// structured metadata (method name + input signal ids +
        /// optional output signal) generator backends need to ship
        /// the handler to the device.
        on_click: crate::derive::Action,
        /// Icon rendered before the label (left in LTR layouts).
        /// Backends render this natively: `UIButton.setImage` on iOS,
        /// compound drawable on Android, inline SVG on web.
        leading_icon: Option<primitives::icon::IconData>,
        /// Icon rendered after the label (right in LTR layouts).
        trailing_icon: Option<primitives::icon::IconData>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        /// Optional reactive disabled flag. When the closure returns
        /// true, the framework: (1) flips the `DISABLED` state bit on
        /// the styled node so any `state disabled { ... }` overlay
        /// applies, (2) tells the backend to mark the native widget
        /// inert (`disabled` attr on web, `setEnabled(false)` on
        /// native). The closure is wrapped in an `Effect` so changes
        /// propagate automatically.
        disabled: Option<Box<dyn Fn() -> bool>>,
        accessibility: AccessibilityProps,
        #[cfg(feature = "robot")]
        test_id: Option<&'static str>,
    },
    /// Clickable container — like [`Primitive::View`] but with a
    /// press callback. Renders to a tappable native control whose
    /// visual is entirely supplied by `children` and `style`. No
    /// UA chrome (no `<button>` border on web, no
    /// `UIButton` system styling on iOS) — backends create a bare
    /// container with a click/tap recognizer attached.
    ///
    /// Use when you want button *behavior* without button *visuals*:
    /// custom-styled buttons whose look is owned by the stylesheet,
    /// option rows in a menu, tappable card surfaces. For a plain
    /// label-only button with native semantics (form submission,
    /// default focus ring, etc.) use [`Primitive::Button`].
    ///
    /// The state machinery (`state hovered`, `state pressed`,
    /// `state focused`, `state disabled`) works just like on any
    /// other styled primitive.
    Pressable {
        children: Vec<Primitive>,
        on_click: Rc<dyn Fn()>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        /// Same semantics as [`Primitive::Button::disabled`].
        disabled: Option<Box<dyn Fn() -> bool>>,
        accessibility: AccessibilityProps,
        #[cfg(feature = "robot")]
        test_id: Option<&'static str>,
    },
    /// Image primitive. Source is reactive (`Box<dyn Fn() -> String>`)
    /// so authors can pass a static URL or a closure reading a signal.
    ///
    /// When constructed via [`image_asset`](primitives::image::image_asset),
    /// `asset` carries the declared [`Asset<kinds::Image>`](crate::assets::Asset)
    /// so the walker can register it with the backend (and over the
    /// wire) before `create_image` runs. In that case `src()` returns
    /// the sentinel `"asset://{id}"`; the backend's `create_image`
    /// recognizes the prefix and substitutes its locally-resolved URL.
    Image {
        src: Box<dyn Fn() -> String>,
        /// Optional accessibility label. Maps to `alt` on web,
        /// `accessibilityLabel` on iOS, `contentDescription` on Android.
        ///
        /// **Note**: this field predates the cross-primitive
        /// `accessibility` prop bag. Backend impls read `alt` as a
        /// shortcut for the a11y label; if `accessibility.label` is
        /// also set, the explicit `accessibility.label` wins. New code
        /// should prefer `accessibility.label` for consistency with
        /// other primitives.
        alt: Option<String>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        /// `Some` when the source is an [`Asset`](crate::assets::Asset)
        /// rather than a free-form URL. Drives `Backend::register_asset`
        /// just before `Backend::create_image`.
        asset: Option<crate::assets::Asset<crate::assets::kinds::Image>>,
        accessibility: AccessibilityProps,
        #[cfg(feature = "robot")]
        test_id: Option<&'static str>,
    },
    /// Vector icon primitive. Renders static `IconData` path strings
    /// as an inline SVG on web, `CAShapeLayer` on iOS, `VectorDrawable`
    /// on Android. Only icons referenced by application code end up in
    /// the binary — the linker drops unreferenced `IconData` constants.
    ///
    /// Supports stroke-draw animations: the path progressively reveals
    /// itself, driven either by a reactive `stroke` closure (0.0–1.0)
    /// or a fire-once `draw_in` animation on mount.
    Icon {
        data: primitives::icon::IconData,
        /// Optional reactive color override. `None` means inherit
        /// (currentColor on web, label color on native).
        color: Option<Box<dyn Fn() -> crate::style::Color>>,
        /// Reactive stroke progress (0.0 = nothing drawn, 1.0 = full).
        /// When `Some`, the walker installs an Effect that calls
        /// `update_icon_stroke` on the backend.
        stroke: Option<Box<dyn Fn() -> f32>>,
        /// Mount animation: draw the stroke from→to over duration.
        /// Applied once after creation via `animate_icon_stroke`.
        draw_in: Option<primitives::icon::StrokeAnimation>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
    },
    /// Controlled text input. The parent owns the value as a
    /// `Signal<String>`; on every native input event the framework
    /// fires `on_change` with the new text, the parent updates the
    /// signal, the framework's effect re-fires and writes the new
    /// value back to the native widget. Cyclic but stable — widgets
    /// no-op when set to their current value.
    TextInput {
        value: Signal<String>,
        on_change: Rc<dyn Fn(String)>,
        /// Pre-default-action keyboard hook. Fires on every keydown
        /// while the input has focus; returning
        /// [`KeyOutcome::PreventDefault`] suppresses the platform's
        /// default behaviour for that key (typing the character,
        /// focus-traversal on Tab, submit on Enter, …). See
        /// [`primitives::key`](crate::primitives::key) for the
        /// cross-platform contract.
        on_key_down: Option<Rc<dyn Fn(&crate::primitives::key::KeyEvent) -> crate::primitives::key::KeyOutcome>>,
        placeholder: Option<String>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
        #[cfg(feature = "robot")]
        test_id: Option<&'static str>,
    },
    /// Controlled multi-line text editor — same controlled pattern
    /// as `TextInput`, but the native widget accepts newlines. Web:
    /// `<textarea>`. iOS: `UITextView`. Android: `EditText` with
    /// `inputType="textMultiLine"`. The wgpu render backend currently
    /// renders an Unsupported placeholder; a native multi-line editor
    /// on that side is a follow-up.
    TextArea {
        value: Signal<String>,
        on_change: Rc<dyn Fn(String)>,
        /// Pre-default-action keyboard hook. See
        /// [`Primitive::TextInput::on_key_down`] for semantics — the
        /// surface is identical between the two primitives.
        on_key_down: Option<Rc<dyn Fn(&crate::primitives::key::KeyEvent) -> crate::primitives::key::KeyOutcome>>,
        placeholder: Option<String>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
        #[cfg(feature = "robot")]
        test_id: Option<&'static str>,
    },
    /// Controlled toggle (switch / checkbox). Same controlled
    /// pattern as `TextInput`: `value: Signal<bool>` round-trips
    /// through `on_change`.
    Toggle {
        value: Signal<bool>,
        on_change: Rc<dyn Fn(bool)>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
        #[cfg(feature = "robot")]
        test_id: Option<&'static str>,
    },
    /// Scroll container. Children scroll along `horizontal`'s opposite
    /// axis (vertical by default). Web: a div with `overflow: scroll`.
    /// iOS: `UIScrollView`. Android: `ScrollView` or
    /// `HorizontalScrollView`.
    ScrollView {
        children: Vec<Primitive>,
        horizontal: bool,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        /// Per-side opt-in for safe-area padding — same semantics as
        /// `View::safe_area_sides`. Common use: a vertical scroll
        /// view at the screen root opts into `TOP | BOTTOM` so
        /// scrolling content can pass under the status bar / home
        /// indicator while header/footer rows respect the inset.
        safe_area_sides: crate::SafeAreaSides,
        /// Optional callback fired every time the user (or programmatic
        /// `scroll_to`) changes the scroll offset. Arguments are
        /// `(scroll_left_px, scroll_top_px)` in CSS pixels / native
        /// points — uniform across every backend. Backends bind this
        /// to their native scroll observer (web `scroll` event, iOS
        /// `UIScrollViewDelegate::scrollViewDidScroll`, Android
        /// `OnScrollChangeListener`, etc.).
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        accessibility: AccessibilityProps,
    },
    /// Controlled numeric slider. Like `TextInput`/`Toggle`, the parent
    /// owns the value signal. If `step` is set, the framework snaps
    /// the incoming `on_change` value to the nearest step before
    /// dispatching — so behavior is identical across web (which clamps
    /// natively), iOS (no native step), and Android.
    Slider {
        value: Signal<f32>,
        on_change: Rc<dyn Fn(f32)>,
        min: f32,
        max: f32,
        step: Option<f32>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
        #[cfg(feature = "robot")]
        test_id: Option<&'static str>,
    },
    /// Indeterminate loading spinner. No methods — passive widget.
    ActivityIndicator {
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<Color>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
    },
    /// Virtualized list. Runtime backends consume the closures
    /// (`render_item` / `item_count.compute` / `item_key`) and
    /// drive their native virtualization widget; generator
    /// backends (Roku) consume the structured metadata
    /// (`item_count` as a `Derived<usize>` + the pre-built
    /// `row_template` with `row_index_signal_id` for per-row
    /// remapping) and emit a wire op the device-side runtime
    /// realizes against `MarkupList` / `RowList` / similar.
    ///
    /// The `flat_list<T>(...)` wrapper in `primitives::flat_list`
    /// is the author-facing typed entry point.
    Virtualizer {
        /// Reactive item count. Generator backends use the
        /// structured form (`method` + `inputs`); runtime backends
        /// call `compute` inside an Effect.
        item_count: crate::derive::Derived<usize>,
        item_key: Box<dyn Fn(usize) -> primitives::virtualizer::ItemKey>,
        item_size: primitives::virtualizer::ItemSize,
        /// Closure for runtime backends to materialize a row at a
        /// given index. Generator backends ignore this; they use
        /// `row_template` instead.
        render_item: Rc<dyn Fn(usize) -> Primitive>,
        /// Pre-built row produced by calling `render_item` once at
        /// snapshot time. Generator backends serialize this and
        /// remap node ids per row instance on the device.
        /// `None` when the constructor came in through the legacy
        /// closure-only path — generator backends report a
        /// build-time error if they encounter a Virtualizer
        /// without one.
        row_template: Option<Box<Primitive>>,
        /// Snapshot-time signal id that `render_item`'s closure
        /// captured as its row-index signal. Generator backends
        /// use this to mint a fresh synthetic per-row signal and
        /// substitute references inside `row_template`'s commands.
        /// `None` for the closure-only path.
        row_index_signal_id: Option<u64>,
        overscan: f32,
        horizontal: bool,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
    },
    /// GPU canvas. The author owns rendering: `on_init` runs once
    /// after the backend has a `wgpu` device ready and produces the
    /// user's render state; `on_paint` runs on every requested redraw
    /// and mutates that state. The framework does not interpret any
    /// of it — the GPU context is type-erased so runtime-core
    /// stays wgpu-free.
    ///
    /// `on_init` is wrapped in `Option` because it's `FnOnce`: the
    /// build walker takes it out of the primitive when it hands
    /// ownership to the backend.
    Graphics {
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
    },
    /// Reactive conditional. Renders `then()` while `cond` evaluates
    /// to true and `otherwise()` when it's false. `cond` is a
    /// `Derived<bool>` carrying both the runtime callable and the
    /// structured metadata (method name + input signal ids) generator
    /// backends serialize. Runtime backends call `cond.compute()`
    /// inside an Effect that re-fires on every signal change in
    /// `cond.inputs`; the prior subtree's effects drop on each flip.
    When {
        cond: crate::derive::Derived<bool>,
        then: Box<dyn Fn() -> Primitive>,
        otherwise: Box<dyn Fn() -> Primitive>,
        style: Option<StyleSource>,
    },
    /// Reactive multi-way conditional, the type-erased shape behind
    /// the `switch()` constructor. The walker re-runs `key()` inside
    /// an Effect, compares the result to the previously-seen key via
    /// `eq`, and only re-builds the subtree (dropping the old scope)
    /// when the key actually changes. State inside the old subtree
    /// is freed atomically, mirroring `When`.
    ///
    /// N-way reactive conditional. `discriminant` is a
    /// `Derived<serde_json::Value>` carrying both the runtime
    /// callable (for runtime backends) and the structured metadata
    /// (for generator backends). On each fire the framework
    /// compares the discriminant against each arm's `pattern` via
    /// JSON equality and renders the first match; if no arm
    /// matches, `default` is rendered.
    ///
    /// Keys are constrained to JSON-serializable types because the
    /// match must round-trip through both the host-side closure
    /// path and the generator-side wire format with the same
    /// equality semantics. For runtime backends this means
    /// `Effect::new(re-evaluate-discriminant + diff-against-pattern)`;
    /// for generator backends it means emitting a wire op that the
    /// device-side runtime evaluates after each signal change.
    Switch {
        discriminant: crate::derive::Derived<crate::__serde_json::Value>,
        /// Per-arm: `(pattern, subtree_builder)`. Builder closure is
        /// called once at snapshot for generator backends (so all
        /// arms ship to the device pre-built); on runtime backends
        /// it's called when the arm becomes active.
        arms: Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Primitive>)>,
        /// Fallback subtree when no arm matches. Always present.
        default: Box<dyn Fn() -> Primitive>,
        style: Option<StyleSource>,
    },
    /// Bulk children: build `count` rows from `row_builder(i)` and
    /// insert them in one batch. The build walker uses this to
    /// collapse `for i in 0..n { ... }` lowerings — instead of
    /// walking N child primitives and calling `insert()` N times,
    /// the backend gets ONE `insert_many` call with all the row
    /// nodes preassembled.
    ///
    /// On web this maps to a `DocumentFragment`: append each row
    /// to the fragment, then append the fragment to the parent
    /// view in a single FFI call. Future optimization: detect that
    /// rows are structurally identical and use `cloneNode` to
    /// build them, which collapses per-row `createElement` calls
    /// into one `cloneNode` each.
    Repeat {
        count: usize,
        row_builder: Box<dyn Fn(usize) -> Primitive>,
    },
    /// Declarative navigation. Wraps content; activation dispatches
    /// a `NavCommand` against an ambient navigator captured at
    /// construction time. See [`primitives::link`] for the surface
    /// and rationale.
    Link {
        children: Vec<Primitive>,
        /// Route name (stable; matches `Route::name()`).
        route: &'static str,
        /// Concrete URL produced by `params.to_path(route.path)`
        /// at construction time. Web emits `<a href=url>` and uses
        /// it for right-click affordances; native backends ignore.
        url: String,
        /// Type-erased params source. Each activation calls this to
        /// produce a fresh `Box<dyn Any>` for the `NavCommand`.
        /// `link<P>` boxes `P: Clone` and reproduces on demand.
        make_params: Rc<dyn Fn() -> Box<dyn Any>>,
        /// Push / Replace / Reset.
        kind: primitives::link::NavKind,
        /// Captured ambient `NavigatorControl` at construction.
        /// `None` ⇒ no navigator was active and activation silently
        /// no-ops (matches handle-before-build posture).
        target: Option<Rc<primitives::navigator::NavigatorControl>>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
    },
    /// External — third-party primitive. The framework itself knows
    /// nothing about the specific kind; backends consult their own
    /// [`ExternalRegistry`](crate::external::ExternalRegistry) to
    /// dispatch on `type_id`. Unregistered kinds render a platform-
    /// native "not supported" placeholder via the backend's
    /// `create_external` impl.
    ///
    /// `type_name` is captured at construction (via
    /// `std::any::type_name::<T>()`) and carried alongside `type_id`
    /// purely for debug/error messages. The `type_id` is what drives
    /// dispatch.
    ///
    /// See [`crate::external`] for the third-party extension model
    /// and the constructor `external::<T>(props)`.
    External {
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: Rc<dyn Any>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
    },
    /// Navigator extension — the unified entry point for any
    /// registered navigator kind. The framework owns the routing
    /// substrate (route table, screen scopes, ambient capture,
    /// `NavigatorControl`, hardware-back coordination); the SDK
    /// crate that supplies the navigator kind owns the *presentation*
    /// (native chrome, transitions, gestures).
    ///
    /// `type_id` keys the per-backend
    /// [`NavigatorRegistry`](primitives::navigator::NavigatorRegistry)
    /// lookup that resolves to the handler factory. `presentation` is
    /// the SDK's typed payload, passed through unchanged to
    /// [`NavigatorHandler::init`](primitives::navigator::NavigatorHandler).
    /// `config` carries the shared routing inputs (route table,
    /// initial route, layout closure, default screen options) the
    /// framework consumes to build
    /// [`NavigatorHost`](primitives::navigator::NavigatorHost).
    ///
    /// Per-slot styling (`header`, `tab_bar`, `drawer_scrim`, …) is
    /// SDK-defined: each SDK declares its slot names and the walker
    /// dispatches each `(slot, style)` pair through
    /// `Backend::apply_navigator_slot_style`.
    ///
    /// Boxed config because `screens` is a `HashMap`.
    Navigator {
        type_id: std::any::TypeId,
        type_name: &'static str,
        presentation: Rc<dyn Any>,
        config: Box<primitives::navigator::NavigatorConfig>,
        /// Body style (analogous to a view's `with_style`). Applied
        /// via the regular `Backend::apply_style` path on the returned
        /// navigator node.
        style: Option<StyleSource>,
        /// SDK-defined per-slot styles. Each entry's `slot` is an
        /// opaque string identifier the SDK's handler understands —
        /// the walker dispatches each via
        /// `apply_navigator_slot_style`. Empty when the SDK
        /// builder recorded none.
        slot_styles: Vec<(&'static str, StyleSource)>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
    },
    /// Portal — render `children` at `target` (viewport root, an
    /// anchored element, or a named container) escaping the parent's
    /// layout and clipping context. The lowest-level render-elsewhere
    /// primitive; modals/popovers/tooltips compose on top.
    ///
    /// See [`primitives::portal`] for the target model, dismissal
    /// contract, and platform mapping.
    Portal {
        children: Vec<Primitive>,
        target: primitives::portal::PortalTarget,
        /// Fired when the platform requests dismissal (Escape on
        /// web, back gesture on Android, swipe-down on iOS modal
        /// presentations). The host flips its open-state signal in
        /// response; the framework doesn't auto-unmount.
        on_dismiss: Option<Rc<dyn Fn()>>,
        /// When `true`, the backend confines keyboard /
        /// accessibility focus inside the portal subtree until it
        /// closes. Default `false` — compositions like `modal()`
        /// flip it to `true` at their level.
        trap_focus: bool,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
    },
    /// Presence — mount/unmount with enter and exit animations. See
    /// [`primitives::presence`] for the model. The host's
    /// open/close `Signal<bool>` is exposed via `present`; the
    /// walker defers unmount by `exit.duration_ms` so the exit
    /// animation can play before the scope drops.
    Presence {
        child: Box<dyn Fn() -> Primitive>,
        present: Box<dyn Fn() -> bool>,
        enter: Option<primitives::presence::PresenceAnim>,
        exit: Option<primitives::presence::PresenceAnim>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
    },
    /// Lazy — code-splitting boundary. The subtree is shipped as a
    /// separate wasm chunk on web and loaded on first mount; on
    /// native targets the chunk crate is a normal cargo dep and the
    /// content is rendered inline. See
    /// [`primitives::lazy`](crate::primitives::lazy) and the design
    /// proposal at `docs/proposals/lazy-primitive.md`.
    ///
    /// `chunk` identifies the chunk in the project's
    /// `[package.metadata.idealyst.chunks]` table; author code
    /// references it via the codegen'd `chunks::*` constants for
    /// compile-time typo protection.
    ///
    /// `payload` carries the props passed to the chunk crate's
    /// `app(props)`. The framework type-erases via `Rc<dyn Any>`;
    /// the web backend serializes through `bridge.serialize` for
    /// transport across the wasm boundary, the native backend
    /// hands `payload` to the registered thunk for direct dispatch.
    Lazy {
        chunk: primitives::lazy::ChunkId,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: Rc<dyn Any>,
        /// Codegen'd serializer + dispatch helpers. See
        /// [`LazyBridge`](primitives::lazy::LazyBridge).
        bridge: primitives::lazy::LazyBridge,
        /// Reactive observer of lifecycle transitions. `None`
        /// when the author doesn't care about loading / error
        /// states (rare — at minimum most apps want to render a
        /// spinner).
        on_state: Option<Rc<dyn Fn(primitives::lazy::LazyState)>>,
        /// Subtree mounted immediately as a fallback while the
        /// chunk loads (web) or as a placeholder slot that's
        /// instantly replaced (native). `None` renders an empty
        /// view.
        placeholder: Option<Box<dyn Fn() -> Primitive>>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        accessibility: AccessibilityProps,
    },
}

impl Primitive {
    /// Attaches a test ID to this primitive for robot/automation queries.
    /// Only available when the `robot` feature is enabled.
    #[cfg(feature = "robot")]
    pub fn with_test_id(mut self, id: &'static str) -> Self {
        match &mut self {
            Primitive::View { test_id, .. }
            | Primitive::Text { test_id, .. }
            | Primitive::Button { test_id, .. }
            | Primitive::Pressable { test_id, .. }
            | Primitive::Image { test_id, .. }
            | Primitive::TextInput { test_id, .. }
            | Primitive::TextArea { test_id, .. }
            | Primitive::Toggle { test_id, .. }
            | Primitive::Slider { test_id, .. } => {
                *test_id = Some(id);
            }
            _ => {
                // Other primitives don't carry test_id — no-op.
            }
        }
        self
    }

    /// Read the test_id if set (robot feature only).
    #[cfg(feature = "robot")]
    pub fn test_id(&self) -> Option<&'static str> {
        match self {
            Primitive::View { test_id, .. }
            | Primitive::Text { test_id, .. }
            | Primitive::Button { test_id, .. }
            | Primitive::Pressable { test_id, .. }
            | Primitive::Image { test_id, .. }
            | Primitive::TextInput { test_id, .. }
            | Primitive::TextArea { test_id, .. }
            | Primitive::Toggle { test_id, .. }
            | Primitive::Slider { test_id, .. } => *test_id,
            _ => None,
        }
    }

    /// Attaches a style to this primitive. Replaces any previously-set
    /// style. The style argument can be either a `StyleApplication`
    /// (static) or a closure returning one (reactive).
    pub fn with_style<S: IntoStyleSource>(mut self, style: S) -> Self {
        let src = style.into_style_source();
        match &mut self {
            Primitive::View { style, .. }
            | Primitive::Text { style, .. }
            | Primitive::Button { style, .. }
            | Primitive::Pressable { style, .. }
            | Primitive::Image { style, .. }
            | Primitive::Icon { style, .. }
            | Primitive::TextInput { style, .. }
            | Primitive::TextArea { style, .. }
            | Primitive::Toggle { style, .. }
            | Primitive::ScrollView { style, .. }
            | Primitive::Slider { style, .. }
            | Primitive::ActivityIndicator { style, .. }
            | Primitive::Virtualizer { style, .. }
            | Primitive::Graphics { style, .. }
            | Primitive::When { style, .. }
            | Primitive::Switch { style, .. }
            | Primitive::Link { style, .. }
            | Primitive::Portal { style, .. }
            | Primitive::External { style, .. }
            | Primitive::Lazy { style, .. }
            | Primitive::Navigator { style, .. } => {
                *style = Some(src);
            }
            Primitive::Repeat { .. } => {
                // Repeat is a children-list primitive; styling
                // doesn't apply at this level. The caller should
                // style the surrounding View/ScrollView instead.
                // No-op (we ignore the style) so the surrounding
                // `.with_style(...)` builder pattern doesn't panic
                // when a macro emits it unconditionally.
            }
            Primitive::Presence { .. } => {
                // Presence is a wrapper that handles mount/unmount
                // animations on its child; styling belongs on the
                // child View, not on the Presence node. No-op.
            }
        }
        self
    }
}
