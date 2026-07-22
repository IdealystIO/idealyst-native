//! Hand-curated registration table for [`PrimitiveEntry`].
//!
//! Lives in this crate (not `runtime-core`) because `PrimitiveEntry`'s
//! private `_seal: ()` field can only be constructed inside this crate's
//! privacy boundary. That's the lock — third-party crates can read every
//! `pub` field but cannot submit their own entries.
//!
//! Most entries correspond 1:1 to an `Element` enum variant in
//! `runtime-core::primitive`, but the table is keyed by the author-facing
//! `ui!`/`jsx!` TAG (see `canonical_primitive` in
//! `crates/runtime/macros/src/primitives.rs`), not by the variant. So it
//! also carries COMPOSITIONS that are real tags yet have no dedicated
//! variant — `overlay` / `anchored_overlay` lower to `Element::Portal`.
//! Conversely, some `Element` variants (`Portal`, `Pressable`) are NOT
//! author tags; they're catalogued with a caveat steering to the tag that
//! is (`overlay`, the `idea-ui` wrapper).
//!
//! Drift between this table and the framework is caught by
//! `.claude/audits/primitive-catalog.md` — a human-readable drift audit.
//! (There is no `tests/primitive_coverage.rs`; the emission/round-trip
//! coverage lives in `tests/registers_component.rs`, which asserts a core
//! subset is present rather than an exhaustive variant match — precisely so
//! tag-only compositions like `overlay` can be listed without a variant.)

use crate::{PrimitiveCategory, PrimitiveEntry, PropFieldSpec};

const ALL_BACKENDS: &[&str] = &["ios", "android", "web", "macos"];
const NATIVE_ONLY: &[&str] = &["ios", "android", "macos"];

const COMMON_STYLE_FIELD: PropFieldSpec = PropFieldSpec {
    name: "style",
    type_str: "Option<StyleSource>",
    doc: "Optional reactive style binding. Applied via an independent `Effect` so a content change doesn't re-fire the style effect.",
    constraint: "",
};
const COMMON_ACCESSIBILITY_FIELD: PropFieldSpec = PropFieldSpec {
    name: "accessibility",
    type_str: "AccessibilityProps",
    doc: "Per-primitive accessibility prop bag (label, role override, traits, hint). Default infers everything from the primitive type.",
    constraint: "",
};
const COMMON_REF_FILL_FIELD: PropFieldSpec = PropFieldSpec {
    name: "ref",
    type_str: "Option<Ref<...Handle>>",
    doc: "Optional `Ref` slot the framework fills with the primitive's native handle on mount.",
    constraint: "",
};

