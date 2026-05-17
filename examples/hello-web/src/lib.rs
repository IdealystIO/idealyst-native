#[cfg(not(feature = "dev-hot-reload"))]
use backend_web::WebBackend;
use std::cell::RefCell;
#[cfg(not(feature = "dev-hot-reload"))]
use std::rc::Rc;
use wasm_bindgen::prelude::*;

// Replace the default allocator with `lol_alloc`'s much smaller
// implementation on the WASM target. The default Rust allocator
// (`dlmalloc`) pulls in significant code; `lol_alloc::FreeListAllocator`
// is a few KB. Only active on wasm32 — other targets keep the default.
#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

thread_local! {
    /// The render call returns an Owner that must outlive the page. Storing
    /// it in a thread-local keeps it alive for the lifetime of the WASM
    /// instance (i.e. until the user navigates away).
    static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };

    /// Hold the render-telemetry raf loop for its lifetime. Dropping
    /// it cancels the registered `requestAnimationFrame` callback.
    /// Only populated when the `debug-stats` feature is on.
    #[cfg(feature = "debug-stats")]
    static TELEMETRY_LOOP: RefCell<Option<framework_core::RafLoop>> =
        const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn start() {
    // Print Rust panics with their message + stack instead of the
    // default `RuntimeError: unreachable` from __rust_abort. Saves
    // the diagnostic dance during development.
    console_error_panic_hook::set_once();

    #[cfg(feature = "dev-hot-reload")]
    {
        dev_hot_reload::start_dev_client();
        // The dev client renders into `#app` via the wire instead
        // of mounting the local tree below. The function returns
        // immediately; the connection lives in a thread-local so
        // it survives `start()` returning.
        #[cfg(feature = "debug-stats")]
        start_render_telemetry();
        return;
    }

    #[cfg(not(feature = "dev-hot-reload"))]
    {
        let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
        let owner = framework_core::render(backend, hello::app());
        OWNER.with(|slot| *slot.borrow_mut() = Some(owner));

        // Start the per-frame render-telemetry pump. The loop
        // drains the framework's event log every frame; when there
        // are events (any user interaction triggers some), it
        // formats a single-line summary + a phase breakdown and
        // logs to the console. Off in release builds via the
        // feature gate.
        #[cfg(feature = "debug-stats")]
        start_render_telemetry();
    }
}

// --- Hot-reload dev client integration -------------------------------------
//
// This whole module is gated behind `dev-hot-reload`. When the
// feature is off, the module — and the `dev-client`
// dependency it pulls in — disappears entirely from the build.

#[cfg(feature = "dev-hot-reload")]
mod dev_hot_reload {
    use std::cell::RefCell;
    use std::rc::Rc;

    use backend_web::WebBackend;
    use dev_client::{connect_web, OutboundSender, WebClientHandle, WireBackend};
    use wasm_bindgen::{JsCast, JsValue};

    const HOST_SELECTOR: &str = "#app";
    /// Name of the JS global the `web-dev-host` binary injects
    /// into served HTML before the wasm boots. Holds the AAS
    /// dev-server's `ws://host:port` URL discovered via Bonjour.
    /// `null` when the host hasn't yet found a matching server —
    /// we retry until it appears.
    const AAS_URL_GLOBAL: &str = "IDEALYST_AAS_URL";

    type AppWire = Rc<RefCell<WireBackend<WebBackend>>>;

    thread_local! {
        /// The persistent `WireBackend` — built once on first
        /// connect, kept alive across reconnects. Its `nodes` map
        /// is the source of truth for what's currently mounted in
        /// the DOM; idempotent apply on the next snapshot only
        /// touches nodes that actually changed.
        static WIRE: RefCell<Option<AppWire>> = const { RefCell::new(None) };
        /// The current WebSocket connection. Drop = disconnect.
        /// Replaced (not torn down with WIRE) on every reconnect.
        static CLIENT: RefCell<Option<WebClientHandle>> = const { RefCell::new(None) };
    }

    /// Entry point — called once from `start()`. Builds the wire,
    /// then attempts the first connect.
    pub fn start_dev_client() {
        connect_attempt();
    }

    fn connect_attempt() {
        // First call: build the persistent wire. Subsequent calls
        // (reconnects) re-use the existing wire — its DOM state
        // survives.
        WIRE.with(|slot| {
            if slot.borrow().is_none() {
                let real_backend = WebBackend::new(HOST_SELECTOR);
                let outbound = OutboundSender::new();
                let wire = Rc::new(RefCell::new(WireBackend::new(real_backend, outbound)));
                *slot.borrow_mut() = Some(wire);
            }
        });
        let wire = WIRE.with(|slot| slot.borrow().as_ref().unwrap().clone());

        // Read the URL from the `window.IDEALYST_AAS_URL` global
        // injected by `web-dev-host`. Null when discovery hasn't
        // found a matching dev-server yet — we just retry until
        // it does.
        let url = match aas_url_from_window() {
            Some(u) => u,
            None => {
                web_sys::console::log_1(
                    &"[hello-web] AAS URL not yet available; waiting…".into(),
                );
                schedule_retry();
                return;
            }
        };

        let on_disconnect: Rc<dyn Fn()> = Rc::new(|| {
            connect_attempt();
        });

        match connect_web(&url, wire, on_disconnect) {
            Ok(handle) => {
                CLIENT.with(|slot| *slot.borrow_mut() = Some(handle));
                web_sys::console::log_1(
                    &format!("[hello-web] hot-reload connected to {}", url).into(),
                );
            }
            Err(e) => {
                web_sys::console::warn_2(
                    &"[hello-web] hot-reload connect failed; retrying:".into(),
                    &e,
                );
                schedule_retry();
            }
        }
    }

