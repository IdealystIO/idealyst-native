//! `offload-demo` — a CPU-heavy job run off the main thread with one call
//! site: `offload::run(offload::handle!(count_primes), &req).await`.
//!
//! Press **Count primes** → the job runs in a Web Worker (web: the same app
//! wasm, instantiated in the worker) or on a `std::thread` (native) while the
//! UI stays live; the result lands in a signal. **Panic in a job** shows the
//! failure path: the awaited result is `Err(OffloadError::Canceled)`, not a
//! hung future.

use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use runtime_core::{signal, ui, Element, Signal};
use serde::{Deserialize, Serialize};

/// Nothing to register: offload renders nothing.
pub fn register_scene_extensions<H>(_registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::caps::ExternalOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
}

/// Android entry.
pub fn scene_app() -> Element {
    app()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PrimeReq {
    pub below: u32,
}

#[derive(Serialize, Deserialize)]
pub struct PrimeOut {
    pub count: u32,
    pub largest: u32,
}

/// Deliberately naive trial division — the point is CPU time.
#[offload::job]
pub fn count_primes(req: PrimeReq) -> PrimeOut {
    let mut count = 0;
    let mut largest = 0;
    for n in 2..req.below {
        if (2..).take_while(|d| d * d <= n).all(|d| n % d != 0) {
            count += 1;
            largest = n;
        }
    }
    PrimeOut { count, largest }
}

#[offload::job]
pub fn always_panics(_: u32) -> u32 {
    panic!("this job panics on purpose");
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let status: Signal<String> = signal("Idle".to_string());

    let on_count = move || {
        status.set("Counting in the background…".to_string());
        runtime_core::driver::spawn_async(async move {
            let req = PrimeReq { below: 2_000_000 };
            status.set(match offload::run(offload::handle!(count_primes), &req).await {
                Ok(out) => format!("{} primes below 2000000; largest {}", out.count, out.largest),
                Err(e) => format!("Error: {e}"),
            });
        });
    };
    let on_panic = move || {
        runtime_core::driver::spawn_async(async move {
            status.set(match offload::run(offload::handle!(always_panics), &0).await {
                Ok(v) => format!("unexpected result {v}"),
                Err(e) => format!("Job failed: {e}"),
            });
        });
    };

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
            Typography(content = "Offload".to_string(), kind = idea_ui::typography_kind::H1)
            Typography(
                content = "A CPU-heavy job runs off the main thread: a Web Worker on web, \
                    a thread natively. Same call site everywhere."
                    .to_string(),
                muted = true,
            )
            text { move || status.get() }
            button(label = "Count primes".to_string(), on_click = on_count)
            button(label = "Panic in a job".to_string(), on_click = on_panic)
        }
    }
}