inventory::submit! {
    PrimitiveEntry {
        name: "view",
        pascal_name: "View",
        docs: "Container primitive — holds zero or more child primitives in a layout box. Maps to UIView (iOS), FrameLayout (Android), <div> (web), and NSView (macOS). Supports per-side safe-area opt-in via `safe_area_sides` and raw touch via `on_touch`.",
        props: &[
            PropFieldSpec {
                name: "children",
                type_str: "Vec<Element>",
                doc: "Child primitives. Pass via the `children![...]` macro or inline `{ ... }` block inside `ui!`.",
                constraint: "",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            PropFieldSpec {
                name: "safe_area_sides",
                type_str: "SafeAreaSides",
                doc: "Per-side opt-in for system safe-area inset padding. Reactive to orientation flips.",
                constraint: "",
            },
            PropFieldSpec {
                name: "on_touch",
                type_str: "Option<TouchHandler>",
                doc: "Optional raw-touch handler. Author-level novel gesture surface — bubbles via the `consumed` flag.",
                constraint: "",
            },
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Structural,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "text",
        pascal_name: "Text",
        docs: "Renders a string. A literal interpolates `{name}` placeholders f-string-style — signal slots are LIVE by type, `Display` values bake in (`text { \"count: {count}\" }`); a closure is reactive (`text { move || … }`); plain literals are static. Backends use native text widgets (`UILabel`, `TextView`, `<span>`, `NSTextField`). KNOWN LIMITATION (interim doc caveat — tracked framework bug, not final behavior): only a BARE identifier is a real interpolation slot (`{count}`). A field path, index, or method call inside braces — `{item.name}`, `{items[0]}`, `{obj.field()}` — is NOT interpolated; it renders LITERALLY as the text `{item.name}` with NO compile error. Any list/detail screen hits this silently. Workaround: pull the value into a local first (`let name = item.name.clone(); text { \"{name}\" }`) or, when it must stay reactive, use a closure that reads it directly: `text { move || item.name.clone() }`.",
        props: &[
            PropFieldSpec {
                name: "source",
                type_str: "TextSource",
                doc: "Static string, f-string literal (`\"count: {count}\"` — slots live-or-static by type), or reactive closure (`move || format!(…)` for positional/Debug formatting). INTERPOLATION LIMITATION (known bug): only a bare identifier is a slot — a field path/index/call (`{item.name}`) renders literally with no error. Use a reactive closure (`move || item.name.clone()`) for anything that isn't a plain variable name.",
                constraint: "brace slots take a BARE identifier only; `{a.b}` / `{a[0]}` / `{a()}` are NOT interpolated (render literally) — use a closure",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Display,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "button",
        pascal_name: "Button",
        docs: "Native push button. Carries an action plus optional leading/trailing icons; the backend renders the platform-native button (`UIButton`, MaterialButton, `<button>`, `NSButton`). Supports `disabled` reactive prop.",
        props: &[
            PropFieldSpec {
                name: "label",
                type_str: "TextSource",
                doc: "Button label. Same shape as `Text::source` — static or reactive.",
                constraint: "",
            },
            PropFieldSpec {
                name: "on_click",
                type_str: "Action",
                doc: "Press handler. Generated backends ship the method name + input/output signal ids to the device.",
                constraint: "",
            },
            PropFieldSpec {
                name: "leading_icon",
                type_str: "Option<IconData>",
                doc: "Icon rendered before the label (left in LTR).",
                constraint: "",
            },
            PropFieldSpec {
                name: "trailing_icon",
                type_str: "Option<IconData>",
                doc: "Icon rendered after the label (right in LTR).",
                constraint: "",
            },
            PropFieldSpec {
                name: "disabled",
                type_str: "Option<impl Fn() -> bool>",
                doc: "Reactive disabled flag. Flips the `DISABLED` state bit + tells the backend to mark the widget inert.",
                constraint: "",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Input,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "pressable",
        pascal_name: "Pressable",
        docs: "Tappable region with no native chrome — for building custom-looking interactive surfaces. Use `Button` if you want the platform's native button. CAVEAT: `pressable` is NOT an author `ui!`/`jsx!` tag — it's absent from the macro's primitive tag table, so `ui! { pressable() { … } }` will NOT compile. It's an `Element` variant the framework and compositions build directly (e.g. `overlay`'s dismiss backdrop). Authors reach for a tappable surface via the `idea-ui` styled wrapper (`Pressable`, a PascalCase `#[component]`) rather than the bare primitive; for a native button use `button`.",
        props: &[
            PropFieldSpec {
                name: "children",
                type_str: "Vec<Element>",
                doc: "Content rendered inside the pressable region.",
                constraint: "",
            },
            PropFieldSpec {
                name: "on_click",
                type_str: "Action",
                doc: "Tap/press handler.",
                constraint: "",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Input,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "image",
        pascal_name: "Image",
        docs: "Bitmap / vector image. Source is platform-aware (asset path, URL, base64); backends use `UIImageView` (iOS), `ImageView` (Android), `<img>` (web), and a layer-backed image view (macOS). Content fit is controlled by the `object_fit` style property (`Fill` / `Contain` / `Cover`); the default is `Contain` (aspect-fit) on every backend. Optional load observers: `.on_load(|ev| ...)` fires once the bitmap decodes with its natural `ev.width`/`ev.height`; `.on_error(|| ...)` fires on load/decode failure. Both are delivered on web + Apple and are a no-op on Android (no URL loader) / headless backends.",
        props: &[
            PropFieldSpec {
                name: "source",
                type_str: "ImageSource",
                doc: "Asset path, URL, or in-memory bytes.",
                constraint: "",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Display,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "icon",
        pascal_name: "Icon",
        docs: "Vector icon from the registered icon system. Pass the icon name as a string; the backend looks it up in the framework's icon registry.",
        props: &[
            PropFieldSpec {
                name: "name",
                type_str: "&str",
                doc: "Icon identifier — must be registered in the icon registry.",
                constraint: "Must be a known icon name",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Display,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "text_input",
        pascal_name: "TextInput",
        docs: "Single-line text-entry widget. Backed by `UITextField` (iOS), `EditText` (Android), `<input>` (web), `NSTextField` (macOS). The `value` signal is the source of truth; `on_change` fires per native input event (typical body: `value.set(new_text)`). Enter-to-submit: chain the builder method `.on_key_down(|e| if e.key == \"Enter\" { submit(); KeyOutcome::PreventDefault } else { KeyOutcome::Default })` after the `ui!` call — see the `input_with_submit` recipe.",
        props: &[
            PropFieldSpec {
                name: "value",
                type_str: "Signal<String>",
                doc: "Two-way bound text value. Reads reflect the widget's current text; writes update it.",
                constraint: "",
            },
            PropFieldSpec {
                name: "on_change",
                type_str: "Fn(String)",
                doc: "Fires for every native input event with the new text. Typical pattern: `value.set(new_text)` (the redundant write-back is optimized away).",
                constraint: "",
            },
            PropFieldSpec {
                name: "placeholder",
                type_str: "impl Into<Reactive<Option<String>>>",
                doc: "Placeholder shown when the value is empty. A plain String is static; a Signal/rx! makes it live.",
                constraint: "",
            },
            PropFieldSpec {
                name: "secure",
                type_str: "impl Into<Reactive<bool>>",
                doc: "Mask entered text (password entry) via each backend's native secure mode. Reactive source allows runtime show/hide.",
                constraint: "",
            },
            PropFieldSpec {
                name: "on_key_down",
                type_str: "Fn(&KeyEvent) -> KeyOutcome",
                doc: "Keydown hook while focused. Return KeyOutcome::PreventDefault to suppress the platform default — this is how Enter-to-submit is built.",
                constraint: "builder method ONLY (chain `.on_key_down(..)` after the ui! call) — an inline prop is silently dropped",
            },
            PropFieldSpec {
                name: "on_blur",
                type_str: "Fn() -> BlurOutcome",
                doc: "Consulted when the input is about to lose focus via the dismiss path. Return BlurOutcome::Keep to veto and keep focus (keyboard stays up on mobile).",
                constraint: "builder method (.on_blur(..)) — check ui! support before using as an inline prop",
            },
            PropFieldSpec {
                name: "on_focus",
                type_str: "Fn(bool)",
                doc: "Focus-change notification: true on gain, false on loss. No veto — for driving focus-dependent chrome (e.g. a parent's focus ring).",
                constraint: "builder method (.on_focus(..)) — check ui! support before using as an inline prop",
            },
            PropFieldSpec {
                name: "ref",
                type_str: "Ref<TextInputHandle>",
                doc: "Imperative handle: focus(), blur(), select_all(), insert_text(text).",
                constraint: "bind via `.bind(ref)` builder method",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Input,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "text_area",
        pascal_name: "TextArea",
        docs: "Multi-line text input. Same model as `TextInput` (value signal + on_change) but with native multi-line widgets (`UITextView`, `EditText` with `inputType=textMultiLine`, `<textarea>`, `NSTextView`). No key/blur/focus hooks — those are `text_input`-only today.",
        props: &[
            PropFieldSpec {
                name: "value",
                type_str: "Signal<String>",
                doc: "Two-way bound text value.",
                constraint: "",
            },
            PropFieldSpec {
                name: "on_change",
                type_str: "Fn(String)",
                doc: "Fires per native input event with the new text; typical body is `value.set(new_text)`.",
                constraint: "",
            },
            PropFieldSpec {
                name: "placeholder",
                type_str: "String",
                doc: "Static placeholder shown when the value is empty.",
                constraint: "",
            },
            PropFieldSpec {
                name: "wrap",
                type_str: "bool",
                doc: "Soft-wrap long lines (default true).",
                constraint: "",
            },
            PropFieldSpec {
                name: "ref",
                type_str: "Ref<TextAreaHandle>",
                doc: "Imperative handle: focus(), blur(), select_all(), insert_text(text).",
                constraint: "",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Input,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "toggle",
        pascal_name: "Toggle",
        docs: "On/off switch. Backed by `UISwitch`, MaterialSwitch, `<input type=checkbox>` (or web `<input type=switch>` polyfill), `NSSwitch`.",
        props: &[
            PropFieldSpec {
                name: "value",
                type_str: "Signal<bool>",
                doc: "Signal → widget binding: the switch reflects the signal. The user's taps arrive via `on_change` — a bare `toggle(value = sig)` renders but never updates the signal.",
                constraint: "",
            },
            PropFieldSpec {
                name: "on_change",
                type_str: "Fn(bool)",
                doc: "Fires with the new state on user interaction. Typical body: `value.set(new_state)`. REQUIRED for the toggle to do anything.",
                constraint: "",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Input,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "scroll_view",
        pascal_name: "ScrollView",
        docs: "Scrollable container. Backed by `UIScrollView` (iOS), `ScrollView` (Android), CSS `overflow: auto` (web), `NSScrollView` (macOS). Preserves scroll position across layout passes (see [[ios_scrollview_bounds_origin]]).",
        props: &[
            PropFieldSpec {
                name: "children",
                type_str: "Vec<Element>",
                doc: "Scrolled content.",
                constraint: "",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Structural,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "slider",
        pascal_name: "Slider",
        docs: "Continuous-range scalar input. Backed by `UISlider`, `SeekBar`, `<input type=range>`, `NSSlider`.",
        props: &[
            PropFieldSpec {
                name: "value",
                type_str: "Signal<f32>",
                doc: "Two-way bound value within `[min, max]`.",
                constraint: "",
            },
            PropFieldSpec {
                name: "min",
                type_str: "f32",
                doc: "Lower bound of the value range.",
                constraint: "",
            },
            PropFieldSpec {
                name: "max",
                type_str: "f32",
                doc: "Upper bound of the value range.",
                constraint: "",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Input,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "activity_indicator",
        pascal_name: "ActivityIndicator",
        docs: "Platform-native spinner. `UIActivityIndicatorView`, `ProgressBar`, CSS spinner, `NSProgressIndicator`.",
        props: &[
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Display,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "virtualizer",
        pascal_name: "Virtualizer",
        docs: "Recycled-row list primitive. Renders only the items currently in the viewport; intended for long lists where `ScrollView` over `Repeat` would blow up the view tree. Author-facing surface is typically the `FlatList` wrapper.",
        props: &[
            PropFieldSpec {
                name: "items",
                type_str: "Signal<Vec<T>>",
                doc: "Items to virtualize. The framework reads `.len()` to size the scroll content and renders a window of children around the current viewport.",
                constraint: "",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Advanced,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "graphics",
        pascal_name: "Graphics",
        docs: "GPU-rendered drawing surface. Local-render only — emits a placeholder under runtime-server (see [[aas_graphics_unsupported]]).",
        props: &[
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Advanced,
        backends: NATIVE_ONLY,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "when",
        pascal_name: "When",
        docs: "Conditional rendering. Renders children only while the reactive condition is true; preserves the surrounding tree shape so the walker can install/remove just the gated subtree.",
        props: &[
            PropFieldSpec {
                name: "cond",
                type_str: "impl Fn() -> bool",
                doc: "Reactive predicate. Children mount when `true`, unmount when `false`.",
                constraint: "",
            },
            PropFieldSpec {
                name: "children",
                type_str: "Vec<Element>",
                doc: "Gated subtree.",
                constraint: "",
            },
        ],
        category: PrimitiveCategory::ControlFlow,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "switch",
        pascal_name: "Switch",
        docs: "N-way conditional. Renders the first matching arm; arms are evaluated reactively. Use over chained `When` blocks when you have mutually-exclusive cases.",
        props: &[
            PropFieldSpec {
                name: "arms",
                type_str: "Vec<(impl Fn() -> bool, Element)>",
                doc: "Predicate + subtree pairs. First matching arm wins.",
                constraint: "",
            },
        ],
        category: PrimitiveCategory::ControlFlow,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "repeat",
        pascal_name: "Repeat",
        docs: "Reactive list rendering — for each item in a signal-backed `Vec`, render one subtree. Use `Virtualizer`/`FlatList` instead for long lists.",
        props: &[
            PropFieldSpec {
                name: "items",
                type_str: "Signal<Vec<T>>",
                doc: "Reactive item list.",
                constraint: "",
            },
            PropFieldSpec {
                name: "render",
                type_str: "impl Fn(&T) -> Element",
                doc: "Per-item subtree builder.",
                constraint: "",
            },
        ],
        category: PrimitiveCategory::ControlFlow,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "link",
        pascal_name: "Link",
        docs: "Navigation link — backend-specific URL handling. Native opens via the platform's URL scheme handler; web is `<a href>`; navigates within navigator routes when the URL matches one.",
        props: &[
            PropFieldSpec {
                name: "url",
                type_str: "&str",
                doc: "Target URL or internal navigator path.",
                constraint: "",
            },
            PropFieldSpec {
                name: "children",
                type_str: "Vec<Element>",
                doc: "Link content (text, icon, etc.).",
                constraint: "",
            },
            COMMON_STYLE_FIELD,
            COMMON_REF_FILL_FIELD,
            COMMON_ACCESSIBILITY_FIELD,
        ],
        category: PrimitiveCategory::Composition,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "external",
        pascal_name: "External",
        docs: "Third-party extension escape hatch. Use the per-backend `ExternalRegistry` to register a renderer keyed by the payload type; the runtime resolves the registered impl at mount time. Reference impls: maps, webview (see [[third_party_extension]]). Heavy SDK used in only one corner of a web app? Register the handler LAZILY from inside a `lazy!` chunk via [[defer_external_registration]] instead of eagerly at boot — eager registration anchors the whole SDK in `main.wasm`, defeating code-splitting.",
        props: &[
            PropFieldSpec {
                name: "kind",
                type_str: "&str",
                doc: "Registry key — must match a `register_external` call on each backend.",
                constraint: "Must be a registered external name",
            },
            PropFieldSpec {
                name: "props",
                type_str: "Box<dyn Any>",
                doc: "Opaque payload handed to the registered renderer.",
                constraint: "",
            },
        ],
        category: PrimitiveCategory::Advanced,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "portal",
        pascal_name: "Portal",
        docs: "Renders children at the root of the view tree regardless of where the `Portal` appears. Used for modals, tooltips, and any UI that should escape layout / overflow clipping. CAVEAT: `portal` is NOT an author `ui!`/`jsx!` tag — there is no `portal` primitive in the macro's tag table, so `ui! { portal() { … } }` will NOT compile. It's the low-level `Element` variant that the `overlay` / `anchored_overlay` compositions lower to. For modals/drawers/sheets use `overlay` (viewport-anchored, with backdrop + focus-trap wiring); for popovers/tooltips/dropdowns/menus use `anchored_overlay`. Reach for the bare `Element::Portal` only when hand-assembling a backdrop-less teleport that the compositions can't express.",
        props: &[
            PropFieldSpec {
                name: "children",
                type_str: "Vec<Element>",
                doc: "Subtree to teleport to the root.",
                constraint: "",
            },
        ],
        category: PrimitiveCategory::Composition,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "presence",
        pascal_name: "Presence",
        docs: "THE canonical primitive for animated show/hide. Wrap children whose mount/unmount should animate — the framework applies the `enter` state before first paint then interpolates to rest, and on hide plays the `exit` state before actually dropping the subtree. This is the DECLARATIVE animation tool (contrast the imperative `animated!` value driver): reach for `presence` whenever a panel/modal/toast should fade or slide in and out. The prop is `present` (a reactive `Fn() -> bool`) — NOT `when` (that's the `when` control-flow primitive; a `presence` with a `when` prop silently never hides). `enter`/`exit` are `PresenceAnim` values built from a `PresenceState` (opacity + 2D translate + uniform scale — the cross-backend-cheap vocabulary) plus a duration and `Easing`.",
        props: &[
            PropFieldSpec {
                name: "present",
                type_str: "impl Fn() -> bool",
                doc: "Reactive presence predicate. Children mount + play `enter` when it flips true; play `exit` then unmount when it flips false. Defaults to always-present if unset — real call sites always bind it. This is the prop, NOT `when`.",
                constraint: "builder method `.present(..)` — set it or the subtree never hides",
            },
            PropFieldSpec {
                name: "enter",
                type_str: "PresenceAnim",
                doc: "Entrance animation: the `PresenceState` applied before first paint, interpolated back to rest over `duration_ms`/`easing`. Build via `PresenceAnim::new(PresenceState::rest().opacity(0.0).translate_y(8.0), 200, Easing::EaseOut)` or the `PresenceAnim::fade(ms, easing)` helper.",
                constraint: "builder method `.enter(anim)`",
            },
            PropFieldSpec {
                name: "exit",
                type_str: "PresenceAnim",
                doc: "Exit animation: the `PresenceState` interpolated toward before the scope drops. Same shape as `enter` (mirror it for a symmetric fade/slide). A mid-exit flip back to `present` reverses the in-flight interpolation without rebuilding the child.",
                constraint: "builder method `.exit(anim)`",
            },
            PropFieldSpec {
                name: "children",
                type_str: "Vec<Element>",
                doc: "Animated subtree. Rebuilt only on a real mount (first appearance, or after a full exit completes) — signals/refs inside survive a near-miss flicker.",
                constraint: "",
            },
        ],
        category: PrimitiveCategory::Composition,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

// `overlay` / `anchored_overlay` are COMPOSITIONS, not `Element` enum
// variants: the `ui!` macro lowers them to `Element::Portal` (adding the
// backdrop + focus-trap wiring around the caller's children — see
// `emit_overlay`/`emit_anchored_overlay` in `crates/runtime/macros/src/ui.rs`
// and the builders in `crates/runtime/core/src/primitives/overlay.rs`). They
// ARE first-class author `ui!`/`jsx!` tags (in `canonical_primitive`), which
// is why they're catalogued here even though they have no dedicated variant.
inventory::submit! {
    PrimitiveEntry {
        name: "overlay",
        pascal_name: "Overlay",
        docs: "THE modal / drawer / full-screen-sheet tag. Viewport-anchored composition that teleports its children above everything (lowering to `Element::Portal`) and wires the backdrop scrim + focus trap for you. Use it for confirm dialogs, modals, bottom sheets, drawers — anything centered or edge-pinned over a dimmed background. Gate visibility with reactive control flow (`when(open, || ui!{ overlay(...) { … } })`) or wrap in `presence` for an animated open/close. For popovers/tooltips/dropdowns/context-menus anchored to a specific element, use `anchored_overlay` instead. Defaults: `Center` placement, `Dismiss` backdrop (tap-scrim-to-close), focus-trap ON.",
        props: &[
            PropFieldSpec {
                name: "placement",
                type_str: "ViewportPlacement",
                doc: "Where the content sits in the viewport (`Center`, edges, corners). Default `Center`.",
                constraint: "",
            },
            PropFieldSpec {
                name: "backdrop",
                type_str: "BackdropMode",
                doc: "Scrim behavior: `Dismiss` (tap the scrim fires `on_dismiss` — default), `Opaque` (scrim swallows taps, host drives close), or `None` (no scrim, viewport behind stays interactive).",
                constraint: "",
            },
            PropFieldSpec {
                name: "on_dismiss",
                type_str: "impl Fn()",
                doc: "Called when the backdrop is tapped under `BackdropMode::Dismiss` (and by the framework's dismiss path). Typical body flips your open signal false: `move || open.set(false)`. Runs batched.",
                constraint: "",
            },
            PropFieldSpec {
                name: "trap_focus",
                type_str: "bool",
                doc: "Keep keyboard/AT focus inside the overlay while open. Default `true` (modal semantics).",
                constraint: "",
            },
            PropFieldSpec {
                name: "backdrop_style",
                type_str: "Option<StyleSource>",
                doc: "Style override for the scrim layer (e.g. tint / opacity). Ignored when `backdrop = None`.",
                constraint: "",
            },
            PropFieldSpec {
                name: "children",
                type_str: "Vec<Element>",
                doc: "The overlay content, rendered above the backdrop. Wrapped in a styleable content view (see `with_style`).",
                constraint: "",
            },
            PropFieldSpec {
                name: "with_style",
                type_str: "IntoStyleSource",
                doc: "Style for the content wrapper view. Builder-only: chain `.with_style(..)` after the `ui!` call — not an inline `ui!` prop.",
                constraint: "builder method `.with_style(..)` — not lowered as an inline prop",
            },
            PropFieldSpec {
                name: "click_through",
                type_str: "bool",
                doc: "Make the portal root `pointer-events: none` so empty areas pass clicks to the page beneath (e.g. a full-width toast strip); interactive descendants opt back in with `pointer_events: Auto`. Web-only effect; no-op on native. Builder-only.",
                constraint: "builder method `.click_through(..)` — web-only; pair with `backdrop = None`",
            },
            COMMON_REF_FILL_FIELD,
        ],
        category: PrimitiveCategory::Composition,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    PrimitiveEntry {
        name: "anchored_overlay",
        pascal_name: "AnchoredOverlay",
        docs: "Element-anchored overlay for popovers, tooltips, dropdowns, and context menus — content positioned relative to a target element rather than the viewport. Like `overlay`, it's a composition that lowers to `Element::Portal`, but it takes a required `target` (an `AnchorTarget`, typically a `Ref` to the trigger) and positions with `side`/`align`/`offset`. Defaults suit the typical popover: side `Below`, align `Start`, offset `0`, backdrop `None` (page behind stays interactive), focus-trap OFF. For centered/edge modals use `overlay`.",
        props: &[
            PropFieldSpec {
                name: "target",
                type_str: "AnchorTarget",
                doc: "REQUIRED. The element to anchor to (usually a `Ref` filled by the trigger). Passed positionally by the lowering; omitting it is a compile error.",
                constraint: "required — `target = ...`",
            },
            PropFieldSpec {
                name: "side",
                type_str: "ElementSide",
                doc: "Which side of the target the content sits on (`Above`/`Below`/`Start`/`End`). Default `Below`.",
                constraint: "",
            },
            PropFieldSpec {
                name: "align",
                type_str: "ElementAlign",
                doc: "Cross-axis alignment against the target edge (`Start`/`Center`/`End`). Default `Start`.",
                constraint: "",
            },
            PropFieldSpec {
                name: "offset",
                type_str: "f32",
                doc: "Gap in px between the target and the content along `side`. Default `0`.",
                constraint: "",
            },
            PropFieldSpec {
                name: "backdrop",
                type_str: "BackdropMode",
                doc: "Scrim behavior. Default `None` (page stays interactive — typical popover UX). Set `Dismiss` for a click-away scrim that fires `on_dismiss`.",
                constraint: "",
            },
            PropFieldSpec {
                name: "on_dismiss",
                type_str: "impl Fn()",
                doc: "Fired by the dismiss path (and a `Dismiss` backdrop tap). Flip your open signal false here. Runs batched.",
                constraint: "",
            },
            PropFieldSpec {
                name: "trap_focus",
                type_str: "bool",
                doc: "Keep focus inside the popover while open. Default `false` (popovers usually don't trap).",
                constraint: "",
            },
            PropFieldSpec {
                name: "backdrop_style",
                type_str: "Option<StyleSource>",
                doc: "Style override for the scrim layer. Ignored under `backdrop = None`.",
                constraint: "",
            },
            PropFieldSpec {
                name: "children",
                type_str: "Vec<Element>",
                doc: "Popover / menu content, positioned against `target`.",
                constraint: "",
            },
            COMMON_REF_FILL_FIELD,
        ],
        category: PrimitiveCategory::Composition,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}