    /// Read `window.IDEALYST_AAS_URL` set by `web-dev-host`. Returns
    /// `None` when missing or `null`, which the caller treats as
    /// "retry shortly" — the dev-host might be browsing for the
    /// AAS server still.
    fn aas_url_from_window() -> Option<String> {
        let window = web_sys::window()?;
        let value = js_sys::Reflect::get(&window, &JsValue::from_str(AAS_URL_GLOBAL)).ok()?;
        if value.is_null() || value.is_undefined() {
            return None;
        }
        value.as_string()
    }

    fn schedule_retry() {
        if let Some(window) = web_sys::window() {
            let cb = wasm_bindgen::closure::Closure::once_into_js(|| {
                connect_attempt();
            });
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                250,
            );
        }
    }
}

/// Per-frame render-telemetry loop. Drains the framework's debug
/// event log on each `requestAnimationFrame`. When non-empty,
/// formats a one-line render summary + an apply-style phase
/// breakdown and pushes both to the browser console.
///
/// The loop is held in a thread-local so it survives across the
/// `start()` function's return; dropping the thread-local entry
/// (e.g. on a future teardown path) cancels the underlying rAF
/// callback.
#[cfg(feature = "debug-stats")]
fn start_render_telemetry() {
    use framework_core::debug;

    let raf = framework_core::raf_loop(|| {
        let events = debug::take_events();
        if events.is_empty() {
            return;
        }
        log_render_summary(&events);

        // Apply-style phase counters are reported by the web
        // backend (one entry per sub-phase). They aggregate
        // across calls, so we drain them per-frame too.
        let phases = debug::take_phase_counters();
        if !phases.is_empty() {
            log_phase_summary(&phases);
        }
    });

    TELEMETRY_LOOP.with(|slot| *slot.borrow_mut() = Some(raf));
}

/// Format and log a one-frame render summary. Aggregates:
/// - Components: total time per `#[component]` function.
/// - Backend creates: count per primitive kind.
/// - Apply-style cycles: count + cache hit/miss.
/// - Effect fires.
///
/// One `console.group` collapses all per-frame lines into a
/// single expandable entry so the console isn't flooded with
/// per-event noise across many navigations.
#[cfg(feature = "debug-stats")]
fn log_render_summary(events: &[framework_core::debug::DebugEvent]) {
    use framework_core::debug::{self, DebugEvent, PrimitiveKind};
    use std::collections::HashMap;

    // Frame bounds — earliest enter / latest exit across all
    // events. Gives a coarse "how long did this frame's reactive
    // work take" number.
    let mut min_us = u64::MAX;
    let mut max_us = 0u64;
    for e in events {
        let t = event_at_us(e);
        if t < min_us {
            min_us = t;
        }
        if t > max_us {
            max_us = t;
        }
    }
    let frame_dur_us = max_us.saturating_sub(min_us);

    // Component time aggregator.
    let component_summary = debug::component_summary(events);
    let mut components: Vec<_> = component_summary.iter().collect();
    components.sort_by_key(|(_, s)| std::cmp::Reverse(s.total_inclusive_us));

    // Backend-create + build counts per primitive kind.
    let mut backend_create_counts: HashMap<PrimitiveKind, u32> = HashMap::new();
    let mut effect_fires = 0u32;
    let mut style_hits = 0u32;
    let mut style_misses = 0u32;
    let mut apply_style_cycles = 0u32;
    for e in events {
        match e {
            DebugEvent::BackendCreateEnter { kind, .. } => {
                *backend_create_counts.entry(*kind).or_default() += 1;
            }
            DebugEvent::EffectFired { .. } => effect_fires += 1,
            DebugEvent::StyleCacheHit { .. } => style_hits += 1,
            DebugEvent::StyleCacheMiss { .. } => style_misses += 1,
            DebugEvent::ApplyStyleEnter { .. } => apply_style_cycles += 1,
            _ => {}
        }
    }

    // Render the group header — collapsed by default so each
    // navigation produces one terse line + an expandable detail.
    let header = format!(
        "[render] {} events, span={:.2}ms",
        events.len(),
        frame_dur_us as f64 / 1000.0,
    );
    web_sys::console::group_collapsed_1(&header.into());

    if !components.is_empty() {
        web_sys::console::log_1(&"components:".into());
        for (name, s) in components.iter().take(10) {
            web_sys::console::log_1(
                &format!(
                    "  {:>24} {:>3} call(s)  total {:>6.2}ms  max {:>5.2}ms",
                    name,
                    s.call_count,
                    s.total_inclusive_us as f64 / 1000.0,
                    s.max_inclusive_us as f64 / 1000.0,
                )
                .into(),
            );
        }
    }

    if !backend_create_counts.is_empty() {
        let mut bc: Vec<_> = backend_create_counts.iter().collect();
        bc.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        let parts: Vec<String> = bc
            .iter()
            .map(|(k, n)| format!("{:?}×{}", k, n))
            .collect();
        web_sys::console::log_1(&format!("backend creates: {}", parts.join("  ")).into());
    }

    if apply_style_cycles > 0 || style_hits > 0 || style_misses > 0 {
        web_sys::console::log_1(
            &format!(
                "apply-style: {} cycle(s), cache hits={} misses={}",
                apply_style_cycles, style_hits, style_misses,
            )
            .into(),
        );
    }

    if effect_fires > 0 {
        web_sys::console::log_1(&format!("effects fired: {}", effect_fires).into());
    }

    web_sys::console::group_end();
}

