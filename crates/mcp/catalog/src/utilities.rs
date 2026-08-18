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
        docs: "Create a reactive `Signal<T>` from an initial value — the unit of mutable state in a component. A plain function (the historical `signal!` macro was removed; drop the `!`). `T` is inferred. Read with `.get()` (subscribes the surrounding reactive scope). Write surface: `.set(v)` is equality-guarded (`T: PartialEq` — a same-value write wakes no subscribers); `.set_always(v)` writes and always notifies (for deliberate same-value retriggers — NOT an escape from the `PartialEq` bound, which is on the whole handle: a type with no `PartialEq` cannot be stored in a signal at all; give it a pointer-identity impl, or wrap it in `runtime_core::ByIdentity<T>` / `ByIdentityArc<T>` when it is not yours to change); `.touch()` notifies without writing; `.set_untracked(v)` writes without notifying; `.update(|v| …)` mutates in place and always notifies. Equivalent to `Signal::new(value)`; the fn form is canonical. Capability halves: `.split()` → `(ReadSignal, WriteSignal)`, `.read_only()`, `.write_only()` — same slot, but the type only permits reading / writing. Type a prop `ReadSignal<T>` when the component observes without mutating. See [[reactivity]].",
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
        name: "spawn_then",
        module_path: "runtime_core",
        docs: "Bridge async APIs into synchronous UI code: `spawn_then(future, |result| { … })` runs `future` detached, then applies `result` in the callback. THE way to call an `async` SDK/server fn from a handler or component body (crate feature `async-driver`; generated wrappers enable it). Put ALL signal reads and writes in the CALLBACK, never in the future. Why: every `.await` is a flush boundary — the host flushes after each future poll, so the world commits, structural drivers run, and scopes are torn down BETWEEN two adjacent lines of one async block. A `Signal<T>` is `Copy` and captures into an `async move` with nothing in the types objecting, so a write after the await lands on a freed slot and aborts the app with `idealyst[stale-signal-handle]` (the classic case: a save handler that navigates on success — the navigation drops the screen and the trailing `busy.set(false)` dies every time). The callback is `FnOnce`, not a future, so it cannot suspend: it runs inside one turn with the liveness check immediately before it, making the update ATOMIC — every write lands or none does, which no per-write `is_alive()` guard can promise. Reads are covered too, and that matters more: a stale READ can never be made benign (there is no value to synthesize). The in-flight IO still completes; only its result is discarded, so a save is never abandoned mid-write. For declarative async state prefer `resource(deps, fetcher)` (fetch-and-store) or `mutation(handler)` (submit-and-settle), which carry the same guard. `runtime_core::driver::spawn_async` remains for genuinely detached work that must OUTLIVE the component (a background upload, a storage write-through). The `signal-across-await` lint flags the raw form. See [[reactivity-in-depth]].",
        params: &[
            ParamSpec {
                name: "task",
                type_str: "impl Future<Output = T>",
                type_short_name: "Future",
            },
            ParamSpec {
                name: "then",
                type_str: "impl FnOnce(T)",
                type_short_name: "FnOnce",
            },
        ],
        return_type: "()",
        return_type_short: "()",
        category: UtilityCategory::Reactive,
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "memo",
        module_path: "runtime_core",
        docs: "Cached derived signal: `memo(move || expr)` recomputes when a signal the closure reads changes, and notifies subscribers only when the value actually differs (`T: PartialEq`). A plain function (the historical `memo!` macro was removed — write the `move ||` yourself). Returns the READ half only (`ReadSignal<T>`): a memo is a pure derivation, so its output is not writable. Use for derived state read in several places or expensive to compute — the work runs once per dependency change, not once per read. For a cheap derivation, a plain closure or `rx!` is lighter; for a near-equality comparison (float tolerance) call `memo_with(eq, f)` — it narrows the comparison but does not lift the bound, so a type with no equality at all still needs a `PartialEq` impl or a `runtime_core::ByIdentity<T>` wrapper. Body must be pure — a `.set()` inside the compute panics. See [[reactivity]].",
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
