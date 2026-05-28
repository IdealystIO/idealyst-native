//! Hand-curated registration table for [`UtilityEntry`].
//!
//! Same lock pattern as `primitives.rs`: `UtilityEntry` carries a
//! private `_seal: ()` so only this crate can construct one. Third
//! parties wanting to expose chat-callable helpers should use
//! `#[idealyst_tool]` (open by design) rather than reaching for this
//! slice.

use crate::{ParamSpec, UtilityCategory, UtilityEntry};

inventory::submit! {
    UtilityEntry {
        name: "platform",
        module_path: "runtime_core",
        docs: "Returns the current runtime platform (`Ios`, `Android`, `Web`, `MacOs`). Use to branch on backend for legitimate platform variance (different keyboard shortcuts, different copy). Per [[backend_owns_rendering]], do NOT use this to paper over rendering differences — those belong in the backend.",
        params: &[],
        return_type: "Platform",
        return_type_short: "Platform",
        category: UtilityCategory::Platform,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "open_url",
        module_path: "runtime_core",
        docs: "Open an external URL in the host's default handler — a new browser tab on web, Safari/Mail via `UIApplication.open` on iOS, an `ACTION_VIEW` intent on Android, the default browser via `NSWorkspace` on macOS. For *leaving* the app (external pages, `mailto:`, `tel:`); in-app navigation must use the `Link` primitive so web stays single-page. Fire-and-forget — a logged no-op on backends with no opener (terminal, CPU, runtime-server).",
        params: &[
            ParamSpec {
                name: "url",
                type_str: "& str",
                type_short_name: "str",
            },
        ],
        return_type: "()",
        return_type_short: "()",
        category: UtilityCategory::Platform,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "parse_color",
        module_path: "runtime_core::color",
        docs: "Parse a CSS-ish color string (`#abc`, `#aabbcc`, `#aabbccdd`, `rgb(r,g,b)`, `rgba(r,g,b,a)`, named colors) into the canonical `Rgba` byte intermediate. Centralized in runtime-core; backends use 1-line shims.",
        params: &[
            ParamSpec {
                name: "input",
                type_str: "& str",
                type_short_name: "str",
            },
        ],
        return_type: "Option<Rgba>",
        return_type_short: "Rgba",
        category: UtilityCategory::Color,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "lighten",
        module_path: "runtime_core::color",
        docs: "Returns `color` lightened by `amount` (0.0–1.0). Operates in linear sRGB; pass through `parse_color` first.",
        params: &[
            ParamSpec {
                name: "color",
                type_str: "Rgba",
                type_short_name: "Rgba",
            },
            ParamSpec {
                name: "amount",
                type_str: "f32",
                type_short_name: "f32",
            },
        ],
        return_type: "Rgba",
        return_type_short: "Rgba",
        category: UtilityCategory::Color,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "darken",
        module_path: "runtime_core::color",
        docs: "Returns `color` darkened by `amount` (0.0–1.0). Symmetric counterpart to `lighten`.",
        params: &[
            ParamSpec {
                name: "color",
                type_str: "Rgba",
                type_short_name: "Rgba",
            },
            ParamSpec {
                name: "amount",
                type_str: "f32",
                type_short_name: "f32",
            },
        ],
        return_type: "Rgba",
        return_type_short: "Rgba",
        category: UtilityCategory::Color,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "now_micros",
        module_path: "runtime_core::time",
        docs: "Current time in microseconds since the platform's monotonic reference. Wraps the active backend's clock (web: `performance.now()`, native: `mach_absolute_time` / `clock_gettime`). The backend MUST install a time source via `install_time_source(...)` before this returns non-zero on wasm32.",
        params: &[],
        return_type: "u64",
        return_type_short: "u64",
        category: UtilityCategory::Time,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "active_theme",
        module_path: "runtime_core::theme",
        docs: "Returns the currently-installed theme. Panics if no theme was installed before render (see [[install_theme_required]]). The returned `Theme` is reactive — bind to it inside an effect to observe theme switches.",
        params: &[],
        return_type: "Theme",
        return_type_short: "Theme",
        category: UtilityCategory::Theme,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "is_dark_mode",
        module_path: "runtime_core::theme",
        docs: "Reactive shorthand for the current theme's dark-mode flag. Equivalent to `active_theme().is_dark()` but cheaper to read in hot reactive paths because the framework caches the bool.",
        params: &[],
        return_type: "bool",
        return_type_short: "bool",
        category: UtilityCategory::Theme,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "safe_area_insets",
        module_path: "runtime_core::layout",
        docs: "Current platform safe-area insets (top, right, bottom, left) in device-independent pixels. Reactive: orientation flips and dynamic-island changes propagate without a rebuild. Prefer `View::safe_area_sides` for the typical per-side opt-in.",
        params: &[],
        return_type: "EdgeInsets",
        return_type_short: "EdgeInsets",
        category: UtilityCategory::Layout,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "viewport_size",
        module_path: "runtime_core",
        docs: "Reactive `Signal<ViewportSize>` carrying the host window / root view's logical size in device-independent pixels. Updates on rotation / window-resize / browser-resize. Read inside an effect or derived to subscribe; build a `current_breakpoint()`-style helper on top by comparing width against the theme's thresholds.",
        params: &[],
        return_type: "Signal<ViewportSize>",
        return_type_short: "Signal<ViewportSize>",
        category: UtilityCategory::Layout,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "current_breakpoint",
        module_path: "runtime_core::theme",
        docs: "Current breakpoint enum value derived from the active theme's breakpoint thresholds and `viewport_size()`. Use in `.responsive()`-style flows; prefer this over hand-comparing widths so the threshold lives in the theme, not the call site.",
        params: &[],
        return_type: "Breakpoint",
        return_type_short: "Breakpoint",
        category: UtilityCategory::Theme,
        _seal: (),
    }
}