/// Format and log the apply-style phase counters. Reported by the
/// web backend for each sub-step of `apply_styled_states`
/// (insert_rule, set_attribute, content_key hash, etc.). Drained
/// per frame so each render's breakdown is independent.
#[cfg(feature = "debug-stats")]
fn log_phase_summary(
    phases: &std::collections::HashMap<&'static str, framework_core::debug::PhaseCounter>,
) {
    let mut sorted: Vec<_> = phases.iter().collect();
    sorted.sort_by_key(|(_, c)| std::cmp::Reverse(c.total_us));

    web_sys::console::group_collapsed_1(&"[apply-style phases]".into());
    for (phase, c) in sorted {
        let avg_us = c.total_us / c.call_count.max(1);
        web_sys::console::log_1(
            &format!(
                "  {:>24} {:>4} call(s)  total {:>6.2}ms  max {:>5.2}ms  avg {:>4}μs",
                phase,
                c.call_count,
                c.total_us as f64 / 1000.0,
                c.max_us as f64 / 1000.0,
                avg_us,
            )
            .into(),
        );
    }
    web_sys::console::group_end();
}

/// Extract the `at_us` timestamp from any `DebugEvent` variant.
#[cfg(feature = "debug-stats")]
fn event_at_us(e: &framework_core::debug::DebugEvent) -> u64 {
    use framework_core::debug::DebugEvent::*;
    match e {
        ComponentEnter { at_us, .. } | ComponentExit { at_us, .. }
        | BuildEnter { at_us, .. } | BuildExit { at_us, .. }
        | BackendCreateEnter { at_us, .. } | BackendCreateExit { at_us, .. }
        | ApplyStyleEnter { at_us } | ApplyStyleExit { at_us }
        | StyleCacheHit { at_us } | StyleCacheMiss { at_us }
        | EffectFired { at_us } => *at_us,
        VirtualizerMount { at_us, .. } | VirtualizerRelease { at_us, .. } => *at_us,
    }
}

/// Drain the debug event log and dump a summary to the browser
/// console. Wired only when the `debug-stats` feature is on; with the
/// feature off the `framework_core::debug` module doesn't exist, so
/// this function isn't compiled at all.
///
/// Call from JS as `wasm_bindgen.dump_debug_events()` (or whatever
/// name the JS bindings expose it under) after exercising the UI.
#[cfg(feature = "debug-stats")]
#[wasm_bindgen]
pub fn dump_debug_events() {
    use framework_core::debug;
    let events = debug::take_events();
    let summary = debug::component_summary(&events);
    web_sys::console::log_1(&format!("[debug] {} events total", events.len()).into());
    let mut sorted: Vec<_> = summary.iter().collect();
    sorted.sort_by_key(|(_, s)| std::cmp::Reverse(s.total_inclusive_us));
    for (name, s) in sorted {
        web_sys::console::log_1(
            &format!(
                "[debug] {:>24} calls={:<4} total={}us max={}us avg={}us",
                name,
                s.call_count,
                s.total_inclusive_us,
                s.max_inclusive_us,
                s.total_inclusive_us / s.call_count.max(1),
            )
            .into(),
        );
    }

    // Apply-style phase breakdown. The web backend reports per-phase
    // timings (`set_attribute_fast`, `insert_rule`, `content_key`, …)
    // from inside `impl_apply_styled_states`; the totals show which
    // sub-step dominates a theme-toggle storm.
    let phases = debug::take_phase_counters();
    if !phases.is_empty() {
        web_sys::console::log_1(&"[debug] apply-style phases:".into());
        let mut p_sorted: Vec<_> = phases.iter().collect();
        p_sorted.sort_by_key(|(_, c)| std::cmp::Reverse(c.total_us));
        for (phase, c) in p_sorted {
            let avg = c.total_us / c.call_count.max(1);
            web_sys::console::log_1(
                &format!(
                    "[debug] {:>24} calls={:<5} total={}us max={}us avg={}us",
                    phase, c.call_count, c.total_us, c.max_us, avg,
                )
                .into(),
            );
        }
    }
}
