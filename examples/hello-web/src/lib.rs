use backend_web::WebBackend;
use std::cell::RefCell;
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
}

#[wasm_bindgen(start)]
pub fn start() {
    // Print Rust panics with their message + stack instead of the
    // default `RuntimeError: unreachable` from __rust_abort. Saves
    // the diagnostic dance during development.
    console_error_panic_hook::set_once();

    let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
    let owner = framework_core::render(backend, hello::app());
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
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
