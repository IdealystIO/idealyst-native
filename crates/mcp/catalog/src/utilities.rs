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
        snippet: "let ${1:name} = signal(${2:value});",
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
        snippet: "spawn_then(\n\tasync move { ${1:task}.await },\n\tmove |${2:result}| {\n\t\t$0\n\t},\n);",
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
        snippet: "let ${1:name} = memo(move || ${2:expr});",
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
        snippet: "",
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
        snippet: "open_url(${1:url});",
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
        snippet: "parse(${1:input})",
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
        snippet: "",
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
        snippet: "",
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
        snippet: "",
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
        snippet: "",
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
        snippet: "",
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "memo_with",
        module_path: "runtime_core",
        docs: "`memo` for a `T` without `PartialEq`: you supply the equality the change gate uses (`memo_with(|a, b| a.id == b.id, move || …)`). Same contract otherwise — pure body, no `.set()` inside, read-only output. See [[reactivity-in-depth]].",
        params: &[
            ParamSpec {
                name: "eq",
                type_str: "impl Fn(&T, &T) -> bool",
                type_short_name: "Fn",
            },
            ParamSpec {
                name: "f",
                type_str: "impl Fn() -> T",
                type_short_name: "Fn",
            },
        ],
        return_type: "ReadSignal<T>",
        return_type_short: "ReadSignal",
        category: UtilityCategory::Reactive,
        snippet: "let ${1:name} = memo_with(|a, b| ${2:a == b}, move || ${3:expr});",
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "untrack",
        module_path: "runtime_core",
        docs: "Read signals inside `f` WITHOUT subscribing the running effect or memo to them — a deliberate snapshot. Use it when an effect must react to signal A but merely consult signal B (`effect!({ let a = a.get(); let b = untrack(|| b.get()); … })`). Suspension is global to the code region, so nested reads and cross-world reads are covered. For a single signal outside any effect, `.peek()` says the same thing more directly. See [[reactivity-in-depth]].",
        params: &[
            ParamSpec {
                name: "f",
                type_str: "impl FnOnce() -> R",
                type_short_name: "FnOnce",
            },
        ],
        return_type: "R",
        return_type_short: "R",
        category: UtilityCategory::Reactive,
        snippet: "untrack(|| ${1:signal}.get())",
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "on_cleanup",
        module_path: "runtime_core",
        docs: "Register teardown for the RUNNING effect: `f` fires before the effect's next re-run and once more when the effect is disposed. Must be called inside an effect body — it panics otherwise (a component body is not an effect: the mount walk runs unanchored). For a component-lifetime teardown use `on_scope_drop`. See [[reactivity]].",
        params: &[
            ParamSpec {
                name: "f",
                type_str: "impl FnOnce()",
                type_short_name: "FnOnce",
            },
        ],
        return_type: "()",
        return_type_short: "()",
        category: UtilityCategory::Reactive,
        snippet: "on_cleanup(move || {\n\t$0\n});",
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "on_scope_drop",
        module_path: "runtime_core",
        docs: "Register teardown that fires when the enclosing scope is dropped — a component's unmount, a `when` branch hiding, a navigator screen popping. Inside an effect it degrades to `on_cleanup`; inside a world but outside an effect it anchors to a dependency-free keepalive effect owned by the enclosing collector; outside any world it is inert. The right hook for releasing a platform resource a component body acquired (a listener, a timer handle, an observer). See [[reactivity-in-depth]].",
        params: &[
            ParamSpec {
                name: "f",
                type_str: "impl FnOnce()",
                type_short_name: "FnOnce",
            },
        ],
        return_type: "()",
        return_type_short: "()",
        category: UtilityCategory::Reactive,
        snippet: "on_scope_drop(move || {\n\t$0\n});",
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "provide",
        module_path: "runtime_core",
        docs: "Publish a value to every descendant scope, keyed by its TYPE — newtype to disambiguate two values of the same type (`provide(Theme(dark))`). Owned by the providing scope and retracted when it drops, like a signal; a world-lifetime service is `unscoped(|| provide(v))`. Read it below with `inject::<T>()`. Panics outside a scope. See [[reactivity-in-depth]].",
        params: &[
            ParamSpec {
                name: "value",
                type_str: "T",
                type_short_name: "T",
            },
        ],
        return_type: "()",
        return_type_short: "()",
        category: UtilityCategory::Reactive,
        snippet: "provide(${1:value});",
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "inject",
        module_path: "runtime_core",
        docs: "Read the nearest ancestor's `provide`d value of type `T` — `None` when no ancestor provided one (a component rendered outside the provider). Lookup walks the scope tree at call time; it does not subscribe, so provide a `Signal<T>` when descendants must react to changes. See [[reactivity-in-depth]].",
        params: &[
        ],
        return_type: "Option<T>",
        return_type_short: "Option",
        category: UtilityCategory::Reactive,
        snippet: "let ${1:value} = inject::<${2:Type}>();",
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "watch",
        module_path: "runtime_core",
        docs: "React to signals from OUTSIDE the component tree — app init, an async callback, a platform/service install — where `effect!` has no owning scope. Runs `f` now and re-runs it whenever a signal it read changes, until the returned `Subscription` drops: `#[must_use]`, so store it, or `.leak()` for a process-lifetime pin. Inside a component body prefer `effect!`. See [[reactivity]].",
        params: &[
            ParamSpec {
                name: "f",
                type_str: "impl FnMut()",
                type_short_name: "FnMut",
            },
        ],
        return_type: "Subscription",
        return_type_short: "Subscription",
        category: UtilityCategory::Reactive,
        snippet: "let ${1:sub} = watch(move || {\n\t$0\n});",
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "reducer",
        module_path: "runtime_core",
        docs: "Action-dispatched state: `let (state, dispatch) = reducer(initial, |state, action| next)` returns a `Signal<S>` plus a dispatch fn. Each dispatch folds on the STAGED value, so several dispatches in one turn compose; it always notifies and never subscribes the caller. Reach for it when a component's transitions are easier to name than to inline (`Increment`, `Reset`, `Load(page)`). See [[reactivity-in-depth]].",
        params: &[
            ParamSpec {
                name: "initial",
                type_str: "S",
                type_short_name: "S",
            },
            ParamSpec {
                name: "f",
                type_str: "impl Fn(&S, A) -> S",
                type_short_name: "Fn",
            },
        ],
        return_type: "(Signal<S>, impl Fn(A))",
        return_type_short: "Signal",
        category: UtilityCategory::Reactive,
        snippet: "let (${1:state}, ${2:dispatch}) = reducer(${3:initial}, |state, action| {\n\t$0\n});",
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "resource",
        module_path: "runtime_core",
        docs: "Declarative async data keyed on signals: `resource(deps, |deps, cancel| async move { … })` runs the fetcher eagerly and again whenever `deps` (a signal, a tuple of signals, or any `Trackable`) changes, cancelling the previous run (`cancel.on_cancel(…)` bridges to an AbortController or similar). The result is a `Resource<T, E>` read as `Loading` / `Error` / `Success` / `Idle`; `.refetch()` re-runs with the current deps. Carries the same stale-scope guard as `spawn_then`, so completion after unmount is discarded, never applied. For submit-and-settle flows use `mutation`. See [[reactivity-in-depth]].",
        params: &[
            ParamSpec {
                name: "deps",
                type_str: "impl Trackable",
                type_short_name: "Trackable",
            },
            ParamSpec {
                name: "fetcher",
                type_str: "impl Fn(D::Value, ResourceCancel) -> impl Future<Output = Result<T, E>>",
                type_short_name: "Fn",
            },
        ],
        return_type: "Resource<T, E>",
        return_type_short: "Resource",
        category: UtilityCategory::Reactive,
        snippet: "let ${1:data} = resource(${2:deps}, |${3:deps}, _cancel| async move {\n\t$0\n});",
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "mutation",
        module_path: "runtime_core",
        docs: "Callback-driven async state for submit-and-settle flows: `let save = mutation(|input| async move { … })`, then `save.trigger(value)` from a handler and read `save.loading()` / the settled result in the UI. `Clone` — capture it into several closures; clones share one state slot. Anchors to the registering scope like `resource`, so do not trigger it after its creating component unmounts. See [[reactivity-in-depth]].",
        params: &[
            ParamSpec {
                name: "handler",
                type_str: "impl Fn(I) -> impl Future<Output = Result<T, E>>",
                type_short_name: "Fn",
            },
        ],
        return_type: "Mutation<I, T, E>",
        return_type_short: "Mutation",
        category: UtilityCategory::Reactive,
        snippet: "let ${1:save} = mutation(|${2:input}| async move {\n\t$0\n});",
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "after_ms_scoped",
        module_path: "runtime_core",
        docs: "One-shot timer that DIES WITH THE REGISTERING SCOPE: `after_ms_scoped(delay_ms, move || …)` fires once after `delay_ms` unless the component (or `when` branch, or screen) that registered it has been torn down. Re-enters the registering scope on fire, so a nested re-arm attaches to the same anchor. The only author-facing timer — the older `runtime_core::scheduling::*` spellings are crate-private. See [[reactivity-in-depth]].",
        params: &[
            ParamSpec {
                name: "delay_ms",
                type_str: "i32",
                type_short_name: "i32",
            },
            ParamSpec {
                name: "f",
                type_str: "impl FnOnce()",
                type_short_name: "FnOnce",
            },
        ],
        return_type: "()",
        return_type_short: "()",
        category: UtilityCategory::Reactive,
        snippet: "after_ms_scoped(${1:delay_ms}, move || {\n\t$0\n});",
        _seal: (),
    }
}

inventory::submit! {
    UtilityEntry {
        name: "raf_loop_scoped",
        module_path: "runtime_core",
        docs: "Recurring animation-frame loop that dies with the registering scope: `raf_loop_scoped(move || …)` runs `f` every frame until the component unmounts. For value animation prefer `animated!` + an animator; reach for this when something must be sampled per frame (a scroll-linked read, a canvas redraw). See [[reactivity-in-depth]].",
        params: &[
            ParamSpec {
                name: "f",
                type_str: "impl FnMut()",
                type_short_name: "FnMut",
            },
        ],
        return_type: "()",
        return_type_short: "()",
        category: UtilityCategory::Reactive,
        snippet: "raf_loop_scoped(move || {\n\t$0\n});",
        _seal: (),
    }
}
