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
        name: "signal",
        module_path: "runtime_core",
        docs: "Create a reactive `Signal<T>` from an initial value — the unit of mutable state in a component. A plain function (the historical `signal!` macro was removed; drop the `!`). `T` is inferred. Read with `.get()` (subscribes the surrounding reactive scope), write with `.set(v)` / `.update(|v| …)`. Equivalent to `Signal::new(value)`; the fn form is canonical. Capability halves: `.split()` → `(ReadSignal, WriteSignal)`, `.read_only()`, `.write_only()` — same slot, but the type only permits reading / writing. Type a prop `ReadSignal<T>` when the component observes without mutating. See [[reactivity]].",
        params: &[
            ParamSpec {
                name: "value",
                type_str: "T",
                type_short_name: "T",
            },
        ],
        return_type: "Signal<T>",
        return_type_short: "Signal",
        category: UtilityCategory::Reactive,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "memo",
        module_path: "runtime_core",
        docs: "Cached derived signal: `memo(move || expr)` recomputes when a signal the closure reads changes, and notifies subscribers only when the value actually differs (`T: PartialEq`). A plain function (the historical `memo!` macro was removed — write the `move ||` yourself). Returns the READ half only (`ReadSignal<T>`): a memo is a pure derivation, so its output is not writable. Use for derived state read in several places or expensive to compute — the work runs once per dependency change, not once per read. For a cheap derivation, a plain closure or `rx!` is lighter; for a type without `PartialEq`, call `memo_with(eq, f)`. Body must be pure — a `.set()` inside the compute panics. See [[reactivity]].",
        params: &[
            ParamSpec {
                name: "f",
                type_str: "impl Fn() -> T",
                type_short_name: "Fn",
            },
        ],
        return_type: "ReadSignal<T>",
        return_type_short: "ReadSignal",
        category: UtilityCategory::Reactive,
        _seal: (),
    }
}

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
        name: "defer_external_registration",
        module_path: "runtime_core",
        docs: "Register a third-party `External` handler LAZILY, from inside a lazy component's body (`#[component(lazy)]`), to keep a heavy SDK out of the web main bundle. Problem it solves: an `External` handler installed eagerly (at boot via `register_extensions`, or an `inventory::submit!` drained at backend construction) is statically reachable from `main.wasm`, so wasm-split keeps the whole SDK in the main bundle — wrapping the *usage* in a lazy component doesn't help, because REGISTRATION is the anchor, not rendering. Fix: have the SDK expose a `register_lazy()` that the app calls from within the lazy component's body — `defer_external_registration::<WebBackend, _>(|b| b.register_external::<Props,_>(handler))`. The closure (and the handler + heavy code it captures) is then reachable only from the chunk, so wasm-split keeps the SDK's code out of main (its data leaves main only under the experimental opt-in `idealyst build --web --release --data-prune`); the backend's `create_external` applies the queued registration (via `drain_external_registrations`, guarded by `has_pending_external_registrations`) before dispatching the chunk's own `External`. `B` is the concrete backend type (`WebBackend` on web); native registers eagerly (no chunk, no bundle cost) so `register_lazy` is a no-op there. See [[External]] and the [[lazy-loading]] guide.",
        params: &[
            ParamSpec {
                name: "apply",
                type_str: "impl FnOnce(&mut B)",
                type_short_name: "FnOnce",
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
        name: "parse",
        module_path: "runtime_core::color",
        docs: "Parse a CSS-ish color string (`#abc`, `#aabbcc`, `#aabbccdd`, `rgb(r,g,b)`, `rgba(r,g,b,a)`, named colors) into the canonical `Rgba` byte intermediate. Centralized in runtime-core; backends use 1-line shims. See `parse_or` for an infallible variant with a fallback.",
        params: &[
            ParamSpec {
                name: "input",
                type_str: "& str",
                type_short_name: "str",
            },
        ],
        return_type: "Result<Rgba, ColorParseError>",
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
        name: "color_scheme",
        module_path: "runtime_core",
        docs: "Returns the platform's light/dark color-scheme default (`Auto`, `Light`, `Dark`), stashed at mount like `platform()`. Install a matching theme to avoid a flash. The framework-level accessor; theme objects themselves live in the `idea-theme` SDK, not here.",
        params: &[],
        return_type: "ColorScheme",
        return_type_short: "ColorScheme",
        category: UtilityCategory::Platform,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "safe_area_insets",
        module_path: "runtime_core",
        docs: "Current platform safe-area insets (top, right, bottom, left) in device-independent pixels, as a reactive `Signal<EdgeInsets>`. Orientation flips and dynamic-island changes propagate without a rebuild. Prefer `View::safe_area_sides` for the typical per-side opt-in.",
        params: &[],
        return_type: "Signal<EdgeInsets>",
        return_type_short: "Signal<EdgeInsets>",
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
        module_path: "runtime_core",
        docs: "Reactive `Signal<Breakpoint>` derived from the active theme's breakpoint thresholds and `viewport_size()`. Use in `.responsive()`-style flows; prefer this over hand-comparing widths so the threshold lives in the theme, not the call site.",
        params: &[],
        return_type: "Signal<Breakpoint>",
        return_type_short: "Signal<Breakpoint>",
        category: UtilityCategory::Layout,
        _seal: (),
    }
}
