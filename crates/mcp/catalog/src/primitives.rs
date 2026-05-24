//! Hand-curated registration table for [`PrimitiveEntry`].
//!
//! Lives in this crate (not `runtime-core`) because `PrimitiveEntry`'s
//! private `_seal: ()` field can only be constructed inside this crate's
//! privacy boundary. That's the lock — third-party crates can read every
//! `pub` field but cannot submit their own entries.
//!
//! Drift between this table and the `Primitive` enum in
//! `runtime-core::primitive` is caught by:
//! - `tests/primitive_coverage.rs` — asserts every enum variant has a
//!   matching entry name here (compile-time exhaustive match).
//! - `.claude/audits/primitive-catalog.md` — human-readable drift audit.

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
                type_str: "Vec<Primitive>",
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
        docs: "Renders a string. Pass `\"literal\"` for static content or a closure / `text_fmt!(...)` for reactive content. Backends use native text widgets (`UILabel`, `TextView`, `<span>`, `NSTextField`).",
        props: &[
            PropFieldSpec {
                name: "source",
                type_str: "TextSource",
                doc: "Static string or reactive closure. `text_fmt!` is the sugared form for reactive interpolation.",
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
        docs: "Tappable region with no native chrome — for building custom-looking interactive surfaces. Use `Button` if you want the platform's native button. Authors typically reach for this via the `idea-ui` styled wrapper rather than the bare primitive.",
        props: &[
            PropFieldSpec {
                name: "children",
                type_str: "Vec<Primitive>",
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
        docs: "Bitmap / vector image. Source is platform-aware (asset path, URL, base64); backends use `UIImageView`, `ImageView`, `<img>`, `NSImageView`.",
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
        docs: "Single-line text-entry widget. Backed by `UITextField` (iOS), `EditText` (Android), `<input>` (web), `NSTextField` (macOS). Carries a value signal + change/submit handlers.",
        props: &[
            PropFieldSpec {
                name: "value",
                type_str: "Signal<String>",
                doc: "Two-way bound text value. Reads reflect the widget's current text; writes update it.",
                constraint: "",
            },
            PropFieldSpec {
                name: "placeholder",
                type_str: "Option<TextSource>",
                doc: "Placeholder text shown when the value is empty.",
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
        name: "text_area",
        pascal_name: "TextArea",
        docs: "Multi-line text input. Same model as `TextInput` but with native multi-line widgets (`UITextView`, `EditText` with `inputType=textMultiLine`, `<textarea>`, `NSTextView`).",
        props: &[
            PropFieldSpec {
                name: "value",
                type_str: "Signal<String>",
                doc: "Two-way bound text value.",
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
                doc: "Two-way bound boolean state.",
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
                type_str: "Vec<Primitive>",
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
                type_str: "Vec<Primitive>",
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
                type_str: "Vec<(impl Fn() -> bool, Primitive)>",
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
                type_str: "impl Fn(&T) -> Primitive",
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
                type_str: "Vec<Primitive>",
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
        docs: "Third-party extension escape hatch. Use the per-backend `ExternalRegistry` to register a renderer keyed by name; the runtime resolves the registered impl at mount time. Reference impls: maps, webview (see [[third_party_extension]]).",
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
        docs: "Renders children at the root of the view tree regardless of where the `Portal` appears. Used for modals, tooltips, and any UI that should escape layout / overflow clipping.",
        props: &[
            PropFieldSpec {
                name: "children",
                type_str: "Vec<Primitive>",
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
        docs: "Enter/exit animation surface. Wrap children whose mount/unmount should animate — the framework keeps the children in the tree long enough for the exit animation to complete before unmounting.",
        props: &[
            PropFieldSpec {
                name: "when",
                type_str: "impl Fn() -> bool",
                doc: "Reactive presence predicate.",
                constraint: "",
            },
            PropFieldSpec {
                name: "children",
                type_str: "Vec<Primitive>",
                doc: "Animated subtree.",
                constraint: "",
            },
        ],
        category: PrimitiveCategory::Composition,
        backends: ALL_BACKENDS,
        _seal: (),
    }
}
